//! Copying a consistent project snapshot into a new file.
//!
//! The transfer deliberately uses a redb read transaction rather than
//! `std::fs::copy`: the source database may be live. Every destination is
//! created in `initializing` state and only switched to `ready` after all
//! tables have been copied and committed.

use std::path::Path;

use redb::{
    Database, Durability, Key, ReadTransaction, ReadableDatabase, ReadableTable, Value,
    WriteTransaction,
};

use super::Result;
use super::backend;
use super::schema::{self, ProjectMeta, STATE_READY};

/// Rows copied per write transaction. Keeps peak page-dirty memory bounded on
/// large projects while still amortising commit overhead.
const COPY_BATCH_ROWS: usize = 512;

/// Copy every data table from `source` into a new `destination` file. The
/// destination must not exist. On error the incomplete file is removed.
pub fn copy_to_new_file(
    source: &Database,
    destination: &Path,
    meta: &ProjectMeta,
) -> Result<Database> {
    let destination_db = backend::create_initializing(destination, meta)?;
    if let Err(err) = copy_contents(source, &destination_db, meta) {
        drop(destination_db);
        backend::remove_incomplete(destination);
        return Err(err);
    }
    Ok(destination_db)
}

fn copy_contents(source: &Database, destination: &Database, meta: &ProjectMeta) -> Result<()> {
    let read = source.begin_read()?;

    copy_table(destination, &read, schema::HTTP_EXCHANGES)?;
    copy_table(destination, &read, schema::HTTP_BODIES)?;
    copy_table(destination, &read, schema::HTTP_BY_TIME)?;
    copy_table(destination, &read, schema::HTTP_BY_HOST)?;
    copy_table(destination, &read, schema::HTTP_BY_STATUS)?;
    copy_table(destination, &read, schema::HTTP_IN_FLIGHT)?;
    copy_table(destination, &read, schema::WS_MESSAGES)?;
    copy_table(destination, &read, schema::WS_PAYLOADS)?;
    copy_table(destination, &read, schema::WS_BY_TIME)?;
    copy_table(destination, &read, schema::WS_BY_CONNECTION)?;
    copy_table(destination, &read, schema::WS_CONNECTIONS)?;
    copy_table(destination, &read, schema::WORKSPACE)?;
    copy_table(destination, &read, schema::ANNOTATIONS)?;

    let mut final_meta = meta.clone();
    final_meta.state = STATE_READY.to_string();
    let mut tx = destination.begin_write()?;
    tx.set_durability(Durability::Immediate)?;
    schema::mark_ready(&tx, &final_meta)?;
    tx.commit()?;
    Ok(())
}

fn copy_table<K, V>(
    destination: &Database,
    read: &ReadTransaction,
    definition: redb::TableDefinition<'static, K, V>,
) -> Result<()>
where
    K: Key + 'static,
    V: Value + 'static,
{
    let source_table = read.open_table(definition)?;
    let mut rows = source_table.iter()?;
    let mut exhausted = false;
    while !exhausted {
        let mut tx = destination.begin_write()?;
        tx.set_durability(Durability::Immediate)?;
        {
            let mut table = tx.open_table(definition)?;
            for _ in 0..COPY_BATCH_ROWS {
                let Some(item) = rows.next() else {
                    exhausted = true;
                    break;
                };
                let (key, value) = item?;
                table.insert(key.value(), value.value())?;
            }
        }
        tx.commit()?;
    }
    Ok(())
}

/// Copy a live project to a new file, leaving the source open.
pub fn save_copy(source: &Database, destination: &Path) -> Result<()> {
    let read = source.begin_read()?;
    let meta = schema::read_meta(&read)?;
    drop(read);
    let destination_db = copy_to_new_file(source, destination, &meta)?;
    drop(destination_db);
    Ok(())
}

#[allow(dead_code)]
fn _assert_tx_is_send(_tx: WriteTransaction) {}

#[allow(dead_code)]
fn _project_error_is_send_sync() -> Option<Box<dyn std::error::Error + Send + Sync>> {
    Some(Box::new(super::ProjectError::InvalidProject(String::new())))
}
