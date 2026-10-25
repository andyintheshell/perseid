//! Runtime state shared by the project handle, writer, and UI.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use redb::Database;

use super::super::schema::{self, ProjectMeta, STATE_MIGRATING};
use super::{ProjectMode, ProjectState, ProjectStatus};

pub(crate) struct Shared {
    pub(super) database: RwLock<Option<Arc<Database>>>,
    pub(super) status: Mutex<ProjectStatus>,
    pub(super) next_http_id: AtomicU64,
    pub(super) next_ws_id: AtomicU64,
    pub(super) next_ws_connection_id: AtomicU64,
    pub(super) next_http_number: AtomicU64,
    pub(super) next_ws_number: AtomicU64,
    pub(super) in_flight: Mutex<HashMap<u64, u64>>,
    pub(super) ws_connections: Mutex<HashMap<u64, u64>>,
    pub(super) pending_loss: AtomicU64,
    pub(super) generation: AtomicU64,
}

impl Shared {
    pub(super) fn new(database: Arc<Database>, meta: &ProjectMeta, mode: ProjectMode) -> Arc<Self> {
        Arc::new(Self {
            database: RwLock::new(Some(database)),
            status: Mutex::new(ProjectStatus {
                mode,
                state: state_from_meta(&meta.state),
                generation: 1,
                http_count: meta.http_count,
                ws_count: meta.ws_count,
                last_committed_sequence: meta.last_committed_sequence,
                capture_loss_count: meta.capture_loss_count,
                event_loss_count: meta.event_loss_count,
                retention_evictions: meta.retention_evictions,
                last_error: None,
            }),
            next_http_id: AtomicU64::new(meta.next_http_id),
            next_ws_id: AtomicU64::new(meta.next_ws_id),
            next_ws_connection_id: AtomicU64::new(meta.next_ws_connection_id),
            next_http_number: AtomicU64::new(meta.next_http_number),
            next_ws_number: AtomicU64::new(meta.next_ws_number),
            in_flight: Mutex::new(HashMap::new()),
            ws_connections: Mutex::new(HashMap::new()),
            pending_loss: AtomicU64::new(0),
            generation: AtomicU64::new(1),
        })
    }

    pub(super) fn database(&self) -> Option<Arc<Database>> {
        self.database.read().ok().and_then(|db| db.clone())
    }

    pub(super) fn set_database(&self, db: Arc<Database>) {
        if let Ok(mut slot) = self.database.write() {
            *slot = Some(db);
        }
    }

    pub(super) fn install_meta(&self, meta: &ProjectMeta, mode: ProjectMode) {
        self.reset_counters(meta);
        self.apply_status(meta, Some(mode), true);
    }

    /// Refresh after an ordinary write without changing generation. Counters
    /// use `fetch_max`: the UI reserves project IDs before events reach the
    /// writer, so persisted metadata can legitimately lag the atomic values.
    /// Lowering them here would hand out an ID twice.
    pub(super) fn refresh_meta(&self, meta: &ProjectMeta) {
        self.reserve_counters(meta);
        self.apply_status(meta, None, false);
    }

    /// A Save As keeps the same project identity and any IDs already reserved
    /// by queued capture events.
    pub(super) fn install_saved_meta(&self, meta: &ProjectMeta, mode: ProjectMode) {
        self.reserve_counters(meta);
        self.apply_status(meta, Some(mode), true);
    }

    pub(super) fn apply_status(
        &self,
        meta: &ProjectMeta,
        mode: Option<ProjectMode>,
        bump_generation: bool,
    ) {
        if let Ok(mut status) = self.status.lock() {
            if let Some(mode) = mode {
                status.mode = mode;
                status.last_error = None;
            }
            status.state = state_from_meta(&meta.state);
            status.http_count = meta.http_count;
            status.ws_count = meta.ws_count;
            status.last_committed_sequence = meta.last_committed_sequence;
            status.capture_loss_count = meta.capture_loss_count;
            status.event_loss_count = meta.event_loss_count;
            status.retention_evictions = meta.retention_evictions;
            if bump_generation {
                status.generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
            }
        }
    }

    pub(super) fn reset_counters(&self, meta: &ProjectMeta) {
        self.next_http_id.store(meta.next_http_id, Ordering::SeqCst);
        self.next_ws_id.store(meta.next_ws_id, Ordering::SeqCst);
        self.next_ws_connection_id
            .store(meta.next_ws_connection_id, Ordering::SeqCst);
        self.next_http_number
            .store(meta.next_http_number, Ordering::SeqCst);
        self.next_ws_number
            .store(meta.next_ws_number, Ordering::SeqCst);
    }

    pub(super) fn reserve_counters(&self, meta: &ProjectMeta) {
        self.next_http_id
            .fetch_max(meta.next_http_id, Ordering::SeqCst);
        self.next_ws_id.fetch_max(meta.next_ws_id, Ordering::SeqCst);
        self.next_ws_connection_id
            .fetch_max(meta.next_ws_connection_id, Ordering::SeqCst);
        self.next_http_number
            .fetch_max(meta.next_http_number, Ordering::SeqCst);
        self.next_ws_number
            .fetch_max(meta.next_ws_number, Ordering::SeqCst);
    }

    pub(super) fn set_status_error(&self, error: String) {
        if let Ok(mut status) = self.status.lock() {
            status.last_error = Some(error);
        }
    }

    pub(super) fn clear_mappings(&self) {
        if let Ok(mut map) = self.in_flight.lock() {
            map.clear();
        }
        if let Ok(mut map) = self.ws_connections.lock() {
            map.clear();
        }
    }

    pub(super) fn status(&self) -> ProjectStatus {
        let mut status = self
            .status
            .lock()
            .map(|status| status.clone())
            .unwrap_or(ProjectStatus {
                mode: ProjectMode::Temporary,
                state: ProjectState::Ready,
                generation: self.generation.load(Ordering::SeqCst),
                http_count: 0,
                ws_count: 0,
                last_committed_sequence: 0,
                capture_loss_count: 0,
                event_loss_count: 0,
                retention_evictions: 0,
                last_error: Some("project status unavailable".to_string()),
            });
        status.capture_loss_count = status
            .capture_loss_count
            .saturating_add(self.pending_loss.load(Ordering::SeqCst));
        status
    }
}

fn state_from_meta(state: &str) -> ProjectState {
    match state {
        schema::STATE_INITIALIZING => ProjectState::Initializing,
        STATE_MIGRATING => ProjectState::Migrating,
        _ => ProjectState::Ready,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_states_map_to_project_states() {
        assert_eq!(
            state_from_meta(schema::STATE_INITIALIZING),
            ProjectState::Initializing
        );
        assert_eq!(state_from_meta(STATE_MIGRATING), ProjectState::Migrating);
        assert_eq!(state_from_meta(schema::STATE_READY), ProjectState::Ready);
        assert_eq!(state_from_meta("future-state"), ProjectState::Ready);
    }
}
