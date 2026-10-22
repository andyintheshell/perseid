//! Intercepted-message table and message editor.
//!
//! Burp holds intercepted messages in a list rather than showing only the
//! front of a queue. The table keeps every waiting message visible, supports
//! Burp-style single, range, and toggle selection, and edits the most recently
//! selected message below it. Each entry keeps its own draft so switching rows
//! never discards edits that have not been forwarded yet. Text bodies are
//! editable in both Pretty and Raw, with edits synchronized between views.

use std::collections::BTreeSet;

use crate::proxy::{InterceptError, InterceptItem, InterceptKey, InterceptKind};
use egui::{Align, Label};
use egui_extras::{Column, TableBuilder};

use super::message_view::{message_editor, message_tabs, message_text, message_view_text};
use super::util::request_info_url;
use super::widgets::{empty_state, table_frame};
use super::*;

const INTERCEPT_EDITOR_ID: &str = "perseid.proxy.intercept.editor";
/// Keep interception message editor state separate from history entries, which
/// can use the same exchange id.
const INTERCEPT_MESSAGE_ID: u64 = 1 << 63;

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

    pub(super) fn key(&self) -> InterceptKey {
        self.item.key()
    }
}

/// UI-side copy of a message waiting for a decision.
pub(super) struct InterceptDraft {
    pub head: String,
    pub body: String,
    pub body_editable: bool,
    pub view: MessageView,
    /// Complete Raw buffer, including the body when it is editable.
    pub raw: String,
    /// Complete Pretty buffer for an editable text body.
    pub pretty: String,
    /// Body portion of [`InterceptDraft::pretty`] when it was last generated.
    /// Comparing it on an edit avoids rewriting the body when only a header
    /// changed in the Pretty tab.
    pretty_body_baseline: String,
    /// Read-only Pretty/Hex representations rebuilt after edits. Pretty is
    /// only read from here when the body itself is not editable.
    pub views: MessageViews,
    pub views_dirty: bool,
    /// A Raw edit invalidated [`InterceptDraft::pretty`].
    pub pretty_dirty: bool,
    pub error: Option<String>,
}

impl InterceptDraft {
    fn from_item(item: &InterceptItem) -> Self {
        let body = item.body();
        let body_editable = body.is_some() && item.body_is_text();
        let body_text = if body_editable {
            String::from_utf8_lossy(body.unwrap_or_default()).into_owned()
        } else {
            String::new()
        };
        let head = view::canonicalize_editor_head(item.head());
        let raw = if body_editable {
            format!("{head}{body_text}")
        } else {
            head.clone()
        };
        let views = view::editor_views(&head, body.unwrap_or_default());
        let pretty = if body_editable {
            view::editor_pretty(&head, body.unwrap_or_default())
        } else {
            views.pretty.clone()
        };
        let pretty_body_baseline = normalized_pretty_body(&pretty);

        Self {
            head,
            body: body_text,
            body_editable,
            view: MessageView::Pretty,
            raw,
            pretty,
            pretty_body_baseline,
            views,
            views_dirty: false,
            pretty_dirty: false,
            error: None,
        }
    }

    /// Split an edited Raw message back into the head and body that forwarding
    /// expects. Pretty/Hex formatting is deferred until those views are opened
    /// so typing in a large body does not reformat it on every keystroke.
    fn sync_from_raw(&mut self) {
        let normalized = self.raw.replace("\r\n", "\n");
        let (head, body) = split_editor_message(&normalized);
        self.head = head.to_string();
        self.body = body.to_string();
        self.views_dirty = true;
        self.pretty_dirty = self.body_editable;
        self.error = None;
    }

    /// Split an edited Pretty message back into the head and body that
    /// forwarding expects. The Raw buffer follows immediately so switching
    /// tabs never loses the edit; bounded Hex formatting stays deferred.
    fn sync_from_pretty(&mut self) {
        let normalized = self.pretty.replace("\r\n", "\n");
        let (head, body) = split_editor_message(&normalized);
        self.head = head.to_string();
        if body != self.pretty_body_baseline {
            self.body = body.to_string();
            self.pretty_body_baseline = body.to_string();
        }
        self.raw = if self.body_editable {
            format!("{}{}", self.head, self.body)
        } else {
            self.head.clone()
        };
        self.views_dirty = true;
        self.pretty_dirty = false;
        self.error = None;
    }

    /// Rebuild the complete editable Pretty buffer after a Raw edit.
    fn refresh_pretty(&mut self) {
        if !self.pretty_dirty {
            return;
        }
        self.pretty = view::editor_pretty(&self.head, self.body.as_bytes());
        self.pretty_body_baseline = normalized_pretty_body(&self.pretty);
        self.pretty_dirty = false;
    }

    /// Rebuild the bounded Pretty/Hex previews after an edit.
    fn refresh_views(&mut self, binary_body: &[u8]) {
        if !self.views_dirty {
            return;
        }
        let body = if self.body_editable {
            self.body.as_bytes()
        } else {
            binary_body
        };
        self.views = view::editor_views(&self.head, body);
        self.views_dirty = false;
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

        egui::Frame::NONE
            .fill(theme::SURFACE)
            .inner_margin(egui::Margin::symmetric(10, 4))
            .show(ui, |ui| {
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
                        .size(12.0)
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
                    .corner_radius(3.0)
                    .min_size(egui::vec2(112.0, 24.0));
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

                    if pending > 0 {
                        ui.with_layout(
                            egui::Layout::right_to_left(egui::Align::Center),
                            |ui| {
                                ui.colored_label(theme::AMBER, format!("{pending} held"));
                            },
                        );
                    }
                });
                if let Some(error) = &self.proxy_error {
                    ui.label(RichText::new(error).small().color(theme::RED));
                }
            });

        let pending = self.intercept_queue.len();
        if pending > 0 {
            egui::Panel::bottom(INTERCEPT_EDITOR_ID)
                .resizable(true)
                .default_size(320.0)
                .min_size(180.0)
                .frame(
                    egui::Frame::NONE
                        .fill(theme::SURFACE)
                        .inner_margin(egui::Margin::same(5)),
                )
                .show(ui, |ui| self.intercept_editor(ui));
        }

        // Decide after the editor has processed this frame's input so a click
        // on Forward cannot forward the draft from before the last keystroke.
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

        table_frame(ui, |ui| {
            let mut table = TableBuilder::new(ui)
                .striped(true)
                .resizable(true)
                .sense(egui::Sense::click())
                .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                .column(Column::exact(64.0))
                .column(Column::exact(44.0))
                .column(Column::exact(68.0))
                .column(Column::exact(56.0))
                .column(Column::initial(500.0).at_least(180.0))
                .column(Column::exact(68.0))
                .column(Column::remainder().at_least(64.0));

            if let Some(index) = scroll_to {
                table = table.scroll_to_row(index, Some(Align::Center));
            }

            table
                .header(20.0, |mut header| {
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
                            ui.label(RichText::new(title).strong().small().color(theme::TEXT));
                        });
                    }
                })
                .body(|body| {
                    body.rows(20.0, rows, |mut row| {
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
                                    .color(theme::TEXT),
                            );
                        });
                        row.col(|ui| {
                            ui.label(RichText::new("HTTP").small().color(theme::TEXT));
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
                            let color = status.map(status_color).unwrap_or(theme::TEXT);
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
                            ui.label(RichText::new(length).small().color(theme::TEXT));
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

        let InterceptEntry { item, draft } = &mut self.intercept_queue[index];
        let kind = item.kind();
        let method = item.request().method.clone();
        let path = item.request().path.clone();
        let body_buffered = item.body().is_some();
        let body_len = item.body().map(<[u8]>::len).unwrap_or(0);
        let body_is_text = item.body_is_text();
        let head_has_body = head_declares_body(item.head());
        let id = item.id() ^ INTERCEPT_MESSAGE_ID;
        let pane = match kind {
            InterceptKind::Request => MessagePane::Request,
            InterceptKind::Response => MessagePane::Response,
        };
        let title = match kind {
            InterceptKind::Request => "Request",
            InterceptKind::Response => "Response",
        };
        let detail = match kind {
            InterceptKind::Request => format!("{method} {path}"),
            InterceptKind::Response => format!("response to {method} {path}"),
        };

        ui.horizontal(|ui| {
            ui.label(RichText::new(title).strong().color(theme::TEXT));
            ui.label(
                RichText::new(detail)
                    .monospace()
                    .small()
                    .color(theme::TEXT_MUTED),
            );
            if selected_count > 1 {
                ui.colored_label(theme::TEXT_MUTED, format!("{selected_count} selected"));
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if let Some(error) = &draft.error {
                    ui.add(Label::new(RichText::new(error).small().color(theme::RED)).truncate())
                        .on_hover_text(error);
                }
                let (body_status, body_color) = if body_buffered {
                    let size = view::human_bytes(body_len as u64);
                    if body_is_text && matches!(draft.view, MessageView::Pretty | MessageView::Raw)
                    {
                        (format!("{size} · editable"), theme::TEXT_MUTED)
                    } else if body_is_text {
                        (format!("{size} · edit in Pretty or Raw"), theme::TEXT_MUTED)
                    } else {
                        (
                            format!("{size} · binary body, read-only (see Hex)"),
                            theme::AMBER,
                        )
                    }
                } else if head_has_body {
                    (
                        "body not buffered · header edits only".to_string(),
                        theme::AMBER,
                    )
                } else {
                    ("no body".to_string(), theme::TEXT_MUTED)
                };
                ui.label(RichText::new(body_status).small().color(body_color));
            });
        });
        ui.add_space(1.0);
        message_tabs(ui, &mut draft.view);
        ui.add_space(1.0);

        let mode = draft.view;
        match mode {
            MessageView::Raw => {
                if message_editor(ui, id, pane, mode, &mut draft.raw) {
                    draft.sync_from_raw();
                }
            }
            MessageView::Pretty if draft.body_editable => {
                draft.refresh_pretty();
                if message_editor(ui, id, pane, mode, &mut draft.pretty) {
                    draft.sync_from_pretty();
                }
            }
            MessageView::Pretty | MessageView::Hex => {
                draft.refresh_views(item.body().unwrap_or_default());
                message_text(ui, id, pane, mode, message_view_text(&draft.views, mode));
            }
        }
    }
}

fn split_editor_message(text: &str) -> (&str, &str) {
    match text.find("\n\n") {
        Some(index) => (&text[..index + 2], &text[index + 2..]),
        None => (text, ""),
    }
}

fn normalized_pretty_body(pretty: &str) -> String {
    let normalized = pretty.replace("\r\n", "\n");
    split_editor_message(&normalized).1.to_string()
}

fn head_declares_body(head: &str) -> bool {
    head.lines()
        .skip(1)
        .take_while(|line| !line.trim().is_empty())
        .any(|line| {
            let Some((name, value)) = line.split_once(':') else {
                return false;
            };
            if name.trim().eq_ignore_ascii_case("content-length") {
                value.trim().parse::<u64>().is_ok_and(|len| len > 0)
            } else {
                name.trim().eq_ignore_ascii_case("transfer-encoding")
            }
        })
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

#[cfg(test)]
mod tests {
    use super::*;

    fn draft_with_raw(raw: &str) -> InterceptDraft {
        let head = "POST /items HTTP/1.1\nHost: example.com\n\n";
        InterceptDraft {
            head: head.to_string(),
            body: "old".to_string(),
            body_editable: true,
            view: MessageView::Raw,
            raw: raw.to_string(),
            pretty: view::editor_pretty(head, b"old"),
            pretty_body_baseline: "old\n".to_string(),
            views: view::editor_views(head, b"old"),
            views_dirty: false,
            pretty_dirty: false,
            error: Some("stale error".to_string()),
        }
    }

    #[test]
    fn raw_edits_split_into_head_and_body_on_the_first_blank_line() {
        let mut draft =
            draft_with_raw("POST /changed HTTP/1.1\nHost: example.com\n\nnew body\nwith lines");
        draft.sync_from_raw();

        assert_eq!(draft.head, "POST /changed HTTP/1.1\nHost: example.com\n\n");
        assert_eq!(draft.body, "new body\nwith lines");
        assert!(draft.views_dirty, "formatting should be deferred");
        assert!(draft.error.is_none());
    }

    #[test]
    fn deferred_views_refresh_after_a_raw_edit() {
        let mut draft = draft_with_raw("POST /items HTTP/1.1\nHost: example.com\n\nnew body");
        draft.sync_from_raw();
        draft.refresh_views(&[]);

        assert!(!draft.views_dirty);
        assert!(draft.views.raw.contains("new body"));
    }

    #[test]
    fn pretty_edits_split_into_head_and_body_and_update_raw() {
        let mut draft = draft_with_raw("POST /items HTTP/1.1\nHost: example.com\n\nold");
        draft.pretty =
            "POST /pretty HTTP/1.1\nHost: example.com\n\n{\n  \"ok\": true\n}".to_string();
        draft.sync_from_pretty();

        assert_eq!(draft.head, "POST /pretty HTTP/1.1\nHost: example.com\n\n");
        assert_eq!(draft.body, "{\n  \"ok\": true\n}");
        assert_eq!(
            draft.raw,
            "POST /pretty HTTP/1.1\nHost: example.com\n\n{\n  \"ok\": true\n}"
        );
        assert!(draft.views_dirty, "Hex should refresh before it is opened");
        assert!(!draft.pretty_dirty, "the Pretty buffer is already current");
        assert!(draft.error.is_none());
    }

    #[test]
    fn pretty_header_edits_do_not_reformat_the_forwarded_body() {
        let mut draft = draft_with_raw("POST /items HTTP/1.1\nHost: example.com\n\nold");
        draft.pretty = "POST /items HTTP/1.1\nHost: changed.example\n\nold\n".to_string();
        draft.sync_from_pretty();

        assert_eq!(
            draft.head,
            "POST /items HTTP/1.1\nHost: changed.example\n\n"
        );
        assert_eq!(draft.body, "old", "the original wire body should be kept");
        assert_eq!(
            draft.raw,
            "POST /items HTTP/1.1\nHost: changed.example\n\nold"
        );
    }

    #[test]
    fn raw_edits_rebuild_the_pretty_buffer_when_it_is_opened() {
        let mut draft = draft_with_raw(
            "POST /items HTTP/1.1\nHost: example.com\nContent-Type: application/json\n\n{\"ok\":true}",
        );
        draft.sync_from_raw();

        assert!(draft.pretty_dirty);
        draft.refresh_pretty();

        assert!(!draft.pretty_dirty);
        assert!(draft.pretty.contains("\"ok\": true"));
    }

    #[test]
    fn head_body_status_distinguishes_empty_and_unbuffered_bodies() {
        assert!(head_declares_body("POST / HTTP/1.1\nContent-Length: 3\n\n"));
        assert!(!head_declares_body(
            "POST / HTTP/1.1\nContent-Length: 0\n\n"
        ));
        assert!(head_declares_body(
            "POST / HTTP/1.1\nTransfer-Encoding: chunked\n\n"
        ));
        assert!(!head_declares_body("GET / HTTP/1.1\nHost: example.com\n\n"));
    }
}
