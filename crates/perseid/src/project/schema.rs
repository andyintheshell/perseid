//! On-disk schema for a `.perseid` project.
//!
//! A project is a normal `redb` database. The constants in this module are the
//! schema-v1 contract: changing a table name, key encoding or value encoding
//! requires a migration and a new schema version.

use redb::{
    Database, ReadTransaction, ReadableDatabase, TableDefinition, TableHandle, WriteTransaction,
};

use super::{ProjectError, Result};

/// Marker stored in the metadata table. A redb file without it is not ours.
pub const APPLICATION_ID: &str = "perseid-project";
pub const SCHEMA_VERSION: u64 = 1;
pub const RECORD_VERSION: u16 = 1;

pub const META: TableDefinition<'static, &str, &[u8]> = TableDefinition::new("meta");
pub const HTTP_EXCHANGES: TableDefinition<'static, u64, &[u8]> =
    TableDefinition::new("http_exchanges");
pub const HTTP_BODIES: TableDefinition<'static, (u64, u8), &[u8]> =
    TableDefinition::new("http_bodies");
pub const HTTP_BY_TIME: TableDefinition<'static, (u64, u64), ()> =
    TableDefinition::new("http_by_time");
pub const HTTP_BY_HOST: TableDefinition<'static, (String, u64), ()> =
    TableDefinition::new("http_by_host");
pub const HTTP_BY_STATUS: TableDefinition<'static, (u16, u64), ()> =
    TableDefinition::new("http_by_status");
pub const HTTP_IN_FLIGHT: TableDefinition<'static, u64, ()> =
    TableDefinition::new("http_in_flight");
pub const WS_MESSAGES: TableDefinition<'static, u64, &[u8]> = TableDefinition::new("ws_messages");
pub const WS_PAYLOADS: TableDefinition<'static, u64, &[u8]> = TableDefinition::new("ws_payloads");
pub const WS_BY_TIME: TableDefinition<'static, (u64, u64), ()> = TableDefinition::new("ws_by_time");
pub const WS_BY_CONNECTION: TableDefinition<'static, (u64, u64), ()> =
    TableDefinition::new("ws_by_connection");
pub const WS_CONNECTIONS: TableDefinition<'static, u64, &[u8]> =
    TableDefinition::new("ws_connections");
pub const WORKSPACE: TableDefinition<'static, &str, &[u8]> = TableDefinition::new("workspace");
pub const ANNOTATIONS: TableDefinition<'static, (u8, u64), &[u8]> =
    TableDefinition::new("annotations");

pub const KEY_APPLICATION_ID: &str = "application_id";
pub const KEY_SCHEMA_VERSION: &str = "schema_version";
pub const KEY_PROJECT_ID: &str = "project_id";
pub const KEY_CREATED_AT_MS: &str = "created_at_ms";
pub const KEY_LAST_WRITER_VERSION: &str = "last_writer_version";
pub const KEY_STATE: &str = "state";
pub const KEY_NEXT_HTTP_ID: &str = "next_http_id";
pub const KEY_NEXT_WS_ID: &str = "next_ws_id";
pub const KEY_NEXT_WS_CONNECTION_ID: &str = "next_ws_connection_id";
pub const KEY_NEXT_HTTP_NUMBER: &str = "next_http_number";
pub const KEY_NEXT_WS_NUMBER: &str = "next_ws_number";
pub const KEY_NEXT_EVENT_SEQUENCE: &str = "next_event_sequence";
pub const KEY_LAST_COMMITTED_SEQUENCE: &str = "last_committed_sequence";
pub const KEY_CAPTURE_LOSS_COUNT: &str = "capture_loss_count";
pub const KEY_EVENT_LOSS_COUNT: &str = "event_loss_count";
pub const KEY_HTTP_COUNT: &str = "http_count";
pub const KEY_WS_COUNT: &str = "ws_count";
pub const KEY_HTTP_BODY_BYTES: &str = "http_body_bytes";
pub const KEY_WS_BODY_BYTES: &str = "ws_body_bytes";
pub const KEY_RETENTION_EVICTIONS: &str = "retention_evictions";

pub const STATE_INITIALIZING: &str = "initializing";
pub const STATE_READY: &str = "ready";
pub const STATE_MIGRATING: &str = "migrating";

pub const WORKSPACE_KEY: &str = "workspace/v1";

/// Values read from a project's metadata table on open.
#[derive(Clone, Debug)]
pub struct ProjectMeta {
    pub project_id: String,
    pub created_at_ms: u64,
    pub schema_version: u64,
    pub state: String,
    pub next_http_id: u64,
    pub next_ws_id: u64,
    pub next_ws_connection_id: u64,
    pub next_http_number: u64,
    pub next_ws_number: u64,
    pub next_event_sequence: u64,
    pub last_committed_sequence: u64,
    pub capture_loss_count: u64,
    pub event_loss_count: u64,
    pub http_count: u64,
    pub ws_count: u64,
    /// Retained request/response body bytes for temporary-project retention.
    pub http_body_bytes: u64,
    /// Retained WebSocket payload bytes for temporary-project retention.
    pub ws_body_bytes: u64,
    /// Records removed by temporary-project retention limits.
    pub retention_evictions: u64,
}

impl ProjectMeta {
    pub fn new(project_id: String) -> Self {
        Self {
            project_id,
            created_at_ms: now_ms(),
            schema_version: SCHEMA_VERSION,
            state: STATE_READY.to_string(),
            next_http_id: 1,
            next_ws_id: 1,
            next_ws_connection_id: 1,
            next_http_number: 1,
            next_ws_number: 1,
            next_event_sequence: 1,
            last_committed_sequence: 0,
            capture_loss_count: 0,
            event_loss_count: 0,
            http_count: 0,
            ws_count: 0,
            http_body_bytes: 0,
            ws_body_bytes: 0,
            retention_evictions: 0,
        }
    }
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

/// Create every schema-v1 table and write the application metadata.
pub fn initialize(tx: &WriteTransaction, meta: &ProjectMeta) -> Result<()> {
    {
        let mut table = tx.open_table(META)?;
        table.insert(KEY_APPLICATION_ID, APPLICATION_ID.as_bytes())?;
        table.insert(
            KEY_SCHEMA_VERSION,
            meta.schema_version.to_string().as_bytes(),
        )?;
        table.insert(KEY_PROJECT_ID, meta.project_id.as_bytes())?;
        table.insert(KEY_CREATED_AT_MS, meta.created_at_ms.to_string().as_bytes())?;
        table.insert(
            KEY_LAST_WRITER_VERSION,
            env!("CARGO_PKG_VERSION").as_bytes(),
        )?;
        table.insert(KEY_STATE, meta.state.as_bytes())?;
        write_counters(&mut table, meta)?;
    }
    // Creating (and immediately dropping) each table makes the schema
    // discoverable even when the project is still empty.
    tx.open_table(HTTP_EXCHANGES)?;
    tx.open_table(HTTP_BODIES)?;
    tx.open_table(HTTP_BY_TIME)?;
    tx.open_table(HTTP_BY_HOST)?;
    tx.open_table(HTTP_BY_STATUS)?;
    tx.open_table(HTTP_IN_FLIGHT)?;
    tx.open_table(WS_MESSAGES)?;
    tx.open_table(WS_PAYLOADS)?;
    tx.open_table(WS_BY_TIME)?;
    tx.open_table(WS_BY_CONNECTION)?;
    tx.open_table(WS_CONNECTIONS)?;
    tx.open_table(WORKSPACE)?;
    tx.open_table(ANNOTATIONS)?;
    Ok(())
}

pub fn write_counters(table: &mut redb::Table<'_, &str, &[u8]>, meta: &ProjectMeta) -> Result<()> {
    table.insert(KEY_NEXT_HTTP_ID, meta.next_http_id.to_string().as_bytes())?;
    table.insert(KEY_NEXT_WS_ID, meta.next_ws_id.to_string().as_bytes())?;
    table.insert(
        KEY_NEXT_WS_CONNECTION_ID,
        meta.next_ws_connection_id.to_string().as_bytes(),
    )?;
    table.insert(
        KEY_NEXT_HTTP_NUMBER,
        meta.next_http_number.to_string().as_bytes(),
    )?;
    table.insert(
        KEY_NEXT_WS_NUMBER,
        meta.next_ws_number.to_string().as_bytes(),
    )?;
    table.insert(
        KEY_NEXT_EVENT_SEQUENCE,
        meta.next_event_sequence.to_string().as_bytes(),
    )?;
    table.insert(
        KEY_LAST_COMMITTED_SEQUENCE,
        meta.last_committed_sequence.to_string().as_bytes(),
    )?;
    table.insert(
        KEY_CAPTURE_LOSS_COUNT,
        meta.capture_loss_count.to_string().as_bytes(),
    )?;
    table.insert(
        KEY_EVENT_LOSS_COUNT,
        meta.event_loss_count.to_string().as_bytes(),
    )?;
    table.insert(KEY_HTTP_COUNT, meta.http_count.to_string().as_bytes())?;
    table.insert(KEY_WS_COUNT, meta.ws_count.to_string().as_bytes())?;
    table.insert(
        KEY_HTTP_BODY_BYTES,
        meta.http_body_bytes.to_string().as_bytes(),
    )?;
    table.insert(KEY_WS_BODY_BYTES, meta.ws_body_bytes.to_string().as_bytes())?;
    table.insert(
        KEY_RETENTION_EVICTIONS,
        meta.retention_evictions.to_string().as_bytes(),
    )?;
    Ok(())
}

/// Read and validate the metadata of an existing project.
pub fn read_meta(tx: &ReadTransaction) -> Result<ProjectMeta> {
    let table = tx.open_table(META).map_err(|err| match err {
        redb::TableError::TableDoesNotExist(_) => {
            ProjectError::InvalidProject("this is not a Perseid project".to_string())
        }
        other => ProjectError::Table(other),
    })?;
    let application_id = get_str(&table, KEY_APPLICATION_ID)?
        .ok_or_else(|| ProjectError::InvalidProject("missing application marker".to_string()))?;
    if application_id != APPLICATION_ID {
        return Err(ProjectError::InvalidProject(format!(
            "unexpected application marker `{application_id}`"
        )));
    }
    let schema_version = get_u64(&table, KEY_SCHEMA_VERSION)?.unwrap_or(0);
    if schema_version > SCHEMA_VERSION {
        return Err(ProjectError::FutureSchema {
            found: schema_version,
            supported: SCHEMA_VERSION,
        });
    }
    if schema_version == 0 {
        return Err(ProjectError::InvalidProject(
            "missing schema version".to_string(),
        ));
    }
    let project_id = get_str(&table, KEY_PROJECT_ID)?
        .ok_or_else(|| ProjectError::InvalidProject("missing project id".to_string()))?;
    let state = get_str(&table, KEY_STATE)?
        .ok_or_else(|| ProjectError::InvalidProject("missing project state".to_string()))?;
    Ok(ProjectMeta {
        project_id,
        created_at_ms: get_u64(&table, KEY_CREATED_AT_MS)?.unwrap_or(0),
        schema_version,
        state,
        next_http_id: get_u64(&table, KEY_NEXT_HTTP_ID)?.unwrap_or(1).max(1),
        next_ws_id: get_u64(&table, KEY_NEXT_WS_ID)?.unwrap_or(1).max(1),
        next_ws_connection_id: get_u64(&table, KEY_NEXT_WS_CONNECTION_ID)?
            .unwrap_or(1)
            .max(1),
        next_http_number: get_u64(&table, KEY_NEXT_HTTP_NUMBER)?.unwrap_or(1).max(1),
        next_ws_number: get_u64(&table, KEY_NEXT_WS_NUMBER)?.unwrap_or(1).max(1),
        next_event_sequence: get_u64(&table, KEY_NEXT_EVENT_SEQUENCE)?
            .unwrap_or(1)
            .max(1),
        last_committed_sequence: get_u64(&table, KEY_LAST_COMMITTED_SEQUENCE)?.unwrap_or(0),
        capture_loss_count: get_u64(&table, KEY_CAPTURE_LOSS_COUNT)?.unwrap_or(0),
        event_loss_count: get_u64(&table, KEY_EVENT_LOSS_COUNT)?.unwrap_or(0),
        http_count: get_u64(&table, KEY_HTTP_COUNT)?.unwrap_or(0),
        ws_count: get_u64(&table, KEY_WS_COUNT)?.unwrap_or(0),
        http_body_bytes: get_u64(&table, KEY_HTTP_BODY_BYTES)?.unwrap_or(0),
        ws_body_bytes: get_u64(&table, KEY_WS_BODY_BYTES)?.unwrap_or(0),
        retention_evictions: get_u64(&table, KEY_RETENTION_EVICTIONS)?.unwrap_or(0),
    })
}

/// Mark a fully copied destination as ready in one durable transaction.
pub fn mark_ready(tx: &WriteTransaction, meta: &ProjectMeta) -> Result<()> {
    let mut table = tx.open_table(META)?;
    table.insert(KEY_STATE, STATE_READY.as_bytes())?;
    table.insert(
        KEY_SCHEMA_VERSION,
        meta.schema_version.to_string().as_bytes(),
    )?;
    table.insert(
        KEY_LAST_WRITER_VERSION,
        env!("CARGO_PKG_VERSION").as_bytes(),
    )?;
    write_counters(&mut table, meta)?;
    Ok(())
}

fn get_str(table: &redb::ReadOnlyTable<&str, &[u8]>, key: &str) -> Result<Option<String>> {
    let Some(value) = table.get(key)? else {
        return Ok(None);
    };
    let bytes = value.value();
    Ok(Some(
        std::str::from_utf8(bytes)
            .map_err(|_| ProjectError::InvalidProject(format!("`{key}` is not UTF-8")))?
            .to_string(),
    ))
}

fn get_u64(table: &redb::ReadOnlyTable<&str, &[u8]>, key: &str) -> Result<Option<u64>> {
    let Some(value) = table.get(key)? else {
        return Ok(None);
    };
    let text = std::str::from_utf8(value.value())
        .map_err(|_| ProjectError::InvalidProject(format!("`{key}` is not UTF-8")))?;
    text.parse::<u64>()
        .map(Some)
        .map_err(|_| ProjectError::InvalidProject(format!("`{key}` is not an integer")))
}

/// Check that a database has all tables required by schema v1.
pub fn validate_tables(db: &Database) -> Result<()> {
    let tx = db.begin_read()?;
    for name in [
        "meta",
        "http_exchanges",
        "http_bodies",
        "http_by_time",
        "http_by_host",
        "http_by_status",
        "http_in_flight",
        "ws_messages",
        "ws_payloads",
        "ws_by_time",
        "ws_by_connection",
        "ws_connections",
        "workspace",
        "annotations",
    ] {
        if !tx.list_tables()?.any(|table| table.name() == name) {
            return Err(ProjectError::InvalidProject(format!(
                "missing table `{name}`"
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_schema_version_is_stable() {
        assert_eq!(SCHEMA_VERSION, 1);
        assert_eq!(RECORD_VERSION, 1);
    }
}
