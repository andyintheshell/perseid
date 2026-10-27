//! Read-only project queries.
//!
//! Every function opens a short-lived redb read transaction. Callers must not
//! retain a transaction for the lifetime of a UI panel or search window.
//!
//! The paged row/cursor queries are the storage side of history paging. The
//! current history and site-map tables load a bounded newest-first snapshot
//! and fetch details by ID; Search uses [`scan_http`] and [`scan_ws`] to read
//! every stored capture in short-lived batches. Wiring the tables themselves
//! to these accessors is the next step, so keep them even while the initial UI
//! only uses the snapshot helpers.

#![allow(dead_code)]

use std::ops::Bound;

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

/// Number of index keys read per short-lived transaction during a stored scan.
const SCAN_BATCH: usize = 256;

/// Visit every stored HTTP exchange oldest-first.
///
/// The visitor receives the decoded record and its one-based position plus the
/// total row count, which lets a background search report progress. Returning
/// `false` stops the scan, for example after a cancellation or result cap.
/// Keys and records are read in bounded batches so a long search does not pin
/// one read transaction or database snapshot for its entire lifetime.
pub fn scan_http(
    db: &Database,
    mut visit: impl FnMut(HistoryItem, usize, usize) -> bool,
) -> Result<()> {
    let total = {
        let tx = db.begin_read()?;
        let table = tx.open_table(schema::HTTP_BY_TIME)?;
        table.len()? as usize
    };
    let mut after = None;
    let mut scanned = 0usize;
    loop {
        let tx = db.begin_read()?;
        let table = tx.open_table(schema::HTTP_BY_TIME)?;
        let start = after.map_or(Bound::Unbounded, Bound::Excluded);
        let mut keys = Vec::with_capacity(SCAN_BATCH);
        for entry in table.range((start, Bound::Unbounded))?.take(SCAN_BATCH) {
            let (key, _) = entry?;
            keys.push(key.value());
        }
        if keys.is_empty() {
            break;
        }
        let batch_len = keys.len();
        for (_, id) in &keys {
            scanned += 1;
            let Some(record) = read_http_record(&tx, *id)? else {
                continue;
            };
            let (request_body, response_body) = read_http_bodies(&tx, *id)?;
            let item = record.to_history_item(request_body, response_body);
            if !visit(item, scanned, total) {
                return Ok(());
            }
        }
        after = keys.last().copied();
        if batch_len < SCAN_BATCH {
            break;
        }
    }
    Ok(())
}

/// Visit every stored WebSocket message oldest-first. See [`scan_http`] for
/// the progress, cancellation, and batching contract.
pub fn scan_ws(
    db: &Database,
    mut visit: impl FnMut(WebSocketMessageItem, usize, usize) -> bool,
) -> Result<()> {
    let total = {
        let tx = db.begin_read()?;
        let table = tx.open_table(schema::WS_BY_TIME)?;
        table.len()? as usize
    };
    let mut after = None;
    let mut scanned = 0usize;
    loop {
        let tx = db.begin_read()?;
        let table = tx.open_table(schema::WS_BY_TIME)?;
        let start = after.map_or(Bound::Unbounded, Bound::Excluded);
        let mut keys = Vec::with_capacity(SCAN_BATCH);
        for entry in table.range((start, Bound::Unbounded))?.take(SCAN_BATCH) {
            let (key, _) = entry?;
            keys.push(key.value());
        }
        if keys.is_empty() {
            break;
        }
        let batch_len = keys.len();
        let connections = tx.open_table(schema::WS_CONNECTIONS)?;
        for (_, id) in &keys {
            scanned += 1;
            let Some(record) = read_ws_message(&tx, *id)? else {
                continue;
            };
            let payload = read_ws_payload(&tx, *id)?;
            let connection = read_ws_connection_from(&connections, record.connection_id)?
                .unwrap_or_else(|| placeholder_connection(record.connection_id));
            let item = record.to_history_item(connection, payload);
            if !visit(item, scanned, total) {
                return Ok(());
            }
        }
        after = keys.last().copied();
        if batch_len < SCAN_BATCH {
            break;
        }
    }
    Ok(())
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

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use crate::proxy::{RequestInfo, ResponseHead, WebSocketDirection, WebSocketOpcode};

    use super::super::backend;
    use super::super::records::{
        self, BodyCaptureRecord, HttpPhaseRecord, WsDirectionRecord, WsOpcodeRecord,
    };
    use super::*;

    fn memory_db() -> Database {
        backend::create_memory(&schema::ProjectMeta::new("queries-tests".to_string()))
            .expect("create in-memory project")
    }

    fn request_info(method: &str, host: &str, path: &str) -> RequestInfo {
        RequestInfo {
            method: method.to_string(),
            uri: format!("http://{host}{path}"),
            scheme: "http".to_string(),
            host: host.to_string(),
            path: path.to_string(),
            version: "HTTP/1.1".to_string(),
            headers: vec![("host".to_string(), host.to_string())],
        }
    }

    fn completed_http(
        id: u64,
        number: u64,
        timestamp_ms: u64,
        method: &str,
        host: &str,
        path: &str,
        status: u16,
    ) -> HttpExchangeRecord {
        let mut record = HttpExchangeRecord::started(
            id,
            number,
            records::system_time_from_ms(timestamp_ms),
            request_info(method, host, path),
        );
        record.response = Some(ResponseHead {
            version: "HTTP/1.1".to_string(),
            status,
            reason: String::new(),
            headers: vec![("content-type".to_string(), "text/plain".to_string())],
        });
        record.duration_ms = Some(3);
        record.phase = HttpPhaseRecord::Completed { duration_ms: 3 };
        record
    }

    fn insert_http(
        db: &Database,
        record: &HttpExchangeRecord,
        timestamp_ms: u64,
        request_body: &[u8],
        response_body: &[u8],
    ) {
        let tx = db.begin_write().expect("begin write transaction");
        {
            let encoded = codec::encode(record).expect("encode record");
            tx.open_table(schema::HTTP_EXCHANGES)
                .expect("exchanges table")
                .insert(record.id, encoded.as_slice())
                .expect("insert exchange");
            tx.open_table(schema::HTTP_BY_TIME)
                .expect("time index")
                .insert((timestamp_ms, record.id), ())
                .expect("insert time index");
            if !request_body.is_empty() {
                tx.open_table(schema::HTTP_BODIES)
                    .expect("bodies table")
                    .insert((record.id, 0u8), request_body)
                    .expect("insert request body");
            }
            if !response_body.is_empty() {
                tx.open_table(schema::HTTP_BODIES)
                    .expect("bodies table")
                    .insert((record.id, 1u8), response_body)
                    .expect("insert response body");
            }
        }
        tx.commit().expect("commit");
    }

    fn ws_record(
        id: u64,
        number: u64,
        timestamp_ms: u64,
        connection_id: u64,
        opcode: WsOpcodeRecord,
        observed_len: u64,
    ) -> WsMessageRecord {
        WsMessageRecord {
            record_version: schema::RECORD_VERSION,
            id,
            display_number: number,
            timestamp_ms,
            connection_id,
            direction: WsDirectionRecord::ToClient,
            opcode,
            payload: BodyCaptureRecord {
                observed_len,
                stored_len: observed_len,
                truncated: false,
                complete: true,
            },
        }
    }

    fn ws_connection(id: u64) -> WsConnectionRecord {
        WsConnectionRecord {
            record_version: schema::RECORD_VERSION,
            id,
            url: "wss://example.com/socket".to_string(),
            host: "example.com".to_string(),
            tls: true,
            listener_port: 8080,
            protocol: Some("chat".to_string()),
        }
    }

    fn insert_ws(
        db: &Database,
        message: &WsMessageRecord,
        connection: Option<&WsConnectionRecord>,
        payload: &[u8],
    ) {
        let tx = db.begin_write().expect("begin write transaction");
        {
            let encoded = codec::encode(message).expect("encode message");
            tx.open_table(schema::WS_MESSAGES)
                .expect("messages table")
                .insert(message.id, encoded.as_slice())
                .expect("insert message");
            tx.open_table(schema::WS_BY_TIME)
                .expect("time index")
                .insert((message.timestamp_ms, message.id), ())
                .expect("insert time index");
            tx.open_table(schema::WS_BY_CONNECTION)
                .expect("connection index")
                .insert((message.connection_id, message.id), ())
                .expect("insert connection index");
            if let Some(connection) = connection {
                let encoded = codec::encode(connection).expect("encode connection");
                tx.open_table(schema::WS_CONNECTIONS)
                    .expect("connections table")
                    .insert(connection.id, encoded.as_slice())
                    .expect("insert connection");
            }
            if !payload.is_empty() {
                tx.open_table(schema::WS_PAYLOADS)
                    .expect("payloads table")
                    .insert(message.id, payload)
                    .expect("insert payload");
            }
        }
        tx.commit().expect("commit");
    }

    #[test]
    fn snapshot_returns_newest_rows_oldest_first_and_reports_the_total() {
        let db = memory_db();
        for (id, timestamp_ms, body) in [
            (1u64, 1_000u64, b"one".as_slice()),
            (2, 2_000, b"two"),
            (3, 3_000, b"three"),
        ] {
            let record = completed_http(id, id, timestamp_ms, "GET", "example.com", "/", 200);
            insert_http(&db, &record, timestamp_ms, b"", body);
        }

        let (items, total) = load_http_snapshot(&db, 2, usize::MAX).expect("load snapshot");
        assert_eq!(total, 3, "total counts every stored exchange");
        let ids: Vec<u64> = items.iter().map(|item| item.id).collect();
        assert_eq!(ids, vec![2, 3], "newest rows load but stay oldest-first");
        assert_eq!(
            items[1].response_body().expect("response body").data(),
            b"three"
        );

        let (empty, total) = load_http_snapshot(&db, 0, usize::MAX).expect("zero limit");
        assert!(empty.is_empty());
        assert_eq!(total, 3);
    }

    #[test]
    fn snapshot_stops_at_the_byte_budget_but_keeps_the_newest_row() {
        let db = memory_db();
        for (id, timestamp_ms) in [(1u64, 1_000u64), (2, 2_000), (3, 3_000)] {
            let record = completed_http(id, id, timestamp_ms, "GET", "example.com", "/", 200);
            insert_http(&db, &record, timestamp_ms, b"req-", b"resp");
        }

        // Each row costs eight body bytes. Even a five-byte budget always
        // includes the newest row, then stops before the second.
        let (items, total) = load_http_snapshot(&db, 100, 5).expect("load snapshot");
        assert_eq!(total, 3);
        assert_eq!(
            items.iter().map(|item| item.id).collect::<Vec<_>>(),
            vec![3]
        );
    }

    #[test]
    fn ws_snapshot_orders_newest_last_and_falls_back_without_connection_metadata() {
        let db = memory_db();
        insert_ws(
            &db,
            &ws_record(1, 1, 1_000, 10, WsOpcodeRecord::Text, 2),
            Some(&ws_connection(10)),
            b"aa",
        );
        insert_ws(
            &db,
            &ws_record(2, 2, 2_000, 10, WsOpcodeRecord::Binary, 3),
            Some(&ws_connection(10)),
            b"bbb",
        );
        insert_ws(
            &db,
            &ws_record(3, 3, 3_000, 20, WsOpcodeRecord::Text, 4),
            None,
            b"cccc",
        );

        let (items, total) = load_ws_snapshot(&db, 10, usize::MAX).expect("load snapshot");
        assert_eq!(total, 3);
        assert_eq!(
            items.iter().map(|item| item.id).collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
        assert_eq!(items[0].direction, WebSocketDirection::ToClient);
        assert_eq!(items[1].opcode, WebSocketOpcode::Binary);
        assert_eq!(items[1].payload.data(), b"bbb");
        assert_eq!(items[0].connection.url, "wss://example.com/socket");
        assert_eq!(items[2].connection.id, 20);
        assert_eq!(items[2].connection.url, "<connection metadata unavailable>");

        let (items, _) = load_ws_snapshot(&db, 10, 2).expect("tiny budget");
        assert_eq!(
            items.iter().map(|item| item.id).collect::<Vec<_>>(),
            vec![3]
        );
    }

    #[test]
    fn get_http_returns_none_for_missing_ids_and_applies_body_metadata() {
        let db = memory_db();
        assert!(get_http(&db, 7).expect("query").is_none());

        let mut record = completed_http(1, 1, 1_000, "POST", "api.example.com", "/login", 201);
        record.request_body = BodyCaptureRecord {
            observed_len: 10,
            stored_len: 3,
            truncated: true,
            complete: false,
        };
        record.response_body = BodyCaptureRecord {
            observed_len: 6,
            stored_len: 6,
            truncated: false,
            complete: true,
        };
        insert_http(&db, &record, 1_000, b"abc", b"secret");

        let item = get_http(&db, 1).expect("query").expect("stored record");
        assert_eq!(item.number, 1);
        assert_eq!(item.status(), Some(201));
        assert_eq!(item.request_body.data(), b"abc");
        assert_eq!(item.request_body.total_len(), 10);
        assert!(item.request_body.is_truncated());
        assert!(!item.request_body.is_complete());
        assert_eq!(
            item.response_body().expect("response body").data(),
            b"secret"
        );
        assert_eq!(item.duration(), Some(Duration::from_millis(3)));
    }

    #[test]
    fn get_ws_reads_the_payload_when_connection_metadata_is_missing() {
        let db = memory_db();
        assert!(get_ws(&db, 5).expect("query").is_none());

        insert_ws(
            &db,
            &ws_record(5, 1, 1_000, 7, WsOpcodeRecord::Text, 5),
            None,
            b"hello",
        );

        let item = get_ws(&db, 5).expect("query").expect("stored message");
        assert_eq!(item.payload.data(), b"hello");
        assert_eq!(item.payload.total_len(), 5);
        assert_eq!(item.connection.id, 7);
        assert_eq!(item.connection.url, "<connection metadata unavailable>");
    }

    #[test]
    fn list_http_pages_newest_first_and_honours_the_cursor() {
        let db = memory_db();
        for (id, timestamp_ms) in [(1u64, 100u64), (2, 200), (3, 300), (4, 400), (5, 500)] {
            let record = completed_http(id, id, timestamp_ms, "GET", "example.com", "/x", 200);
            insert_http(&db, &record, timestamp_ms, b"", b"");
        }

        let first = list_http(&db, None, 2).expect("first page");
        assert_eq!(
            first
                .iter()
                .map(|row| (row.timestamp_ms, row.id))
                .collect::<Vec<_>>(),
            vec![(500, 5), (400, 4)]
        );

        let cursor = HistoryCursor {
            timestamp_ms: first[1].timestamp_ms,
            id: first[1].id,
        };
        let second = list_http(&db, Some(cursor), 2).expect("second page");
        assert_eq!(
            second
                .iter()
                .map(|row| (row.timestamp_ms, row.id))
                .collect::<Vec<_>>(),
            vec![(300, 3), (200, 2)]
        );

        let cursor = HistoryCursor {
            timestamp_ms: second[1].timestamp_ms,
            id: second[1].id,
        };
        let third = list_http(&db, Some(cursor), 2).expect("third page");
        assert_eq!(third.iter().map(|row| row.id).collect::<Vec<_>>(), vec![1]);
    }

    #[test]
    fn list_http_breaks_timestamp_ties_by_id_and_resumes_after_the_cursor() {
        let db = memory_db();
        for (id, path) in [(1u64, "/a"), (2, "/b")] {
            let record = completed_http(id, id, 1_000, "GET", "example.com", path, 200);
            insert_http(&db, &record, 1_000, b"", b"");
        }

        let first = list_http(&db, None, 1).expect("first page");
        assert_eq!((first[0].timestamp_ms, first[0].id), (1_000, 2));

        let cursor = HistoryCursor {
            timestamp_ms: first[0].timestamp_ms,
            id: first[0].id,
        };
        let second = list_http(&db, Some(cursor), 10).expect("second page");
        assert_eq!(second.iter().map(|row| row.id).collect::<Vec<_>>(), vec![1]);
    }

    #[test]
    fn list_http_rows_carry_metadata_counts_and_in_flight_state() {
        let db = memory_db();
        let mut finished = completed_http(1, 1, 1_000, "POST", "api.example.com", "/login", 401);
        finished.request_body = BodyCaptureRecord {
            observed_len: 4,
            stored_len: 0,
            truncated: false,
            complete: true,
        };
        finished.response_body = BodyCaptureRecord {
            observed_len: 5,
            stored_len: 0,
            truncated: false,
            complete: true,
        };
        insert_http(&db, &finished, 1_000, b"", b"");

        let in_flight = HttpExchangeRecord::started(
            2,
            2,
            records::system_time_from_ms(2_000),
            request_info("GET", "example.com", "/"),
        );
        insert_http(&db, &in_flight, 2_000, b"", b"");

        let rows = list_http(&db, None, 10).expect("rows");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].id, 2);
        assert!(rows[0].in_flight);
        assert_eq!(rows[0].status, None);
        assert_eq!(rows[0].duration_ms, None);
        assert_eq!(rows[0].method, "GET");

        assert_eq!(rows[1].id, 1);
        assert!(!rows[1].in_flight);
        assert_eq!(rows[1].status, Some(401));
        assert_eq!(rows[1].duration_ms, Some(3));
        assert_eq!(rows[1].number, 1);
        assert_eq!(rows[1].method, "POST");
        assert_eq!(rows[1].scheme, "http");
        assert_eq!(rows[1].host, "api.example.com");
        assert_eq!(rows[1].path, "/login");
        assert_eq!(rows[1].uri, "http://api.example.com/login");
        assert_eq!(rows[1].request_len, 4);
        assert_eq!(rows[1].response_len, 5);
    }

    #[test]
    fn list_http_skips_dangling_time_entries_and_clamps_the_limit() {
        let db = memory_db();
        let record = completed_http(1, 1, 1_000, "GET", "example.com", "/", 200);
        insert_http(&db, &record, 1_000, b"", b"");

        // A dangling index entry models a crash between index and record writes.
        let tx = db.begin_write().expect("begin write transaction");
        tx.open_table(schema::HTTP_BY_TIME)
            .expect("time index")
            .insert((2_000u64, 2u64), ())
            .expect("insert dangling entry");
        tx.commit().expect("commit");

        let rows = list_http(&db, None, 10).expect("rows");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, 1);
        assert_eq!(
            list_http(&db, None, 0).expect("clamped").len(),
            1,
            "a zero limit still returns one row"
        );
    }

    #[test]
    fn list_ws_reports_opcodes_payload_lengths_and_paging() {
        let db = memory_db();
        insert_ws(
            &db,
            &ws_record(1, 1, 1_000, 10, WsOpcodeRecord::Text, 2),
            Some(&ws_connection(10)),
            b"aa",
        );
        insert_ws(
            &db,
            &ws_record(2, 2, 2_000, 10, WsOpcodeRecord::Binary, 3),
            Some(&ws_connection(10)),
            b"bbb",
        );
        insert_ws(
            &db,
            &ws_record(3, 3, 3_000, 20, WsOpcodeRecord::Text, 4),
            None,
            b"cccc",
        );

        let first = list_ws(&db, None, 2).expect("first page");
        assert_eq!(
            first.iter().map(|row| row.id).collect::<Vec<_>>(),
            vec![3, 2]
        );
        assert!(first[0].text);
        assert_eq!(first[0].payload_len, 4);
        assert_eq!(first[0].number, 3);
        assert_eq!(first[0].connection.id, 20);
        assert_eq!(first[0].connection.url, "<connection metadata unavailable>");
        assert!(!first[1].text);
        assert_eq!(first[1].payload_len, 3);
        assert_eq!(first[1].connection.url, "wss://example.com/socket");

        let cursor = HistoryCursor {
            timestamp_ms: first[1].timestamp_ms,
            id: first[1].id,
        };
        let second = list_ws(&db, Some(cursor), 2).expect("second page");
        assert_eq!(second.iter().map(|row| row.id).collect::<Vec<_>>(), vec![1]);
        assert_eq!(list_ws(&db, None, 0).expect("clamped").len(), 1);
    }

    #[test]
    fn search_http_matches_metadata_case_insensitively() {
        let db = memory_db();
        let first = completed_http(1, 1, 1_000, "POST", "api.example.com", "/login", 401);
        insert_http(&db, &first, 1_000, b"", b"");
        let second = completed_http(2, 2, 2_000, "GET", "other.test", "/", 200);
        insert_http(&db, &second, 2_000, b"", b"");

        assert_eq!(
            search_http(&db, "POST", false, 10).expect("search"),
            vec![1]
        );
        assert_eq!(
            search_http(&db, "API.EXAMPLE", false, 10).expect("search"),
            vec![1]
        );
        assert_eq!(search_http(&db, "401", false, 10).expect("search"), vec![1]);
        assert_eq!(
            search_http(&db, "/", false, 10).expect("search"),
            vec![1, 2]
        );
        assert!(
            search_http(&db, "nowhere", false, 10)
                .expect("search")
                .is_empty()
        );
        assert!(
            search_http(&db, "", false, 10)
                .expect("empty needle")
                .is_empty()
        );
    }

    #[test]
    fn search_http_checks_bodies_only_when_requested() {
        let db = memory_db();
        let record = completed_http(1, 1, 1_000, "GET", "example.com", "/", 200);
        insert_http(&db, &record, 1_000, b"top-SECRET-token", b"");

        assert!(
            search_http(&db, "secret", false, 10)
                .expect("metadata search")
                .is_empty()
        );
        assert_eq!(
            search_http(&db, "secret", true, 10).expect("body search"),
            vec![1]
        );
        assert_eq!(
            search_http(&db, "TOKEN", true, 10).expect("body search"),
            vec![1]
        );
    }

    #[test]
    fn search_http_stops_at_the_limit() {
        let db = memory_db();
        for id in 1..=3 {
            let record = completed_http(id, id, id * 1_000, "GET", "example.com", "/same", 200);
            insert_http(&db, &record, id * 1_000, b"", b"");
        }

        assert_eq!(
            search_http(&db, "same", false, 2).expect("search"),
            vec![1, 2]
        );
    }

    #[test]
    fn records_from_a_newer_record_version_are_rejected() {
        let db = memory_db();
        let mut record = completed_http(1, 1, 1_000, "GET", "example.com", "/", 200);
        record.record_version = schema::RECORD_VERSION + 1;
        insert_http(&db, &record, 1_000, b"", b"");

        let error = get_http(&db, 1).expect_err("future records must be rejected");
        assert!(matches!(
            error,
            ProjectError::UnsupportedRecord { found, supported }
                if found == schema::RECORD_VERSION + 1 && supported == schema::RECORD_VERSION
        ));

        let read = db.begin_read().expect("read transaction");
        assert!(matches!(
            read_http_record(&read, 1),
            Err(ProjectError::UnsupportedRecord { .. })
        ));
        assert!(matches!(
            load_http_snapshot(&db, 10, usize::MAX),
            Err(ProjectError::UnsupportedRecord { .. })
        ));
        assert!(matches!(
            list_http(&db, None, 10),
            Err(ProjectError::UnsupportedRecord { .. })
        ));
        assert!(matches!(
            ensure_valid_record_version(schema::RECORD_VERSION + 1),
            Err(ProjectError::UnsupportedRecord { .. })
        ));
        assert!(ensure_valid_record_version(schema::RECORD_VERSION).is_ok());
    }

    #[test]
    fn ws_records_from_a_newer_record_version_are_rejected() {
        let db = memory_db();
        let mut message = ws_record(1, 1, 1_000, 10, WsOpcodeRecord::Text, 2);
        message.record_version = schema::RECORD_VERSION + 1;
        insert_ws(&db, &message, None, b"aa");

        assert!(matches!(
            get_ws(&db, 1),
            Err(ProjectError::UnsupportedRecord { .. })
        ));
        assert!(matches!(
            load_ws_snapshot(&db, 10, usize::MAX),
            Err(ProjectError::UnsupportedRecord { .. })
        ));
        assert!(matches!(
            list_ws(&db, None, 10),
            Err(ProjectError::UnsupportedRecord { .. })
        ));
    }

    #[test]
    fn corrupt_stored_records_surface_encoding_errors() {
        let db = memory_db();
        let tx = db.begin_write().expect("begin write transaction");
        {
            tx.open_table(schema::HTTP_EXCHANGES)
                .expect("exchanges table")
                .insert(1u64, b"not json".as_slice())
                .expect("insert corrupt record");
            tx.open_table(schema::HTTP_BY_TIME)
                .expect("time index")
                .insert((1_000u64, 1u64), ())
                .expect("insert time index");
        }
        tx.commit().expect("commit");

        assert!(matches!(get_http(&db, 1), Err(ProjectError::Encoding(_))));
        assert!(matches!(
            list_http(&db, None, 10),
            Err(ProjectError::Encoding(_))
        ));
    }

    #[test]
    fn scans_visit_stored_http_rows_oldest_first_and_can_stop() {
        let db = memory_db();
        for (id, timestamp_ms) in [(1u64, 1_000u64), (2, 2_000), (3, 3_000)] {
            let record = completed_http(id, id, timestamp_ms, "GET", "example.com", "/", 200);
            insert_http(
                &db,
                &record,
                timestamp_ms,
                b"",
                format!("body-{id}").as_bytes(),
            );
        }

        let mut seen = Vec::new();
        scan_http(&db, |item, index, total| {
            seen.push((item.id, index, total));
            true
        })
        .expect("scan");
        assert_eq!(seen, vec![(1, 1, 3), (2, 2, 3), (3, 3, 3)]);

        let mut first_only = Vec::new();
        scan_http(&db, |item, _, _| {
            first_only.push(item.id);
            false
        })
        .expect("cancelled scan");
        assert_eq!(first_only, vec![1], "a false visitor return stops the scan");
    }

    #[test]
    fn scans_cross_batch_boundaries_without_repeating_rows() {
        let db = memory_db();
        for id in 1..=300u64 {
            let record = completed_http(id, id, id * 1_000, "GET", "example.com", "/", 200);
            insert_http(&db, &record, id * 1_000, b"", b"");
        }

        let mut seen = Vec::new();
        scan_http(&db, |item, index, total| {
            seen.push((item.id, index, total));
            true
        })
        .expect("scan");

        assert_eq!(seen.len(), 300);
        assert_eq!(
            seen.iter().map(|(id, _, _)| *id).collect::<Vec<_>>(),
            (1..=300).collect::<Vec<_>>()
        );
        assert_eq!(seen.first().map(|(_, index, _)| *index), Some(1));
        assert_eq!(seen.last().map(|(_, index, _)| *index), Some(300));
        assert!(seen.iter().all(|(_, _, total)| *total == 300));
    }

    #[test]
    fn scans_visit_websocket_messages_with_connection_metadata() {
        let db = memory_db();
        insert_ws(
            &db,
            &ws_record(1, 1, 1_000, 10, WsOpcodeRecord::Text, 2),
            Some(&ws_connection(10)),
            b"aa",
        );
        insert_ws(
            &db,
            &ws_record(2, 2, 2_000, 20, WsOpcodeRecord::Binary, 3),
            None,
            b"bbb",
        );

        let mut seen = Vec::new();
        scan_ws(&db, |item, index, total| {
            seen.push((
                item.id,
                index,
                total,
                item.connection.url.clone(),
                item.payload.data().to_vec(),
            ));
            true
        })
        .expect("scan");
        assert_eq!(seen.len(), 2);
        assert_eq!(
            seen[0],
            (
                1,
                1,
                2,
                "wss://example.com/socket".to_string(),
                b"aa".to_vec()
            )
        );
        assert_eq!(
            seen[1].3, "<connection metadata unavailable>",
            "missing connection rows keep a placeholder"
        );
        assert_eq!(seen[1].4, b"bbb");
    }

    #[test]
    fn counts_report_stored_rows() {
        let db = memory_db();
        assert_eq!(count_http(&db).expect("count"), 0);
        assert_eq!(count_ws(&db).expect("count"), 0);

        let record = completed_http(1, 1, 1_000, "GET", "example.com", "/", 200);
        insert_http(&db, &record, 1_000, b"", b"");
        insert_ws(
            &db,
            &ws_record(1, 1, 1_000, 10, WsOpcodeRecord::Text, 1),
            Some(&ws_connection(10)),
            b"x",
        );

        assert_eq!(count_http(&db).expect("count"), 1);
        assert_eq!(count_ws(&db).expect("count"), 1);
    }

    #[test]
    fn case_insensitive_body_matching_handles_edges() {
        assert!(contains_ascii_case_insensitive(b"xxABCyy", "abc"));
        assert!(!contains_ascii_case_insensitive(b"ab", "abc"));
        assert!(!contains_ascii_case_insensitive(b"abc", ""));
    }
}
