//! Database creation and opening.
//!
//! Temporary projects live in redb's [`InMemoryBackend`]; persistent projects
//! are one redb file. New files are created exclusively. An explicit overwrite
//! is written to a temporary sibling first so the previous project is never
//! truncated in place.

use std::fs::OpenOptions;
use std::path::Path;

use redb::{Database, DatabaseError};

use super::schema::{self, ProjectMeta, STATE_INITIALIZING};
use super::{ProjectError, Result};

/// redb's default cache is intentionally large. Projects use a measured,
/// modest value so a multi-gigabyte history does not pin a huge page cache.
pub const MEMORY_CACHE_BYTES: usize = 32 * 1024 * 1024;
pub const DISK_CACHE_BYTES: usize = 64 * 1024 * 1024;

pub fn create_memory(meta: &ProjectMeta) -> Result<Database> {
    let mut builder = Database::builder();
    builder.set_cache_size(MEMORY_CACHE_BYTES);
    let db = builder.create_with_backend(redb::backends::InMemoryBackend::new())?;
    let mut tx = db.begin_write()?;
    tx.set_durability(redb::Durability::Immediate)?;
    schema::initialize(&tx, meta)?;
    tx.commit()?;
    Ok(db)
}

pub fn open_file(path: &Path) -> Result<Database> {
    // Fail before redb touches the file and include the selected path in the
    // diagnostic. Never create a missing file as a side effect of Open.
    let metadata = std::fs::metadata(path).map_err(|err| ProjectError::FileAccess {
        path: path.to_path_buf(),
        reason: format!("{err}; check the path and file permissions"),
    })?;
    if !metadata.is_file() || metadata.permissions().readonly() {
        return Err(ProjectError::FileAccess { path: path.to_path_buf(), reason: "select a writable project file (copy read-only projects to a writable location first)".to_string() });
    }
    let mut builder = Database::builder();
    builder.set_cache_size(DISK_CACHE_BYTES);
    let db = builder.open(path).map_err(|err| match err {
        DatabaseError::DatabaseAlreadyOpen => ProjectError::AlreadyOpen(path.to_path_buf()),
        other => ProjectError::FileAccess {
            path: path.to_path_buf(),
            reason: format!("{other}; select a valid writable project or restore a backup"),
        },
    })?;
    Ok(db)
}

/// Create a brand-new file, refusing to replace anything that already exists.
pub fn create_initializing(path: &Path, meta: &ProjectMeta) -> Result<Database> {
    let mut options = OpenOptions::new();
    // redb needs to read the file it initializes, so the handle must be
    // readable as well as writable.
    options.read(true).write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(path).map_err(|err| {
        if err.kind() == std::io::ErrorKind::AlreadyExists {
            ProjectError::DestinationExists(path.to_path_buf())
        } else {
            ProjectError::Io(err)
        }
    })?;

    let mut builder = Database::builder();
    builder.set_cache_size(DISK_CACHE_BYTES);
    let db = builder.create_file(file)?;
    let mut initializing = meta.clone();
    initializing.state = STATE_INITIALIZING.to_string();
    let mut tx = db.begin_write()?;
    tx.set_durability(redb::Durability::Immediate)?;
    schema::initialize(&tx, &initializing)?;
    tx.commit()?;
    Ok(db)
}

/// Remove a failed or incomplete destination. Best effort: a crash can leave
/// an `initializing` file, which `open` will reject with a clear message.
pub fn remove_incomplete(path: &Path) {
    if let Err(err) = std::fs::remove_file(path)
        && err.kind() != std::io::ErrorKind::NotFound
    {
        log::warn!(
            "could not remove incomplete project {}: {err}",
            path.display()
        );
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use redb::ReadableDatabase;

    use crate::project::queries;

    use super::*;

    /// A unique path under the system temporary directory, removed on drop.
    struct TempFile {
        path: PathBuf,
    }

    impl TempFile {
        fn new(label: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "perseid-backend-{label}-{}.perseid",
                uuid::Uuid::new_v4()
            ));
            Self { path }
        }
    }

    impl Drop for TempFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.path);
        }
    }

    fn meta() -> ProjectMeta {
        ProjectMeta::new("backend-tests".to_string())
    }

    #[test]
    fn memory_project_starts_ready_with_the_schema_in_place() {
        let meta = meta();
        let db = create_memory(&meta).expect("create memory project");
        schema::validate_tables(&db).expect("schema tables");

        let read = db.begin_read().expect("read transaction");
        let stored = schema::read_meta(&read).expect("read meta");
        assert_eq!(stored.project_id, meta.project_id);
        assert_eq!(stored.state, schema::STATE_READY);
        assert_eq!(stored.schema_version, schema::SCHEMA_VERSION);
        assert_eq!(stored.next_http_id, 1);
        assert_eq!(stored.http_count, 0);
    }

    #[test]
    fn memory_projects_do_not_share_data() {
        let first = create_memory(&meta()).expect("first project");
        let second = create_memory(&meta()).expect("second project");

        let tx = first.begin_write().expect("begin write transaction");
        {
            tx.open_table(schema::HTTP_EXCHANGES)
                .expect("exchanges table")
                .insert(1u64, b"{}".as_slice())
                .expect("insert record");
        }
        tx.commit().expect("commit");

        assert_eq!(queries::count_http(&first).expect("count"), 1);
        assert_eq!(queries::count_http(&second).expect("count"), 0);
    }

    #[test]
    fn creating_an_initializing_file_refuses_to_replace_existing_files() {
        let file = TempFile::new("exists");
        std::fs::write(&file.path, b"do not replace").expect("write existing file");

        let error = create_initializing(&file.path, &meta()).expect_err("must refuse");
        assert!(matches!(error, ProjectError::DestinationExists(ref path) if path == &file.path));
        assert_eq!(
            std::fs::read(&file.path).expect("read file"),
            b"do not replace"
        );
    }

    #[test]
    fn create_initializing_marks_the_project_initializing_until_ready() {
        let file = TempFile::new("state");
        let db = create_initializing(&file.path, &meta()).expect("create file");

        let read = db.begin_read().expect("read transaction");
        assert_eq!(
            schema::read_meta(&read).expect("meta").state,
            STATE_INITIALIZING
        );
        drop(read);
        drop(db);

        let reopened = open_file(&file.path).expect("reopen file");
        let read = reopened.begin_read().expect("read transaction");
        assert_eq!(
            schema::read_meta(&read).expect("meta").state,
            STATE_INITIALIZING
        );
    }

    #[cfg(unix)]
    #[test]
    fn initializing_files_are_owner_only_on_unix() {
        use std::os::unix::fs::PermissionsExt;

        let file = TempFile::new("mode");
        drop(create_initializing(&file.path, &meta()).expect("create file"));

        let mode = std::fs::metadata(&file.path)
            .expect("metadata")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600, "projects must not be world readable");
    }

    #[test]
    fn opening_a_file_that_is_already_open_reports_already_open() {
        let file = TempFile::new("already-open");
        drop(create_initializing(&file.path, &meta()).expect("create file"));

        let first = open_file(&file.path).expect("first open");
        let error = open_file(&file.path).expect_err("second open must fail");
        assert!(matches!(error, ProjectError::AlreadyOpen(ref path) if path == &file.path));
        drop(first);

        open_file(&file.path).expect("open after the first handle is dropped");
    }

    #[test]
    fn opening_a_non_database_file_fails() {
        let file = TempFile::new("junk");
        std::fs::write(&file.path, b"this is not a redb database").expect("write junk");

        let error = open_file(&file.path).expect_err("junk must be rejected");
        assert!(matches!(error, ProjectError::FileAccess { .. }));
        assert!(error.to_string().contains("restore a backup"));
    }

    #[test]
    fn a_valid_foreign_database_fails_schema_validation() {
        let file = TempFile::new("foreign");
        drop(
            redb::Database::builder()
                .create(&file.path)
                .expect("create foreign database"),
        );

        let db = open_file(&file.path).expect("redb can open its own database");
        let error = schema::validate_tables(&db).expect_err("foreign databases must be rejected");
        assert!(matches!(error, ProjectError::InvalidProject(_)));
    }

    #[test]
    fn remove_incomplete_deletes_files_and_tolerates_missing_paths() {
        let file = TempFile::new("incomplete");
        std::fs::write(&file.path, b"partial").expect("write file");

        remove_incomplete(&file.path);
        assert!(!file.path.exists());
        remove_incomplete(&file.path);
    }
}
