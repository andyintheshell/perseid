//! Editable Repeater: request tabs, a raw request editor, and direct replay.
//!
//! The layout follows Burp's Repeater: a strip of request tabs, a target bar
//! with Send, the editable request on the left, and the response on the right.
//! Repeater uses its own replay engine rather than going back through the local
//! proxy listener, so it also works when the proxy is stopped.

use std::time::{Duration, Instant};

use egui::TextEdit;
use perseid_proxy::{
    DEFAULT_REPEATER_TIMEOUT, Repeater, RepeaterConfig, RepeaterRequest, RepeaterResult,
};

use super::message_view::message_text;
use super::widgets::{empty_state, message_mode_tab, section_tab};
use super::*;
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

    fn from_history(id: u64, item: &crate::history::HistoryItem) -> Self {
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
        let body = &item.request_body;
        let mut notes = Vec::new();
        if body.is_truncated() {
            notes.push("the captured request body was truncated at the capture limit");
        }
        if !body.is_complete() {
            notes.push("the captured request body did not finish");
        }
        if body.data().len() > view::TEXT_PREVIEW_LIMIT {
            notes.push("the editor contains a bounded body preview");
        }
        if !body.data().is_empty() && std::str::from_utf8(body.data()).is_err() {
            notes.push(
                "the captured body was encoded or binary; the editor uses a best-effort text view",
            );
        }

        let note = (!notes.is_empty()).then(|| {
            format!(
                "Seeded from history: {}. Review the body before sending.",
                notes.join("; ")
            )
        });

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
                    .inner_margin(egui::Margin::symmetric(12, 4)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("Repeater")
                            .size(15.0)
                            .strong()
                            .color(theme::TEXT),
                    );
                    ui.separator();
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
                    "Select an exchange in Proxy > HTTP history and choose Send to Repeater, \
                     or start with a blank tab.",
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
                    .inner_margin(egui::Margin::symmetric(12, 6)),
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
                    .min_size(egui::vec2(76.0, 30.0));
                    if ui
                        .add_enabled(tab.pending.is_none(), send_button)
                        .on_hover_text("Send the edited request (Ctrl+Enter)")
                        .on_disabled_hover_text("Waiting for the current response")
                        .clicked()
                    {
                        send = true;
                    }

                    ui.add_space(6.0);
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
                            repeater_status_color(reply.status),
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
        let default_width = (ui.available_width() * 0.5).max(300.0);

        egui::Panel::left(REPEATER_REQUEST_ID)
            .resizable(true)
            .default_size(default_width)
            .min_size(220.0)
            .frame(
                egui::Frame::NONE
                    .fill(theme::SURFACE)
                    .inner_margin(egui::Margin::same(8)),
            )
            .show(ui, |ui| {
                let tab = &mut self.repeater_tabs[index];
                request_editor(ui, tab);
            });

        egui::CentralPanel::default()
            .frame(
                egui::Frame::NONE
                    .fill(theme::SURFACE)
                    .inner_margin(egui::Margin::same(8)),
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
        if index >= self.repeater_tabs.len() {
            return;
        }
        self.repeater_tabs.remove(index);
        if self.repeater_tabs.is_empty() {
            self.repeater_active = 0;
        } else if self.repeater_active > index {
            self.repeater_active -= 1;
        } else if self.repeater_active >= self.repeater_tabs.len() {
            self.repeater_active = self.repeater_tabs.len() - 1;
        }
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
                tab.response = None;
                tab.error = Some(err.to_string());
                // A closed job channel means the worker is gone even if the
                // runtime thread is still winding down. Drop the handle so the
                // next Send can start a fresh engine.
                self.repeater = None;
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
                repeater_status_color(reply.status),
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

fn split_raw_request(raw: &str) -> (&str, &str) {
    if let Some(position) = raw.find("\r\n\r\n") {
        (&raw[..position + 4], &raw[position + 4..])
    } else if let Some(position) = raw.find("\n\n") {
        (&raw[..position + 2], &raw[position + 2..])
    } else {
        (raw, "")
    }
}

fn repeater_status_text(status: u16, reason: &str) -> String {
    if reason.is_empty() {
        status.to_string()
    } else {
        format!("{status} {reason}")
    }
}

fn repeater_status_color(status: u16) -> Color32 {
    match status / 100 {
        2 => theme::GREEN,
        3 => theme::BLUE,
        4 => theme::AMBER,
        5 => theme::RED,
        _ => theme::GRAY,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
