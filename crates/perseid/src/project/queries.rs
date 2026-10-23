//! Read-only project queries.
//!
//! Every function opens a short-lived redb read transaction. Callers must not
//! retain a transaction for the lifetime of a UI panel or search window.
//!
//! The paged row/cursor queries are the storage side of history paging. The
//! current UI loads a bounded newest-first snapshot and fetches by ID for
//! tests; wiring the table to these accessors is the next step, so keep them
//! even while the initial UI only uses the snapshot helpers.

#![allow(dead_code)]

use redb::{Database, ReadableDatabase, ReadableTable, ReadableTableMetadata};

use crate::history::HistoryItem;
use crate::proxy::WebSocketConnectionInfo;
use crate::websocket_history::WebSocketMessageItem;

use super::codec;
use super::records::{HttpExchangeRecord, WsConnectionRecord, WsMessageRecord};
use super::schema;
use super::{ProjectError, Result};

/// Lightweight row returned by paged history queries.
#[derive(Clone, Debug)]
pub struct HttpRow {
    pub id: u64,
    pub number: u64,
    pub timestamp_ms: u64,
    pub method: String,
    pub scheme: String,
    pub host: String,
    pub path: String,
    pub uri: String,
    pub status: Option<u16>,
    pub duration_ms: Option<u64>,
    pub request_len: u64,
    pub response_len: u64,
    pub in_flight: bool,
}

#[derive(Clone, Debug)]
pub struct WsRow {
    pub id: u64,
    pub number: u64,
    pub timestamp_ms: u64,
    pub connection: WebSocketConnectionInfo,
    pub payload_len: u64,
    pub text: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HistoryCursor {
    pub timestamp_ms: u64,
    pub id: u64,
}

/// Load newest HTTP records, oldest first in the returned vector.
///
/// `max_bytes` bounds the captured body bytes loaded into the UI cache. Rows
/// beyond the budget are left in the project and can be fetched by ID or by a
/// later page query.
pub fn load_http_snapshot(
    db: &Database,
    limit: usize,
    max_bytes: usize,
) -> Result<(Vec<HistoryItem>, u64)> {
    let tx = db.begin_read()?;
    let table = tx.open_table(schema::HTTP_BY_TIME)?;
    let total = table.len()?;
    let mut selected = Vec::new();
    let mut bytes = 0usize;
    for entry in table.iter()?.rev() {
        if selected.len() >= limit {
            break;
        }
        let (key, _) = entry?;
        let (_, id) = key.value();
        let Some(record) = read_http_record(&tx, id)? else {
            continue;
        };
        let (request_body, response_body) = read_http_bodies(&tx, id)?;
        let row_bytes = request_body.len() + response_body.len();
        if !selected.is_empty() && bytes.saturating_add(row_bytes) > max_bytes {
            break;
        }
        bytes = bytes.saturating_add(row_bytes);
        selected.push(record.to_history_item(request_body, response_body));
    }
    selected.reverse();
    Ok((selected, total))
}

/// Load newest WebSocket messages, oldest first in the returned vector.
pub fn load_ws_snapshot(
    db: &Database,
    limit: usize,
    max_bytes: usize,
) -> Result<(Vec<WebSocketMessageItem>, u64)> {
    let tx = db.begin_read()?;
    let table = tx.open_table(schema::WS_BY_TIME)?;
    let total = table.len()?;
    let connections = tx.open_table(schema::WS_CONNECTIONS)?;
    let mut selected = Vec::new();
    let mut bytes = 0usize;
    for entry in table.iter()?.rev() {
        if selected.len() >= limit {
            break;
        }
        let (key, _) = entry?;
        let (_, id) = key.value();
        let Some(record) = read_ws_message(&tx, id)? else {
            continue;
        };
        let payload = read_ws_payload(&tx, id)?;
        if !selected.is_empty() && bytes.saturating_add(payload.len()) > max_bytes {
            break;
        }
        bytes = bytes.saturating_add(payload.len());
        let connection = read_ws_connection_from(&connections, record.connection_id)?
            .unwrap_or_else(|| placeholder_connection(record.connection_id));
        selected.push(record.to_history_item(connection, payload));
    }
    selected.reverse();
    Ok((selected, total))
}

pub fn get_http(db: &Database, id: u64) -> Result<Option<HistoryItem>> {
    let tx = db.begin_read()?;
    let Some(record) = read_http_record(&tx, id)? else {
        return Ok(None);
    };
    let (request_body, response_body) = read_http_bodies(&tx, id)?;
    Ok(Some(record.to_history_item(request_body, response_body)))
}

pub fn get_ws(db: &Database, id: u64) -> Result<Option<WebSocketMessageItem>> {
    let tx = db.begin_read()?;
    let Some(record) = read_ws_message(&tx, id)? else {
        return Ok(None);
    };
    let payload = read_ws_payload(&tx, id)?;
    let connection = read_ws_connection(&tx, record.connection_id)?
        .unwrap_or_else(|| placeholder_connection(record.connection_id));
    Ok(Some(record.to_history_item(connection, payload)))
}

pub fn list_http(
    db: &Database,
    before: Option<HistoryCursor>,
    limit: usize,
) -> Result<Vec<HttpRow>> {
    let limit = limit.clamp(1, 1000);
    let tx = db.begin_read()?;
    let table = tx.open_table(schema::HTTP_BY_TIME)?;
    let mut rows = Vec::new();
    let iter = match before {
        Some(cursor) => table.range(..(cursor.timestamp_ms, cursor.id))?.rev(),
        None => table.iter()?.rev(),
    };
    for entry in iter {
        if rows.len() >= limit {
            break;
        }
        let (key, _) = entry?;
        let (timestamp_ms, id) = key.value();
        if let Some(record) = read_http_record(&tx, id)? {
            rows.push(http_row(&record, timestamp_ms));
        }
    }
    Ok(rows)
}

pub fn list_ws(db: &Database, before: Option<HistoryCursor>, limit: usize) -> Result<Vec<WsRow>> {
    let limit = limit.clamp(1, 1000);
    let tx = db.begin_read()?;
    let table = tx.open_table(schema::WS_BY_TIME)?;
    let connections = tx.open_table(schema::WS_CONNECTIONS)?;
    let mut rows = Vec::new();
    let iter = match before {
        Some(cursor) => table.range(..(cursor.timestamp_ms, cursor.id))?.rev(),
        None => table.iter()?.rev(),
    };
    for entry in iter {
        if rows.len() >= limit {
            break;
        }
        let (key, _) = entry?;
        let (timestamp_ms, id) = key.value();
        if let Some(record) = read_ws_message(&tx, id)? {
            let connection = read_ws_connection_from(&connections, record.connection_id)?
                .unwrap_or_else(|| placeholder_connection(record.connection_id));
            rows.push(WsRow {
                id,
                number: record.display_number,
                timestamp_ms,
                connection,
                payload_len: record.payload.observed_len,
                text: matches!(record.opcode, super::records::WsOpcodeRecord::Text),
            });
        }
    }
    Ok(rows)
}

/// Search metadata first, optionally scanning stored body bytes.
pub fn search_http(
    db: &Database,
    needle: &str,
    include_bodies: bool,
    limit: usize,
) -> Result<Vec<u64>> {
    let needle = needle.to_lowercase();
    if needle.is_empty() {
        return Ok(Vec::new());
    }
    let limit = limit.clamp(1, 10_000);
    let tx = db.begin_read()?;
    let table = tx.open_table(schema::HTTP_EXCHANGES)?;
    let mut matches = Vec::new();
    for entry in table.iter()? {
        let (key, value) = entry?;
        let record: HttpExchangeRecord = codec::decode(value.value())?;
        let haystack = format!(
            "{} {} {} {} {}",
            record.request.method,
            record.request.host,
            record.request.path,
            record.request.uri,
            record
                .response
                .as_ref()
                .map(|head| head.status.to_string())
                .unwrap_or_default()
        )
        .to_lowercase();
        let mut matched = haystack.contains(&needle);
        if !matched && include_bodies {
            let (request, response) = read_http_bodies(&tx, key.value())?;
            matched = contains_ascii_case_insensitive(&request, &needle)
                || contains_ascii_case_insensitive(&response, &needle);
        }
        if matched {
            matches.push(key.value());
            if matches.len() >= limit {
                break;
            }
        }
    }
    Ok(matches)
}

fn contains_ascii_case_insensitive(haystack: &[u8], needle: &str) -> bool {
    let needle = needle.as_bytes();
    if needle.is_empty() || needle.len() > haystack.len() {
        return false;
    }
    haystack
        .windows(needle.len())
        .any(|window| window.eq_ignore_ascii_case(needle))
}

fn http_row(record: &HttpExchangeRecord, timestamp_ms: u64) -> HttpRow {
    HttpRow {
        id: record.id,
        number: record.display_number,
        timestamp_ms,
        method: record.request.method.clone(),
        scheme: record.request.scheme.clone(),
        host: record.request.host.clone(),
        path: record.request.path.clone(),
        uri: record.request.uri.clone(),
        status: record.response.as_ref().map(|head| head.status),
        duration_ms: record.duration_ms,
        request_len: record.request_body.observed_len,
        response_len: record.response_body.observed_len,
        in_flight: matches!(record.phase, super::records::HttpPhaseRecord::InFlight),
    }
}

pub fn read_http_record(tx: &redb::ReadTransaction, id: u64) -> Result<Option<HttpExchangeRecord>> {
    let table = tx.open_table(schema::HTTP_EXCHANGES)?;
    let Some(value) = table.get(id)? else {
        return Ok(None);
    };
    let record: HttpExchangeRecord = codec::decode(value.value())?;
    ensure_valid_record_version(record.record_version)?;
    Ok(Some(record))
}

pub fn read_http_bodies(tx: &redb::ReadTransaction, id: u64) -> Result<(Vec<u8>, Vec<u8>)> {
    let table = tx.open_table(schema::HTTP_BODIES)?;
    let request = table
        .get((id, 0))?
        .map(|value| value.value().to_vec())
        .unwrap_or_default();
    let response = table
        .get((id, 1))?
        .map(|value| value.value().to_vec())
        .unwrap_or_default();
    Ok((request, response))
}

pub fn read_ws_message(tx: &redb::ReadTransaction, id: u64) -> Result<Option<WsMessageRecord>> {
    let table = tx.open_table(schema::WS_MESSAGES)?;
    let Some(value) = table.get(id)? else {
        return Ok(None);
    };
    let record: WsMessageRecord = codec::decode(value.value())?;
    ensure_valid_record_version(record.record_version)?;
    Ok(Some(record))
}

pub fn read_ws_payload(tx: &redb::ReadTransaction, id: u64) -> Result<Vec<u8>> {
    let table = tx.open_table(schema::WS_PAYLOADS)?;
    Ok(table
        .get(id)?
        .map(|value| value.value().to_vec())
        .unwrap_or_default())
}

pub fn read_ws_connection(
    tx: &redb::ReadTransaction,
    id: u64,
) -> Result<Option<WebSocketConnectionInfo>> {
    let table = tx.open_table(schema::WS_CONNECTIONS)?;
    read_ws_connection_from(&table, id)
}

fn read_ws_connection_from(
    table: &redb::ReadOnlyTable<u64, &[u8]>,
    id: u64,
) -> Result<Option<WebSocketConnectionInfo>> {
    let Some(value) = table.get(id)? else {
        return Ok(None);
    };
    let record: WsConnectionRecord = codec::decode(value.value())?;
    ensure_valid_record_version(record.record_version)?;
    Ok(Some(record.to_info()))
}

fn placeholder_connection(id: u64) -> WebSocketConnectionInfo {
    WebSocketConnectionInfo {
        id,
        url: "<connection metadata unavailable>".to_string(),
        host: String::new(),
        tls: false,
        listener_port: 0,
        protocol: None,
    }
}

pub fn count_http(db: &Database) -> Result<u64> {
    let tx = db.begin_read()?;
    let table = tx.open_table(schema::HTTP_EXCHANGES)?;
    Ok(table.len()?)
}

pub fn count_ws(db: &Database) -> Result<u64> {
    let tx = db.begin_read()?;
    let table = tx.open_table(schema::WS_MESSAGES)?;
    Ok(table.len()?)
}

pub fn ensure_valid_record_version(record_version: u16) -> Result<()> {
    if record_version > schema::RECORD_VERSION {
        return Err(ProjectError::UnsupportedRecord {
            found: record_version,
            supported: schema::RECORD_VERSION,
        });
    }
    Ok(())
}
