//! The single project writer and the UI-facing handle.
//!
//! All writes are serialized on one dedicated thread. The UI and proxy never
//! touch a write transaction directly, so capture ordering, index maintenance
//! and file transfers are deterministic. Reads use short-lived snapshot
//! transactions through [`super::queries`].
//!
//! The module is split into [`shared`] (runtime state shared by the handle and
//! writer), [`writer`] (the command loop), and [`persistence`] (the redb
//! operations the writer executes).

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, Receiver, Sender, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use redb::{Database, ReadableDatabase};

use crate::history::HistoryItem;
use crate::proxy::ProxyEvent;
use crate::websocket_history::WebSocketMessageItem;

use super::backend;
use super::queries::{self, HistoryCursor, HttpRow, WsRow};
use super::records::{WorkspaceState, normalized_event};
use super::schema::{self, ProjectMeta};
use super::{ProjectError, Result};

mod persistence;
mod shared;
mod writer;

use self::shared::Shared;
use self::writer::run_writer;

#[cfg(test)]
mod tests;

const COMMAND_QUEUE: usize = 4096;

/// Default count cap for a temporary project. The app derives its own default
/// from `--history-limit`; this keeps the library constructor bounded too.
pub const DEFAULT_TEMPORARY_RETENTION_RECORDS: u64 = 2_000;
/// Default retained-body cap for a temporary project.
pub const DEFAULT_TEMPORARY_RETENTION_BYTES: u64 = 256 * 1024 * 1024;

/// Aggregate retention limits for the in-memory project.
///
/// A zero value disables that dimension. Limits are applied oldest-first after
/// each capture transaction commits; persistent projects never evict records.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RetentionLimits {
    pub max_records: u64,
    pub max_body_bytes: u64,
}

impl RetentionLimits {
    pub const fn new(max_records: u64, max_body_bytes: u64) -> Self {
        Self {
            max_records,
            max_body_bytes,
        }
    }

    pub(crate) fn count_exceeded(self, records: u64) -> bool {
        self.max_records > 0 && records > self.max_records
    }

    pub(crate) fn bytes_exceeded(self, bytes: u64) -> bool {
        self.max_body_bytes > 0 && bytes > self.max_body_bytes
    }
}

impl Default for RetentionLimits {
    fn default() -> Self {
        Self::new(
            DEFAULT_TEMPORARY_RETENTION_RECORDS,
            DEFAULT_TEMPORARY_RETENTION_BYTES,
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProjectMode {
    Temporary,
    Persistent { path: PathBuf },
}

impl ProjectMode {
    pub fn path(&self) -> Option<&Path> {
        match self {
            Self::Temporary => None,
            Self::Persistent { path } => Some(path),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ProjectState {
    #[default]
    Ready,
    Initializing,
    Migrating,
}

#[derive(Clone, Debug)]
pub struct ProjectStatus {
    pub mode: ProjectMode,
    pub state: ProjectState,
    pub generation: u64,
    pub http_count: u64,
    pub ws_count: u64,
    pub last_committed_sequence: u64,
    pub capture_loss_count: u64,
    pub event_loss_count: u64,
    pub retention_evictions: u64,
    pub last_error: Option<String>,
}

#[derive(Clone, Debug)]
#[allow(dead_code)] // Receipts are inspected by tests and future save status UI.
pub struct CommitReceipt {
    pub sequence: u64,
    pub http_count: u64,
    pub ws_count: u64,
}

#[derive(Clone, Debug)]
pub struct OpenedProject {
    pub http: Vec<HistoryItem>,
    pub ws: Vec<WebSocketMessageItem>,
    pub workspace: WorkspaceState,
    pub http_total: u64,
    pub ws_total: u64,
}

/// A capture after project IDs have been assigned.
#[derive(Clone, Debug)]
pub struct CaptureOutcome {
    pub event: Option<ProxyEvent>,
}

enum Command {
    Capture(ProxyEvent),
    SetWorkspace(Box<WorkspaceState>),
    ClearHttp(Sender<Result<CommitReceipt>>),
    ClearWs(Sender<Result<CommitReceipt>>),
    InterruptInFlight {
        reason: String,
    },
    SaveAs {
        path: PathBuf,
        overwrite: bool,
        reply: Sender<Result<()>>,
    },
    SaveCopy {
        path: PathBuf,
        overwrite: bool,
        reply: Sender<Result<()>>,
    },
    Open {
        path: PathBuf,
        ui_limit: usize,
        ui_bytes: usize,
        reply: Sender<Result<OpenedProject>>,
    },
    New {
        ui_limit: usize,
        ui_bytes: usize,
        reply: Sender<Result<OpenedProject>>,
    },
    Flush {
        reply: Sender<Result<CommitReceipt>>,
    },
    Shutdown {
        reply: Sender<Result<()>>,
    },
}

/// UI-facing handle. Dropping it flushes and stops the writer thread.
pub struct ProjectHandle {
    sender: SyncSender<Command>,
    shared: Arc<Shared>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl ProjectHandle {
    /// Start a new temporary, in-memory project with the default retention caps.
    #[allow(dead_code)] // Used by unit tests; the app supplies explicit CLI limits.
    pub fn temporary() -> Result<Self> {
        Self::temporary_with_retention(RetentionLimits::default())
    }

    /// Start a new temporary project with explicit aggregate retention limits.
    pub fn temporary_with_retention(limits: RetentionLimits) -> Result<Self> {
        let meta = ProjectMeta::new(new_project_id());
        let db = backend::create_memory(&meta)?;
        Self::from_database(db, ProjectMode::Temporary, limits)
    }

    fn from_database(
        db: Database,
        mode: ProjectMode,
        temporary_retention: RetentionLimits,
    ) -> Result<Self> {
        // Re-read the metadata written by the backend so the service and the
        // file can never disagree about counters or identity.
        let read = db.begin_read()?;
        let meta = schema::read_meta(&read)?;
        drop(read);
        let db = Arc::new(db);
        let shared = Shared::new(db.clone(), &meta, mode.clone());
        let (sender, receiver) = mpsc::sync_channel(COMMAND_QUEUE);
        let thread = {
            let shared = shared.clone();
            thread::Builder::new()
                .name("perseid-project-writer".to_string())
                .spawn(move || {
                    run_writer(
                        receiver,
                        shared,
                        db,
                        meta,
                        mode.clone(),
                        temporary_retention,
                    )
                })
                .map_err(ProjectError::Io)?
        };
        Ok(Self {
            sender,
            shared,
            thread: Mutex::new(Some(thread)),
        })
    }

    pub fn status(&self) -> ProjectStatus {
        self.shared.status()
    }

    /// Assign stable project IDs and enqueue the event for durable storage.
    ///
    /// The returned event is the one the UI should apply to its bounded cache.
    pub fn capture(&self, event: &ProxyEvent) -> CaptureOutcome {
        if let ProxyEvent::Started { request, .. } = event
            && request.method.eq_ignore_ascii_case("CONNECT")
        {
            return CaptureOutcome { event: None };
        }
        let normalized = match event {
            ProxyEvent::Started { .. } => {
                let id = self.shared.next_http_id.fetch_add(1, Ordering::SeqCst);
                if let Ok(mut map) = self.shared.in_flight.lock() {
                    map.insert(event.id(), id);
                }
                normalized_event(event, id)
            }
            ProxyEvent::ResponseHead { .. } => {
                let mapped = self
                    .shared
                    .in_flight
                    .lock()
                    .ok()
                    .and_then(|map| map.get(&event.id()).copied());
                match mapped {
                    Some(id) => normalized_event(event, id),
                    None => {
                        return CaptureOutcome {
                            event: Some(event.clone()),
                        };
                    }
                }
            }
            ProxyEvent::Finished { .. } => {
                let mapped = self
                    .shared
                    .in_flight
                    .lock()
                    .ok()
                    .and_then(|mut map| map.remove(&event.id()));
                match mapped {
                    Some(id) => normalized_event(event, id),
                    None => {
                        return CaptureOutcome {
                            event: Some(event.clone()),
                        };
                    }
                }
            }
            ProxyEvent::WebSocketMessage {
                timestamp,
                connection,
                direction,
                opcode,
                payload,
                ..
            } => {
                let id = self.shared.next_ws_id.fetch_add(1, Ordering::SeqCst);
                let connection_id = {
                    let mut map = match self.shared.ws_connections.lock() {
                        Ok(map) => map,
                        Err(_) => {
                            return CaptureOutcome {
                                event: Some(event.clone()),
                            };
                        }
                    };
                    *map.entry(connection.id).or_insert_with(|| {
                        self.shared
                            .next_ws_connection_id
                            .fetch_add(1, Ordering::SeqCst)
                    })
                };
                let mut connection = connection.clone();
                connection.id = connection_id;
                ProxyEvent::WebSocketMessage {
                    id,
                    timestamp: *timestamp,
                    connection,
                    direction: *direction,
                    opcode: *opcode,
                    payload: payload.clone(),
                }
            }
        };

        match self.sender.try_send(Command::Capture(normalized.clone())) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                self.shared.pending_loss.fetch_add(1, Ordering::SeqCst);
            }
            Err(TrySendError::Disconnected(_)) => {}
        }
        CaptureOutcome {
            event: Some(normalized),
        }
    }

    /// Queue a workbench snapshot. Returns false when the queue is full, so
    /// the caller can retry instead of believing the state was persisted.
    pub fn set_workspace(&self, workspace: WorkspaceState) -> bool {
        self.sender
            .try_send(Command::SetWorkspace(Box::new(workspace)))
            .is_ok()
    }

    pub fn interrupt_in_flight(&self, reason: impl Into<String>) {
        self.shared.clear_mappings();
        let _ = self.sender.try_send(Command::InterruptInFlight {
            reason: reason.into(),
        });
    }

    /// Save the current project as `path` and make that file active.
    ///
    /// When `overwrite` is false an existing destination is refused. When it
    /// is true the complete replacement is written alongside the destination
    /// first, so a failed save leaves the previous file intact.
    pub fn save_as(&self, path: PathBuf, overwrite: bool) -> Receiver<Result<()>> {
        let (tx, rx) = mpsc::channel();
        self.send_control(
            Command::SaveAs {
                path,
                overwrite,
                reply: tx,
            },
            &rx,
        );
        rx
    }

    /// Write a copy of the current project to `path` without switching to it.
    pub fn save_copy(&self, path: PathBuf, overwrite: bool) -> Receiver<Result<()>> {
        let (tx, rx) = mpsc::channel();
        self.send_control(
            Command::SaveCopy {
                path,
                overwrite,
                reply: tx,
            },
            &rx,
        );
        rx
    }

    pub fn open(
        &self,
        path: PathBuf,
        ui_limit: usize,
        ui_bytes: usize,
    ) -> Receiver<Result<OpenedProject>> {
        let (tx, rx) = mpsc::channel();
        self.send_control(
            Command::Open {
                path,
                ui_limit,
                ui_bytes,
                reply: tx,
            },
            &rx,
        );
        rx
    }

    pub fn new_project(&self, ui_limit: usize, ui_bytes: usize) -> Receiver<Result<OpenedProject>> {
        let (tx, rx) = mpsc::channel();
        self.send_control(
            Command::New {
                ui_limit,
                ui_bytes,
                reply: tx,
            },
            &rx,
        );
        rx
    }

    pub fn flush(&self) -> Receiver<Result<CommitReceipt>> {
        let (tx, rx) = mpsc::channel();
        self.send_control(Command::Flush { reply: tx }, &rx);
        rx
    }

    pub fn clear_http(&self) -> Receiver<Result<CommitReceipt>> {
        let (tx, rx) = mpsc::channel();
        self.send_control(Command::ClearHttp(tx), &rx);
        rx
    }

    pub fn clear_ws(&self) -> Receiver<Result<CommitReceipt>> {
        let (tx, rx) = mpsc::channel();
        self.send_control(Command::ClearWs(tx), &rx);
        rx
    }

    fn send_control<T>(&self, command: Command, receiver: &Receiver<T>) {
        if self.sender.send(command).is_err() {
            // The receiver would otherwise wait forever. Dropping the reply
            // sender happens inside the failed command; callers treat a
            // disconnected channel as an error.
            log::error!("project writer is no longer running");
            let _ = receiver;
        }
    }

    pub fn shutdown(&self) {
        let (tx, rx) = mpsc::channel();
        if self.sender.send(Command::Shutdown { reply: tx }).is_ok() {
            let _ = rx.recv_timeout(Duration::from_secs(30));
        }
        if let Ok(mut thread) = self.thread.lock()
            && let Some(thread) = thread.take()
        {
            let _ = thread.join();
        }
    }

    #[allow(dead_code)] // Used by the paged-history UI and tests.
    pub fn list_http(&self, before: Option<HistoryCursor>, limit: usize) -> Result<Vec<HttpRow>> {
        let db = self.current_db()?;
        queries::list_http(&db, before, limit)
    }

    #[allow(dead_code)]
    pub fn list_ws(&self, before: Option<HistoryCursor>, limit: usize) -> Result<Vec<WsRow>> {
        let db = self.current_db()?;
        queries::list_ws(&db, before, limit)
    }

    #[allow(dead_code)]
    pub fn get_http(&self, id: u64) -> Result<Option<HistoryItem>> {
        let db = self.current_db()?;
        queries::get_http(&db, id)
    }

    #[allow(dead_code)]
    pub fn get_ws(&self, id: u64) -> Result<Option<WebSocketMessageItem>> {
        let db = self.current_db()?;
        queries::get_ws(&db, id)
    }

    #[allow(dead_code)]
    pub fn search_http(
        &self,
        needle: &str,
        include_bodies: bool,
        limit: usize,
    ) -> Result<Vec<u64>> {
        let db = self.current_db()?;
        queries::search_http(&db, needle, include_bodies, limit)
    }

    #[allow(dead_code)]
    fn current_db(&self) -> Result<Arc<Database>> {
        self.shared
            .database()
            .ok_or(ProjectError::ServiceUnavailable)
    }
}

impl Drop for ProjectHandle {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn new_project_id() -> String {
    uuid::Uuid::new_v4().to_string()
}
