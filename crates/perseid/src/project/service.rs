//! The single project writer and the UI-facing handle.
//!
//! All writes are serialized on one dedicated thread. The UI and proxy never
//! touch a write transaction directly, so capture ordering, index maintenance
//! and file transfers are deterministic. Reads use short-lived snapshot
//! transactions through [`super::queries`].

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender, TrySendError};
use std::sync::{Arc, Mutex, RwLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use redb::{Database, Durability, ReadableDatabase, ReadableTable};

use crate::history::HistoryItem;
use crate::proxy::{ExchangeResult, ProxyEvent};
use crate::websocket_history::WebSocketMessageItem;

use super::backend;
use super::codec;
use super::queries::{self, HistoryCursor, HttpRow, WsRow};
use super::records::{
    BodyCaptureRecord, HttpExchangeRecord, HttpPhaseRecord, WorkspaceState, WsConnectionRecord,
    WsMessageRecord, normalized_event,
};
use super::schema::{self, ProjectMeta, STATE_MIGRATING, STATE_READY};
use super::{ProjectError, Result};

const COMMAND_QUEUE: usize = 4096;
const CAPTURE_BATCH: usize = 128;
const CAPTURE_BATCH_WINDOW: Duration = Duration::from_millis(50);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProjectMode {
    Temporary,
    Persistent { path: PathBuf },
}

impl ProjectMode {
    pub fn path(&self) -> Option<&Path> {
        match self {
            Self::Temporary => None,
            Self::Persistent { path } => Some(path),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ProjectState {
    #[default]
    Ready,
    Initializing,
    Migrating,
}

#[derive(Clone, Debug)]
pub struct ProjectStatus {
    pub mode: ProjectMode,
    pub state: ProjectState,
    pub generation: u64,
    pub http_count: u64,
    pub ws_count: u64,
    pub last_committed_sequence: u64,
    pub capture_loss_count: u64,
    pub event_loss_count: u64,
    pub last_error: Option<String>,
}

#[derive(Clone, Debug)]
#[allow(dead_code)] // Receipts are inspected by tests and future save status UI.
pub struct CommitReceipt {
    pub sequence: u64,
    pub http_count: u64,
    pub ws_count: u64,
}

#[derive(Clone, Debug)]
pub struct OpenedProject {
    pub http: Vec<HistoryItem>,
    pub ws: Vec<WebSocketMessageItem>,
    pub workspace: WorkspaceState,
    pub http_total: u64,
    pub ws_total: u64,
}

/// A capture after project IDs have been assigned.
#[derive(Clone, Debug)]
pub struct CaptureOutcome {
    pub event: Option<ProxyEvent>,
}

enum Command {
    Capture(ProxyEvent),
    SetWorkspace(WorkspaceState),
    ClearHttp(Sender<Result<CommitReceipt>>),
    ClearWs(Sender<Result<CommitReceipt>>),
    InterruptInFlight {
        reason: String,
    },
    SaveAs {
        path: PathBuf,
        reply: Sender<Result<()>>,
    },
    SaveCopy {
        path: PathBuf,
        reply: Sender<Result<()>>,
    },
    Open {
        path: PathBuf,
        ui_limit: usize,
        ui_bytes: usize,
        reply: Sender<Result<OpenedProject>>,
    },
    New {
        ui_limit: usize,
        ui_bytes: usize,
        reply: Sender<Result<OpenedProject>>,
    },
    Flush {
        reply: Sender<Result<CommitReceipt>>,
    },
    Shutdown {
        reply: Sender<Result<()>>,
    },
}

pub(crate) struct Shared {
    database: RwLock<Option<Arc<Database>>>,
    status: Mutex<ProjectStatus>,
    next_http_id: AtomicU64,
    next_ws_id: AtomicU64,
    next_ws_connection_id: AtomicU64,
    next_http_number: AtomicU64,
    next_ws_number: AtomicU64,
    in_flight: Mutex<HashMap<u64, u64>>,
    ws_connections: Mutex<HashMap<u64, u64>>,
    pending_loss: AtomicU64,
    generation: AtomicU64,
}

impl Shared {
    fn new(database: Arc<Database>, meta: &ProjectMeta, mode: ProjectMode) -> Arc<Self> {
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

    fn database(&self) -> Option<Arc<Database>> {
        self.database.read().ok().and_then(|db| db.clone())
    }

    fn set_database(&self, db: Arc<Database>) {
        if let Ok(mut slot) = self.database.write() {
            *slot = Some(db);
        }
    }

    fn install_meta(&self, meta: &ProjectMeta, mode: ProjectMode) {
        self.reset_counters(meta);
        self.apply_status(meta, Some(mode), true);
    }

    /// Refresh after an ordinary write without changing generation. Counters
    /// use `fetch_max`: the UI reserves project IDs before events reach the
    /// writer, so persisted metadata can legitimately lag the atomic values.
    /// Lowering them here would hand out an ID twice.
    fn refresh_meta(&self, meta: &ProjectMeta) {
        self.reserve_counters(meta);
        self.apply_status(meta, None, false);
    }

    /// A Save As keeps the same project identity and any IDs already reserved
    /// by queued capture events.
    fn install_saved_meta(&self, meta: &ProjectMeta, mode: ProjectMode) {
        self.reserve_counters(meta);
        self.apply_status(meta, Some(mode), true);
    }

    fn apply_status(&self, meta: &ProjectMeta, mode: Option<ProjectMode>, bump_generation: bool) {
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
            if bump_generation {
                status.generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
            }
        }
    }

    fn reset_counters(&self, meta: &ProjectMeta) {
        self.next_http_id.store(meta.next_http_id, Ordering::SeqCst);
        self.next_ws_id.store(meta.next_ws_id, Ordering::SeqCst);
        self.next_ws_connection_id
            .store(meta.next_ws_connection_id, Ordering::SeqCst);
        self.next_http_number
            .store(meta.next_http_number, Ordering::SeqCst);
        self.next_ws_number
            .store(meta.next_ws_number, Ordering::SeqCst);
    }

    fn reserve_counters(&self, meta: &ProjectMeta) {
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

    fn set_status_error(&self, error: String) {
        if let Ok(mut status) = self.status.lock() {
            status.last_error = Some(error);
        }
    }

    fn clear_mappings(&self) {
        if let Ok(mut map) = self.in_flight.lock() {
            map.clear();
        }
        if let Ok(mut map) = self.ws_connections.lock() {
            map.clear();
        }
    }

    fn status(&self) -> ProjectStatus {
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

/// UI-facing handle. Dropping it flushes and stops the writer thread.
pub struct ProjectHandle {
    sender: SyncSender<Command>,
    shared: Arc<Shared>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl ProjectHandle {
    /// Start a new temporary, in-memory project.
    pub fn temporary() -> Result<Self> {
        let meta = ProjectMeta::new(new_project_id());
        let db = backend::create_memory(&meta)?;
        Self::from_database(db, ProjectMode::Temporary)
    }

    fn from_database(db: Database, mode: ProjectMode) -> Result<Self> {
        // Re-read the metadata written by the backend so the service and the
        // file can never disagree about counters or identity.
        let read = db.begin_read()?;
        let meta = schema::read_meta(&read)?;
        drop(read);
        let db = Arc::new(db);
        let shared = Shared::new(db.clone(), &meta, mode);
        let (sender, receiver) = mpsc::sync_channel(COMMAND_QUEUE);
        let thread = {
            let shared = shared.clone();
            thread::Builder::new()
                .name("perseid-project-writer".to_string())
                .spawn(move || run_writer(receiver, shared, db, meta))
                .map_err(ProjectError::Io)?
        };
        Ok(Self {
            sender,
            shared,
            thread: Mutex::new(Some(thread)),
        })
    }

    pub fn status(&self) -> ProjectStatus {
        self.shared.status()
    }

    /// Assign stable project IDs and enqueue the event for durable storage.
    ///
    /// The returned event is the one the UI should apply to its bounded cache.
    pub fn capture(&self, event: &ProxyEvent) -> CaptureOutcome {
        if let ProxyEvent::Started { request, .. } = event
            && request.method.eq_ignore_ascii_case("CONNECT")
        {
            return CaptureOutcome { event: None };
        }
        let normalized = match event {
            ProxyEvent::Started { .. } => {
                let id = self.shared.next_http_id.fetch_add(1, Ordering::SeqCst);
                if let Ok(mut map) = self.shared.in_flight.lock() {
                    map.insert(event.id(), id);
                }
                normalized_event(event, id)
            }
            ProxyEvent::ResponseHead { .. } => {
                let mapped = self
                    .shared
                    .in_flight
                    .lock()
                    .ok()
                    .and_then(|map| map.get(&event.id()).copied());
                match mapped {
                    Some(id) => normalized_event(event, id),
                    None => {
                        return CaptureOutcome {
                            event: Some(event.clone()),
                        };
                    }
                }
            }
            ProxyEvent::Finished { .. } => {
                let mapped = self
                    .shared
                    .in_flight
                    .lock()
                    .ok()
                    .and_then(|mut map| map.remove(&event.id()));
                match mapped {
                    Some(id) => normalized_event(event, id),
                    None => {
                        return CaptureOutcome {
                            event: Some(event.clone()),
                        };
                    }
                }
            }
            ProxyEvent::WebSocketMessage {
                timestamp,
                connection,
                direction,
                opcode,
                payload,
                ..
            } => {
                let id = self.shared.next_ws_id.fetch_add(1, Ordering::SeqCst);
                let connection_id = {
                    let mut map = match self.shared.ws_connections.lock() {
                        Ok(map) => map,
                        Err(_) => {
                            return CaptureOutcome {
                                event: Some(event.clone()),
                            };
                        }
                    };
                    *map.entry(connection.id).or_insert_with(|| {
                        self.shared
                            .next_ws_connection_id
                            .fetch_add(1, Ordering::SeqCst)
                    })
                };
                let mut connection = connection.clone();
                connection.id = connection_id;
                ProxyEvent::WebSocketMessage {
                    id,
                    timestamp: *timestamp,
                    connection,
                    direction: *direction,
                    opcode: *opcode,
                    payload: payload.clone(),
                }
            }
        };

        match self.sender.try_send(Command::Capture(normalized.clone())) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                self.shared.pending_loss.fetch_add(1, Ordering::SeqCst);
            }
            Err(TrySendError::Disconnected(_)) => {}
        }
        CaptureOutcome {
            event: Some(normalized),
        }
    }

    /// Queue a workbench snapshot. Returns false when the queue is full, so
    /// the caller can retry instead of believing the state was persisted.
    pub fn set_workspace(&self, workspace: WorkspaceState) -> bool {
        self.sender
            .try_send(Command::SetWorkspace(workspace))
            .is_ok()
    }

    pub fn interrupt_in_flight(&self, reason: impl Into<String>) {
        self.shared.clear_mappings();
        let _ = self.sender.try_send(Command::InterruptInFlight {
            reason: reason.into(),
        });
    }

    pub fn save_as(&self, path: PathBuf) -> Receiver<Result<()>> {
        let (tx, rx) = mpsc::channel();
        self.send_control(Command::SaveAs { path, reply: tx }, &rx);
        rx
    }

    pub fn save_copy(&self, path: PathBuf) -> Receiver<Result<()>> {
        let (tx, rx) = mpsc::channel();
        self.send_control(Command::SaveCopy { path, reply: tx }, &rx);
        rx
    }

    pub fn open(
        &self,
        path: PathBuf,
        ui_limit: usize,
        ui_bytes: usize,
    ) -> Receiver<Result<OpenedProject>> {
        let (tx, rx) = mpsc::channel();
        self.send_control(
            Command::Open {
                path,
                ui_limit,
                ui_bytes,
                reply: tx,
            },
            &rx,
        );
        rx
    }

    pub fn new_project(&self, ui_limit: usize, ui_bytes: usize) -> Receiver<Result<OpenedProject>> {
        let (tx, rx) = mpsc::channel();
        self.send_control(
            Command::New {
                ui_limit,
                ui_bytes,
                reply: tx,
            },
            &rx,
        );
        rx
    }

    pub fn flush(&self) -> Receiver<Result<CommitReceipt>> {
        let (tx, rx) = mpsc::channel();
        self.send_control(Command::Flush { reply: tx }, &rx);
        rx
    }

    pub fn clear_http(&self) -> Receiver<Result<CommitReceipt>> {
        let (tx, rx) = mpsc::channel();
        self.send_control(Command::ClearHttp(tx), &rx);
        rx
    }

    pub fn clear_ws(&self) -> Receiver<Result<CommitReceipt>> {
        let (tx, rx) = mpsc::channel();
        self.send_control(Command::ClearWs(tx), &rx);
        rx
    }

    fn send_control<T>(&self, command: Command, receiver: &Receiver<T>) {
        if self.sender.send(command).is_err() {
            // The receiver would otherwise wait forever. Dropping the reply
            // sender happens inside the failed command; callers treat a
            // disconnected channel as an error.
            log::error!("project writer is no longer running");
            let _ = receiver;
        }
    }

    pub fn shutdown(&self) {
        let (tx, rx) = mpsc::channel();
        if self.sender.send(Command::Shutdown { reply: tx }).is_ok() {
            let _ = rx.recv_timeout(Duration::from_secs(30));
        }
        if let Ok(mut thread) = self.thread.lock()
            && let Some(thread) = thread.take()
        {
            let _ = thread.join();
        }
    }

    #[allow(dead_code)] // Used by the paged-history UI and tests.
    pub fn list_http(&self, before: Option<HistoryCursor>, limit: usize) -> Result<Vec<HttpRow>> {
        let db = self.current_db()?;
        queries::list_http(&db, before, limit)
    }

    #[allow(dead_code)]
    pub fn list_ws(&self, before: Option<HistoryCursor>, limit: usize) -> Result<Vec<WsRow>> {
        let db = self.current_db()?;
        queries::list_ws(&db, before, limit)
    }

    #[allow(dead_code)]
    pub fn get_http(&self, id: u64) -> Result<Option<HistoryItem>> {
        let db = self.current_db()?;
        queries::get_http(&db, id)
    }

    #[allow(dead_code)]
    pub fn get_ws(&self, id: u64) -> Result<Option<WebSocketMessageItem>> {
        let db = self.current_db()?;
        queries::get_ws(&db, id)
    }

    #[allow(dead_code)]
    pub fn search_http(
        &self,
        needle: &str,
        include_bodies: bool,
        limit: usize,
    ) -> Result<Vec<u64>> {
        let db = self.current_db()?;
        queries::search_http(&db, needle, include_bodies, limit)
    }

    #[allow(dead_code)]
    fn current_db(&self) -> Result<Arc<Database>> {
        self.shared
            .database()
            .ok_or(ProjectError::ServiceUnavailable)
    }
}

impl Drop for ProjectHandle {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn new_project_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

fn run_writer(
    receiver: Receiver<Command>,
    shared: Arc<Shared>,
    initial_db: Arc<Database>,
    mut meta: ProjectMeta,
) {
    let mut db = initial_db;
    let mut pending: VecDeque<Command> = VecDeque::new();
    let mut last_workspace: Option<Vec<u8>> = None;
    loop {
        let command = match pending.pop_front() {
            Some(command) => command,
            None => match receiver.recv() {
                Ok(command) => command,
                Err(_) => break,
            },
        };
        match command {
            Command::Capture(first) => {
                let mut events = vec![first];
                let deadline = Instant::now() + CAPTURE_BATCH_WINDOW;
                while events.len() < CAPTURE_BATCH {
                    let now = Instant::now();
                    if now >= deadline {
                        break;
                    }
                    match receiver.recv_timeout(deadline.saturating_duration_since(now)) {
                        Ok(Command::Capture(event)) => events.push(event),
                        Ok(other) => {
                            pending.push_back(other);
                            break;
                        }
                        Err(RecvTimeoutError::Timeout) => break,
                        Err(RecvTimeoutError::Disconnected) => break,
                    }
                }
                absorb_pending_loss(&shared, &mut meta);
                match apply_capture_batch(&db, &mut meta, &events) {
                    Ok(()) => {
                        shared.refresh_meta(&meta);
                    }
                    Err(err) => {
                        log::error!("project capture write failed: {err}");
                        shared.set_status_error(err.to_string());
                    }
                }
            }
            Command::SetWorkspace(workspace) => match codec::encode(&workspace) {
                Ok(bytes) => {
                    if last_workspace.as_deref() != Some(bytes.as_slice()) {
                        match write_workspace(&db, &bytes) {
                            Ok(()) => last_workspace = Some(bytes),
                            Err(err) => shared.set_status_error(err.to_string()),
                        }
                    }
                }
                Err(err) => shared.set_status_error(err.to_string()),
            },
            Command::ClearHttp(reply) => {
                absorb_pending_loss(&shared, &mut meta);
                let result = clear_http(&db, &mut meta);
                finish(reply, result, &shared, &meta);
            }
            Command::ClearWs(reply) => {
                absorb_pending_loss(&shared, &mut meta);
                let result = clear_ws(&db, &mut meta);
                finish(reply, result, &shared, &meta);
            }
            Command::InterruptInFlight { reason } => {
                match interrupt_in_flight(&db, &mut meta, &reason) {
                    Ok(()) => shared.refresh_meta(&meta),
                    Err(err) => shared.set_status_error(err.to_string()),
                }
            }
            Command::SaveAs { path, reply } => {
                absorb_pending_loss(&shared, &mut meta);
                let result = save_as(&mut db, &shared, &mut meta, &path);
                match result {
                    Ok(()) => {
                        let _ = reply.send(Ok(()));
                    }
                    Err(err) => {
                        shared.set_status_error(err.to_string());
                        let _ = reply.send(Err(err));
                    }
                }
            }
            Command::SaveCopy { path, reply } => {
                let result = super::transfer::save_copy(&db, &path);
                match result {
                    Ok(()) => {
                        let _ = reply.send(Ok(()));
                    }
                    Err(err) => {
                        shared.set_status_error(err.to_string());
                        let _ = reply.send(Err(err));
                    }
                }
            }
            Command::Open {
                path,
                ui_limit,
                ui_bytes,
                reply,
            } => match open_project(&db, &path, ui_limit, ui_bytes) {
                Ok((new_db, new_meta, opened)) => {
                    db = new_db;
                    meta = new_meta;
                    shared.set_database(db.clone());
                    shared.install_meta(&meta, ProjectMode::Persistent { path });
                    shared.clear_mappings();
                    last_workspace = None;
                    let _ = reply.send(Ok(opened));
                }
                Err(err) => {
                    shared.set_status_error(err.to_string());
                    let _ = reply.send(Err(err));
                }
            },
            Command::New {
                ui_limit,
                ui_bytes,
                reply,
            } => {
                let new_meta = ProjectMeta::new(new_project_id());
                match backend::create_memory(&new_meta) {
                    Ok(new_db) => {
                        let new_db = Arc::new(new_db);
                        let opened = build_snapshot(&new_db, &new_meta, ui_limit, ui_bytes);
                        db = new_db;
                        meta = new_meta;
                        shared.set_database(db.clone());
                        shared.install_meta(&meta, ProjectMode::Temporary);
                        shared.clear_mappings();
                        last_workspace = None;
                        let _ = reply.send(Ok(opened));
                    }
                    Err(err) => {
                        let _ = reply.send(Err(err));
                    }
                }
            }
            Command::Flush { reply } => {
                absorb_pending_loss(&shared, &mut meta);
                // The metadata write also makes the loss counter and counters
                // durable if no capture write occurred since the last batch.
                let result = commit_counters(&db, &meta);
                match result {
                    Ok(()) => {
                        let _ = reply.send(Ok(receipt(&meta)));
                    }
                    Err(err) => {
                        shared.set_status_error(err.to_string());
                        let _ = reply.send(Err(err));
                    }
                }
            }
            Command::Shutdown { reply } => {
                absorb_pending_loss(&shared, &mut meta);
                let result = commit_counters(&db, &meta);
                match result {
                    Ok(()) => {
                        let _ = reply.send(Ok(()));
                    }
                    Err(err) => {
                        shared.set_status_error(err.to_string());
                        let _ = reply.send(Err(err));
                    }
                }
                break;
            }
        }
    }
}

fn finish<T>(reply: Sender<Result<T>>, result: Result<T>, shared: &Shared, meta: &ProjectMeta) {
    match result {
        Ok(value) => {
            shared.refresh_meta(meta);
            let _ = reply.send(Ok(value));
        }
        Err(err) => {
            shared.set_status_error(err.to_string());
            let _ = reply.send(Err(err));
        }
    }
}

fn absorb_pending_loss(shared: &Shared, meta: &mut ProjectMeta) {
    let pending = shared.pending_loss.swap(0, Ordering::SeqCst);
    if pending > 0 {
        meta.capture_loss_count = meta.capture_loss_count.saturating_add(pending);
    }
}

fn receipt(meta: &ProjectMeta) -> CommitReceipt {
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

fn apply_capture_batch(db: &Database, meta: &mut ProjectMeta, events: &[ProxyEvent]) -> Result<()> {
    let tx = begin_write(db)?;
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
                    let number = meta.next_http_number;
                    meta.next_http_number = meta.next_http_number.saturating_add(1);
                    meta.next_http_id = meta.next_http_id.max(id.saturating_add(1));
                    let record =
                        HttpExchangeRecord::started(*id, number, *timestamp, request.clone());
                    http.insert(*id, codec::encode(&record)?.as_slice())?;
                    by_time.insert((record.started_at_ms, *id), ())?;
                    let host = normalized_host(&record.request.host);
                    if !host.contains('\0') {
                        by_host.insert((host, *id), ())?;
                    }
                    in_flight.insert(*id, ())?;
                    meta.http_count = meta.http_count.saturating_add(1);
                }
                ProxyEvent::ResponseHead { id, head } => {
                    let Some(value) = http.get(*id)? else {
                        meta.event_loss_count = meta.event_loss_count.saturating_add(1);
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
                        meta.event_loss_count = meta.event_loss_count.saturating_add(1);
                        continue;
                    };
                    let mut record: HttpExchangeRecord = codec::decode(value.value())?;
                    drop(value);
                    record.duration_ms = Some(end.duration.as_millis() as u64);
                    record.request_body = BodyCaptureRecord::from_body(&end.request_body);
                    if !end.request_body.data().is_empty() {
                        bodies.insert((*id, 0), end.request_body.data())?;
                    }
                    match &end.result {
                        ExchangeResult::Response { body } => {
                            record.phase = HttpPhaseRecord::Completed {
                                duration_ms: end.duration.as_millis() as u64,
                            };
                            record.response_body = BodyCaptureRecord::from_body(body);
                            if !body.data().is_empty() {
                                bodies.insert((*id, 1), body.data())?;
                            }
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
                        ExchangeResult::Tunnel { .. } | ExchangeResult::InterceptedTls { .. } => {
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
                        ws_connections.insert(connection.id, codec::encode(&record)?.as_slice())?;
                        meta.next_ws_connection_id = meta
                            .next_ws_connection_id
                            .max(connection.id.saturating_add(1));
                    }
                    let number = meta.next_ws_number;
                    meta.next_ws_number = meta.next_ws_number.saturating_add(1);
                    meta.next_ws_id = meta.next_ws_id.max(id.saturating_add(1));
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
                    if !payload.data().is_empty() {
                        ws_payloads.insert(*id, payload.data())?;
                    }
                    ws_by_time.insert((record.timestamp_ms, *id), ())?;
                    ws_by_connection.insert((connection.id, *id), ())?;
                    meta.ws_count = meta.ws_count.saturating_add(1);
                }
            }
            meta.next_event_sequence = meta.next_event_sequence.saturating_add(1);
            meta.last_committed_sequence = meta.next_event_sequence.saturating_sub(1);
        }
        let mut table = tx.open_table(schema::META)?;
        schema::write_counters(&mut table, meta)?;
    }
    tx.commit()?;
    Ok(())
}

fn normalized_host(host: &str) -> String {
    host.trim_end_matches('.').to_ascii_lowercase()
}

fn write_workspace(db: &Database, bytes: &[u8]) -> Result<()> {
    let tx = begin_write(db)?;
    {
        let mut table = tx.open_table(schema::WORKSPACE)?;
        table.insert(schema::WORKSPACE_KEY, bytes)?;
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
    let workspace: WorkspaceState = codec::decode(value.value())?;
    if workspace.version > WorkspaceState::CURRENT_VERSION {
        return Err(ProjectError::UnsupportedRecord {
            found: workspace.version,
            supported: WorkspaceState::CURRENT_VERSION,
        });
    }
    Ok(workspace)
}

fn clear_http(db: &Database, meta: &mut ProjectMeta) -> Result<CommitReceipt> {
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
        meta.http_count = 0;
        meta.next_http_number = 1;
        meta.last_committed_sequence = meta.next_event_sequence.saturating_sub(1);
        let mut table = tx.open_table(schema::META)?;
        schema::write_counters(&mut table, meta)?;
    }
    tx.commit()?;
    Ok(receipt(meta))
}

fn clear_ws(db: &Database, meta: &mut ProjectMeta) -> Result<CommitReceipt> {
    let tx = begin_write(db)?;
    {
        tx.open_table(schema::WS_MESSAGES)?.retain(|_, _| false)?;
        tx.open_table(schema::WS_PAYLOADS)?.retain(|_, _| false)?;
        tx.open_table(schema::WS_BY_TIME)?.retain(|_, _| false)?;
        tx.open_table(schema::WS_BY_CONNECTION)?
            .retain(|_, _| false)?;
        tx.open_table(schema::WS_CONNECTIONS)?
            .retain(|_, _| false)?;
        meta.ws_count = 0;
        meta.next_ws_number = 1;
        meta.last_committed_sequence = meta.next_event_sequence.saturating_sub(1);
        let mut table = tx.open_table(schema::META)?;
        schema::write_counters(&mut table, meta)?;
    }
    tx.commit()?;
    Ok(receipt(meta))
}

fn interrupt_in_flight(db: &Database, meta: &mut ProjectMeta, reason: &str) -> Result<()> {
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
            meta.event_loss_count = meta.event_loss_count.saturating_add(count);
            meta.last_committed_sequence = meta.next_event_sequence.saturating_sub(1);
            let mut table = tx.open_table(schema::META)?;
            schema::write_counters(&mut table, meta)?;
        }
    }
    tx.commit()?;
    Ok(())
}

fn commit_counters(db: &Database, meta: &ProjectMeta) -> Result<()> {
    let tx = begin_write(db)?;
    {
        let mut table = tx.open_table(schema::META)?;
        schema::write_counters(&mut table, meta)?;
    }
    tx.commit()?;
    Ok(())
}

fn save_as(
    db: &mut Arc<Database>,
    shared: &Shared,
    meta: &mut ProjectMeta,
    path: &Path,
) -> Result<()> {
    let read = db.begin_read()?;
    let source_meta = schema::read_meta(&read)?;
    drop(read);
    let destination = super::transfer::copy_to_new_file(db, path, &source_meta)?;
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

fn open_project(
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
    let opened = build_snapshot(&db, &meta, ui_limit, ui_bytes);
    Ok((db, meta, opened))
}

fn build_snapshot(
    db: &Database,
    meta: &ProjectMeta,
    ui_limit: usize,
    ui_bytes: usize,
) -> OpenedProject {
    let limit = ui_limit.max(1);
    let http_budget = ui_bytes.saturating_mul(3) / 4;
    let ws_budget = ui_bytes.saturating_sub(http_budget);
    let (http, http_total) = queries::load_http_snapshot(db, limit, http_budget.max(1))
        .unwrap_or_else(|err| {
            log::warn!("could not load the HTTP snapshot: {err}");
            (Vec::new(), meta.http_count)
        });
    let (ws, ws_total) =
        queries::load_ws_snapshot(db, limit, ws_budget.max(1)).unwrap_or_else(|err| {
            log::warn!("could not load the WebSocket snapshot: {err}");
            (Vec::new(), meta.ws_count)
        });
    let workspace = read_workspace(db).unwrap_or_else(|err| {
        log::warn!("could not load project workspace state: {err}");
        WorkspaceState::default()
    });
    OpenedProject {
        http,
        ws,
        workspace,
        http_total,
        ws_total,
    }
}

fn now_ms() -> u64 {
    schema::now_ms()
}

#[cfg(test)]
mod tests {
    use std::time::SystemTime;

    use crate::proxy::{
        CapturedBody, ExchangeEnd, ExchangeResult, RequestInfo, ResponseHead,
        WebSocketConnectionInfo, WebSocketDirection, WebSocketOpcode,
    };

    use super::*;

    const REPLY_TIMEOUT: Duration = Duration::from_secs(30);

    /// A unique path under the system temporary directory, removed on drop.
    struct TempProject {
        path: PathBuf,
    }

    impl TempProject {
        fn new(label: &str) -> Self {
            let path = std::env::temp_dir()
                .join(format!("perseid-{label}-{}.perseid", uuid::Uuid::new_v4()));
            Self { path }
        }
    }

    impl Drop for TempProject {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.path);
        }
    }

    fn timestamp(offset_ms: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_millis(1_700_000_000_000 + offset_ms)
    }

    fn started(wire_id: u64, method: &str) -> ProxyEvent {
        ProxyEvent::Started {
            id: wire_id,
            timestamp: timestamp(wire_id),
            request: RequestInfo {
                method: method.to_string(),
                uri: "http://example.com/api?x=1".to_string(),
                scheme: "http".to_string(),
                host: "example.com".to_string(),
                path: "/api?x=1".to_string(),
                version: "HTTP/1.1".to_string(),
                headers: vec![("host".to_string(), "example.com".to_string())],
            },
        }
    }

    fn response_head(wire_id: u64, status: u16) -> ProxyEvent {
        ProxyEvent::ResponseHead {
            id: wire_id,
            head: ResponseHead {
                version: "HTTP/1.1".to_string(),
                status,
                reason: String::new(),
                headers: vec![("content-type".to_string(), "text/plain".to_string())],
            },
        }
    }

    fn finished(wire_id: u64, request: &[u8], response: &[u8]) -> ProxyEvent {
        ProxyEvent::Finished {
            id: wire_id,
            end: ExchangeEnd {
                duration: Duration::from_millis(7),
                request_body: CapturedBody::from_bytes(request.to_vec()),
                result: ExchangeResult::Response {
                    body: CapturedBody::from_bytes(response.to_vec()),
                },
            },
        }
    }

    fn ws_message(wire_id: u64, wire_connection_id: u64, payload: &[u8]) -> ProxyEvent {
        ProxyEvent::WebSocketMessage {
            id: wire_id,
            timestamp: timestamp(wire_id),
            connection: WebSocketConnectionInfo {
                id: wire_connection_id,
                url: "wss://example.com/socket".to_string(),
                host: "example.com".to_string(),
                tls: true,
                listener_port: 8080,
                protocol: Some("chat".to_string()),
            },
            direction: WebSocketDirection::ToClient,
            opcode: WebSocketOpcode::Text,
            payload: CapturedBody::from_bytes(payload.to_vec()),
        }
    }

    fn wait<T>(receiver: Receiver<Result<T>>) -> Result<T> {
        receiver
            .recv_timeout(REPLY_TIMEOUT)
            .expect("project writer dropped the reply channel")
    }

    fn capture_all(project: &ProjectHandle, events: &[ProxyEvent]) -> Vec<u64> {
        events
            .iter()
            .filter_map(|event| project.capture(event).event)
            .map(|event| event.id())
            .collect()
    }

    #[test]
    fn temporary_project_assigns_distinct_ids() {
        let project = ProjectHandle::temporary().expect("temporary project");
        let event = started(1, "GET");
        let first = project.capture(&event).event.expect("normalized event");
        let second = project
            .capture(&event)
            .event
            .expect("duplicate normalized event");
        assert_ne!(first.id(), second.id());
        project.shutdown();
    }

    #[test]
    fn capture_round_trip_persists_http_and_websocket() {
        let project = ProjectHandle::temporary().expect("temporary project");

        let ids = capture_all(
            &project,
            &[
                started(42, "POST"),
                response_head(42, 201),
                finished(42, b"request-body", b"response-body"),
            ],
        );
        assert_eq!(ids, vec![1, 1, 1]);
        let http_id = ids[0];

        let first_ws = project
            .capture(&ws_message(100, 7, b"hello"))
            .event
            .expect("normalized websocket message");
        let second_ws = project
            .capture(&ws_message(101, 7, b"again"))
            .event
            .expect("normalized websocket message");
        let ProxyEvent::WebSocketMessage {
            id: ws_id,
            connection,
            ..
        } = &first_ws
        else {
            panic!("expected a websocket event");
        };
        assert_eq!(connection.id, 1, "wire connection IDs are project-local");
        assert_eq!(second_ws.id(), ws_id + 1);
        let ProxyEvent::WebSocketMessage {
            connection: second, ..
        } = &second_ws
        else {
            panic!("expected a websocket event");
        };
        assert_eq!(second.id, connection.id);

        let receipt = wait(project.flush()).expect("flush");
        assert_eq!(receipt.http_count, 1);
        assert_eq!(receipt.ws_count, 2);

        let http = project
            .get_http(http_id)
            .expect("read HTTP record")
            .expect("stored HTTP record");
        assert_eq!(http.id, http_id);
        assert_eq!(http.number, 1);
        assert_eq!(http.request.method, "POST");
        assert_eq!(http.status(), Some(201));
        assert_eq!(http.request_body.data(), b"request-body");
        assert_eq!(
            http.response_body().expect("response body").data(),
            b"response-body"
        );
        assert_eq!(http.duration(), Some(Duration::from_millis(7)));
        assert!(!http.is_in_flight());

        let ws = project
            .get_ws(*ws_id)
            .expect("read websocket record")
            .expect("stored websocket record");
        assert_eq!(ws.id, *ws_id);
        assert_eq!(ws.payload.data(), b"hello");
        assert_eq!(ws.connection.id, connection.id);
        assert_eq!(ws.connection.url, "wss://example.com/socket");
        assert_eq!(ws.direction, WebSocketDirection::ToClient);
        assert_eq!(ws.opcode, WebSocketOpcode::Text);

        let status = project.status();
        assert_eq!(status.http_count, 1);
        assert_eq!(status.ws_count, 2);
        assert_eq!(status.mode, ProjectMode::Temporary);
    }

    #[test]
    fn save_as_switches_to_a_persistent_project_that_reopens() {
        let project = ProjectHandle::temporary().expect("temporary project");
        let ids = capture_all(
            &project,
            &[
                started(5, "GET"),
                response_head(5, 200),
                finished(5, b"", b"ok"),
                ws_message(6, 3, b"message"),
            ],
        );
        let http_id = ids[0];

        let mut workspace = WorkspaceState::default();
        workspace.scope.enabled = true;
        workspace.scope.hosts = vec!["*.example.com".to_string()];
        assert!(project.set_workspace(workspace.clone()));
        wait(project.flush()).expect("flush");

        let file = TempProject::new("save-as");
        wait(project.save_as(file.path.clone())).expect("save as");
        let status = project.status();
        assert_eq!(status.mode.path(), Some(file.path.as_path()));
        assert_eq!(status.http_count, 1);
        assert_eq!(status.ws_count, 1);
        drop(project);

        let reopened = ProjectHandle::temporary().expect("temporary project");
        let opened =
            wait(reopened.open(file.path.clone(), 100, 1024 * 1024)).expect("open project");
        assert_eq!(opened.http_total, 1);
        assert_eq!(opened.ws_total, 1);
        assert_eq!(opened.http.len(), 1);
        assert_eq!(opened.ws.len(), 1);
        assert_eq!(opened.http[0].id, http_id);
        assert_eq!(opened.http[0].status(), Some(200));
        assert_eq!(opened.workspace, workspace);
        assert_eq!(
            reopened.status().mode.path(),
            Some(file.path.as_path()),
            "opening switches the writer to the project file"
        );

        let persisted = reopened
            .get_http(http_id)
            .expect("read persisted HTTP record")
            .expect("persisted HTTP record");
        assert_eq!(
            persisted.response_body().expect("response body").data(),
            b"ok"
        );
    }

    #[test]
    fn save_copy_leaves_the_source_open_and_creates_a_reopenable_file() {
        let project = ProjectHandle::temporary().expect("temporary project");
        let ids = capture_all(&project, &[started(9, "GET"), finished(9, b"", b"copy")]);
        let http_id = ids[0];
        wait(project.flush()).expect("flush");

        let file = TempProject::new("save-copy");
        wait(project.save_copy(file.path.clone())).expect("save copy");

        assert_eq!(project.status().mode, ProjectMode::Temporary);
        assert!(project.status().mode.path().is_none());
        let source = project
            .get_http(http_id)
            .expect("read source record")
            .expect("source record");
        assert_eq!(
            source.response_body().expect("response body").data(),
            b"copy"
        );
        drop(project);

        let reopened = ProjectHandle::temporary().expect("temporary project");
        let opened = wait(reopened.open(file.path.clone(), 100, 1024 * 1024)).expect("open copy");
        assert_eq!(opened.http_total, 1);
        assert_eq!(opened.http[0].id, http_id);
    }

    #[test]
    fn save_as_refuses_to_replace_an_existing_file() {
        let project = ProjectHandle::temporary().expect("temporary project");
        let file = TempProject::new("existing");
        std::fs::write(&file.path, b"do not replace me").expect("write existing file");

        let error = wait(project.save_as(file.path.clone())).expect_err("save must be refused");
        assert!(matches!(
            error,
            ProjectError::DestinationExists(ref path) if path == &file.path
        ));
        assert_eq!(
            std::fs::read(&file.path).expect("read existing file"),
            b"do not replace me"
        );
        assert_eq!(project.status().mode, ProjectMode::Temporary);
        assert!(project.get_http(1).expect("read project").is_none());
    }

    #[test]
    fn open_rejects_invalid_files_without_switching_projects() {
        let project = ProjectHandle::temporary().expect("temporary project");
        let ids = capture_all(&project, &[started(11, "GET"), finished(11, b"", b"kept")]);
        let http_id = ids[0];
        wait(project.flush()).expect("flush");

        let file = TempProject::new("junk");
        std::fs::write(&file.path, b"this is not a redb database").expect("write junk");
        wait(project.open(file.path.clone(), 100, 1024 * 1024))
            .expect_err("opening junk must fail");

        let status = project.status();
        assert_eq!(status.mode, ProjectMode::Temporary);
        assert_eq!(status.http_count, 1);
        let kept = project
            .get_http(http_id)
            .expect("read current project")
            .expect("current project still open");
        assert_eq!(kept.response_body().expect("response body").data(), b"kept");
    }

    #[test]
    fn open_rejects_an_interrupted_creation() {
        let file = TempProject::new("incomplete");
        let meta = ProjectMeta::new(new_project_id());
        let db = super::super::backend::create_initializing(&file.path, &meta)
            .expect("create initializing project");
        drop(db);

        let project = ProjectHandle::temporary().expect("temporary project");
        let error = wait(project.open(file.path.clone(), 100, 1024 * 1024))
            .expect_err("incomplete projects must be rejected");
        assert!(matches!(
            error,
            ProjectError::IncompleteProject(ref path) if path == &file.path
        ));
        assert_eq!(project.status().mode, ProjectMode::Temporary);
    }

    #[test]
    fn open_rejects_a_newer_schema_version() {
        let project = ProjectHandle::temporary().expect("temporary project");
        let file = TempProject::new("future-schema");
        wait(project.save_as(file.path.clone())).expect("save as");
        drop(project);

        // Bump the schema version behind the service's back.
        {
            let db = super::super::backend::open_file(&file.path).expect("open raw project");
            let tx = db.begin_write().expect("begin write");
            {
                let mut table = tx.open_table(schema::META).expect("meta table");
                table
                    .insert(
                        schema::KEY_SCHEMA_VERSION,
                        (schema::SCHEMA_VERSION + 1).to_string().as_bytes(),
                    )
                    .expect("bump schema version");
            }
            tx.commit().expect("commit");
        }

        let reopened = ProjectHandle::temporary().expect("temporary project");
        let error = wait(reopened.open(file.path.clone(), 100, 1024 * 1024))
            .expect_err("a newer schema must be rejected");
        assert!(matches!(
            error,
            ProjectError::FutureSchema { found, supported }
                if found == schema::SCHEMA_VERSION + 1 && supported == schema::SCHEMA_VERSION
        ));
        assert_eq!(reopened.status().mode, ProjectMode::Temporary);
    }

    #[test]
    fn open_repairs_exchanges_left_in_flight() {
        let project = ProjectHandle::temporary().expect("temporary project");
        let ids = capture_all(&project, &[started(21, "GET")]);
        let http_id = ids[0];
        wait(project.flush()).expect("flush");

        let file = TempProject::new("in-flight");
        wait(project.save_as(file.path.clone())).expect("save as");
        drop(project);

        let reopened = ProjectHandle::temporary().expect("temporary project");
        let opened =
            wait(reopened.open(file.path.clone(), 100, 1024 * 1024)).expect("open project");
        assert_eq!(opened.http_total, 1);
        let item = &opened.http[0];
        assert_eq!(item.id, http_id);
        assert!(!item.is_in_flight());
        assert_eq!(
            item.error(),
            Some("application terminated before exchange completed")
        );
        assert_eq!(reopened.status().event_loss_count, 1);
    }

    #[test]
    fn clearing_history_is_persisted() {
        let project = ProjectHandle::temporary().expect("temporary project");
        let ids = capture_all(
            &project,
            &[
                started(31, "GET"),
                finished(31, b"", b"http"),
                ws_message(32, 2, b"ws"),
            ],
        );
        let http_id = ids[0];
        let ws_id = ids[2];
        wait(project.flush()).expect("flush");

        let http_receipt = wait(project.clear_http()).expect("clear HTTP history");
        assert_eq!(http_receipt.http_count, 0);
        assert!(project.get_http(http_id).expect("read HTTP").is_none());
        assert_eq!(project.status().http_count, 0);

        let ws_receipt = wait(project.clear_ws()).expect("clear WebSocket history");
        assert_eq!(ws_receipt.ws_count, 0);
        assert!(project.get_ws(ws_id).expect("read WebSocket").is_none());
        assert_eq!(project.status().ws_count, 0);

        let file = TempProject::new("cleared");
        wait(project.save_as(file.path.clone())).expect("save as");
        drop(project);

        let reopened = ProjectHandle::temporary().expect("temporary project");
        let opened =
            wait(reopened.open(file.path.clone(), 100, 1024 * 1024)).expect("open project");
        assert_eq!(opened.http_total, 0);
        assert_eq!(opened.ws_total, 0);
        assert!(opened.http.is_empty());
        assert!(opened.ws.is_empty());
    }

    #[test]
    fn cleared_ids_are_not_reused() {
        let project = ProjectHandle::temporary().expect("temporary project");
        let ids = capture_all(&project, &[started(41, "GET"), finished(41, b"", b"one")]);
        let first_id = ids[0];
        wait(project.flush()).expect("flush");
        wait(project.clear_http()).expect("clear HTTP history");

        let new_id = capture_all(&project, &[started(42, "GET"), finished(42, b"", b"two")])[0];
        assert!(
            new_id > first_id,
            "clearing a project must not reuse durable IDs"
        );
        wait(project.flush()).expect("flush");
        assert!(project.get_http(first_id).expect("read old ID").is_none());
        assert_eq!(
            project
                .get_http(new_id)
                .expect("read new HTTP record")
                .expect("new record")
                .response_body()
                .expect("response body")
                .data(),
            b"two"
        );
        assert_eq!(project.status().http_count, 1);
    }

    #[test]
    fn workspace_is_persisted_without_switching_projects() {
        let project = ProjectHandle::temporary().expect("temporary project");
        let mut workspace = WorkspaceState::default();
        workspace.scope.enabled = true;
        workspace.scope.hosts = vec!["example.com".to_string()];
        assert!(project.set_workspace(workspace.clone()));
        wait(project.flush()).expect("flush");

        let file = TempProject::new("workspace");
        wait(project.save_copy(file.path.clone())).expect("save copy");
        drop(project);

        let reopened = ProjectHandle::temporary().expect("temporary project");
        let opened = wait(reopened.open(file.path.clone(), 100, 1024 * 1024)).expect("open copy");
        assert_eq!(opened.workspace, workspace);
    }
}
