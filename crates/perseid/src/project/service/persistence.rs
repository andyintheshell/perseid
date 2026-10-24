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
use super::{CommitReceipt, OpenedProject, ProjectMode, RetentionLimits};

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
    retention: Option<RetentionLimits>,
    events: &[ProxyEvent],
) -> Result<()> {
    // Metadata is staged until the transaction commits. A failed decode,
    // index update, or commit must leave the in-memory counters exactly as
    // they were before this batch.
    let mut next_meta = meta.clone();
    let tx = begin_write(db)?;
    {
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
                        let number = next_meta.next_http_number;
                        next_meta.next_http_number = next_meta.next_http_number.saturating_add(1);
                        next_meta.next_http_id = next_meta.next_http_id.max(id.saturating_add(1));
                        let record =
                            HttpExchangeRecord::started(*id, number, *timestamp, request.clone());
                        http.insert(*id, codec::encode(&record)?.as_slice())?;
                        by_time.insert((record.started_at_ms, *id), ())?;
                        let host = normalized_host(&record.request.host);
                        if !host.contains('\0') {
                            by_host.insert((host, *id), ())?;
                        }
                        in_flight.insert(*id, ())?;
                        next_meta.http_count = next_meta.http_count.saturating_add(1);
                    }
                    ProxyEvent::ResponseHead { id, head } => {
                        let Some(value) = http.get(*id)? else {
                            next_meta.event_loss_count =
                                next_meta.event_loss_count.saturating_add(1);
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
                            next_meta.event_loss_count =
                                next_meta.event_loss_count.saturating_add(1);
                            continue;
                        };
                        let mut record: HttpExchangeRecord = codec::decode(value.value())?;
                        drop(value);
                        record.duration_ms = Some(end.duration.as_millis() as u64);
                        record.request_body = BodyCaptureRecord::from_body(&end.request_body);
                        replace_http_body(
                            &mut bodies,
                            (*id, 0),
                            end.request_body.data(),
                            &mut next_meta.http_body_bytes,
                        )?;
                        match &end.result {
                            ExchangeResult::Response { body } => {
                                record.phase = HttpPhaseRecord::Completed {
                                    duration_ms: end.duration.as_millis() as u64,
                                };
                                record.response_body = BodyCaptureRecord::from_body(body);
                                replace_http_body(
                                    &mut bodies,
                                    (*id, 1),
                                    body.data(),
                                    &mut next_meta.http_body_bytes,
                                )?;
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
                            ExchangeResult::Tunnel { .. }
                            | ExchangeResult::InterceptedTls { .. } => {
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
                            ws_connections
                                .insert(connection.id, codec::encode(&record)?.as_slice())?;
                            next_meta.next_ws_connection_id = next_meta
                                .next_ws_connection_id
                                .max(connection.id.saturating_add(1));
                        }
                        let number = next_meta.next_ws_number;
                        next_meta.next_ws_number = next_meta.next_ws_number.saturating_add(1);
                        next_meta.next_ws_id = next_meta.next_ws_id.max(id.saturating_add(1));
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
                        replace_ws_payload(
                            &mut ws_payloads,
                            *id,
                            payload.data(),
                            &mut next_meta.ws_body_bytes,
                        )?;
                        ws_by_time.insert((record.timestamp_ms, *id), ())?;
                        ws_by_connection.insert((connection.id, *id), ())?;
                        next_meta.ws_count = next_meta.ws_count.saturating_add(1);
                    }
                }
                next_meta.next_event_sequence = next_meta.next_event_sequence.saturating_add(1);
                next_meta.last_committed_sequence = next_meta.next_event_sequence.saturating_sub(1);
            }
        }
        if let Some(limits) = retention {
            enforce_retention(&tx, &mut next_meta, limits)?;
        }
        let mut table = tx.open_table(schema::META)?;
        schema::write_counters(&mut table, &next_meta)?;
    }
    tx.commit()?;
    *meta = next_meta;
    Ok(())
}

fn replace_http_body(
    table: &mut redb::Table<'_, (u64, u8), &[u8]>,
    key: (u64, u8),
    bytes: &[u8],
    retained: &mut u64,
) -> Result<()> {
    if bytes.is_empty() {
        if let Some(old) = table.remove(key)? {
            *retained = retained.saturating_sub(old.value().len() as u64);
        }
    } else {
        if let Some(old) = table.insert(key, bytes)? {
            *retained = retained.saturating_sub(old.value().len() as u64);
        }
        *retained = retained.saturating_add(bytes.len() as u64);
    }
    Ok(())
}

fn replace_ws_payload(
    table: &mut redb::Table<'_, u64, &[u8]>,
    key: u64,
    bytes: &[u8],
    retained: &mut u64,
) -> Result<()> {
    if bytes.is_empty() {
        if let Some(old) = table.remove(key)? {
            *retained = retained.saturating_sub(old.value().len() as u64);
        }
    } else {
        if let Some(old) = table.insert(key, bytes)? {
            *retained = retained.saturating_sub(old.value().len() as u64);
        }
        *retained = retained.saturating_add(bytes.len() as u64);
    }
    Ok(())
}

/// Evict the oldest HTTP exchange or WebSocket message until both the record
/// and retained-body limits are satisfied. The newest record is always kept,
/// even when it alone exceeds a configured limit.
fn enforce_retention(
    tx: &redb::WriteTransaction,
    meta: &mut ProjectMeta,
    limits: RetentionLimits,
) -> Result<()> {
    if limits.max_records == 0 && limits.max_body_bytes == 0 {
        return Ok(());
    }
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

    loop {
        let records = meta.http_count.saturating_add(meta.ws_count);
        let bytes = meta.http_body_bytes.saturating_add(meta.ws_body_bytes);
        let over_count = limits.count_exceeded(records);
        let over_bytes = limits.bytes_exceeded(bytes);
        // Keep at least the newest record, mirroring the bounded UI cache.
        if (!over_count && !over_bytes) || records <= 1 {
            break;
        }

        let oldest_http = by_time.first()?.map(|(key, _)| key.value());
        let oldest_ws = ws_by_time.first()?.map(|(key, _)| key.value());
        let evict_http = match (oldest_http, oldest_ws) {
            (Some((http_time, _)), Some((ws_time, _))) => http_time <= ws_time,
            (Some(_), None) => true,
            (None, Some(_)) => false,
            (None, None) => break,
        };

        if evict_http {
            let Some((timestamp, id)) = oldest_http else {
                break;
            };
            by_time.remove((timestamp, id))?;
            let Some(value) = http.get(id)? else {
                // Drop dangling index entries without corrupting the counters.
                continue;
            };
            let record: HttpExchangeRecord = codec::decode(value.value())?;
            drop(value);
            let host = normalized_host(&record.request.host);
            if !host.contains('\0') {
                by_host.remove((host, id))?;
            }
            if let Some(response) = record.response {
                by_status.remove((response.status, id))?;
            }
            let request_bytes = bodies
                .remove((id, 0))?
                .map(|body| body.value().len() as u64)
                .unwrap_or(0);
            let response_bytes = bodies
                .remove((id, 1))?
                .map(|body| body.value().len() as u64)
                .unwrap_or(0);
            http.remove(id)?;
            in_flight.remove(id)?;
            meta.http_body_bytes = meta
                .http_body_bytes
                .saturating_sub(request_bytes.saturating_add(response_bytes));
            meta.http_count = meta.http_count.saturating_sub(1);
            meta.retention_evictions = meta.retention_evictions.saturating_add(1);
        } else {
            let Some((timestamp, id)) = oldest_ws else {
                break;
            };
            ws_by_time.remove((timestamp, id))?;
            let Some(value) = ws_messages.get(id)? else {
                continue;
            };
            let record: WsMessageRecord = codec::decode(value.value())?;
            drop(value);
            ws_by_connection.remove((record.connection_id, id))?;
            let payload_bytes = ws_payloads
                .remove(id)?
                .map(|payload| payload.value().len() as u64)
                .unwrap_or(0);
            ws_messages.remove(id)?;
            let connection_still_used = ws_by_connection
                .range((record.connection_id, 0)..=(record.connection_id, u64::MAX))?
                .next()
                .is_some();
            if !connection_still_used {
                ws_connections.remove(record.connection_id)?;
            }
            meta.ws_body_bytes = meta.ws_body_bytes.saturating_sub(payload_bytes);
            meta.ws_count = meta.ws_count.saturating_sub(1);
            meta.retention_evictions = meta.retention_evictions.saturating_add(1);
        }
    }
    Ok(())
}

fn normalized_host(host: &str) -> String {
    host.trim_end_matches('.').to_ascii_lowercase()
}

/// Chunk size for workspace JSON that exceeds one record. Retained result
/// bodies make the workspace much larger than ordinary metadata records.
const WORKSPACE_CHUNK_BYTES: usize = 8 * 1024 * 1024;
/// Read cap for an assembled workspace. This is deliberately far above the
/// per-record limit while still bounding memory for a corrupt file.
const MAX_WORKSPACE_BYTES: usize = 512 * 1024 * 1024;
const WORKSPACE_CHUNK_PREFIX: &str = "workspace/v1/chunk/";
const WORKSPACE_CHUNK_MAGIC: &[u8] = b"perseid-workspace-chunks-v1\n";

fn workspace_chunk_key(index: u32) -> String {
    format!("{WORKSPACE_CHUNK_PREFIX}{index:08}")
}

pub(super) fn write_workspace(db: &Database, bytes: &[u8]) -> Result<()> {
    write_workspace_chunks(db, bytes, codec::MAX_RECORD_BYTES, WORKSPACE_CHUNK_BYTES)
}

fn write_workspace_chunks(
    db: &Database,
    bytes: &[u8],
    inline_limit: usize,
    chunk_bytes: usize,
) -> Result<()> {
    debug_assert!(chunk_bytes > 0);
    if bytes.len() > MAX_WORKSPACE_BYTES {
        return Err(ProjectError::RecordTooLarge {
            size: bytes.len(),
            limit: MAX_WORKSPACE_BYTES,
        });
    }
    let tx = begin_write(db)?;
    {
        let mut table = tx.open_table(schema::WORKSPACE)?;
        let chunk_count = if bytes.len() <= inline_limit {
            table.insert(schema::WORKSPACE_KEY, bytes)?;
            0
        } else {
            let count = bytes.len().div_ceil(chunk_bytes);
            let count = u32::try_from(count).map_err(|_| ProjectError::RecordTooLarge {
                size: bytes.len(),
                limit: MAX_WORKSPACE_BYTES,
            })?;
            let mut manifest = Vec::with_capacity(WORKSPACE_CHUNK_MAGIC.len() + 12);
            manifest.extend_from_slice(WORKSPACE_CHUNK_MAGIC);
            manifest.extend_from_slice(&count.to_be_bytes());
            manifest.extend_from_slice(&(bytes.len() as u64).to_be_bytes());
            table.insert(schema::WORKSPACE_KEY, manifest.as_slice())?;
            for (index, chunk) in bytes.chunks(chunk_bytes).enumerate() {
                table.insert(workspace_chunk_key(index as u32).as_str(), chunk)?;
            }
            count
        };
        // Drop chunks left over from an earlier, larger snapshot.
        let stale: Vec<String> = table
            .iter()?
            .filter_map(|entry| {
                let (key, _) = entry.ok()?;
                let key = key.value();
                let index = key
                    .strip_prefix(WORKSPACE_CHUNK_PREFIX)?
                    .parse::<u32>()
                    .ok()?;
                (index >= chunk_count).then(|| key.to_string())
            })
            .collect();
        for key in stale {
            table.remove(key.as_str())?;
        }
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
    let bytes = value.value();
    let workspace: WorkspaceState = if let Some(manifest) =
        bytes.strip_prefix(WORKSPACE_CHUNK_MAGIC)
    {
        if manifest.len() < 12 {
            return Err(ProjectError::InvalidProject(
                "workspace manifest is truncated".to_string(),
            ));
        }
        let count = u32::from_be_bytes(manifest[0..4].try_into().expect("checked manifest length"));
        let length =
            u64::from_be_bytes(manifest[4..12].try_into().expect("checked manifest length"));
        let length = usize::try_from(length).map_err(|_| {
            ProjectError::InvalidProject("workspace length does not fit in memory".to_string())
        })?;
        if length > MAX_WORKSPACE_BYTES {
            return Err(ProjectError::RecordTooLarge {
                size: length,
                limit: MAX_WORKSPACE_BYTES,
            });
        }
        if u64::from(count) > length as u64 + 1 {
            return Err(ProjectError::InvalidProject(
                "workspace manifest has an invalid chunk count".to_string(),
            ));
        }
        let mut data = Vec::with_capacity(length);
        for index in 0..count {
            let key = workspace_chunk_key(index);
            let Some(chunk) = table.get(key.as_str())? else {
                return Err(ProjectError::InvalidProject(format!(
                    "workspace chunk {index} is missing"
                )));
            };
            data.extend_from_slice(chunk.value());
        }
        if data.len() != length {
            return Err(ProjectError::InvalidProject(format!(
                "workspace is {} bytes; expected {length}",
                data.len()
            )));
        }
        codec::decode_large(&data)?
    } else {
        codec::decode(bytes)?
    };
    if workspace.version > WorkspaceState::CURRENT_VERSION {
        return Err(ProjectError::UnsupportedRecord {
            found: workspace.version,
            supported: WorkspaceState::CURRENT_VERSION,
        });
    }
    Ok(workspace)
}

pub(super) fn clear_http(db: &Database, meta: &mut ProjectMeta) -> Result<CommitReceipt> {
    let mut next_meta = meta.clone();
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
        next_meta.http_count = 0;
        next_meta.http_body_bytes = 0;
        next_meta.next_http_number = 1;
        next_meta.last_committed_sequence = next_meta.next_event_sequence.saturating_sub(1);
        let mut table = tx.open_table(schema::META)?;
        schema::write_counters(&mut table, &next_meta)?;
    }
    tx.commit()?;
    *meta = next_meta;
    Ok(receipt(meta))
}

pub(super) fn clear_ws(db: &Database, meta: &mut ProjectMeta) -> Result<CommitReceipt> {
    let mut next_meta = meta.clone();
    let tx = begin_write(db)?;
    {
        tx.open_table(schema::WS_MESSAGES)?.retain(|_, _| false)?;
        tx.open_table(schema::WS_PAYLOADS)?.retain(|_, _| false)?;
        tx.open_table(schema::WS_BY_TIME)?.retain(|_, _| false)?;
        tx.open_table(schema::WS_BY_CONNECTION)?
            .retain(|_, _| false)?;
        tx.open_table(schema::WS_CONNECTIONS)?
            .retain(|_, _| false)?;
        next_meta.ws_count = 0;
        next_meta.ws_body_bytes = 0;
        next_meta.next_ws_number = 1;
        next_meta.last_committed_sequence = next_meta.next_event_sequence.saturating_sub(1);
        let mut table = tx.open_table(schema::META)?;
        schema::write_counters(&mut table, &next_meta)?;
    }
    tx.commit()?;
    *meta = next_meta;
    Ok(receipt(meta))
}

pub(super) fn interrupt_in_flight(
    db: &Database,
    meta: &mut ProjectMeta,
    reason: &str,
) -> Result<()> {
    let mut next_meta = meta.clone();
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
            next_meta.event_loss_count = next_meta.event_loss_count.saturating_add(count);
            next_meta.last_committed_sequence = next_meta.next_event_sequence.saturating_sub(1);
            let mut table = tx.open_table(schema::META)?;
            schema::write_counters(&mut table, &next_meta)?;
        }
    }
    tx.commit()?;
    if count > 0 {
        *meta = next_meta;
    }
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
    let opened = build_snapshot(&db, ui_limit, ui_bytes)?;
    Ok((db, meta, opened))
}

pub(super) fn build_snapshot(
    db: &Database,
    ui_limit: usize,
    ui_bytes: usize,
) -> Result<OpenedProject> {
    let limit = ui_limit.max(1);
    let (http_budget, ws_budget) = snapshot_budgets(ui_bytes);
    let (http, http_total) =
        queries::load_http_snapshot(db, limit, http_budget.max(1)).map_err(|source| {
            ProjectError::Snapshot {
                component: "HTTP history",
                source: Box::new(source),
            }
        })?;
    let (ws, ws_total) =
        queries::load_ws_snapshot(db, limit, ws_budget.max(1)).map_err(|source| {
            ProjectError::Snapshot {
                component: "WebSocket history",
                source: Box::new(source),
            }
        })?;
    let workspace = read_workspace(db).map_err(|source| ProjectError::Snapshot {
        component: "workspace state",
        source: Box::new(source),
    })?;
    Ok(OpenedProject {
        http,
        ws,
        workspace,
        http_total,
        ws_total,
    })
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
    fn workspace_chunks_round_trip_and_replace_stale_entries() {
        let meta = ProjectMeta::new("workspace-chunks".to_string());
        let db = backend::create_memory(&meta).expect("workspace chunks project");
        let workspace = WorkspaceState {
            ui: crate::project::UiWorkspace {
                http_filter: "filter".repeat(200),
                ..Default::default()
            },
            ..Default::default()
        };
        let bytes = codec::encode_large(&workspace).expect("encode workspace");
        write_workspace_chunks(&db, &bytes, 64, 32).expect("write chunked workspace");

        let (chunked, chunk_keys) = {
            let read = db.begin_read().expect("read transaction");
            let table = read.open_table(schema::WORKSPACE).expect("workspace table");
            let manifest = table
                .get(schema::WORKSPACE_KEY)
                .expect("read manifest")
                .expect("manifest entry");
            let chunked = manifest.value().starts_with(WORKSPACE_CHUNK_MAGIC);
            let chunk_keys = table
                .iter()
                .expect("iterate workspace")
                .filter_map(|entry| {
                    let (key, _) = entry.ok()?;
                    key.value()
                        .starts_with(WORKSPACE_CHUNK_PREFIX)
                        .then_some(())
                })
                .count();
            (chunked, chunk_keys)
        };
        assert!(chunked);
        assert!(chunk_keys > 1);

        let restored = read_workspace(&db).expect("read chunked workspace");
        assert_eq!(restored, workspace);

        // Every field has a serde default, so `{}` is a valid empty workspace.
        write_workspace_chunks(&db, b"{}", 64, 32).expect("replace with inline workspace");
        let stale_chunks = {
            let read = db.begin_read().expect("read transaction");
            let table = read.open_table(schema::WORKSPACE).expect("workspace table");
            table
                .iter()
                .expect("iterate workspace")
                .filter_map(|entry| {
                    let (key, _) = entry.ok()?;
                    key.value()
                        .starts_with(WORKSPACE_CHUNK_PREFIX)
                        .then_some(())
                })
                .count()
        };
        assert_eq!(
            stale_chunks, 0,
            "replacing a chunked workspace removes every stale chunk"
        );
        assert_eq!(
            read_workspace(&db).expect("read inline workspace"),
            WorkspaceState::default()
        );
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
