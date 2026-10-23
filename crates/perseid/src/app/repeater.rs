//! Editable Repeater: request tabs, a raw request editor, and direct replay.
//!
//! The layout follows Burp's Repeater: a strip of request tabs, a target bar
//! with Send, the editable request on the left, and the response on the right.
//! Repeater uses its own replay engine rather than going back through the local
//! proxy listener, so it also works when the proxy is stopped.

use std::time::{Duration, Instant};

use crate::proxy::{
    DEFAULT_REPEATER_TIMEOUT, Repeater, RepeaterConfig, RepeaterError, RepeaterRequest,
    RepeaterResult, split_raw_request,
};
use egui::TextEdit;

use super::message_view::message_text;
use super::widgets::{empty_state, message_mode_tab, section_tab};
use super::*;
use crate::project::RepeaterWorkspace;
use crate::view::{self, MessageViews};

const REPEATER_TABS_ID: &str = "perseid.repeater.tabs";
const REPEATER_TOOLBAR_ID: &str = "perseid.repeater.toolbar";
const REPEATER_REQUEST_ID: &str = "perseid.repeater.request";

pub(super) struct RepeaterTab {
    /// Stable id for egui state and response matching.
    id: u64,
    title: String,
    scheme: String,
    host: String,
    /// The full raw request: request line, headers, blank line, body.
    request: String,
    /// Seed warning, for example when the captured body was truncated.
    note: Option<String>,
    response: Option<RepeaterReply>,
    pending: Option<u64>,
    pending_since: Option<Instant>,
    error: Option<String>,
    response_view: MessageView,
}

struct RepeaterReply {
    views: MessageViews,
    status: u16,
    reason: String,
    duration: Duration,
    size: u64,
    truncated: bool,
    body_complete: bool,
    body_error: Option<String>,
}

impl RepeaterTab {
    pub(super) fn is_pending(&self) -> bool {
        self.pending.is_some()
    }

    fn new(id: u64) -> Self {
        Self {
            id,
            title: "New request".to_string(),
            scheme: "https".to_string(),
            host: String::new(),
            request: "GET / HTTP/1.1\nHost: \n\n".to_string(),
            note: None,
            response: None,
            pending: None,
            pending_since: None,
            error: None,
            response_view: MessageView::Pretty,
        }
    }

    pub(super) fn from_history(id: u64, item: &crate::history::HistoryItem) -> Self {
        let path = if item.request.path.is_empty() {
            &item.request.host
        } else {
            &item.request.path
        };
        let target = path.split('?').next().unwrap_or(path);
        let mut title = format!("#{} {} {}", item.number, item.request.method, target);
        if title.chars().count() > 56 {
            title = title.chars().take(53).collect::<String>();
            title.push_str("...");
        }
        let scheme = if item.request.scheme.is_empty() {
            "http".to_string()
        } else {
            item.request.scheme.clone()
        };
        let note = view::seeded_body_note(&item.request_body, "sending");

        Self {
            id,
            title,
            scheme,
            host: item.request.host.clone(),
            // `request_views` applies the same decoded, bounded representation
            // as the Proxy detail pane. That keeps the seed readable for
            // compressed text and avoids copying a huge captured body into the
            // editor.
            request: view::request_views(item).raw,
            note,
            response: None,
            pending: None,
            pending_since: None,
            error: None,
            response_view: MessageView::Pretty,
        }
    }

    /// Seed a representative replay response for the documentation screenshot.
    #[cfg(feature = "screenshots")]
    pub(super) fn seed_screenshot_response(&mut self) {
        let body = crate::proxy::CapturedBody::from_bytes(
            br#"{
  "items": [
    { "id": 1, "name": "Router", "in_stock": true },
    { "id": 2, "name": "Cable", "in_stock": true }
  ],
  "served_by": "demo-api"
}"#
            .to_vec(),
        );
        let headers = vec![
            ("Content-Type".to_string(), "application/json".to_string()),
            ("Content-Length".to_string(), body.total_len().to_string()),
            (
                "Server".to_string(),
                "DemoServer/1.0 Python/3.14.7".to_string(),
            ),
        ];
        self.response = Some(RepeaterReply {
            views: view::response_views_from_parts("HTTP/1.1", 200, "OK", &headers, &body),
            status: 200,
            reason: "OK".to_string(),
            duration: Duration::from_millis(18),
            size: body.total_len(),
            truncated: false,
            body_complete: true,
            body_error: None,
        });
        self.pending = None;
        self.pending_since = None;
        self.error = None;
        self.response_view = MessageView::Pretty;
    }
}

impl PerseidApp {
    /// Send the currently selected HTTP exchange to a new Repeater tab.
    pub(super) fn send_selected_to_repeater(&mut self) {
        let Some(item) = self
            .selected
            .and_then(|id| self.history.get(id))
            .filter(|item| !item.is_tunnel())
        else {
            return;
        };
        let id = self.repeater_next_tab;
        self.repeater_next_tab += 1;
        self.repeater_tabs.push(RepeaterTab::from_history(id, item));
        self.repeater_active = self.repeater_tabs.len() - 1;
        self.main_tab = MainTab::Repeater;
    }

    /// Poll the replay engine and attach results to their request tabs.
    pub(super) fn drain_repeater_results(&mut self) {
        let (responses, running) = {
            let Some(repeater) = &self.repeater else {
                return;
            };
            let mut responses = Vec::new();
            while let Some(response) = repeater.try_recv() {
                responses.push(response);
            }
            (responses, repeater.is_running())
        };

        for response in responses {
            let Some(tab) = self
                .repeater_tabs
                .iter_mut()
                .find(|tab| tab.pending == Some(response.id))
            else {
                continue;
            };
            tab.pending = None;
            tab.pending_since = None;
            match response.result {
                RepeaterResult::Response {
                    version,
                    status,
                    reason,
                    headers,
                    body,
                    body_error,
                } => {
                    tab.error = None;
                    tab.response = Some(RepeaterReply {
                        views: view::response_views_from_parts(
                            &version, status, &reason, &headers, &body,
                        ),
                        status,
                        reason,
                        duration: response.duration,
                        size: body.total_len(),
                        truncated: body.is_truncated(),
                        body_complete: body.is_complete(),
                        body_error,
                    });
                }
                RepeaterResult::Failed { error } => {
                    tab.response = None;
                    tab.error = Some(error);
                }
            }
        }

        if !running {
            // The runtime thread died outside the normal shutdown path. Clear
            // the handle so the next Send can start a fresh engine, and stop
            // any tab from waiting forever.
            self.repeater = None;
            for tab in &mut self.repeater_tabs {
                if tab.pending.take().is_some() {
                    tab.pending_since = None;
                    tab.error = Some("the Repeater engine stopped unexpectedly".to_string());
                }
            }
        }
    }

    pub(super) fn repeater_view(&mut self, ui: &mut egui::Ui) {
        self.repeater_tab_strip(ui);

        if self.repeater_tabs.is_empty() {
            self.repeater_empty(ui);
            return;
        }

        self.repeater_active = self.repeater_active.min(self.repeater_tabs.len() - 1);
        self.repeater_toolbar(ui);
        self.repeater_editor(ui);
    }

    fn repeater_tab_strip(&mut self, ui: &mut egui::Ui) {
        let mut activate = None;
        let mut close = None;
        let mut new_tab = false;

        egui::Panel::top(REPEATER_TABS_ID)
            .frame(
                egui::Frame::NONE
                    .fill(theme::SURFACE)
                    .inner_margin(egui::Margin::symmetric(10, 4)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    if ui.button("+").on_hover_text("New Repeater tab").clicked() {
                        new_tab = true;
                    }
                    egui::ScrollArea::horizontal()
                        .id_salt("perseid.repeater.tabs.scroll")
                        .auto_shrink([true, true])
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                for (index, tab) in self.repeater_tabs.iter().enumerate() {
                                    if section_tab(ui, index == self.repeater_active, &tab.title)
                                        .clicked()
                                    {
                                        activate = Some(index);
                                    }
                                    if ui
                                        .small_button("×")
                                        .on_hover_text("Close this Repeater tab")
                                        .clicked()
                                    {
                                        close = Some(index);
                                    }
                                    ui.add_space(2.0);
                                }
                            });
                        });
                });
            });

        if let Some(index) = activate {
            self.repeater_active = index;
            ui.ctx().memory_mut(|memory| memory.stop_text_input());
        }
        if let Some(index) = close {
            self.close_repeater_tab(index);
        }
        if new_tab {
            self.new_repeater_tab();
        }
    }

    fn repeater_empty(&mut self, ui: &mut egui::Ui) {
        ui.add_space(32.0);
        ui.vertical_centered(|ui| {
            ui.label(
                RichText::new("No Repeater tabs")
                    .size(16.0)
                    .strong()
                    .color(theme::TEXT_MUTED),
            );
            ui.add_space(4.0);
            ui.label(
                RichText::new(
                    "Right-click an exchange in Proxy > HTTP history and choose Send to \
                     Repeater, or start with a blank tab.",
                )
                .color(theme::TEXT_MUTED),
            );
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if ui.button("Go to Proxy history").clicked() {
                    self.main_tab = MainTab::Proxy;
                    self.proxy_section = ProxySection::HttpHistory;
                }
                if ui.button("New blank tab").clicked() {
                    self.new_repeater_tab();
                }
            });
        });
    }

    fn repeater_toolbar(&mut self, ui: &mut egui::Ui) {
        let index = self.repeater_active;
        let mut send = false;

        egui::Panel::top(REPEATER_TOOLBAR_ID)
            .frame(
                egui::Frame::NONE
                    .fill(theme::SURFACE_ALT)
                    .inner_margin(egui::Margin::symmetric(10, 4)),
            )
            .show(ui, |ui| {
                let tab = &mut self.repeater_tabs[index];
                ui.horizontal(|ui| {
                    let send_button = egui::Button::new(
                        RichText::new("Send")
                            .strong()
                            .color(Color32::WHITE),
                    )
                    .fill(theme::ACCENT_DARK)
                    .stroke(Stroke::new(1.0, theme::ACCENT_DARK))
                    .min_size(egui::vec2(60.0, 24.0));
                    if ui
                        .add_enabled(tab.pending.is_none(), send_button)
                        .on_hover_text("Send the edited request (Ctrl+Enter)")
                        .on_disabled_hover_text("Waiting for the current response")
                        .clicked()
                    {
                        send = true;
                    }

                    ui.add_space(4.0);
                    ui.label(RichText::new("Target").small().color(theme::TEXT_MUTED));
                    egui::ComboBox::from_id_salt(("perseid.repeater.scheme", tab.id))
                        .width(70.0)
                        .selected_text(&tab.scheme)
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut tab.scheme, "https".to_string(), "https");
                            ui.selectable_value(&mut tab.scheme, "http".to_string(), "http");
                        });
                    ui.add(
                        TextEdit::singleline(&mut tab.host)
                            .desired_width(280.0)
                            .hint_text("host[:port]"),
                    )
                    .on_hover_text(
                        "Connection target for origin-form requests. The Host header in the request is preserved for virtual-host testing.",
                    );

                    ui.separator();
                    if let Some(since) = tab.pending_since {
                        ui.spinner();
                        ui.label(
                            RichText::new(format!(
                                "Sending… {}",
                                view::format_duration(since.elapsed())
                            ))
                            .small()
                            .color(theme::TEXT_MUTED),
                        );
                    } else if let Some(reply) = &tab.response {
                        ui.colored_label(
                            view::status_class_color(reply.status),
                            RichText::new(repeater_status_text(reply.status, &reply.reason)).strong(),
                        );
                        ui.label(
                            RichText::new(view::format_duration(reply.duration))
                                .small()
                                .color(theme::TEXT_MUTED),
                        );
                        ui.label(
                            RichText::new(view::human_bytes(reply.size))
                                .small()
                                .color(theme::TEXT_MUTED),
                        );
                    } else if let Some(error) = &tab.error {
                        ui.add(
                            egui::Label::new(RichText::new(error).small().color(theme::RED))
                                .truncate(),
                        )
                        .on_hover_text(error);
                    } else {
                        ui.label(
                            RichText::new("Edit the request, then Send.")
                                .small()
                                .color(theme::TEXT_MUTED),
                        );
                    }
                });
            });

        if send {
            self.send_active_repeater();
        }
    }

    fn repeater_editor(&mut self, ui: &mut egui::Ui) {
        let index = self.repeater_active;
        let default_width = (ui.available_width() * 0.5).max(260.0);

        egui::Panel::left(REPEATER_REQUEST_ID)
            .resizable(true)
            .default_size(default_width)
            .min_size(180.0)
            .frame(
                egui::Frame::NONE
                    .fill(theme::SURFACE)
                    .inner_margin(egui::Margin::same(5)),
            )
            .show(ui, |ui| {
                let tab = &mut self.repeater_tabs[index];
                request_editor(ui, tab);
            });

        egui::CentralPanel::default()
            .frame(
                egui::Frame::NONE
                    .fill(theme::SURFACE)
                    .inner_margin(egui::Margin::same(5)),
            )
            .show(ui, |ui| {
                let tab = &mut self.repeater_tabs[index];
                response_pane(ui, tab);
            });
    }

    fn new_repeater_tab(&mut self) {
        let id = self.repeater_next_tab;
        self.repeater_next_tab += 1;
        self.repeater_tabs.push(RepeaterTab::new(id));
        self.repeater_active = self.repeater_tabs.len() - 1;
    }

    fn close_repeater_tab(&mut self, index: usize) {
        let len = self.repeater_tabs.len();
        if index >= len {
            return;
        }
        self.repeater_tabs.remove(index);
        self.repeater_active = tab_index_after_close(len, self.repeater_active, index);
    }

    fn ensure_repeater(&mut self) -> Result<(), String> {
        if self.repeater.is_some() {
            return Ok(());
        }
        let config = RepeaterConfig {
            capture_limit: self.capture_limit,
            timeout: DEFAULT_REPEATER_TIMEOUT,
            extra_roots_pem: Vec::new(),
            notify: Some(self.notify.clone()),
        };
        match Repeater::start(config) {
            Ok(repeater) => {
                self.repeater = Some(repeater);
                Ok(())
            }
            Err(err) => Err(err.to_string()),
        }
    }

    pub(super) fn send_active_repeater(&mut self) {
        let index = self.repeater_active;
        let Some(tab) = self.repeater_tabs.get(index) else {
            return;
        };
        if tab.pending.is_some() {
            return;
        }

        let raw = tab.request.clone();
        let scheme = tab.scheme.clone();
        let host = tab.host.clone();
        let (head, body) = split_raw_request(&raw);
        let request = match RepeaterRequest::parse(head, body.as_bytes().to_vec(), &scheme, &host) {
            Ok(request) => request,
            Err(err) => {
                let tab = &mut self.repeater_tabs[index];
                tab.response = None;
                tab.error = Some(err.to_string());
                return;
            }
        };

        if let Err(err) = self.ensure_repeater() {
            let tab = &mut self.repeater_tabs[index];
            tab.response = None;
            tab.error = Some(err);
            return;
        }

        let result = self
            .repeater
            .as_ref()
            .expect("Repeater was just started")
            .send(request);

        let tab = &mut self.repeater_tabs[index];
        match result {
            Ok(id) => {
                tab.error = None;
                tab.response = None;
                tab.pending = Some(id);
                tab.pending_since = Some(Instant::now());
            }
            Err(err) => {
                let closed = matches!(err, RepeaterError::Closed);
                tab.response = None;
                tab.error = Some(err.to_string());
                // A closed job channel means the worker is gone even if the
                // runtime thread is still winding down. Drop the handle so the
                // next Send can start a fresh engine.
                if closed {
                    self.repeater = None;
                }
            }
        }
    }
}

fn request_editor(ui: &mut egui::Ui, tab: &mut RepeaterTab) {
    ui.horizontal(|ui| {
        ui.label(RichText::new("Request").strong().color(theme::TEXT));
        ui.label(
            RichText::new("editable raw message")
                .small()
                .color(theme::TEXT_MUTED),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                RichText::new("Ctrl+Enter sends")
                    .small()
                    .color(theme::TEXT_MUTED),
            );
        });
    });
    if let Some(note) = &tab.note {
        ui.add_space(2.0);
        ui.label(RichText::new(note).small().color(theme::AMBER));
    }
    ui.add_space(2.0);
    ui.separator();
    ui.add_space(2.0);

    egui::Frame::NONE
        .fill(Color32::WHITE)
        .stroke(Stroke::new(1.0, theme::BORDER))
        .corner_radius(4.0)
        .inner_margin(egui::Margin::same(8))
        .show(ui, |ui| {
            let min_height = ui.available_height().max(180.0);
            ui.add(
                TextEdit::multiline(&mut tab.request)
                    .code_editor()
                    .desired_width(f32::INFINITY)
                    .min_size(egui::vec2(0.0, min_height)),
            );
        });
}

fn response_pane(ui: &mut egui::Ui, tab: &mut RepeaterTab) {
    ui.horizontal(|ui| {
        ui.label(RichText::new("Response").strong().color(theme::TEXT));
        if tab.pending.is_some() {
            ui.spinner();
            ui.label(
                RichText::new("waiting for the origin")
                    .small()
                    .color(theme::TEXT_MUTED),
            );
        } else if let Some(reply) = &tab.response {
            ui.colored_label(
                view::status_class_color(reply.status),
                RichText::new(repeater_status_text(reply.status, &reply.reason)).strong(),
            );
            ui.label(
                RichText::new(view::format_duration(reply.duration))
                    .small()
                    .color(theme::TEXT_MUTED),
            );
            ui.label(
                RichText::new(view::human_bytes(reply.size))
                    .small()
                    .color(theme::TEXT_MUTED),
            );
        } else if let Some(error) = &tab.error {
            ui.add(egui::Label::new(RichText::new(error).small().color(theme::RED)).truncate())
                .on_hover_text(error);
        } else {
            ui.label(
                RichText::new("No response yet")
                    .small()
                    .color(theme::TEXT_MUTED),
            );
        }

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if message_mode_tab(ui, tab.response_view == MessageView::Hex, "Hex").clicked() {
                tab.response_view = MessageView::Hex;
            }
            if message_mode_tab(ui, tab.response_view == MessageView::Raw, "Raw").clicked() {
                tab.response_view = MessageView::Raw;
            }
            if message_mode_tab(ui, tab.response_view == MessageView::Pretty, "Pretty").clicked() {
                tab.response_view = MessageView::Pretty;
            }
        });
    });

    if let Some(reply) = &tab.response
        && (reply.truncated || !reply.body_complete || reply.body_error.is_some())
    {
        let mut notes = Vec::new();
        if reply.truncated {
            notes.push("response body truncated at the capture limit".to_string());
        }
        if !reply.body_complete {
            notes.push("response body did not finish".to_string());
        }
        if let Some(error) = &reply.body_error {
            notes.push(error.clone());
        }
        ui.label(RichText::new(notes.join(" · ")).small().color(theme::AMBER));
    }

    ui.add_space(2.0);
    ui.separator();
    ui.add_space(2.0);

    if let Some(reply) = &tab.response {
        let text = match tab.response_view {
            MessageView::Pretty => &reply.views.pretty,
            MessageView::Raw => &reply.views.raw,
            MessageView::Hex => &reply.views.hex,
        };
        message_text(ui, tab.id, MessagePane::Response, tab.response_view, text);
    } else if tab.pending.is_some() {
        empty_state(
            ui,
            "Waiting for the response",
            "The edited request has been sent directly to the target.",
        );
    } else if let Some(error) = &tab.error {
        empty_state(ui, "Request failed", error);
    } else {
        empty_state(
            ui,
            "No response yet",
            "Click Send to replay the request. The response will appear here.",
        );
    }
}

fn repeater_status_text(status: u16, reason: &str) -> String {
    if reason.is_empty() {
        status.to_string()
    } else {
        format!("{status} {reason}")
    }
}

impl PerseidApp {
    pub(super) fn repeater_workspace(&self) -> Vec<RepeaterWorkspace> {
        self.repeater_tabs
            .iter()
            .map(|tab| RepeaterWorkspace {
                id: tab.id,
                title: tab.title.clone(),
                scheme: tab.scheme.clone(),
                host: tab.host.clone(),
                request: tab.request.clone(),
                note: tab.note.clone(),
            })
            .collect()
    }

    pub(super) fn apply_repeater_workspace(&mut self, tabs: &[RepeaterWorkspace]) {
        self.repeater_tabs.clear();
        self.repeater_active = 0;
        let mut next_id = 1;
        for workspace in tabs {
            let id = workspace.id.max(next_id);
            next_id = id.saturating_add(1).max(next_id.saturating_add(1));
            let mut tab = RepeaterTab::new(id);
            tab.title = workspace.title.clone();
            tab.scheme = workspace.scheme.clone();
            tab.host = workspace.host.clone();
            tab.request = workspace.request.clone();
            tab.note = workspace.note.clone();
            self.repeater_tabs.push(tab);
        }
        self.repeater_next_tab = next_id;
    }
}

/// Active tab index after the tab at `index` is removed from `len` tabs.
///
/// Removing a tab before the active one shifts it left; removing the active
/// tab selects the nearest remaining tab to the right, or the last one.
fn tab_index_after_close(len: usize, active: usize, index: usize) -> usize {
    if len <= 1 {
        return 0;
    }
    if active > index {
        active - 1
    } else if active >= len - 1 {
        len - 2
    } else {
        active
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::{HistoryItem, Phase};
    use crate::proxy::{CapturedBody, RequestInfo};
    use std::time::SystemTime;

    fn history_item(
        number: u64,
        method: &str,
        path: &str,
        scheme: &str,
        body: Vec<u8>,
    ) -> HistoryItem {
        HistoryItem {
            id: number,
            number,
            timestamp: SystemTime::now(),
            request: RequestInfo {
                method: method.to_string(),
                uri: format!("{scheme}://example.com{path}"),
                scheme: scheme.to_string(),
                host: "example.com".to_string(),
                path: path.to_string(),
                version: "HTTP/1.1".to_string(),
                headers: Vec::new(),
            },
            request_body: CapturedBody::from_bytes(body),
            response: None,
            phase: Phase::InFlight,
        }
    }

    #[test]
    fn splits_lf_and_crlf_request_bodies() {
        assert_eq!(
            split_raw_request("POST / HTTP/1.1\r\n\r\nhello"),
            ("POST / HTTP/1.1\r\n\r\n", "hello")
        );
        assert_eq!(
            split_raw_request("POST / HTTP/1.1\n\nhello"),
            ("POST / HTTP/1.1\n\n", "hello")
        );
        assert_eq!(split_raw_request("GET / HTTP/1.1"), ("GET / HTTP/1.1", ""));
    }

    #[test]
    fn seeding_from_history_keeps_the_wire_request_and_a_path_title() {
        let item = history_item(7, "GET", "/search?q=1", "https", Vec::new());
        let tab = RepeaterTab::from_history(1, &item);
        assert_eq!(tab.title, "#7 GET /search");
        assert_eq!(tab.scheme, "https");
        assert_eq!(tab.host, "example.com");
        assert_eq!(tab.request, view::request_views(&item).raw);
        assert!(tab.note.is_none());
    }

    #[test]
    fn seeding_from_history_defaults_an_empty_scheme_to_http() {
        let item = history_item(1, "GET", "/", "", Vec::new());
        let tab = RepeaterTab::from_history(1, &item);
        assert_eq!(tab.scheme, "http");
    }

    #[test]
    fn seeding_from_history_truncates_long_titles() {
        let path = format!("/{}", "a".repeat(80));
        let item = history_item(3, "GET", &path, "https", Vec::new());
        let tab = RepeaterTab::from_history(1, &item);
        assert_eq!(tab.title.chars().count(), 56);
        assert!(tab.title.ends_with("..."));
    }

    #[test]
    fn seeding_from_history_notes_binary_bodies_before_sending() {
        let item = history_item(1, "POST", "/upload", "https", vec![0xff, 0x00]);
        let tab = RepeaterTab::from_history(1, &item);
        let note = tab.note.expect("binary body needs a note");
        assert!(note.contains("Seeded from history"));
        assert!(note.ends_with("Review the body before sending."));
    }

    #[test]
    fn closing_a_tab_selects_the_nearest_remaining_tab() {
        // Removing a tab before the active one shifts the selection left.
        assert_eq!(tab_index_after_close(3, 2, 0), 1);
        // Removing a tab after the active one keeps the selection.
        assert_eq!(tab_index_after_close(3, 0, 2), 0);
        // Removing the active last tab selects the previous one.
        assert_eq!(tab_index_after_close(3, 2, 2), 1);
        // Removing the active middle tab selects the tab that follows it.
        assert_eq!(tab_index_after_close(3, 1, 1), 1);
        // Removing the only tab resets the index.
        assert_eq!(tab_index_after_close(1, 0, 0), 0);
    }
}
