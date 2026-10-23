//! Database creation and opening.
//!
//! Temporary projects live in redb's [`InMemoryBackend`]; persistent projects
//! are one redb file. A fresh file is created exclusively so `Save As` can
//! never overwrite an existing project.

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
    let mut builder = Database::builder();
    builder.set_cache_size(DISK_CACHE_BYTES);
    let db = builder.open(path).map_err(|err| match err {
        DatabaseError::DatabaseAlreadyOpen => ProjectError::AlreadyOpen(path.to_path_buf()),
        other => ProjectError::Database(other),
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
