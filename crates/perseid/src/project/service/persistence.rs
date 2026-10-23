//! redb operations executed by the writer thread.

use std::path::Path;
use std::sync::Arc;

use redb::{Database, Durability, ReadableDatabase, ReadableTable};

use crate::proxy::{ExchangeResult, ProxyEvent};

use super::super::backend;
use super::super::codec;
use super::super::queries;
use super::super::records::{
    BodyCaptureRecord, HttpExchangeRecord, HttpPhaseRecord, WorkspaceState, WsConnectionRecord,
    WsMessageRecord,
};
use super::super::schema::{self, ProjectMeta, STATE_MIGRATING, STATE_READY};
use super::super::{ProjectError, Result};
use super::shared::Shared;
use super::{CommitReceipt, OpenedProject, ProjectMode};

pub(super) fn receipt(meta: &ProjectMeta) -> CommitReceipt {
    CommitReceipt {
        sequence: meta.last_committed_sequence,
        http_count: meta.http_count,
        ws_count: meta.ws_count,
    }
}

fn begin_write(db: &Database) -> Result<redb::WriteTransaction> {
    let mut tx = db.begin_write()?;
    tx.set_durability(Durability::Immediate)?;
    Ok(tx)
}

pub(super) fn apply_capture_batch(
    db: &Database,
    meta: &mut ProjectMeta,
    events: &[ProxyEvent],
) -> Result<()> {
    let tx = begin_write(db)?;
    {
        let mut http = tx.open_table(schema::HTTP_EXCHANGES)?;
        let mut bodies = tx.open_table(schema::HTTP_BODIES)?;
        let mut by_time = tx.open_table(schema::HTTP_BY_TIME)?;
        let mut by_host = tx.open_table(schema::HTTP_BY_HOST)?;
        let mut by_status = tx.open_table(schema::HTTP_BY_STATUS)?;
        let mut in_flight = tx.open_table(schema::HTTP_IN_FLIGHT)?;
        let mut ws_messages = tx.open_table(schema::WS_MESSAGES)?;
        let mut ws_payloads = tx.open_table(schema::WS_PAYLOADS)?;
        let mut ws_by_time = tx.open_table(schema::WS_BY_TIME)?;
        let mut ws_by_connection = tx.open_table(schema::WS_BY_CONNECTION)?;
        let mut ws_connections = tx.open_table(schema::WS_CONNECTIONS)?;

        for event in events {
            match event {
                ProxyEvent::Started {
                    id,
                    timestamp,
                    request,
                } => {
                    if http.get(*id)?.is_some() {
                        continue;
                    }
                    let number = meta.next_http_number;
                    meta.next_http_number = meta.next_http_number.saturating_add(1);
                    meta.next_http_id = meta.next_http_id.max(id.saturating_add(1));
                    let record =
                        HttpExchangeRecord::started(*id, number, *timestamp, request.clone());
                    http.insert(*id, codec::encode(&record)?.as_slice())?;
                    by_time.insert((record.started_at_ms, *id), ())?;
                    let host = normalized_host(&record.request.host);
                    if !host.contains('\0') {
                        by_host.insert((host, *id), ())?;
                    }
                    in_flight.insert(*id, ())?;
                    meta.http_count = meta.http_count.saturating_add(1);
                }
                ProxyEvent::ResponseHead { id, head } => {
                    let Some(value) = http.get(*id)? else {
                        meta.event_loss_count = meta.event_loss_count.saturating_add(1);
                        continue;
                    };
                    let mut record: HttpExchangeRecord = codec::decode(value.value())?;
                    drop(value);
                    if let Some(old) = record.response.as_ref()
                        && old.status != head.status
                    {
                        by_status.remove((old.status, *id))?;
                    }
                    record.response = Some(head.clone());
                    by_status.insert((head.status, *id), ())?;
                    http.insert(*id, codec::encode(&record)?.as_slice())?;
                }
                ProxyEvent::Finished { id, end } => {
                    let Some(value) = http.get(*id)? else {
                        meta.event_loss_count = meta.event_loss_count.saturating_add(1);
                        continue;
                    };
                    let mut record: HttpExchangeRecord = codec::decode(value.value())?;
                    drop(value);
                    record.duration_ms = Some(end.duration.as_millis() as u64);
                    record.request_body = BodyCaptureRecord::from_body(&end.request_body);
                    if !end.request_body.data().is_empty() {
                        bodies.insert((*id, 0), end.request_body.data())?;
                    }
                    match &end.result {
                        ExchangeResult::Response { body } => {
                            record.phase = HttpPhaseRecord::Completed {
                                duration_ms: end.duration.as_millis() as u64,
                            };
                            record.response_body = BodyCaptureRecord::from_body(body);
                            if !body.data().is_empty() {
                                bodies.insert((*id, 1), body.data())?;
                            }
                        }
                        ExchangeResult::Failed { error } => {
                            record.phase = HttpPhaseRecord::Failed {
                                duration_ms: end.duration.as_millis() as u64,
                                error: error.clone(),
                            };
                        }
                        ExchangeResult::Dropped => {
                            record.phase = HttpPhaseRecord::Dropped {
                                duration_ms: end.duration.as_millis() as u64,
                            };
                        }
                        ExchangeResult::Tunnel { .. } | ExchangeResult::InterceptedTls { .. } => {
                            // CONNECT tunnels are filtered before reaching the
                            // project; keep a plain completed phase if one slips
                            // through a future caller.
                            record.phase = HttpPhaseRecord::Completed {
                                duration_ms: end.duration.as_millis() as u64,
                            };
                        }
                    }
                    http.insert(*id, codec::encode(&record)?.as_slice())?;
                    in_flight.remove(*id)?;
                }
                ProxyEvent::WebSocketMessage {
                    id,
                    timestamp,
                    connection,
                    direction,
                    opcode,
                    payload,
                } => {
                    if ws_messages.get(*id)?.is_some() {
                        continue;
                    }
                    if ws_connections.get(connection.id)?.is_none() {
                        let record = WsConnectionRecord::from_info(connection, connection.id);
                        ws_connections.insert(connection.id, codec::encode(&record)?.as_slice())?;
                        meta.next_ws_connection_id = meta
                            .next_ws_connection_id
                            .max(connection.id.saturating_add(1));
                    }
                    let number = meta.next_ws_number;
                    meta.next_ws_number = meta.next_ws_number.saturating_add(1);
                    meta.next_ws_id = meta.next_ws_id.max(id.saturating_add(1));
                    let record = WsMessageRecord::from_event(
                        *id,
                        number,
                        *timestamp,
                        connection.id,
                        *direction,
                        *opcode,
                        payload,
                    );
                    ws_messages.insert(*id, codec::encode(&record)?.as_slice())?;
                    if !payload.data().is_empty() {
                        ws_payloads.insert(*id, payload.data())?;
                    }
                    ws_by_time.insert((record.timestamp_ms, *id), ())?;
                    ws_by_connection.insert((connection.id, *id), ())?;
                    meta.ws_count = meta.ws_count.saturating_add(1);
                }
            }
            meta.next_event_sequence = meta.next_event_sequence.saturating_add(1);
            meta.last_committed_sequence = meta.next_event_sequence.saturating_sub(1);
        }
        let mut table = tx.open_table(schema::META)?;
        schema::write_counters(&mut table, meta)?;
    }
    tx.commit()?;
    Ok(())
}

fn normalized_host(host: &str) -> String {
    host.trim_end_matches('.').to_ascii_lowercase()
}

pub(super) fn write_workspace(db: &Database, bytes: &[u8]) -> Result<()> {
    let tx = begin_write(db)?;
    {
        let mut table = tx.open_table(schema::WORKSPACE)?;
        table.insert(schema::WORKSPACE_KEY, bytes)?;
    }
    tx.commit()?;
    Ok(())
}

fn read_workspace(db: &Database) -> Result<WorkspaceState> {
    let tx = db.begin_read()?;
    let table = tx.open_table(schema::WORKSPACE)?;
    let Some(value) = table.get(schema::WORKSPACE_KEY)? else {
        return Ok(WorkspaceState::default());
    };
    let workspace: WorkspaceState = codec::decode(value.value())?;
    if workspace.version > WorkspaceState::CURRENT_VERSION {
        return Err(ProjectError::UnsupportedRecord {
            found: workspace.version,
            supported: WorkspaceState::CURRENT_VERSION,
        });
    }
    Ok(workspace)
}

pub(super) fn clear_http(db: &Database, meta: &mut ProjectMeta) -> Result<CommitReceipt> {
    let tx = begin_write(db)?;
    {
        tx.open_table(schema::HTTP_EXCHANGES)?
            .retain(|_, _| false)?;
        tx.open_table(schema::HTTP_BODIES)?.retain(|_, _| false)?;
        tx.open_table(schema::HTTP_BY_TIME)?.retain(|_, _| false)?;
        tx.open_table(schema::HTTP_BY_HOST)?.retain(|_, _| false)?;
        tx.open_table(schema::HTTP_BY_STATUS)?
            .retain(|_, _| false)?;
        tx.open_table(schema::HTTP_IN_FLIGHT)?
            .retain(|_, _| false)?;
        meta.http_count = 0;
        meta.next_http_number = 1;
        meta.last_committed_sequence = meta.next_event_sequence.saturating_sub(1);
        let mut table = tx.open_table(schema::META)?;
        schema::write_counters(&mut table, meta)?;
    }
    tx.commit()?;
    Ok(receipt(meta))
}

pub(super) fn clear_ws(db: &Database, meta: &mut ProjectMeta) -> Result<CommitReceipt> {
    let tx = begin_write(db)?;
    {
        tx.open_table(schema::WS_MESSAGES)?.retain(|_, _| false)?;
        tx.open_table(schema::WS_PAYLOADS)?.retain(|_, _| false)?;
        tx.open_table(schema::WS_BY_TIME)?.retain(|_, _| false)?;
        tx.open_table(schema::WS_BY_CONNECTION)?
            .retain(|_, _| false)?;
        tx.open_table(schema::WS_CONNECTIONS)?
            .retain(|_, _| false)?;
        meta.ws_count = 0;
        meta.next_ws_number = 1;
        meta.last_committed_sequence = meta.next_event_sequence.saturating_sub(1);
        let mut table = tx.open_table(schema::META)?;
        schema::write_counters(&mut table, meta)?;
    }
    tx.commit()?;
    Ok(receipt(meta))
}

pub(super) fn interrupt_in_flight(
    db: &Database,
    meta: &mut ProjectMeta,
    reason: &str,
) -> Result<()> {
    let tx = begin_write(db)?;
    let count;
    {
        let in_flight = tx.open_table(schema::HTTP_IN_FLIGHT)?;
        let ids: Vec<u64> = in_flight
            .iter()?
            .filter_map(|item| item.ok().map(|(key, _)| key.value()))
            .collect();
        drop(in_flight);
        let mut http = tx.open_table(schema::HTTP_EXCHANGES)?;
        for id in &ids {
            let Some(value) = http.get(*id)? else {
                continue;
            };
            let mut record: HttpExchangeRecord = codec::decode(value.value())?;
            drop(value);
            let duration = now_ms().saturating_sub(record.started_at_ms);
            record.duration_ms = Some(duration);
            record.phase = HttpPhaseRecord::Interrupted {
                duration_ms: duration,
                reason: reason.to_string(),
            };
            http.insert(*id, codec::encode(&record)?.as_slice())?;
        }
        drop(http);
        tx.open_table(schema::HTTP_IN_FLIGHT)?
            .retain(|_, _| false)?;
        count = ids.len() as u64;
        if count > 0 {
            meta.event_loss_count = meta.event_loss_count.saturating_add(count);
            meta.last_committed_sequence = meta.next_event_sequence.saturating_sub(1);
            let mut table = tx.open_table(schema::META)?;
            schema::write_counters(&mut table, meta)?;
        }
    }
    tx.commit()?;
    Ok(())
}

pub(super) fn commit_counters(db: &Database, meta: &ProjectMeta) -> Result<()> {
    let tx = begin_write(db)?;
    {
        let mut table = tx.open_table(schema::META)?;
        schema::write_counters(&mut table, meta)?;
    }
    tx.commit()?;
    Ok(())
}

pub(super) fn save_as(
    db: &mut Arc<Database>,
    shared: &Shared,
    meta: &mut ProjectMeta,
    path: &Path,
) -> Result<()> {
    let read = db.begin_read()?;
    let source_meta = schema::read_meta(&read)?;
    drop(read);
    let destination = super::super::transfer::copy_to_new_file(db, path, &source_meta)?;
    let destination = Arc::new(destination);
    *db = destination.clone();
    shared.set_database(destination);
    shared.install_saved_meta(
        meta,
        ProjectMode::Persistent {
            path: path.to_path_buf(),
        },
    );
    Ok(())
}

pub(super) fn open_project(
    _current: &Database,
    path: &Path,
    ui_limit: usize,
    ui_bytes: usize,
) -> Result<(Arc<Database>, ProjectMeta, OpenedProject)> {
    let db = backend::open_file(path)?;
    schema::validate_tables(&db)?;
    let read = db.begin_read()?;
    let mut meta = schema::read_meta(&read)?;
    drop(read);
    match meta.state.as_str() {
        schema::STATE_INITIALIZING => {
            return Err(ProjectError::IncompleteProject(path.to_path_buf()));
        }
        STATE_MIGRATING => {
            return Err(ProjectError::InvalidProject(
                "the project was left in a migrating state".to_string(),
            ));
        }
        STATE_READY => {}
        other => {
            return Err(ProjectError::InvalidProject(format!(
                "unknown project state `{other}`"
            )));
        }
    }
    if meta.schema_version < schema::SCHEMA_VERSION {
        return Err(ProjectError::InvalidProject(format!(
            "schema {} requires migration; automatic migration is not available yet",
            meta.schema_version
        )));
    }
    interrupt_in_flight(
        &db,
        &mut meta,
        "application terminated before exchange completed",
    )?;
    let db = Arc::new(db);
    let opened = build_snapshot(&db, &meta, ui_limit, ui_bytes);
    Ok((db, meta, opened))
}

pub(super) fn build_snapshot(
    db: &Database,
    meta: &ProjectMeta,
    ui_limit: usize,
    ui_bytes: usize,
) -> OpenedProject {
    let limit = ui_limit.max(1);
    let (http_budget, ws_budget) = snapshot_budgets(ui_bytes);
    let (http, http_total) = queries::load_http_snapshot(db, limit, http_budget.max(1))
        .unwrap_or_else(|err| {
            log::warn!("could not load the HTTP snapshot: {err}");
            (Vec::new(), meta.http_count)
        });
    let (ws, ws_total) =
        queries::load_ws_snapshot(db, limit, ws_budget.max(1)).unwrap_or_else(|err| {
            log::warn!("could not load the WebSocket snapshot: {err}");
            (Vec::new(), meta.ws_count)
        });
    let workspace = read_workspace(db).unwrap_or_else(|err| {
        log::warn!("could not load project workspace state: {err}");
        WorkspaceState::default()
    });
    OpenedProject {
        http,
        ws,
        workspace,
        http_total,
        ws_total,
    }
}

fn now_ms() -> u64 {
    schema::now_ms()
}

/// Split the UI byte budget between the HTTP snapshot (three quarters) and the
/// WebSocket snapshot (the rest). Saturating arithmetic keeps absurd budgets
/// from wrapping.
fn snapshot_budgets(ui_bytes: usize) -> (usize, usize) {
    let http_budget = ui_bytes.saturating_mul(3) / 4;
    (http_budget, ui_bytes.saturating_sub(http_budget))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalized_hosts_lowercase_and_trim_trailing_dots() {
        assert_eq!(normalized_host("Example.COM."), "example.com");
        assert_eq!(normalized_host("example.com..."), "example.com");
        assert_eq!(normalized_host("127.0.0.1"), "127.0.0.1");
    }

    #[test]
    fn snapshot_budgets_divide_the_ui_budget_without_overflow() {
        assert_eq!(snapshot_budgets(0), (0, 0));
        assert_eq!(snapshot_budgets(100), (75, 25));
        let (http, ws) = snapshot_budgets(usize::MAX);
        assert_eq!(http, usize::MAX / 4);
        assert_eq!(http + ws, usize::MAX);
    }

    #[test]
    fn receipts_report_the_committed_counters() {
        let mut meta = ProjectMeta::new("receipt-test".to_string());
        meta.last_committed_sequence = 7;
        meta.http_count = 3;
        meta.ws_count = 2;
        let receipt = receipt(&meta);
        assert_eq!(receipt.sequence, 7);
        assert_eq!(receipt.http_count, 3);
        assert_eq!(receipt.ws_count, 2);
    }
}
