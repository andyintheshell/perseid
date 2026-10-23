//! Local project persistence.
//!
//! A project is a single redb database. At startup Perseid creates a
//! temporary in-memory project; `Save As` transfers it to a `.perseid` file and
//! switches the writer to that file. Every subsequent accepted mutation is
//! committed automatically.

mod backend;
mod codec;
mod queries;
mod records;
mod schema;
mod service;
mod transfer;

use std::path::PathBuf;

pub use records::{
    DecoderPanelWorkspace, IntruderAttackRecord, IntruderWorkspace, RepeaterWorkspace,
    RuleDirectionRecord, RulePartRecord, RuleWorkspace, ScopeWorkspace, WorkspaceState,
};
pub use service::{CommitReceipt, OpenedProject, ProjectHandle, ProjectMode, ProjectStatus};

/// Errors returned by the project subsystem.
#[derive(Debug, thiserror::Error)]
pub enum ProjectError {
    #[error("could not access the project file: {0}")]
    Io(#[from] std::io::Error),
    #[error("database error: {0}")]
    Database(#[from] redb::DatabaseError),
    #[error("transaction error: {0}")]
    Transaction(#[from] redb::TransactionError),
    #[error("table error: {0}")]
    Table(#[from] redb::TableError),
    #[error("storage error: {0}")]
    Storage(#[from] redb::StorageError),
    #[error("commit error: {0}")]
    Commit(#[from] redb::CommitError),
    #[error("durability error: {0}")]
    Durability(#[from] redb::SetDurabilityError),
    #[error("record encoding error: {0}")]
    Encoding(#[from] serde_json::Error),
    #[error("project record is too large ({size} bytes, limit {limit})")]
    RecordTooLarge { size: usize, limit: usize },
    #[error("this is not a valid Perseid project: {0}")]
    InvalidProject(String),
    #[error("the project was created by a newer version (schema {found}, supported {supported})")]
    FutureSchema { found: u64, supported: u64 },
    #[error("the project uses an unsupported record version ({found}, supported {supported})")]
    UnsupportedRecord { found: u16, supported: u16 },
    #[error("project creation was interrupted and the file is incomplete: {}", .0.display())]
    IncompleteProject(PathBuf),
    #[error("a file already exists at {}", .0.display())]
    DestinationExists(PathBuf),
    #[error("the project is already open for writing: {}", .0.display())]
    AlreadyOpen(PathBuf),
    #[error("the project writer is not available")]
    ServiceUnavailable,
}

type Result<T> = std::result::Result<T, ProjectError>;
