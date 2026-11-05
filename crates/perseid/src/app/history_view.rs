//! One virtual history table over either project backend. Only a metadata
//! window and the selected full capture are retained by each reader.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::Instant;

use super::*;
use crate::project::{HistoryPage, ProjectError};
use crate::websocket_history::WebSocketMessageItem;

const PAGE_SIZE: usize = 500;
type Reply<T> = Receiver<Result<T, ProjectError>>;

pub(super) struct HistoryTable<T, D> {
    pub active: bool,
    generation: u64,
    pub page: Option<HistoryPage<T>>,
    pending: Option<Reply<HistoryPage<T>>>,
    cancel: Arc<AtomicBool>,
    pub wanted: usize,
    pub select_row: Option<usize>,
    pub selected_index: Option<usize>,
    pub detail: Option<(u64, D)>,
    detail_pending: Option<(u64, Reply<Option<D>>)>,
    detail_requested: Option<u64>,
    pub error: Option<String>,
    refreshed: Instant,
    refresh: bool,
}

impl<T, D> Default for HistoryTable<T, D> {
    fn default() -> Self {
        Self {
            active: false,
            generation: 0,
            page: None,
            pending: None,
            cancel: Arc::new(AtomicBool::new(false)),
            wanted: 0,
            select_row: None,
            selected_index: None,
            detail: None,
            detail_pending: None,
            detail_requested: None,
            error: None,
            refreshed: Instant::now(),
            refresh: true,
        }
    }
}

impl<T, D> Drop for HistoryTable<T, D> {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

impl<T, D> HistoryTable<T, D> {
    pub fn total(&self) -> usize {
        self.page.as_ref().map_or(0, |page| page.total)
    }

    pub fn row(&self, index: usize) -> Option<&T> {
        let page = self.page.as_ref()?;
        page.rows.get(index.checked_sub(page.offset)?)
    }

    pub fn item(&self, id: u64) -> Option<&D> {
        self.detail
            .as_ref()
            .filter(|(selected, _)| *selected == id)
            .map(|(_, item)| item)
    }

    pub fn loading(&self, id: u64) -> bool {
        self.detail_pending
            .as_ref()
            .is_some_and(|(selected, _)| *selected == id)
    }

    fn activate(&mut self, generation: u64) {
        if !self.active || self.generation != generation {
            *self = Self::default();
            self.active = true;
            self.generation = generation;
        }
    }

    pub fn invalidate(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        self.cancel = Arc::new(AtomicBool::new(false));
        self.pending = None;
        self.page = None;
        self.selected_index = None;
        self.select_row = None;
        self.error = None;
        self.refresh = true;
    }

    pub fn invalidate_detail(&mut self) {
        self.detail = None;
        self.detail_pending = None;
        self.detail_requested = None;
    }

    pub fn mark_stale(&mut self) {
        // Keep the installed window and any pending read during retention.
        // A completed older read is followed by another refresh.
        self.refresh = true;
        self.selected_index = None;
    }

    fn poll(&mut self) {
        if let Some(receiver) = self.pending.take() {
            match receiver.try_recv() {
                Ok(Ok(page)) => {
                    self.page = Some(page);
                }
                Ok(Err(error)) => self.error = Some(error.to_string()),
                Err(TryRecvError::Empty) => self.pending = Some(receiver),
                Err(TryRecvError::Disconnected) => {
                    self.error = Some("History reader stopped. Change the filter to retry.".into())
                }
            }
        }
        if let Some((id, receiver)) = self.detail_pending.take() {
            match receiver.try_recv() {
                Ok(Ok(Some(item))) => self.detail = Some((id, item)),
                Ok(Ok(None)) => {
                    self.error = Some("The selected capture was deleted or cleared.".into())
                }
                Ok(Err(error)) => self.error = Some(error.to_string()),
                Err(TryRecvError::Empty) => self.detail_pending = Some((id, receiver)),
                Err(TryRecvError::Disconnected) => {
                    self.error = Some("Capture detail reader stopped.".into())
                }
            }
        }
    }

    fn needs_page(&self) -> bool {
        self.pending.is_none()
            && self.error.is_none()
            && (self.refresh
                || self.page.is_none()
                || (self.wanted < self.total() && self.row(self.wanted).is_none())
                || (self.wanted.saturating_add(100) < self.total()
                    && self.row(self.wanted + 100).is_none()))
    }
}

impl PerseidApp {
    pub(super) fn history_reads_pending(&self) -> bool {
        let reading = (self.http_table.active
            && (self.http_table.pending.is_some()
                || self.http_table.detail_pending.is_some()
                || self.http_table.needs_page()))
            || (self.ws_table.active
                && (self.ws_table.pending.is_some()
                    || self.ws_table.detail_pending.is_some()
                    || self.ws_table.needs_page()));
        reading
            || self.project.status().is_some_and(|status| {
                (self.http_table.active
                    && self
                        .visible_key
                        .as_ref()
                        .is_none_or(|key| key.revision != status.last_committed_sequence))
                    || (self.ws_table.active
                        && self
                            .websocket_visible_key
                            .as_ref()
                            .is_none_or(|key| key.1 != status.last_committed_sequence))
            })
    }

    pub(super) fn rebuild_stored_http(&mut self) -> bool {
        let Some(status) = self.project.status() else {
            return false;
        };
        if status.http_count == 0 && !self.http_table.active {
            return false;
        }
        if self.project.history_clearing(false) {
            return true;
        }
        let Some(snapshot) = self
            .project
            .handle()
            .and_then(|handle| handle.database_snapshot())
        else {
            return false;
        };
        self.http_table.activate(status.generation);
        self.http_table.poll();
        if let Some(page) = &self.http_table.page {
            if page.selected_id == self.selected {
                self.http_table.selected_index = page.selected_index;
                if self.selected.is_some() && page.selected_index.is_none() {
                    self.selected = None;
                    self.detail = None;
                    self.http_table.invalidate_detail();
                }
            } else if let Some(index) = page
                .rows
                .iter()
                .position(|row| Some(row.item.id) == self.selected)
            {
                self.http_table.selected_index = Some(page.offset + index);
            }
        }
        let key = HttpVisibleKey {
            quick_search: self.filter.clone(),
            filter: self.history_filter.clone(),
            scope_enabled: self.scope_enabled,
            scope: self.scope.clone(),
            revision: status.last_committed_sequence,
        };
        let filter_changed = self.visible_key.as_ref().is_none_or(|old| {
            old.quick_search != key.quick_search
                || old.filter != key.filter
                || old.scope_enabled != key.scope_enabled
                || old.scope != key.scope
        });
        if filter_changed {
            self.http_table.invalidate();
            self.http_table.wanted = 0;
            self.visible_key = Some(key.clone());
        } else if self.visible_key.as_ref() != Some(&key)
            && self.http_table.pending.is_none()
            && self.http_table.refreshed.elapsed() >= Duration::from_millis(250)
        {
            self.http_table.invalidate_detail();
            self.detail = None;
            // Preserve the installed window while refreshing it.
            self.http_table.refresh = true;
            self.visible_key = Some(key.clone());
        }
        if self.http_table.needs_page() {
            let (sender, receiver) = mpsc::channel();
            self.http_table.pending = Some(receiver);
            self.http_table.refresh = false;
            self.http_table.refreshed = Instant::now();
            let offset =
                self.http_table.wanted.saturating_sub(PAGE_SIZE / 2) / PAGE_SIZE * PAGE_SIZE;
            let selected = self.selected;
            let cancel = self.http_table.cancel.clone();
            let notify = self.notify.clone();
            std::thread::spawn(move || {
                let needle = key.quick_search.trim().to_lowercase();
                let search = key.filter.search();
                let result =
                    snapshot.http_history_page(offset, PAGE_SIZE * 2, selected, &cancel, |row| {
                        (!key.scope_enabled
                            || key.scope.is_empty()
                            || site_map::host_in_scope(&row.item.request.host, &key.scope))
                            && (needle.is_empty() || util::matches_filter(&row.item, &needle))
                            && key.filter.matches_metadata(
                                &row.item,
                                &key.scope,
                                &search,
                                row.summary.parameters,
                            )
                    });
                let _ = sender.send(result);
                notify();
            });
        }
        self.visible = self.http_table.page.as_ref().map_or_else(Vec::new, |page| {
            page.rows.iter().map(|row| row.item.id).collect()
        });
        if let Some(index) = self.http_table.select_row
            && let Some(row) = self.http_table.row(index)
        {
            let id = row.item.id;
            self.http_table.select_row = None;
            self.select_id(id);
        }
        if let Some(id) = self.selected
            && self.http_table.detail_requested != Some(id)
        {
            let (sender, receiver) = mpsc::channel();
            self.http_table.detail_pending = Some((id, receiver));
            self.http_table.detail_requested = Some(id);
            let snapshot = self.project.handle().unwrap().database_snapshot().unwrap();
            let notify = self.notify.clone();
            std::thread::spawn(move || {
                let _ = sender.send(snapshot.get_http(id));
                notify();
            });
        }
        true
    }

    pub(super) fn rebuild_stored_ws(&mut self) -> bool {
        let Some(status) = self.project.status() else {
            return false;
        };
        if status.ws_count == 0 && !self.ws_table.active {
            return false;
        }
        if self.project.history_clearing(true) {
            return true;
        }
        let Some(snapshot) = self
            .project
            .handle()
            .and_then(|handle| handle.database_snapshot())
        else {
            return false;
        };
        self.ws_table.activate(status.generation);
        self.ws_table.poll();
        if let Some(page) = &self.ws_table.page {
            if page.selected_id == self.websocket_selected {
                self.ws_table.selected_index = page.selected_index;
                if self.websocket_selected.is_some() && page.selected_index.is_none() {
                    self.websocket_selected = None;
                    self.websocket_detail = None;
                    self.ws_table.invalidate_detail();
                }
            } else if let Some(index) = page
                .rows
                .iter()
                .position(|row| Some(row.id) == self.websocket_selected)
            {
                self.ws_table.selected_index = Some(page.offset + index);
            }
        }
        let key = (
            self.websocket_filter.clone(),
            status.last_committed_sequence,
        );
        let filter_changed = self
            .websocket_visible_key
            .as_ref()
            .is_none_or(|old| old.0 != key.0);
        if filter_changed {
            self.ws_table.invalidate();
            self.ws_table.wanted = 0;
            self.websocket_visible_key = Some(key.clone());
        } else if self.websocket_visible_key.as_ref() != Some(&key)
            && self.ws_table.pending.is_none()
            && self.ws_table.refreshed.elapsed() >= Duration::from_millis(250)
        {
            self.ws_table.invalidate_detail();
            self.websocket_detail = None;
            self.ws_table.refresh = true;
            self.websocket_visible_key = Some(key.clone());
        }
        if self.ws_table.needs_page() {
            let (sender, receiver) = mpsc::channel();
            self.ws_table.pending = Some(receiver);
            self.ws_table.refresh = false;
            self.ws_table.refreshed = Instant::now();
            let offset = self.ws_table.wanted.saturating_sub(PAGE_SIZE / 2) / PAGE_SIZE * PAGE_SIZE;
            let selected = self.websocket_selected;
            let cancel = self.ws_table.cancel.clone();
            let notify = self.notify.clone();
            let scope = self.scope.clone();
            let scoped = self.scope_enabled;
            std::thread::spawn(move || {
                let needle = key.0.trim().to_lowercase();
                let result =
                    snapshot.ws_history_page(offset, PAGE_SIZE * 2, selected, &cancel, |row| {
                        (!scoped
                            || scope.is_empty()
                            || site_map::host_in_scope(&row.connection.host, &scope))
                            && (needle.is_empty() || util::websocket_matches_filter(row, &needle))
                    });
                let _ = sender.send(result);
                notify();
            });
        }
        self.websocket_visible = self.ws_table.page.as_ref().map_or_else(Vec::new, |page| {
            page.rows.iter().map(|row| row.id).collect()
        });
        if let Some(index) = self.ws_table.select_row
            && let Some(row) = self.ws_table.row(index)
        {
            let id = row.id;
            self.ws_table.select_row = None;
            self.select_websocket_id(id);
        }
        if let Some(id) = self.websocket_selected
            && self.ws_table.detail_requested != Some(id)
        {
            let (sender, receiver) = mpsc::channel();
            self.ws_table.detail_pending = Some((id, receiver));
            self.ws_table.detail_requested = Some(id);
            let snapshot = self.project.handle().unwrap().database_snapshot().unwrap();
            let notify = self.notify.clone();
            std::thread::spawn(move || {
                let _ = sender.send(snapshot.get_ws(id));
                notify();
            });
        }
        true
    }

    pub(super) fn selected_ws_item(&self, id: u64) -> Option<&WebSocketMessageItem> {
        self.ws_table
            .item(id)
            .or_else(|| self.websocket_history.get(id))
    }

    pub(super) fn history_reader_status(&mut self, ui: &mut egui::Ui, websocket: bool) {
        let (pending, error) = if websocket {
            (self.ws_table.pending.is_some(), &self.ws_table.error)
        } else {
            (self.http_table.pending.is_some(), &self.http_table.error)
        };
        if pending {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.weak("Loading history…");
            });
        }
        if let Some(error) = error {
            ui.colored_label(theme::RED, error);
            if ui.button("Retry history read").clicked() {
                if websocket {
                    self.ws_table.invalidate();
                    self.ws_table.invalidate_detail();
                    self.websocket_detail = None;
                } else {
                    self.http_table.invalidate();
                    self.http_table.invalidate_detail();
                    self.detail = None;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(offset: usize, total: usize) -> HistoryPage<u64> {
        HistoryPage {
            offset,
            total,
            rows: (offset as u64..(offset + 1000).min(total) as u64).collect(),
            selected_index: None,
            selected_id: None,
        }
    }

    #[test]
    fn unified_http_controller_navigates_beyond_cache_and_preserves_binary_tool_input() {
        use crate::proxy::{CapturedBody, ExchangeEnd, ExchangeResult, ProxyEvent, RequestInfo};
        let mut app = PerseidApp::test_app();
        for id in 1..=150 {
            for event in [
                ProxyEvent::Started {
                    id,
                    timestamp: std::time::SystemTime::UNIX_EPOCH,
                    request: RequestInfo {
                        method: "POST".into(),
                        uri: format!("http://example.test/{id}"),
                        scheme: "http".into(),
                        host: "example.test".into(),
                        path: format!("/{id}"),
                        version: "HTTP/1.1".into(),
                        headers: Vec::new(),
                    },
                },
                ProxyEvent::Finished {
                    id,
                    end: ExchangeEnd {
                        duration: Duration::ZERO,
                        request_body: CapturedBody::from_bytes(vec![0, 255, 42]),
                        result: ExchangeResult::Response {
                            body: CapturedBody::default(),
                        },
                    },
                },
            ] {
                let captured = app.project.handle().unwrap().capture(&event);
                app.history.apply(captured.event.unwrap());
            }
        }
        app.project
            .handle()
            .unwrap()
            .flush()
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .unwrap();
        assert!(app.history.get(1).is_none());
        fn settle(app: &mut PerseidApp) {
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                app.rebuild_stored_http();
                if app.http_table.pending.is_none() && app.http_table.detail_pending.is_none() {
                    break;
                }
                assert!(Instant::now() < deadline, "history read timed out");
                std::thread::yield_now();
            }
        }
        settle(&mut app);
        assert_eq!(app.http_table.total(), 150);
        app.http_table.selected_index = Some(149);
        app.select_id(9999);
        assert_eq!(app.http_table.selected_index, None);
        app.selected = None;
        app.move_selection(1);
        settle(&mut app);
        assert_eq!(app.selected, Some(1));
        assert_eq!(
            app.selected_http_item(1).unwrap().request_body.data(),
            &[0, 255, 42]
        );
        let wire =
            crate::view::captured_message_bytes(app.selected_http_item(1).unwrap(), false).unwrap();
        assert!(wire.ends_with(&[0, 255, 42]));
        app.move_selection(1000);
        settle(&mut app);
        assert_eq!(app.selected, Some(150));
        app.filter = "no-matching-host".into();
        settle(&mut app);
        assert_eq!(app.http_table.total(), 0);
        assert!(app.selected.is_none());
        app.filter.clear();
        settle(&mut app);
        assert_eq!(app.http_table.total(), 150);
        app.clear_history();
        app.project
            .handle()
            .unwrap()
            .flush()
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .unwrap();
        app.poll_project();
        settle(&mut app);
        assert_eq!(app.http_table.total(), 0);
    }

    #[test]
    fn websocket_controller_reopens_without_body_snapshot_and_loads_old_binary_messages() {
        use crate::proxy::{
            CapturedBody, ProxyEvent, WebSocketConnectionInfo, WebSocketDirection, WebSocketOpcode,
        };
        let mut app = PerseidApp::test_app();
        for id in 1..=150 {
            let event = ProxyEvent::WebSocketMessage {
                id,
                timestamp: std::time::SystemTime::UNIX_EPOCH,
                connection: WebSocketConnectionInfo {
                    id: 1,
                    url: "ws://example.test/socket".into(),
                    host: "example.test".into(),
                    tls: false,
                    listener_port: 8080,
                    protocol: None,
                },
                direction: WebSocketDirection::ToServer,
                opcode: WebSocketOpcode::Binary,
                payload: CapturedBody::from_bytes(vec![0, 255, 42]),
            };
            let captured = app.project.handle().unwrap().capture(&event);
            app.websocket_history.apply(captured.event.unwrap());
        }
        let path = std::env::temp_dir().join(format!(
            "perseid-unified-history-{}.perseid",
            uuid::Uuid::new_v4()
        ));
        let project = app.project.handle().unwrap();
        project
            .save_as(path.clone(), false)
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
            .unwrap();
        project
            .new_project(0, 0)
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
            .unwrap();
        let opened = project
            .open(path.clone(), 0, 0)
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
            .unwrap();
        assert!(opened.http.is_empty() && opened.ws.is_empty());
        assert_eq!(opened.ws_total, 150);
        app.websocket_history.clear();
        fn settle(app: &mut PerseidApp) {
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                app.rebuild_stored_ws();
                if app.ws_table.pending.is_none() && app.ws_table.detail_pending.is_none() {
                    break;
                }
                assert!(Instant::now() < deadline);
                std::thread::yield_now();
            }
        }
        settle(&mut app);
        assert_eq!(app.ws_table.total(), 150);
        app.ws_table.selected_index = Some(149);
        app.select_websocket_id(9999);
        assert_eq!(app.ws_table.selected_index, None);
        app.websocket_selected = None;
        app.move_websocket_selection(1);
        settle(&mut app);
        assert_eq!(app.websocket_selected, Some(1));
        assert_eq!(
            app.selected_ws_item(1).unwrap().payload.data(),
            &[0, 255, 42]
        );
        app.move_websocket_selection(1000);
        settle(&mut app);
        assert_eq!(app.websocket_selected, Some(150));
        app.websocket_filter = "no-matching-host".into();
        settle(&mut app);
        assert_eq!(app.ws_table.total(), 0);
        assert!(app.websocket_selected.is_none());
        drop(app);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn retention_refresh_preserves_window_and_pending_read() {
        let mut table = HistoryTable::<u64, Vec<u8>>::default();
        table.activate(1);
        table.page = Some(page(0, 3));
        table.refresh = false;
        let (sender, receiver) = mpsc::channel();
        table.pending = Some(receiver);
        table.mark_stale();
        assert_eq!(table.row(2), Some(&2));
        assert!(!table.cancel.load(Ordering::Relaxed));
        assert!(table.pending.is_some());
        sender.send(Ok(page(0, 4))).unwrap();
        table.poll();
        assert_eq!(table.total(), 4);
        assert!(
            table.needs_page(),
            "a sweep during an older read must schedule another refresh"
        );
    }

    #[test]
    fn windows_keep_total_and_prefetch_before_visible_rows_run_out() {
        let mut table = HistoryTable::<u64, Vec<u8>>::default();
        table.activate(1);
        table.page = Some(page(0, 3000));
        table.refresh = false;
        assert!(!table.needs_page());
        table.wanted = 950;
        assert!(table.needs_page());
        assert_eq!(table.total(), 3000);
        assert_eq!(table.row(999), Some(&999));
        assert_eq!(table.row(1000), None);
        table.page = Some(page(500, 3000));
        assert!(!table.needs_page());
        assert_eq!(table.row(499), None);
    }

    #[test]
    fn generation_and_filter_changes_cancel_pending_work_and_reject_stale_replies() {
        let mut table = HistoryTable::<u64, Vec<u8>>::default();
        table.activate(1);
        let old_cancel = table.cancel.clone();
        let (sender, receiver) = mpsc::channel();
        table.pending = Some(receiver);
        table.detail = Some((1, vec![0, 255]));
        table.activate(2);
        assert!(old_cancel.load(Ordering::Relaxed));
        assert!(sender.send(Ok(page(0, 3))).is_err());
        assert!(table.page.is_none() && table.detail.is_none());
        let (sender, receiver) = mpsc::channel();
        table.pending = Some(receiver);
        table.invalidate();
        assert!(sender.send(Ok(page(0, 3))).is_err());
        assert!(table.needs_page());
    }

    #[test]
    fn refreshing_keeps_the_installed_window_and_reader_errors_do_not_retry_forever() {
        let mut table = HistoryTable::<u64, Vec<u8>>::default();
        table.page = Some(page(0, 3));
        let (sender, receiver) = mpsc::channel();
        table.pending = Some(receiver);
        table.poll();
        assert_eq!(table.row(2), Some(&2));
        drop(sender);
        table.poll();
        assert!(table.error.is_some());
        assert!(!table.needs_page());
        table.invalidate();
        assert!(table.needs_page());
        let (sender, receiver) = mpsc::channel();
        table.detail_pending = Some((1, receiver));
        table.detail_requested = Some(1);
        sender.send(Ok(None)).unwrap();
        table.poll();
        assert!(!table.loading(1));
        assert_eq!(table.detail_requested, Some(1));
        table.invalidate_detail();
        assert_eq!(table.detail_requested, None);
    }
}
