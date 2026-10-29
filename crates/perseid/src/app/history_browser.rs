//! Bounded stored-history browser. Page queries read only row metadata; a
//! separate background query hydrates the selected exchange on demand.

use std::sync::mpsc::{self, Receiver};

use egui_extras::{Column, TableBuilder};

use super::*;
use crate::history::HistoryItem;
use crate::project::{HistoryCursor, HttpRow, ProjectError, ProjectSnapshot};

const PAGE_SIZE: usize = 500;
type DetailReply = Receiver<Result<Option<HistoryItem>, ProjectError>>;

#[derive(Clone, Copy)]
enum ToolAction {
    Repeater,
    Intruder,
    CompareRequest,
    CompareResponse,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::{
        InspectorNoteWorkspace, InspectorWorkspace, ProjectHandle, RetentionLimits,
    };
    use crate::proxy::{CapturedBody, ExchangeEnd, ExchangeResult, ProxyEvent, RequestInfo};
    use std::time::{Instant, SystemTime};

    fn wait<T>(rx: Receiver<Result<T, ProjectError>>) -> T {
        rx.recv_timeout(Duration::from_secs(30)).unwrap().unwrap()
    }

    fn capture(project: &ProjectHandle, first: u64, count: u64) {
        for id in first..first + count {
            for event in [
                ProxyEvent::Started {
                    id,
                    // Every row ties on timestamp: the ID must break the tie.
                    timestamp: SystemTime::UNIX_EPOCH,
                    request: RequestInfo {
                        method: "POST".into(),
                        uri: format!("http://example.test/{id}"),
                        scheme: "http".into(),
                        host: "example.test".into(),
                        path: format!("/{id}"),
                        version: "HTTP/1.1".into(),
                        headers: vec![],
                    },
                },
                ProxyEvent::Finished {
                    id,
                    end: ExchangeEnd {
                        duration: Duration::from_millis(1),
                        request_body: CapturedBody::from_bytes(vec![0, 255, 1]),
                        result: ExchangeResult::Response {
                            body: CapturedBody::from_bytes(b"stored response".to_vec()),
                        },
                    },
                },
            ] {
                assert!(project.capture(&event).event.is_some());
            }
            if id % 100 == 0 {
                wait(project.flush());
            }
        }
        wait(project.flush());
    }

    fn settle(browser: &mut HistoryBrowser) {
        let deadline = Instant::now() + Duration::from_secs(30);
        while browser.page.is_some() || browser.pending_detail.is_some() {
            browser.poll();
            assert!(Instant::now() < deadline, "history reader timed out");
            std::thread::yield_now();
        }
        assert!(browser.error.is_none(), "{:?}", browser.error);
    }

    #[test]
    fn pages_cover_tied_timestamps_while_new_captures_arrive_and_details_are_lazy() {
        let project = ProjectHandle::temporary_with_retention(RetentionLimits::new(0, 0)).unwrap();
        capture(&project, 1, 1003);
        let snapshot = project.database_snapshot().unwrap();
        let mut browser = HistoryBrowser::default();
        browser.load(snapshot.clone(), None);
        settle(&mut browser);
        let mut seen: Vec<_> = browser.rows.iter().map(|row| row.id).collect();
        assert_eq!(seen.len(), PAGE_SIZE);
        assert!(browser.detail.is_none());
        capture(&project, 1004, 1);
        browser.older(snapshot.clone());
        settle(&mut browser);
        seen.extend(browser.rows.iter().map(|row| row.id));
        browser.older(snapshot.clone());
        settle(&mut browser);
        seen.extend(browser.rows.iter().map(|row| row.id));
        assert!(browser.exhausted);
        assert_eq!(seen, (1..=1003).rev().collect::<Vec<_>>());
        browser.select(snapshot.clone(), 1);
        assert!(browser.detail.is_none());
        settle(&mut browser);
        assert_eq!(
            browser.detail.as_ref().unwrap().0.request_body.data(),
            &[0, 255, 1]
        );
        browser.newer(snapshot.clone());
        settle(&mut browser);
        assert_eq!(browser.rows[0].id, 503);
        browser.previous.clear();
        browser.load(snapshot, None);
        settle(&mut browser);
        assert_eq!(browser.rows[0].id, 1004);
    }

    #[test]
    fn stored_window_selects_old_rows_shows_notes_and_sends_to_repeater() {
        use super::super::project_ui::tests::click_window_text;
        let ctx = egui::Context::default();
        let mut app = PerseidApp::test_app();
        capture(app.project.handle().unwrap(), 1, 12);
        app.inspector.apply_workspace(&InspectorWorkspace {
            notes: vec![InspectorNoteWorkspace {
                id: 1,
                text: "old exchange note".into(),
            }],
            ..Default::default()
        });
        app.project.history_browser.open = true;
        ctx.run_pass(Default::default(), |ctx| app.stored_history_window(ctx))
            .textures_delta
            .clear();
        settle(&mut app.project.history_browser);
        assert!(app.history.get(1).is_none());
        assert!(click_window_text(&ctx, "http://example.test/12", |ctx| app
            .stored_history_window(ctx)));
        settle(&mut app.project.history_browser);
        assert_eq!(app.project.history_browser.selected, Some(12));
        // Select a record absent from the live cache and reuse its project note.
        let snapshot = app.project.handle().unwrap().database_snapshot().unwrap();
        app.project.history_browser.select(snapshot, 1);
        settle(&mut app.project.history_browser);
        assert_eq!(app.inspector.note(1), Some("old exchange note"));
        assert!(click_window_text(&ctx, "Send to Repeater", |ctx| app
            .stored_history_window(ctx)));
        assert_eq!(app.main_tab, MainTab::Repeater);
        assert_eq!(app.repeater_tabs.len(), 1);
        assert!(!app.project.history_browser.open);
        assert!(
            app.history.get(1).is_none(),
            "tool sends do not populate the live cache"
        );
    }

    #[test]
    fn project_switch_discards_pending_pages_and_missing_details_are_reported() {
        let ctx = egui::Context::default();
        let mut app = PerseidApp::test_app();
        capture(app.project.handle().unwrap(), 1, 1);
        let snapshot = app.project.handle().unwrap().database_snapshot().unwrap();
        app.project.history_browser = HistoryBrowser {
            open: true,
            generation: app.project.status().unwrap().generation,
            ..Default::default()
        };
        app.project.history_browser.load(snapshot.clone(), None);
        wait(app.project.handle().unwrap().new_project(10, 1024));
        ctx.run_pass(Default::default(), |ctx| app.stored_history_window(ctx))
            .textures_delta
            .clear();
        settle(&mut app.project.history_browser);
        assert!(app.project.history_browser.rows.is_empty());
        assert!(app.project.history_browser.exhausted);
        let snapshot = app.project.handle().unwrap().database_snapshot().unwrap();
        app.project.history_browser.select(snapshot, 1);
        let deadline = Instant::now() + Duration::from_secs(30);
        while app.project.history_browser.pending_detail.is_some() {
            app.project.history_browser.poll();
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
        assert!(
            app.project
                .history_browser
                .error
                .as_ref()
                .unwrap()
                .contains("cleared or evicted")
        );
        assert!(app.project.history_browser.detail.is_none());
    }
}

#[derive(Default)]
pub(super) struct HistoryBrowser {
    pub open: bool,
    generation: u64,
    before: Option<HistoryCursor>,
    previous: Vec<Option<HistoryCursor>>,
    rows: Vec<HttpRow>,
    page: Option<Receiver<Result<Vec<HttpRow>, ProjectError>>>,
    pending_detail: Option<(u64, DetailReply)>,
    detail: Option<(HistoryItem, DetailCache)>,
    selected: Option<u64>,
    error: Option<String>,
    exhausted: bool,
    retention_evictions: u64,
}

impl HistoryBrowser {
    fn load(&mut self, snapshot: ProjectSnapshot, before: Option<HistoryCursor>) {
        self.before = before;
        self.error = None;
        self.selected = None;
        self.detail = None;
        self.pending_detail = None;
        self.rows.clear();
        let (tx, rx) = mpsc::channel();
        self.page = Some(rx);
        std::thread::spawn(move || {
            let _ = tx.send(snapshot.list_http(before, PAGE_SIZE));
        });
    }

    fn select(&mut self, snapshot: ProjectSnapshot, id: u64) {
        if self.pending_detail.is_some() {
            return;
        }
        self.selected = Some(id);
        self.detail = None;
        self.error = None;
        let (tx, rx) = mpsc::channel();
        self.pending_detail = Some((id, rx));
        std::thread::spawn(move || {
            let _ = tx.send(snapshot.get_http(id));
        });
    }

    fn newer(&mut self, snapshot: ProjectSnapshot) {
        if let Some(before) = self.previous.pop() {
            self.load(snapshot, before);
        }
    }

    fn older(&mut self, snapshot: ProjectSnapshot) {
        if let Some(last) = self.rows.last() {
            let before = Some(HistoryCursor {
                timestamp_ms: last.timestamp_ms,
                id: last.id,
            });
            self.previous.push(self.before);
            self.load(snapshot, before);
        }
    }

    fn poll(&mut self) {
        if let Some(rx) = &self.page {
            match rx.try_recv() {
                Ok(Ok(rows)) => {
                    self.exhausted = rows.len() < PAGE_SIZE;
                    self.rows = rows;
                    self.page = None;
                }
                Ok(Err(err)) => {
                    self.error = Some(err.to_string());
                    self.page = None;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.error = Some("History reader stopped; try Newest again.".to_string());
                    self.page = None;
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        if let Some((id, rx)) = &self.pending_detail {
            match rx.try_recv() {
                Ok(Ok(Some(item))) => {
                    let cache = DetailCache {
                        id: *id,
                        request: view::request_views(&item),
                        response: view::response_views(&item),
                        inspector: super::inspector::InspectorModel::from_history(&item),
                    };
                    self.detail = Some((item, cache));
                    self.pending_detail = None;
                }
                Ok(Ok(None)) => {
                    self.error = Some(
                        "This exchange was cleared or evicted by retention. Refresh the page."
                            .to_string(),
                    );
                    self.pending_detail = None;
                }
                Ok(Err(err)) => {
                    self.error = Some(err.to_string());
                    self.pending_detail = None;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.error =
                        Some("Detail reader stopped; select the exchange again.".to_string());
                    self.pending_detail = None;
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
    }
}

impl PerseidApp {
    pub(super) fn stored_history_button(&mut self, ui: &mut egui::Ui) {
        if self.project.handle().is_some() && ui.button("Browse stored history…").clicked() {
            self.project.history_browser.open = true;
        }
    }

    pub(super) fn stored_history_window(&mut self, ctx: &egui::Context) {
        let Some(status) = self.project.status() else {
            return;
        };
        let Some(snapshot) = self
            .project
            .handle()
            .and_then(|handle| handle.database_snapshot())
        else {
            return;
        };
        let mut browser = std::mem::take(&mut self.project.history_browser);
        if browser.generation != status.generation {
            browser = HistoryBrowser {
                open: browser.open,
                generation: status.generation,
                retention_evictions: status.retention_evictions,
                ..Default::default()
            };
        }
        if !browser.open {
            self.project.history_browser = browser;
            return;
        }
        if browser.retention_evictions != status.retention_evictions
            && browser.page.is_none()
            && browser.pending_detail.is_none()
        {
            browser.retention_evictions = status.retention_evictions;
            browser.load(snapshot.clone(), browser.before);
        }
        if browser.rows.is_empty()
            && browser.page.is_none()
            && browser.error.is_none()
            && !browser.exhausted
        {
            browser.load(snapshot.clone(), None);
        }
        browser.poll();
        let mut open = true;
        let mut import = None;
        egui::Window::new("Stored HTTP history")
            .id(egui::Id::unique("perseid.history.browser"))
            .open(&mut open).collapsible(false).default_size([1000.0, 700.0])
            .show(ctx, |ui| {
                ui.style_mut().interaction.selectable_labels = false;
                ui.horizontal(|ui| {
                    let idle = browser.page.is_none() && browser.pending_detail.is_none();
                    if ui.add_enabled(idle, egui::Button::new("Newest")).clicked() {
                        browser.previous.clear(); browser.load(snapshot.clone(), None);
                    }
                    if ui.add_enabled(idle && !browser.previous.is_empty(), egui::Button::new("Newer")).clicked() {
                        browser.newer(snapshot.clone());
                    }
                    if ui.add_enabled(idle && !browser.exhausted && !browser.rows.is_empty(), egui::Button::new("Older")).clicked() {
                        browser.older(snapshot.clone());
                    }
                    ui.label(format!("{} rows on this page / {} stored", browser.rows.len(), status.http_count));
                });
                ui.weak("Newest first. Display filters on the live table do not hide stored rows here. Use Search to filter all retained messages, including decoded bodies.");
                if let Some(error) = &browser.error { ui.colored_label(theme::RED, error); }
                let mut selected = None;
                TableBuilder::new(ui).id_salt("stored.history.rows").striped(true).sense(egui::Sense::click())
                    .column(Column::exact(60.0)).column(Column::exact(65.0)).column(Column::remainder())
                    .column(Column::exact(55.0)).max_scroll_height(230.0).auto_shrink([false, false])
                    .header(20.0, |mut row| { for title in ["#", "Method", "URL", "Status"] { row.col(|ui| { ui.label(title); }); } })
                    .body(|body| { body.rows(20.0, browser.rows.len(), |mut row| {
                        let item = &browser.rows[row.index()]; row.set_selected(browser.selected == Some(item.id));
                        row.col(|ui| { ui.label(item.number.to_string()); });
                        row.col(|ui| { ui.label(&item.method); });
                        row.col(|ui| { ui.add(egui::Label::new(&item.uri).truncate()); });
                        row.col(|ui| { ui.label(item.status.map_or_else(|| "—".to_string(), |s| s.to_string())); });
                        if row.response().clicked() { selected = Some(item.id); }
                    }); });
                if let Some(id) = selected { browser.select(snapshot.clone(), id); }
                if let Some((item, cache)) = &browser.detail {
                    ui.style_mut().interaction.selectable_labels = true;
                    ui.horizontal(|ui| {
                        for (label, action) in [("Send to Repeater", ToolAction::Repeater), ("Send to Intruder", ToolAction::Intruder), ("Compare request", ToolAction::CompareRequest), ("Compare response", ToolAction::CompareResponse)] {
                            if ui.button(label).clicked() { import = Some((item.clone(), action)); }
                        }
                    });
                    super::inspector::side_panel(ui, &mut self.inspector, item.id, &cache.inspector);
                    ui.columns(2, |columns| {
                        message_view::message_toolbar(&mut columns[0], MessagePane::Request, &mut self.request_view, Some(item));
                        message_view::request_message_text_with_curl(&mut columns[0], item.id, self.request_view, item, message_view::message_view_text(&cache.request, self.request_view));
                        message_view::message_toolbar(&mut columns[1], MessagePane::Response, &mut self.response_view, Some(item));
                        message_view::message_text(&mut columns[1], item.id, MessagePane::Response, self.response_view, message_view::message_view_text(&cache.response, self.response_view));
                    });
                } else if browser.pending_detail.is_some() { ui.spinner(); }
                else { ui.label("Select an exchange to load its request, response, and notes."); }
            });
        browser.open = open;
        if browser.page.is_some() || browser.pending_detail.is_some() {
            ctx.request_repaint_after(Duration::from_millis(30));
        }
        self.project.history_browser = browser;
        if let Some((item, action)) = import {
            match action {
                ToolAction::Repeater => self.send_history_to_repeater(&item),
                ToolAction::Intruder => self.send_history_to_intruder(&item),
                ToolAction::CompareRequest => {
                    self.send_history_to_comparer(&item, MessagePane::Request)
                }
                ToolAction::CompareResponse => {
                    self.send_history_to_comparer(&item, MessagePane::Response)
                }
            }
            self.project.history_browser.open = false;
        }
    }
}
