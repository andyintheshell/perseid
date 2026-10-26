//! Copying a consistent project snapshot into a file.
//!
//! The transfer deliberately uses a redb read transaction rather than
//! `std::fs::copy`: the source database may be live. Every destination is
//! created in `initializing` state and only switched to `ready` after all
//! tables have been copied and committed. Explicit overwrites are staged next
//! to the destination and published only once complete.

use std::path::{Path, PathBuf};

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

/// Copy every data table into `destination`, optionally replacing an existing
/// file after the new project has been fully written.
///
/// Overwrites never truncate `destination` in place. The copy is written to a
/// uniquely named sibling first and only published when it is complete, so a
/// failed transfer leaves the previous file untouched.
pub fn copy_to_file(
    source: &Database,
    destination: &Path,
    meta: &ProjectMeta,
    overwrite: bool,
) -> Result<Database> {
    if !overwrite {
        return copy_to_new_file(source, destination, meta);
    }

    let temporary = temporary_path(destination);
    let copied = copy_to_new_file(source, &temporary, meta);
    let copied = match copied {
        Ok(copied) => copied,
        Err(err) => {
            backend::remove_incomplete(&temporary);
            return Err(err);
        }
    };
    // redb keeps the file handle open; release it before publishing the file.
    drop(copied);
    if let Err(err) = replace_file(&temporary, destination) {
        backend::remove_incomplete(&temporary);
        return Err(err);
    }
    backend::open_file(destination)
}

fn temporary_path(destination: &Path) -> PathBuf {
    let name = destination
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "project.perseid".to_string());
    destination.with_file_name(format!(".{name}.{}.tmp", uuid::Uuid::new_v4()))
}

#[cfg(not(windows))]
fn replace_file(source: &Path, destination: &Path) -> Result<()> {
    std::fs::rename(source, destination)?;
    Ok(())
}

#[cfg(windows)]
fn replace_file(source: &Path, destination: &Path) -> Result<()> {
    // Windows cannot rename over an existing file. Only remove the old file
    // after the rename proves that the replacement is blocked; a different
    // failure must leave the existing project untouched.
    match std::fs::rename(source, destination) {
        Ok(()) => Ok(()),
        Err(err)
            if destination.exists()
                && matches!(
                    err.kind(),
                    std::io::ErrorKind::AlreadyExists | std::io::ErrorKind::PermissionDenied
                ) =>
        {
            std::fs::remove_file(destination)?;
            std::fs::rename(source, destination)?;
            Ok(())
        }
        Err(err) => Err(err.into()),
    }
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

/// Copy a live project to a file, leaving the source open.
pub fn save_copy(source: &Database, destination: &Path, overwrite: bool) -> Result<()> {
    let read = source.begin_read()?;
    let meta = schema::read_meta(&read)?;
    drop(read);
    let destination_db = copy_to_file(source, destination, &meta, overwrite)?;
    drop(destination_db);
    Ok(())
}

/// Compile-time assertion: the writer thread moves write transactions across
/// thread boundaries, so `WriteTransaction` must stay `Send`.
#[allow(dead_code)]
fn _assert_tx_is_send() {
    fn assert_send<T: Send>() {}
    assert_send::<WriteTransaction>();
}

#[allow(dead_code)]
fn _project_error_is_send_sync() -> Option<Box<dyn std::error::Error + Send + Sync>> {
    Some(Box::new(super::ProjectError::InvalidProject(String::new())))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::project::records::{self, HttpExchangeRecord, HttpPhaseRecord};
    use crate::project::schema::STATE_INITIALIZING;
    use crate::project::{ProjectError, codec, queries};
    use crate::proxy::{RequestInfo, ResponseHead};

    use super::*;

    /// A unique path under the system temporary directory, removed on drop.
    struct TempProject {
        path: PathBuf,
    }

    impl TempProject {
        fn new(label: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "perseid-transfer-{label}-{}.perseid",
                uuid::Uuid::new_v4()
            ));
            Self { path }
        }
    }

    impl Drop for TempProject {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.path);
        }
    }

    fn request_info() -> RequestInfo {
        RequestInfo {
            method: "GET".to_string(),
            uri: "http://example.com/".to_string(),
            scheme: "http".to_string(),
            host: "example.com".to_string(),
            path: "/".to_string(),
            version: "HTTP/1.1".to_string(),
            headers: vec![("host".to_string(), "example.com".to_string())],
        }
    }

    fn http_record(id: u64, number: u64, timestamp_ms: u64) -> HttpExchangeRecord {
        let mut record = HttpExchangeRecord::started(
            id,
            number,
            records::system_time_from_ms(timestamp_ms),
            request_info(),
        );
        record.response = Some(ResponseHead {
            version: "HTTP/1.1".to_string(),
            status: 200,
            reason: String::new(),
            headers: Vec::new(),
        });
        record.duration_ms = Some(4);
        record.phase = HttpPhaseRecord::Completed { duration_ms: 4 };
        record
    }

    fn seed_http(
        db: &Database,
        record: &HttpExchangeRecord,
        request_body: &[u8],
        response_body: &[u8],
    ) {
        let tx = db.begin_write().expect("begin write transaction");
        {
            let encoded = codec::encode(record).expect("encode record");
            tx.open_table(schema::HTTP_EXCHANGES)
                .expect("exchanges table")
                .insert(record.id, encoded.as_slice())
                .expect("insert record");
            tx.open_table(schema::HTTP_BY_TIME)
                .expect("time index")
                .insert((record.started_at_ms, record.id), ())
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

    /// A raw redb database with none of the Perseid tables, so the first copy
    /// step fails after the destination file has already been created.
    fn tableless_source() -> Database {
        redb::Database::builder()
            .create_with_backend(redb::backends::InMemoryBackend::new())
            .expect("create empty source database")
    }

    #[test]
    fn copy_produces_a_ready_project_with_the_same_contents() {
        let meta = ProjectMeta::new("transfer-tests".to_string());
        let source = backend::create_memory(&meta).expect("source project");
        seed_http(&source, &http_record(1, 1, 1_000), b"request", b"response");

        let tx = source.begin_write().expect("begin write transaction");
        {
            tx.open_table(schema::WORKSPACE)
                .expect("workspace table")
                .insert(schema::WORKSPACE_KEY, b"state".as_slice())
                .expect("insert workspace");
        }
        tx.commit().expect("commit");

        let destination = TempProject::new("copy");
        let copied = copy_to_new_file(&source, &destination.path, &meta).expect("copy project");

        let read = copied.begin_read().expect("read transaction");
        let copied_meta = schema::read_meta(&read).expect("copied meta");
        assert_eq!(copied_meta.state, STATE_READY);
        assert_eq!(copied_meta.project_id, meta.project_id);

        let item = queries::get_http(&copied, 1)
            .expect("read copied exchange")
            .expect("copied exchange");
        assert_eq!(item.request_body.data(), b"request");
        assert_eq!(
            item.response_body().expect("response body").data(),
            b"response"
        );

        let workspace = read
            .open_table(schema::WORKSPACE)
            .expect("workspace table")
            .get(schema::WORKSPACE_KEY)
            .expect("read workspace")
            .expect("workspace entry");
        assert_eq!(workspace.value(), b"state".as_slice());

        assert_eq!(
            queries::count_http(&source).expect("source stays open"),
            1,
            "copying must not disturb the source"
        );
    }

    #[test]
    fn copy_transfers_tables_larger_than_one_batch() {
        let total_rows = (COPY_BATCH_ROWS + 37) as u64;
        let meta = ProjectMeta::new("transfer-batch".to_string());
        let source = backend::create_memory(&meta).expect("source project");
        for id in 1..=total_rows {
            seed_http(&source, &http_record(id, id, id), b"", b"");
        }

        let destination = TempProject::new("batch");
        let copied = copy_to_new_file(&source, &destination.path, &meta).expect("copy project");

        assert_eq!(
            queries::count_http(&copied).expect("count copied rows"),
            total_rows
        );
        let rows = queries::list_http(&copied, None, 1000).expect("list copied rows");
        assert_eq!(rows.len(), total_rows as usize);
        assert_eq!(
            rows[0].id, total_rows,
            "the newest row survived the batches"
        );
        drop(copied);

        let reopened = backend::open_file(&destination.path).expect("reopen copy");
        assert_eq!(
            queries::count_http(&reopened).expect("count reopened rows"),
            total_rows
        );
    }

    #[test]
    fn save_copy_reads_source_metadata_and_leaves_the_source_open() {
        let meta = ProjectMeta::new("transfer-save".to_string());
        let source = backend::create_memory(&meta).expect("source project");
        seed_http(&source, &http_record(7, 1, 7_000), b"", b"copy");

        let destination = TempProject::new("save-copy");
        save_copy(&source, &destination.path, false).expect("save copy");

        let copied = backend::open_file(&destination.path).expect("open copy");
        let item = queries::get_http(&copied, 7)
            .expect("read copy")
            .expect("copied exchange");
        assert_eq!(item.response_body().expect("response body").data(), b"copy");
        assert_eq!(queries::count_http(&source).expect("source stays open"), 1);
    }

    #[test]
    fn copy_can_replace_an_existing_file_after_the_new_copy_is_complete() {
        let meta = ProjectMeta::new("transfer-overwrite".to_string());
        let source = backend::create_memory(&meta).expect("source project");
        seed_http(&source, &http_record(8, 1, 8_000), b"", b"new");

        let destination = TempProject::new("overwrite");
        std::fs::write(&destination.path, b"old project bytes").expect("write existing file");

        let copied =
            copy_to_file(&source, &destination.path, &meta, true).expect("overwrite project");
        let read = copied.begin_read().expect("read transaction");
        assert_eq!(schema::read_meta(&read).expect("meta").state, STATE_READY);
        drop(read);
        assert_eq!(queries::count_http(&copied).expect("count"), 1);
        drop(copied);

        let reopened = backend::open_file(&destination.path).expect("reopen replacement");
        let item = queries::get_http(&reopened, 8)
            .expect("read replacement")
            .expect("replacement record");
        assert_eq!(item.response_body().expect("response body").data(), b"new");
    }

    #[test]
    fn a_failed_overwrite_leaves_the_existing_file_and_no_temporary_file() {
        let source = tableless_source();
        let meta = ProjectMeta::new("transfer-overwrite-failed".to_string());
        let destination = TempProject::new("overwrite-failed");
        std::fs::write(&destination.path, b"old project bytes").expect("write existing file");

        let error = copy_to_file(&source, &destination.path, &meta, true)
            .expect_err("copying a database without Perseid tables must fail");

        assert!(matches!(error, ProjectError::Table(_)));
        assert_eq!(
            std::fs::read(&destination.path).expect("read existing file"),
            b"old project bytes"
        );
        let parent = destination.path.parent().expect("temp parent");
        let prefix = format!(
            ".{}.",
            destination
                .path
                .file_name()
                .expect("file name")
                .to_string_lossy()
        );
        let leftovers: Vec<_> = std::fs::read_dir(parent)
            .expect("read temp directory")
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with(&prefix) && name.ends_with(".tmp"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "a failed overwrite must clean up temporary files: {leftovers:?}"
        );
    }

    #[test]
    fn copy_without_overwrite_still_refuses_an_existing_file() {
        let meta = ProjectMeta::new("transfer-no-overwrite".to_string());
        let source = backend::create_memory(&meta).expect("source project");
        let destination = TempProject::new("no-overwrite");
        std::fs::write(&destination.path, b"old project bytes").expect("write existing file");

        let error = copy_to_file(&source, &destination.path, &meta, false)
            .expect_err("an existing destination must be refused");
        assert!(matches!(
            error,
            ProjectError::DestinationExists(ref path) if path == &destination.path
        ));
        assert_eq!(
            std::fs::read(&destination.path).expect("read existing file"),
            b"old project bytes"
        );
    }

    #[test]
    fn a_failed_copy_removes_the_incomplete_destination() {
        let source = tableless_source();
        let destination = TempProject::new("failed");

        let error = copy_to_new_file(
            &source,
            &destination.path,
            &ProjectMeta::new("transfer-failed".to_string()),
        )
        .expect_err("copying a database without Perseid tables must fail");

        assert!(matches!(error, ProjectError::Table(_)));
        assert!(
            !destination.path.exists(),
            "a failed copy must not leave a destination file"
        );
        assert!(
            backend::open_file(&destination.path).is_err(),
            "the removed destination cannot be opened"
        );
    }

    #[test]
    fn an_interrupted_copy_leaves_the_destination_initializing() {
        let source = tableless_source();
        let meta = ProjectMeta::new("transfer-interrupted".to_string());
        let destination = TempProject::new("interrupted");
        let created =
            backend::create_initializing(&destination.path, &meta).expect("create destination");

        let error = copy_contents(&source, &created, &meta).expect_err("copy must fail");
        assert!(matches!(error, ProjectError::Table(_)));

        let read = created.begin_read().expect("read transaction");
        assert_eq!(
            schema::read_meta(&read).expect("meta").state,
            STATE_INITIALIZING
        );
        drop(read);
        drop(created);

        let reopened = backend::open_file(&destination.path).expect("reopen destination");
        let read = reopened.begin_read().expect("read transaction");
        assert_eq!(
            schema::read_meta(&read).expect("meta").state,
            STATE_INITIALIZING,
            "an interrupted copy is never marked ready"
        );
    }
}
