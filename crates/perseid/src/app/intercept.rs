//! Intercepted-message table and raw message editor.
//!
//! Burp holds intercepted messages in a list rather than showing only the
//! front of a queue. The table keeps every waiting message visible, supports
//! Burp-style single, range, and toggle selection, and edits the most recently
//! selected message below it. Each entry keeps its own draft so switching rows
//! never discards edits that have not been forwarded yet.

use std::collections::BTreeSet;

use crate::highlight;
use crate::proxy::{CapturedBody, InterceptError, InterceptItem, InterceptKey, InterceptKind};
use egui::{Align, Label, TextEdit};
use egui_extras::{Column, TableBuilder};

use super::util::request_info_url;
use super::widgets::empty_state;
use super::*;

const INTERCEPT_EDITOR_ID: &str = "perseid.proxy.intercept.editor";

/// One held message together with the user's in-progress edits.
pub(super) struct InterceptEntry {
    item: InterceptItem,
    draft: InterceptDraft,
}

impl InterceptEntry {
    pub(super) fn new(item: InterceptItem) -> Self {
        let draft = InterceptDraft::from_item(&item);
        Self { item, draft }
    }

    fn key(&self) -> InterceptKey {
        self.item.key()
    }
}

/// UI-side copy of a message waiting for a decision.
pub(super) struct InterceptDraft {
    pub head: String,
    pub body: String,
    pub body_editable: bool,
    pub binary_preview: Option<String>,
    pub error: Option<String>,
}

impl InterceptDraft {
    fn from_item(item: &InterceptItem) -> Self {
        let body = item.body();
        let body_editable = body.is_some() && item.body_is_text();
        let (body_text, binary_preview) = match body {
            Some(bytes) if body_editable => (String::from_utf8_lossy(bytes).into_owned(), None),
            Some(bytes) => (
                String::new(),
                Some(view::format_binary_preview(&CapturedBody::from_bytes(
                    bytes.to_vec(),
                ))),
            ),
            None => (String::new(), None),
        };

        Self {
            head: item.head().to_string(),
            body: body_text,
            body_editable,
            binary_preview,
            error: None,
        }
    }
}

impl PerseidApp {
    /// Drop all held messages and their selection state, e.g. when the proxy
    /// is restarted or stops. Dropping an [`InterceptItem`] aborts its
    /// exchange, which is the intended behavior for a stopped proxy.
    pub(super) fn clear_intercept_queue(&mut self) {
        self.intercept_queue.clear();
        self.intercept_selected.clear();
        self.intercept_active = None;
        self.intercept_anchor = None;
        self.intercept_scroll_to_active = false;
    }

    /// Reconcile selection with the messages that are still waiting.
    ///
    /// The table always keeps one active message while the queue is non-empty,
    /// so the editor and Forward/Drop buttons have an unambiguous target.
    pub(super) fn sync_intercept_selection(&mut self) {
        let keys: Vec<InterceptKey> = self
            .intercept_queue
            .iter()
            .map(InterceptEntry::key)
            .collect();
        self.intercept_selected.retain(|key| keys.contains(key));

        let active_is_selected = self
            .intercept_active
            .is_some_and(|key| self.intercept_selected.contains(&key));
        if !active_is_selected {
            // Prefer the last selected row in table order, matching Burp's
            // "most recently selected" editor behavior. Otherwise fall back to
            // the oldest held message.
            self.intercept_active = keys
                .iter()
                .rev()
                .find(|key| self.intercept_selected.contains(*key))
                .copied()
                .or_else(|| keys.first().copied());
        }
        if self.intercept_selected.is_empty()
            && let Some(active) = self.intercept_active
        {
            self.intercept_selected.insert(active);
        }
        if self.intercept_anchor.is_none_or(|key| !keys.contains(&key)) {
            self.intercept_anchor = self.intercept_active;
        }
    }

    pub(super) fn intercept_view(&mut self, ui: &mut egui::Ui) {
        if !self.intercept_queue.is_empty() {
            self.sync_intercept_selection();
        }
        let mut settings_changed = false;
        let mut forward_selected = false;
        let mut drop_selected = false;
        let mut forward_all = false;
        let mut drop_all = false;
        let pending = self.intercept_queue.len();
        let target_count = if self.intercept_selected.is_empty() {
            usize::from(self.intercept_active.is_some())
        } else {
            self.intercept_selected.len()
        };
        let listener = self
            .proxy
            .as_ref()
            .map(Proxy::local_addr)
            .unwrap_or(self.listen);

        egui::Frame::NONE
            .fill(theme::SURFACE)
            .inner_margin(egui::Margin::symmetric(12, 6))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("Intercept")
                            .size(17.0)
                            .strong()
                            .color(theme::TEXT),
                    );
                    if pending > 0 {
                        ui.colored_label(theme::AMBER, format!("{pending} held"));
                    }
                    ui.with_layout(
                        egui::Layout::right_to_left(egui::Align::Center),
                        |ui| {
                            self.capture_toggle(ui);
                            ui.add_space(8.0);
                            ui.label(
                                RichText::new(listener.to_string())
                                    .monospace()
                                    .small()
                                    .color(theme::TEXT_MUTED),
                            );
                            if self.proxy.is_some() {
                                ui.colored_label(theme::GREEN, "listening");
                            } else {
                                ui.colored_label(theme::RED, "not running");
                                if ui.button("Retry").clicked() {
                                    let notify = self.notify.clone();
                                    self.start_proxy(notify);
                                }
                            }
                        },
                    );
                });
                ui.add_space(5.0);

                // Burp's Intercept tab has one master on/off control. Requests
                // are held by default; responses are a separate, opt-in rule
                // set and stay off until explicitly enabled here.
                ui.horizontal(|ui| {
                    let on = self.intercept_enabled;
                    let toggle = egui::Button::new(
                        RichText::new(if on {
                            "Intercept is on"
                        } else {
                            "Intercept is off"
                        })
                        .size(13.5)
                        .strong()
                        .color(if on { Color32::WHITE } else { theme::TEXT }),
                    )
                    .fill(if on {
                        theme::ACCENT_DARK
                    } else {
                        theme::SURFACE_ALT
                    })
                    .stroke(Stroke::new(
                        1.0,
                        if on {
                            theme::ACCENT_DARK
                        } else {
                            theme::BORDER
                        },
                    ))
                    .corner_radius(5.0)
                    .min_size(egui::vec2(136.0, 30.0));
                    if ui
                        .add(toggle)
                        .on_hover_text(if on {
                            "Turn interception off. Already held messages still need Forward or Drop."
                        } else {
                            "Hold HTTP requests, and responses when enabled, before they are forwarded."
                        })
                        .clicked()
                    {
                        self.intercept_enabled = !self.intercept_enabled;
                        settings_changed = true;
                    }

                    let mut responses = self.intercept_responses;
                    if ui
                        .checkbox(&mut responses, "Responses")
                        .on_hover_text(
                            "Also hold server responses. Burp keeps response interception off by default.",
                        )
                        .changed()
                    {
                        self.intercept_responses = responses;
                        settings_changed = true;
                    }

                    ui.add_space(4.0);
                    let can_decide = pending > 0;
                    if ui
                        .add_enabled(can_decide, egui::Button::new("Forward"))
                        .on_hover_text(if target_count > 1 {
                            format!("Forward the {target_count} selected messages")
                        } else {
                            "Forward the selected message".to_string()
                        })
                        .on_disabled_hover_text("No intercepted message is held")
                        .clicked()
                    {
                        forward_selected = true;
                    }
                    if ui
                        .add_enabled(can_decide, egui::Button::new("Drop"))
                        .on_hover_text(if target_count > 1 {
                            format!("Drop the {target_count} selected messages")
                        } else {
                            "Drop the selected message".to_string()
                        })
                        .on_disabled_hover_text("No intercepted message is held")
                        .clicked()
                    {
                        drop_selected = true;
                    }
                    ui.separator();
                    if ui
                        .add_enabled(pending > 0, egui::Button::new("Forward all"))
                        .on_hover_text("Forward every held message in table order")
                        .on_disabled_hover_text("No intercepted message is held")
                        .clicked()
                    {
                        forward_all = true;
                    }
                    if ui
                        .add_enabled(pending > 0, egui::Button::new("Drop all"))
                        .on_hover_text("Drop every held message")
                        .on_disabled_hover_text("No intercepted message is held")
                        .clicked()
                    {
                        drop_all = true;
                    }
                });

                ui.add_space(4.0);
                let decrypting = self.tls_intercept && self.tls_ca.is_some();
                let holding_message = if self.intercept_enabled {
                    if self.intercept_responses {
                        if decrypting {
                            "New HTTP and decrypted HTTPS requests and responses will wait here."
                        } else {
                            "New HTTP requests and responses will wait here. HTTPS stays an opaque CONNECT tunnel."
                        }
                    } else if decrypting {
                        "New HTTP and decrypted HTTPS requests will wait here."
                    } else {
                        "New HTTP requests will wait here. HTTPS stays an opaque CONNECT tunnel."
                    }
                } else {
                    "New HTTP traffic is forwarded automatically."
                };
                ui.label(
                    RichText::new(holding_message)
                        .small()
                        .color(if self.intercept_enabled {
                            theme::ACCENT_DARK
                        } else {
                            theme::TEXT_MUTED
                        }),
                );
                let https_hint = if decrypting {
                    "HTTPS is decrypted into ordinary HTTP messages."
                } else {
                    "HTTPS is CONNECT passthrough; enable decryption in Proxy > TLS."
                };
                ui.label(
                    RichText::new(format!(
                        "{https_hint} Bodies over {} are held for header edits only.",
                        view::human_bytes(self.intercept_limit as u64)
                    ))
                    .small()
                    .color(theme::TEXT_MUTED),
                );
                if let Some(error) = &self.proxy_error {
                    ui.label(RichText::new(error).small().color(theme::RED));
                }
            });

        if forward_selected {
            self.decide_intercept(true, false);
        }
        if drop_selected {
            self.decide_intercept(false, false);
        }
        if forward_all {
            self.decide_intercept(true, true);
        }
        if drop_all {
            self.decide_intercept(false, true);
        }

        let pending = self.intercept_queue.len();
        if pending > 0 {
            egui::Panel::bottom(INTERCEPT_EDITOR_ID)
                .resizable(true)
                .default_size(300.0)
                .min_size(220.0)
                .frame(
                    egui::Frame::NONE
                        .fill(theme::SURFACE)
                        .inner_margin(egui::Margin::same(8)),
                )
                .show(ui, |ui| self.intercept_editor(ui));
        }

        let mut clicked = None;
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(theme::SURFACE))
            .show(ui, |ui| self.intercept_table(ui, &mut clicked));
        if let Some((key, toggle, range)) = clicked {
            self.select_intercept(key, toggle, range);
            self.sync_intercept_selection();
        }

        if settings_changed {
            self.apply_proxy_settings();
        }
    }

    fn intercept_table(
        &mut self,
        ui: &mut egui::Ui,
        clicked: &mut Option<(InterceptKey, bool, bool)>,
    ) {
        let rows = self.intercept_queue.len();
        if rows == 0 {
            if self.intercept_enabled {
                empty_state(
                    ui,
                    "Waiting for traffic",
                    if self.intercept_responses {
                        "The next HTTP request or response will appear here before it is forwarded."
                    } else {
                        "The next HTTP request will appear here before it is forwarded."
                    },
                );
            } else {
                empty_state(
                    ui,
                    "Interception is off",
                    "Click Intercept is off above to start holding HTTP requests and responses.",
                );
            }
            return;
        }

        // The row itself is the click target. Selectable labels inside the
        // cells would capture the click before the row response sees it.
        ui.style_mut().interaction.selectable_labels = false;
        let modifiers = ui.input(|input| input.modifiers);
        let scroll_to = self
            .intercept_scroll_to_active
            .then(|| self.active_intercept_index())
            .flatten();
        self.intercept_scroll_to_active = false;
        let selected = &self.intercept_selected;
        let queue = &self.intercept_queue;

        let mut table = TableBuilder::new(ui)
            .striped(true)
            .resizable(true)
            .sense(egui::Sense::click())
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::exact(74.0))
            .column(Column::exact(54.0))
            .column(Column::exact(78.0))
            .column(Column::exact(70.0))
            .column(Column::initial(500.0).at_least(180.0))
            .column(Column::exact(78.0))
            .column(Column::remainder().at_least(64.0));

        if let Some(index) = scroll_to {
            table = table.scroll_to_row(index, Some(Align::Center));
        }

        table
            .header(22.0, |mut header| {
                for title in [
                    "Time",
                    "Type",
                    "Direction",
                    "Method",
                    "URL",
                    "Status code",
                    "Length",
                ] {
                    header.col(|ui| {
                        ui.label(
                            RichText::new(title)
                                .strong()
                                .small()
                                .color(theme::TEXT_MUTED),
                        );
                    });
                }
            })
            .body(|body| {
                body.rows(22.0, rows, |mut row| {
                    let index = row.index();
                    let Some(entry) = queue.get(index) else {
                        return;
                    };
                    let key = entry.key();
                    let item = &entry.item;
                    let request = item.request();
                    let url = request_info_url(request);
                    row.set_selected(selected.contains(&key));

                    row.col(|ui| {
                        ui.label(
                            RichText::new(view::format_time(item.timestamp()))
                                .small()
                                .color(theme::TEXT_MUTED),
                        );
                    });
                    row.col(|ui| {
                        ui.label(RichText::new("HTTP").small().color(theme::TEXT_MUTED));
                    });
                    row.col(|ui| {
                        let (direction, color) = match item.kind() {
                            InterceptKind::Request => ("Request", theme::TEXT),
                            InterceptKind::Response => ("Response", theme::ACCENT_DARK),
                        };
                        ui.label(RichText::new(direction).small().color(color));
                    });
                    row.col(|ui| {
                        ui.label(RichText::new(&request.method).color(theme::TEXT));
                    });
                    row.col(|ui| {
                        ui.add(Label::new(RichText::new(&url).color(theme::TEXT)).truncate())
                            .on_hover_text(&url);
                    });
                    row.col(|ui| {
                        let status = item.response_status();
                        let text = status.map(|code| code.to_string()).unwrap_or_default();
                        let color = status.map(status_color).unwrap_or(theme::TEXT_MUTED);
                        ui.label(RichText::new(text).color(color));
                    });
                    row.col(|ui| {
                        let length = match item.kind() {
                            InterceptKind::Response => item
                                .body()
                                .map(|body| view::human_bytes(body.len() as u64))
                                .unwrap_or_else(|| "—".to_string()),
                            InterceptKind::Request => String::new(),
                        };
                        ui.label(RichText::new(length).small().color(theme::TEXT_MUTED));
                    });

                    let hover = match item.kind() {
                        InterceptKind::Request => format!("{url}\nHeld request"),
                        InterceptKind::Response => {
                            format!(
                                "{url}\nHeld response to {} {}",
                                request.method, request.path
                            )
                        }
                    };
                    if row.response().on_hover_text(hover).clicked() {
                        *clicked = Some((key, modifiers.command, modifiers.shift));
                    }
                });
            });
    }

    fn active_intercept_index(&self) -> Option<usize> {
        let active = self.intercept_active?;
        self.intercept_queue
            .iter()
            .position(|entry| entry.key() == active)
    }

    fn select_intercept(&mut self, key: InterceptKey, toggle: bool, range: bool) {
        if range {
            let Some(anchor) = self.intercept_anchor else {
                self.intercept_selected.clear();
                self.intercept_selected.insert(key);
                self.intercept_active = Some(key);
                self.intercept_anchor = Some(key);
                return;
            };
            let start = self
                .intercept_queue
                .iter()
                .position(|entry| entry.key() == anchor);
            let end = self
                .intercept_queue
                .iter()
                .position(|entry| entry.key() == key);
            if let (Some(start), Some(end)) = (start, end) {
                self.intercept_selected.clear();
                let (first, last) = if start <= end {
                    (start, end)
                } else {
                    (end, start)
                };
                for entry in self
                    .intercept_queue
                    .iter()
                    .skip(first)
                    .take(last - first + 1)
                {
                    self.intercept_selected.insert(entry.key());
                }
            }
            self.intercept_active = Some(key);
        } else if toggle {
            if !self.intercept_selected.remove(&key) {
                self.intercept_selected.insert(key);
            }
            self.intercept_active = Some(key);
            self.intercept_anchor = Some(key);
        } else {
            self.intercept_selected.clear();
            self.intercept_selected.insert(key);
            self.intercept_active = Some(key);
            self.intercept_anchor = Some(key);
        }
    }

    /// Forward or drop the current selection, or every held message when
    /// `all` is set. Messages are processed in table order.
    fn decide_intercept(&mut self, forward: bool, all: bool) {
        let targets: BTreeSet<InterceptKey> = if all {
            self.intercept_queue
                .iter()
                .map(InterceptEntry::key)
                .collect()
        } else if self.intercept_selected.is_empty() {
            self.intercept_active.into_iter().collect()
        } else {
            self.intercept_selected.iter().copied().collect()
        };
        if targets.is_empty() {
            return;
        }

        let mut resolved = BTreeSet::new();
        for entry in &mut self.intercept_queue {
            let key = entry.key();
            if !targets.contains(&key) {
                continue;
            }
            let result = if forward {
                let body = entry
                    .draft
                    .body_editable
                    .then(|| entry.draft.body.as_bytes().to_vec());
                entry.item.forward(&entry.draft.head, body)
            } else {
                entry.item.drop_message()
            };
            match result {
                Ok(()) => {
                    resolved.insert(key);
                }
                // The exchange ended while the message was held; the client
                // closed the connection before a decision could be delivered.
                // Discard it so newer messages are not stuck behind a dead one.
                Err(InterceptError::Closed | InterceptError::AlreadyResolved) => {
                    resolved.insert(key);
                }
                Err(err) => {
                    entry.draft.error = Some(err.to_string());
                }
            }
        }

        if !resolved.is_empty() {
            self.intercept_queue
                .retain(|entry| !resolved.contains(&entry.key()));
            self.intercept_selected
                .retain(|key| !resolved.contains(key));
            self.sync_intercept_selection();
            // Keep the editor pointed at the replacement active row even when
            // the anchor itself was just forwarded or dropped.
            self.intercept_anchor = self.intercept_active;
            self.intercept_scroll_to_active = true;
        }
    }

    fn intercept_editor(&mut self, ui: &mut egui::Ui) {
        let selected_count = self.intercept_selected.len();
        let Some(index) = self.active_intercept_index() else {
            empty_state(
                ui,
                "Select a message",
                "Choose a row in the intercept table to inspect or edit it.",
            );
            return;
        };

        egui::ScrollArea::vertical()
            .id_salt("perseid.intercept.editor.scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let InterceptEntry { item, draft } = &mut self.intercept_queue[index];
                let kind = item.kind();
                let method = item.request().method.clone();
                let path = item.request().path.clone();
                let body_buffered = item.body().is_some();
                let body_len = item.body().map(<[u8]>::len).unwrap_or(0);
                let body_is_text = item.body_is_text();
                let id = item.id();
                let title = match kind {
                    InterceptKind::Request => "Request",
                    InterceptKind::Response => "Response",
                };
                let detail = match kind {
                    InterceptKind::Request => format!("{method} {path}"),
                    InterceptKind::Response => format!("response to {method} {path}"),
                };

                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(title)
                            .size(15.0)
                            .strong()
                            .color(theme::TEXT),
                    );
                    ui.label(
                        RichText::new(detail)
                            .monospace()
                            .small()
                            .color(theme::TEXT_MUTED),
                    );
                    if selected_count > 1 {
                        ui.colored_label(
                            theme::TEXT_MUTED,
                            format!("{selected_count} selected"),
                        );
                    }
                });
                ui.add_space(3.0);

                let body_status = if body_buffered {
                    format!(
                        "Body: {} ({})",
                        view::human_bytes(body_len as u64),
                        if body_is_text {
                            "editable text"
                        } else {
                            "binary, read-only"
                        }
                    )
                } else {
                    "Body: not buffered because it is large or streaming; header edits only"
                        .to_string()
                };
                ui.label(
                    RichText::new(body_status)
                        .small()
                        .color(if body_buffered {
                            theme::TEXT_MUTED
                        } else {
                            theme::AMBER
                        }),
                );
                ui.add_space(3.0);

                let font = egui::FontId::monospace(highlight::MESSAGE_FONT_SIZE);
                let mut head_layouter =
                    |ui: &egui::Ui, buffer: &dyn egui::TextBuffer, wrap_width: f32| {
                        let job = highlight::message_job(buffer.as_str(), wrap_width, &font);
                        ui.fonts_mut(|fonts| fonts.layout_job(job))
                    };
                let mut body_layouter =
                    |ui: &egui::Ui, buffer: &dyn egui::TextBuffer, wrap_width: f32| {
                        let job = highlight::body_job(buffer.as_str(), wrap_width, &font);
                        ui.fonts_mut(|fonts| fonts.layout_job(job))
                    };

                ui.label(RichText::new("Head").strong().color(theme::TEXT));
                egui::Frame::NONE
                    .fill(Color32::WHITE)
                    .stroke(Stroke::new(1.0, theme::BORDER))
                    .corner_radius(4.0)
                    .inner_margin(egui::Margin::same(6))
                    .show(ui, |ui| {
                        if ui
                            .add(
                                TextEdit::multiline(&mut draft.head)
                                    .code_editor()
                                    .font(egui::FontSelection::FontId(font.clone()))
                                    .layouter(&mut head_layouter)
                                    .desired_rows(7)
                                    .desired_width(f32::INFINITY),
                            )
                            .changed()
                        {
                            draft.error = None;
                        }
                    });
                ui.add_space(6.0);

                ui.label(RichText::new("Body").strong().color(theme::TEXT));
                if draft.body_editable {
                    egui::Frame::NONE
                        .fill(Color32::WHITE)
                        .stroke(Stroke::new(1.0, theme::BORDER))
                        .corner_radius(4.0)
                        .inner_margin(egui::Margin::same(6))
                        .show(ui, |ui| {
                            if ui
                                .add(
                                    TextEdit::multiline(&mut draft.body)
                                        .code_editor()
                                        .font(egui::FontSelection::FontId(font.clone()))
                                        .layouter(&mut body_layouter)
                                        .desired_rows(9)
                                        .desired_width(f32::INFINITY),
                                )
                                .changed()
                            {
                                draft.error = None;
                            }
                        });
                } else if let Some(preview) = &draft.binary_preview {
                    egui::Frame::NONE
                        .fill(theme::SURFACE_TINT)
                        .stroke(Stroke::new(1.0, theme::BORDER))
                        .corner_radius(4.0)
                        .inner_margin(egui::Margin::same(6))
                        .show(ui, |ui| {
                            egui::ScrollArea::both()
                                .id_salt(("perseid.intercept.binary", id, kind))
                                .max_height(240.0)
                                .auto_shrink([false, false])
                                .show(ui, |ui| {
                                    ui.add(
                                        Label::new(
                                            RichText::new(preview).monospace().color(theme::TEXT),
                                        )
                                        .selectable(true)
                                        .wrap_mode(egui::TextWrapMode::Extend),
                                    );
                                });
                        });
                } else {
                    ui.label(
                        RichText::new(
                            "This body is not buffered. Forward the message with the original body, or edit the head.",
                        )
                        .small()
                        .color(theme::TEXT_MUTED),
                    );
                }

                if let Some(error) = &draft.error {
                    ui.add_space(4.0);
                    ui.label(RichText::new(error).small().color(theme::RED));
                }
            });
    }
}

fn status_color(status: u16) -> Color32 {
    if status >= 500 {
        theme::RED
    } else if status >= 400 {
        theme::AMBER
    } else {
        theme::GREEN
    }
}
