//! The writer thread: drains commands, batches captures, and commits state.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use redb::Database;

use super::super::backend;
use super::super::codec;
use super::super::schema::ProjectMeta;
use super::persistence::{
    apply_capture_batch, build_snapshot, clear_http, clear_ws, commit_counters,
    interrupt_in_flight, receipt, save_as, write_workspace,
};
use super::shared::Shared;
use super::{Command, ProjectMode, Result, RetentionLimits, new_project_id};

/// Maximum captures folded into one write transaction.
const CAPTURE_BATCH: usize = 128;
/// How long the writer waits for more captures before committing a batch.
const CAPTURE_BATCH_WINDOW: Duration = Duration::from_millis(50);

pub(super) fn run_writer(
    receiver: Receiver<Command>,
    shared: Arc<Shared>,
    initial_db: Arc<Database>,
    mut meta: ProjectMeta,
    initial_mode: ProjectMode,
    temporary_retention: RetentionLimits,
) {
    let mut db = initial_db;
    let mut pending: VecDeque<Command> = VecDeque::new();
    let mut last_workspace: Option<Vec<u8>> = None;
    let mut workspace_error: Option<String> = None;
    let mut retention = match initial_mode {
        ProjectMode::Temporary => Some(temporary_retention),
        ProjectMode::Persistent { .. } => Some(RetentionLimits::new(
            meta.retention_records,
            meta.retention_bytes,
        )),
    };
    loop {
        let command = match pending.pop_front() {
            Some(command) => command,
            None => match receiver.recv() {
                Ok(command) => command,
                Err(_) => break,
            },
        };
        if matches!(
            command,
            Command::SaveAs { .. } | Command::SaveCopy { .. } | Command::Flush { .. }
        ) && let Some(error) = &workspace_error
        {
            reject(
                command,
                super::ProjectError::WorkspacePersistence(error.clone()),
            );
            continue;
        }
        match command {
            Command::WithWorkspace(workspace, command) => {
                let result = codec::encode_large(&workspace).and_then(|bytes| {
                    write_workspace(&db, &bytes)?;
                    last_workspace = Some(bytes);
                    Ok(())
                });
                match result {
                    Ok(()) => {
                        workspace_error = None;
                        pending.push_front(*command);
                    }
                    Err(error) => {
                        workspace_error = Some(error.to_string());
                        shared.set_status_error(error.to_string());
                        reject(*command, error);
                    }
                }
            }
            Command::InstallPrepared(prepared, reply) => {
                db = prepared.db;
                meta = prepared.meta;
                retention = Some(RetentionLimits::new(
                    meta.retention_records,
                    meta.retention_bytes,
                ));
                shared.set_database(db.clone());
                shared.install_meta(
                    &meta,
                    ProjectMode::Persistent {
                        path: prepared.path,
                    },
                );
                shared.clear_mappings();
                last_workspace = None;
                workspace_error = None;
                let _ = reply.send(Ok(*prepared.opened));
            }
            Command::SetRetention(limits, reply) => {
                let result = super::persistence::set_retention(&db, &mut meta, limits);
                if result.is_ok() {
                    retention = Some(limits);
                }
                finish(reply, result, &shared, &meta);
            }
            Command::Capture(first) => {
                let mut events = vec![first];
                let deadline = Instant::now() + CAPTURE_BATCH_WINDOW;
                while events.len() < CAPTURE_BATCH {
                    let now = Instant::now();
                    if now >= deadline {
                        break;
                    }
                    match receiver.recv_timeout(deadline.saturating_duration_since(now)) {
                        Ok(Command::Capture(event)) => events.push(event),
                        Ok(other) => {
                            pending.push_back(other);
                            break;
                        }
                        Err(RecvTimeoutError::Timeout) => break,
                        Err(RecvTimeoutError::Disconnected) => break,
                    }
                }
                absorb_pending_loss(&shared, &mut meta);
                match apply_capture_batch(&db, &mut meta, retention, &events) {
                    Ok(()) => {
                        shared.refresh_meta(&meta);
                    }
                    Err(err) => {
                        log::error!("project capture write failed: {err}");
                        shared.set_status_error(err.to_string());
                    }
                }
            }
            Command::SetWorkspace(workspace) => match codec::encode_large(&workspace) {
                Ok(bytes) => {
                    if last_workspace.as_deref() != Some(bytes.as_slice()) {
                        match write_workspace(&db, &bytes) {
                            Ok(()) => {
                                last_workspace = Some(bytes);
                                workspace_error = None;
                            }
                            Err(err) => {
                                workspace_error = Some(err.to_string());
                                shared.set_status_error(err.to_string());
                            }
                        }
                    } else {
                        workspace_error = None;
                    }
                }
                Err(err) => {
                    workspace_error = Some(err.to_string());
                    shared.set_status_error(err.to_string());
                }
            },
            Command::ClearHttp(reply) => {
                absorb_pending_loss(&shared, &mut meta);
                let result = clear_http(&db, &mut meta);
                finish(reply, result, &shared, &meta);
            }
            Command::ClearWs(reply) => {
                absorb_pending_loss(&shared, &mut meta);
                let result = clear_ws(&db, &mut meta);
                finish(reply, result, &shared, &meta);
            }
            Command::InterruptInFlight { reason } => {
                match interrupt_in_flight(&db, &mut meta, &reason) {
                    Ok(()) => shared.refresh_meta(&meta),
                    Err(err) => shared.set_status_error(err.to_string()),
                }
            }
            Command::SaveAs {
                path,
                overwrite,
                reply,
            } => {
                absorb_pending_loss(&shared, &mut meta);
                let result = save_as(&mut db, &shared, &mut meta, &path, overwrite);
                match result {
                    Ok(()) => {
                        retention = Some(RetentionLimits::new(
                            meta.retention_records,
                            meta.retention_bytes,
                        ));
                        let _ = reply.send(Ok(()));
                    }
                    Err(err) => {
                        shared.set_status_error(err.to_string());
                        let _ = reply.send(Err(err));
                    }
                }
            }
            Command::SaveCopy {
                path,
                overwrite,
                reply,
            } => {
                // A copy onto the file that is already open is a no-op except
                // for making pending metadata durable. Avoid replacing a live
                // database file, which Windows cannot do.
                let result = if overwrite && shared.status().mode.path() == Some(path.as_path()) {
                    commit_counters(&db, &meta)
                } else {
                    super::super::transfer::save_copy(&db, &path, overwrite)
                };
                match result {
                    Ok(()) => {
                        let _ = reply.send(Ok(()));
                    }
                    Err(err) => {
                        shared.set_status_error(err.to_string());
                        let _ = reply.send(Err(err));
                    }
                }
            }
            #[cfg(test)]
            Command::Open {
                path,
                ui_limit,
                ui_bytes,
                reply,
            } => match super::persistence::open_project(&path, ui_limit, ui_bytes) {
                Ok((new_db, new_meta, opened)) => {
                    db = new_db;
                    meta = new_meta;
                    retention = Some(RetentionLimits::new(
                        meta.retention_records,
                        meta.retention_bytes,
                    ));
                    shared.set_database(db.clone());
                    shared.install_meta(&meta, ProjectMode::Persistent { path });
                    shared.clear_mappings();
                    last_workspace = None;
                    let _ = reply.send(Ok(opened));
                }
                Err(err) => {
                    shared.set_status_error(err.to_string());
                    let _ = reply.send(Err(err));
                }
            },
            Command::New {
                ui_limit,
                ui_bytes,
                reply,
            } => {
                let new_meta = ProjectMeta::new(new_project_id());
                let created = backend::create_memory(&new_meta).and_then(|new_db| {
                    let new_db = Arc::new(new_db);
                    let opened = build_snapshot(&new_db, ui_limit, ui_bytes)?;
                    Ok((new_db, opened))
                });
                match created {
                    Ok((new_db, opened)) => {
                        db = new_db;
                        meta = new_meta;
                        retention = Some(temporary_retention);
                        shared.set_database(db.clone());
                        shared.install_meta(&meta, ProjectMode::Temporary);
                        shared.clear_mappings();
                        last_workspace = None;
                        let _ = reply.send(Ok(opened));
                    }
                    Err(err) => {
                        // Match every other failure arm: without this the
                        // status widget would keep showing the previous state
                        // as if the new project had been created.
                        shared.set_status_error(err.to_string());
                        let _ = reply.send(Err(err));
                    }
                }
            }
            Command::Flush { reply } => {
                absorb_pending_loss(&shared, &mut meta);
                // The metadata write also makes the loss counter and counters
                // durable if no capture write occurred since the last batch.
                let result = commit_counters(&db, &meta);
                match result {
                    Ok(()) => {
                        let _ = reply.send(Ok(receipt(&meta)));
                    }
                    Err(err) => {
                        shared.set_status_error(err.to_string());
                        let _ = reply.send(Err(err));
                    }
                }
            }
            Command::Shutdown { reply } => {
                absorb_pending_loss(&shared, &mut meta);
                let result = commit_counters(&db, &meta);
                match result {
                    Ok(()) => {
                        let _ = reply.send(Ok(()));
                    }
                    Err(err) => {
                        shared.set_status_error(err.to_string());
                        let _ = reply.send(Err(err));
                    }
                }
                break;
            }
        }
    }
}

fn finish<T>(reply: Sender<Result<T>>, result: Result<T>, shared: &Shared, meta: &ProjectMeta) {
    match result {
        Ok(value) => {
            shared.refresh_meta(meta);
            let _ = reply.send(Ok(value));
        }
        Err(err) => {
            shared.set_status_error(err.to_string());
            let _ = reply.send(Err(err));
        }
    }
}

fn absorb_pending_loss(shared: &Shared, meta: &mut ProjectMeta) {
    let pending = shared.pending_loss.swap(0, Ordering::SeqCst);
    if pending > 0 {
        meta.capture_loss_count = meta.capture_loss_count.saturating_add(pending);
    }
}

pub(super) fn reject(command: Command, error: super::ProjectError) {
    match command {
        Command::WithWorkspace(_, command) => reject(*command, error),
        Command::SaveAs { reply, .. } | Command::SaveCopy { reply, .. } => {
            let _ = reply.send(Err(error));
        }
        Command::Flush { reply } => {
            let _ = reply.send(Err(error));
        }
        Command::InstallPrepared(_, reply) => {
            let _ = reply.send(Err(error));
        }
        _ => unreachable!("only workspace barriers can be rejected"),
    }
}
