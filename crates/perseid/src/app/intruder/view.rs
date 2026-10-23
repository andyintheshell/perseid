//! Intruder workspace drawing: toolbar, positions, editor, payloads, results,
//! and the selected-result detail pane.

use std::ops::Range;

use egui::text::{CCursor, CCursorRange};
use egui::text_edit::TextEditState;
use egui::{Color32, FontId, Label, RichText, Stroke, TextEdit};
use egui_extras::{Column, TableBuilder};

use crate::proxy::{
    AttackType, MAX_INTRUDER_CONCURRENCY, RepeaterResult, auto_mark, clear_markers, marker_ranges,
    toggle_marker, update_host_header,
};
use crate::theme;
use crate::view;

use super::super::message_view::{message_text, message_view_text};
use super::super::widgets::{empty_state, message_mode_tab, table_frame};
use super::super::{MessagePane, MessageView, PerseidApp};
use super::state::{IntruderState, payloads};

const INTRUDER_TOOLBAR_ID: &str = "perseid.intruder.toolbar";
const INTRUDER_POSITIONS_ID: &str = "perseid.intruder.positions";
const INTRUDER_PAYLOADS_ID: &str = "perseid.intruder.payloads";
const INTRUDER_RESULTS_ID: &str = "perseid.intruder.results";
const INTRUDER_RESULTS_HEADER_ID: &str = "perseid.intruder.results.header";
const INTRUDER_DETAIL_ID: &str = "perseid.intruder.detail";
const INTRUDER_EDITOR_ID: &str = "perseid.intruder.request.editor";

/// Offset message-view ids so a result sequence cannot collide with a history
/// exchange that happens to have the same number.
const RESULT_MESSAGE_ID_BASE: u64 = 1_000_000_000;

impl PerseidApp {
    pub(in crate::app) fn intruder_view(&mut self, ui: &mut egui::Ui) {
        self.intruder_toolbar(ui);

        let positions = marker_ranges(&self.intruder.request)
            .map(|ranges| ranges.len())
            .unwrap_or(0);
        self.intruder.sync_payload_sets(positions);
        self.intruder_positions(ui);

        let positions = marker_ranges(&self.intruder.request)
            .map(|ranges| ranges.len())
            .unwrap_or(0);
        self.intruder.sync_payload_sets(positions);
        self.intruder_results(ui);
        self.intruder_payloads(ui);
        self.intruder_editor(ui);
    }

    fn intruder_toolbar(&mut self, ui: &mut egui::Ui) {
        let mut start = false;
        let mut stop = false;
        let mut host_changed = false;

        egui::Panel::top(INTRUDER_TOOLBAR_ID)
            .frame(
                egui::Frame::NONE
                    .fill(theme::SURFACE_ALT)
                    .inner_margin(egui::Margin::symmetric(10, 4)),
            )
            .show(ui, |ui| {
                let state = &mut self.intruder;
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Attack").small().color(theme::TEXT_MUTED));
                    egui::ComboBox::from_id_salt("perseid.intruder.attack")
                        .width(168.0)
                        .selected_text(state.attack.label())
                        .show_ui(ui, |ui| {
                            for attack in AttackType::ALL {
                                ui.selectable_value(&mut state.attack, attack, attack.label())
                                    .on_hover_text(attack.description());
                            }
                        });

                    ui.add_space(4.0);
                    if state.running {
                        let button = egui::Button::new(
                            RichText::new("Stop attack").strong().color(theme::RED),
                        )
                        .fill(theme::SURFACE)
                        .stroke(Stroke::new(1.0, theme::BORDER))
                        .min_size(egui::vec2(86.0, 24.0));
                        if ui
                            .add_enabled(!state.stopping, button)
                            .on_hover_text(
                                "Stop scheduling new requests. In-flight requests still finish.",
                            )
                            .on_disabled_hover_text("Already stopping")
                            .clicked()
                        {
                            stop = true;
                        }
                        if state.stopping {
                            ui.spinner();
                        }
                    } else {
                        let button = egui::Button::new(
                            RichText::new("Start attack").strong().color(Color32::WHITE),
                        )
                        .fill(theme::ACCENT_DARK)
                        .stroke(Stroke::new(1.0, theme::ACCENT_DARK))
                        .min_size(egui::vec2(94.0, 24.0));
                        if ui
                            .add(button)
                            .on_hover_text(
                                "Validate the template and start the attack (Ctrl+Enter)",
                            )
                            .clicked()
                        {
                            start = true;
                        }
                    }

                    ui.separator();
                    ui.label(RichText::new("Target").small().color(theme::TEXT_MUTED));
                    egui::ComboBox::from_id_salt("perseid.intruder.scheme")
                        .width(70.0)
                        .selected_text(&state.scheme)
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut state.scheme, "https".to_string(), "https");
                            ui.selectable_value(&mut state.scheme, "http".to_string(), "http");
                        });
                    if ui
                        .add(
                            TextEdit::singleline(&mut state.host)
                                .desired_width(230.0)
                                .hint_text("host[:port]"),
                        )
                        .on_hover_text(
                            "Connection target. Absolute-form request targets override this.",
                        )
                        .changed()
                    {
                        host_changed = true;
                    }
                    if ui
                        .checkbox(&mut state.update_host, "Update Host header")
                        .on_hover_text(
                            "Keep the Host header in the request in sync with the target above.",
                        )
                        .changed()
                        && state.update_host
                    {
                        host_changed = true;
                    }

                    ui.separator();
                    ui.label(
                        RichText::new("Concurrency")
                            .small()
                            .color(theme::TEXT_MUTED),
                    );
                    ui.add(
                        egui::DragValue::new(&mut state.concurrency)
                            .range(1..=MAX_INTRUDER_CONCURRENCY)
                            .speed(0.2),
                    )
                    .on_hover_text(format!(
                        "Simultaneous requests, 1–{MAX_INTRUDER_CONCURRENCY}"
                    ));

                    if state.running {
                        ui.separator();
                        ui.spinner();
                        ui.label(
                            RichText::new(format!("{} / {}", state.received, state.total))
                                .small()
                                .color(theme::TEXT_MUTED),
                        );
                    }
                });
            });

        if host_changed && self.intruder.update_host {
            self.intruder.request = update_host_header(&self.intruder.request, &self.intruder.host);
        }
        if start {
            self.start_intruder_attack();
        }
        if stop {
            self.stop_intruder_attack();
        }
    }

    fn intruder_positions(&mut self, ui: &mut egui::Ui) {
        let mut add_marker = false;
        let mut clear_all = false;
        let mut auto = false;

        egui::Panel::top(INTRUDER_POSITIONS_ID)
            .frame(
                egui::Frame::NONE
                    .fill(theme::SURFACE)
                    .inner_margin(egui::Margin::symmetric(10, 4)),
            )
            .show(ui, |ui| {
                let state = &mut self.intruder;
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("Positions")
                            .small()
                            .color(theme::TEXT_MUTED),
                    );
                    if ui
                        .button("Add $$")
                        .on_hover_text(
                            "Wrap the selected text in $$ markers. With no selection, insert an empty position at the cursor. Selecting an existing position removes its markers.",
                        )
                        .clicked()
                    {
                        add_marker = true;
                    }
                    if ui
                        .button("Clear $$")
                        .on_hover_text("Remove every position marker from the request")
                        .clicked()
                    {
                        clear_all = true;
                    }
                    if ui
                        .button("Auto $$")
                        .on_hover_text(
                            "Add positions around query values, form values, and cookie values",
                        )
                        .clicked()
                    {
                        auto = true;
                    }

                    ui.separator();
                    match marker_ranges(&state.request) {
                        Ok(ranges) if ranges.is_empty() => {
                            ui.label(
                                RichText::new("no payload positions")
                                    .small()
                                    .color(theme::TEXT_MUTED),
                            );
                        }
                        Ok(ranges) => {
                            ui.label(
                                RichText::new(format!(
                                    "{} payload position{}",
                                    ranges.len(),
                                    if ranges.len() == 1 { "" } else { "s" }
                                ))
                                .small()
                                .color(theme::ACCENT_DARK),
                            );
                        }
                        Err(_) => {
                            ui.label(
                                RichText::new("unclosed $$ marker")
                                    .small()
                                    .color(theme::RED),
                            );
                        }
                    }
                    if let Some(selection) = &state.selection {
                        ui.label(
                            RichText::new(format!(
                                "selection: {} B",
                                selection.end.saturating_sub(selection.start)
                            ))
                            .small()
                            .color(theme::TEXT_MUTED),
                        );
                    }
                });

                if let Some(error) = &state.error {
                    ui.add(
                        Label::new(RichText::new(error).small().color(theme::RED)).truncate(),
                    )
                    .on_hover_text(error);
                } else if let Some(notice) = &state.notice {
                    ui.add(
                        Label::new(RichText::new(notice).small().color(theme::AMBER)).truncate(),
                    )
                    .on_hover_text(notice);
                }
            });

        if add_marker {
            let target = self.intruder.target_range();
            let (request, range) = toggle_marker(&self.intruder.request, target);
            self.intruder.request = request;
            self.intruder.error = None;
            self.intruder.notice = None;
            self.intruder.select_request(ui.ctx(), range);
        }
        if clear_all {
            self.intruder.request = clear_markers(&self.intruder.request);
            self.intruder.error = None;
            self.intruder.notice = Some("Removed every position marker".to_string());
            self.intruder.selection = None;
            self.intruder.cursor = self.intruder.cursor.min(self.intruder.request.len());
        }
        if auto {
            let (request, count) = auto_mark(&self.intruder.request);
            self.intruder.request = request;
            self.intruder.error = None;
            self.intruder.notice = Some(if count == 0 {
                "No new values to mark".to_string()
            } else {
                format!(
                    "Added {count} payload position{}",
                    if count == 1 { "" } else { "s" }
                )
            });
            self.intruder.selection = None;
            self.intruder.cursor = self.intruder.cursor.min(self.intruder.request.len());
        }
    }

    fn intruder_editor(&mut self, ui: &mut egui::Ui) {
        egui::CentralPanel::default()
            .frame(
                egui::Frame::NONE
                    .fill(theme::SURFACE)
                    .inner_margin(egui::Margin::same(5)),
            )
            .show(ui, |ui| {
                let state = &mut self.intruder;
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Request").strong().color(theme::TEXT));
                    ui.label(
                        RichText::new("editable raw message")
                            .small()
                            .color(theme::TEXT_MUTED),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            RichText::new("Ctrl+Enter starts or stops the attack")
                                .small()
                                .color(theme::TEXT_MUTED),
                        );
                    });
                });
                if let Some(note) = &state.note {
                    ui.add_space(1.0);
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
                        let min_height = ui.available_height().max(160.0);
                        let font = FontId::monospace(crate::highlight::MESSAGE_FONT_SIZE);
                        let mut layouter =
                            |ui: &egui::Ui, buffer: &dyn egui::TextBuffer, wrap_width: f32| {
                                let job = request_layout_job(buffer.as_str(), wrap_width, &font);
                                ui.fonts_mut(|fonts| fonts.layout_job(job))
                            };
                        let output = TextEdit::multiline(&mut state.request)
                            .id(egui::Id::unique(INTRUDER_EDITOR_ID))
                            .code_editor()
                            .desired_width(f32::INFINITY)
                            .min_size(egui::vec2(0.0, min_height))
                            .hint_text("Paste or edit a raw HTTP request, then wrap values in $$")
                            .layouter(&mut layouter)
                            .show(ui);
                        if output.response.has_focus()
                            && let Some(range) = output.state.cursor.char_range()
                        {
                            let range = range.as_sorted_char_range();
                            let start = byte_offset_for_char(&state.request, range.start.0);
                            let end = byte_offset_for_char(&state.request, range.end.0);
                            state.cursor = start;
                            state.selection = (start != end).then_some(start..end);
                        }
                        if output.response.changed() {
                            state.error = None;
                        }
                    });
            });
    }

    fn intruder_payloads(&mut self, ui: &mut egui::Ui) {
        egui::Panel::right(INTRUDER_PAYLOADS_ID)
            .resizable(true)
            .default_size(320.0)
            .min_size(230.0)
            .frame(
                egui::Frame::NONE
                    .fill(theme::SURFACE)
                    .inner_margin(egui::Margin::same(6)),
            )
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("perseid.intruder.payloads.scroll")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let state = &mut self.intruder;
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new("Payloads").strong().color(theme::TEXT),
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.label(
                                        RichText::new(format!(
                                            "{} payload{}",
                                            state.payload_count(),
                                            if state.payload_count() == 1 { "" } else { "s" }
                                        ))
                                        .small()
                                        .color(theme::TEXT_MUTED),
                                    );
                                },
                            );
                        });
                        ui.add_space(2.0);
                        ui.separator();

                        if state.attack.uses_one_set() {
                            ui.label(
                                RichText::new("Payload position")
                                    .small()
                                    .color(theme::TEXT_MUTED),
                            );
                            ui.label(
                                RichText::new("All positions share one payload set")
                                    .color(theme::TEXT),
                            );
                        } else {
                            let positions =
                                marker_ranges(&state.request).map_or(0, |ranges| ranges.len());
                            ui.label(
                                RichText::new("Payload position")
                                    .small()
                                    .color(theme::TEXT_MUTED),
                            );
                            if positions == 0 {
                                ui.label(
                                    RichText::new(
                                        "Add at least one $$ position to the request first.",
                                    )
                                    .small()
                                    .color(theme::TEXT_MUTED),
                                );
                            } else {
                                let active = state.active_set.min(positions - 1);
                                egui::ComboBox::from_id_salt("perseid.intruder.payload.position")
                                    .width(180.0)
                                    .selected_text(format!(
                                        "Position {} of {positions}",
                                        active + 1
                                    ))
                                    .show_ui(ui, |ui| {
                                        for index in 0..positions {
                                            ui.selectable_value(
                                                &mut state.active_set,
                                                index,
                                                format!("Position {}", index + 1),
                                            );
                                        }
                                    });
                            }
                        }

                        ui.add_space(3.0);
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new("Request count")
                                    .small()
                                    .color(theme::TEXT_MUTED),
                            );
                            ui.label(
                                RichText::new(state.request_count().to_string())
                                    .strong()
                                    .color(theme::TEXT),
                            );
                            ui.separator();
                            ui.label(
                                RichText::new(state.attack.label())
                                    .small()
                                    .color(theme::TEXT_MUTED),
                            );
                        });
                        ui.add_space(3.0);
                        ui.separator();
                        ui.add_space(3.0);

                        ui.label(
                            RichText::new("Payload list")
                                .small()
                                .color(theme::TEXT_MUTED),
                        );
                        ui.label(
                            RichText::new("One payload per line; blank lines are ignored.")
                                .small()
                                .color(theme::TEXT_MUTED),
                        );
                        ui.add_space(2.0);
                        let set = state.active_set_mut();
                        ui.add(
                            TextEdit::multiline(&mut set.text)
                                .code_editor()
                                .desired_width(f32::INFINITY)
                                .desired_rows(8)
                                .hint_text("admin\nguest\ntest"),
                        );
                        ui.horizontal(|ui| {
                            if ui
                                .button("Clear list")
                                .on_hover_text("Remove every payload from this set")
                                .clicked()
                            {
                                set.text.clear();
                            }
                            if ui
                                .button("Deduplicate")
                                .on_hover_text("Remove duplicate payloads, keeping the first occurrence")
                                .clicked()
                            {
                                let mut seen = std::collections::BTreeSet::new();
                                let unique: Vec<String> = payloads(&set.text)
                                    .into_iter()
                                    .filter(|payload| seen.insert(payload.clone()))
                                    .collect();
                                set.text = unique.join("\n");
                            }
                        });

                        ui.add_space(6.0);
                        ui.separator();
                        ui.add_space(3.0);
                        ui.label(
                            RichText::new(state.attack.description())
                                .small()
                                .color(theme::TEXT_MUTED),
                        );
                        ui.add_space(4.0);
                        ui.label(
                            RichText::new(
                                "Intruder talks directly to the target, like Repeater. Proxy interception, matching rules, and capture do not apply.",
                            )
                            .small()
                            .color(theme::TEXT_MUTED),
                        );
                    });
            });
    }

    fn intruder_results(&mut self, ui: &mut egui::Ui) {
        if self.intruder.attack_id.is_none() && self.intruder.results.is_empty() {
            return;
        }
        self.intruder.ensure_detail();

        egui::Panel::bottom(INTRUDER_RESULTS_ID)
            .resizable(true)
            .default_size(280.0)
            .min_size(170.0)
            .frame(
                egui::Frame::NONE
                    .fill(theme::SURFACE)
                    .inner_margin(egui::Margin::same(4)),
            )
            .show(ui, |ui| {
                egui::Panel::top(INTRUDER_RESULTS_HEADER_ID)
                    .frame(
                        egui::Frame::NONE
                            .fill(theme::SURFACE_ALT)
                            .inner_margin(egui::Margin::symmetric(8, 3)),
                    )
                    .show(ui, |ui| {
                        let state = &mut self.intruder;
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("Results").strong().color(theme::TEXT));
                            if state.running {
                                ui.spinner();
                            }
                            ui.label(
                                RichText::new(format!("{} / {}", state.received, state.total))
                                    .small()
                                    .color(theme::TEXT_MUTED),
                            );
                            if let Some(note) = &state.finished_note {
                                ui.label(RichText::new(note).small().color(theme::TEXT_MUTED));
                            }
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui
                                        .add_enabled(
                                            !state.running && !state.results.is_empty(),
                                            egui::Button::new("Clear results"),
                                        )
                                        .on_hover_text("Discard the result table")
                                        .on_disabled_hover_text(if state.running {
                                            "Wait for the attack to finish"
                                        } else {
                                            "No results to clear"
                                        })
                                        .clicked()
                                    {
                                        state.clear_results();
                                    }
                                    if state.running
                                        && let Some(started) = state.started_at
                                    {
                                        ui.label(
                                            RichText::new(view::format_duration(started.elapsed()))
                                                .small()
                                                .color(theme::TEXT_MUTED),
                                        );
                                    }
                                },
                            );
                        });
                    });

                egui::Panel::right(INTRUDER_DETAIL_ID)
                    .resizable(true)
                    .default_size((ui.available_width() * 0.42).max(280.0))
                    .min_size(220.0)
                    .frame(
                        egui::Frame::NONE
                            .fill(theme::SURFACE)
                            .inner_margin(egui::Margin::same(4)),
                    )
                    .show(ui, |ui| self.intruder_detail(ui));

                egui::CentralPanel::default()
                    .frame(egui::Frame::NONE.fill(theme::SURFACE))
                    .show(ui, |ui| self.intruder_result_table(ui));
            });
    }

    fn intruder_result_table(&mut self, ui: &mut egui::Ui) {
        let state = &mut self.intruder;
        let rows = state.results.len();
        if rows == 0 {
            empty_state(
                ui,
                if state.running {
                    "Waiting for the first result"
                } else {
                    "No results"
                },
                if state.running {
                    "Requests run concurrently; rows appear as soon as each response arrives."
                } else {
                    "Start an attack to fill the result table."
                },
            );
            return;
        }

        ui.style_mut().interaction.selectable_labels = false;
        let mut clicked = None;
        table_frame(ui, |ui| {
            let table = TableBuilder::new(ui)
                .striped(true)
                .resizable(true)
                .sense(egui::Sense::click())
                .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                .column(Column::exact(46.0))
                .column(Column::initial(220.0).at_least(90.0))
                .column(Column::exact(64.0))
                .column(Column::exact(78.0))
                .column(Column::remainder().at_least(64.0));

            table
                .header(20.0, |mut header| {
                    for title in ["#", "Payloads", "Status", "Length", "Time"] {
                        header.col(|ui| {
                            ui.label(RichText::new(title).strong().small().color(theme::TEXT));
                        });
                    }
                })
                .body(|body| {
                    body.rows(20.0, rows, |mut row| {
                        let result = &state.results[row.index()];
                        row.set_selected(state.selected == Some(result.sequence));
                        row.col(|ui| {
                            ui.label(
                                RichText::new(result.sequence.to_string())
                                    .small()
                                    .color(theme::TEXT),
                            );
                        });
                        let payloads = result.payloads.join(" · ");
                        row.col(|ui| {
                            ui.add(
                                Label::new(
                                    RichText::new(if payloads.is_empty() {
                                        "—".to_string()
                                    } else {
                                        payloads.clone()
                                    })
                                    .small()
                                    .color(theme::TEXT),
                                )
                                .truncate(),
                            )
                            .on_hover_text(&payloads);
                        });
                        row.col(|ui| match &result.result {
                            RepeaterResult::Response { status, .. } => {
                                ui.label(
                                    RichText::new(status.to_string())
                                        .small()
                                        .color(view::status_class_color(*status)),
                                );
                            }
                            RepeaterResult::Failed { .. } => {
                                ui.label(RichText::new("error").small().color(theme::RED));
                            }
                        });
                        row.col(|ui| {
                            let length = match &result.result {
                                RepeaterResult::Response { body, .. } => {
                                    view::human_bytes(body.total_len())
                                }
                                RepeaterResult::Failed { .. } => "—".to_string(),
                            };
                            ui.label(RichText::new(length).small().color(theme::TEXT));
                        });
                        row.col(|ui| {
                            ui.label(
                                RichText::new(view::format_duration(result.duration))
                                    .small()
                                    .color(theme::TEXT_MUTED),
                            );
                        });

                        let hover = match &result.result {
                            RepeaterResult::Response {
                                status,
                                reason,
                                body,
                                ..
                            } => format!(
                                "Payloads: {}\nStatus: {status} {reason}\nLength: {}",
                                if payloads.is_empty() {
                                    "—"
                                } else {
                                    payloads.as_str()
                                },
                                view::human_bytes(body.total_len())
                            ),
                            RepeaterResult::Failed { error } => {
                                format!("Payloads: {payloads}\nError: {error}")
                            }
                        };
                        let response = row.response().on_hover_text(hover);
                        if response.clicked() {
                            clicked = Some(result.sequence);
                        }
                    });
                });
        });

        if let Some(sequence) = clicked {
            state.selected = Some(sequence);
            state.detail = None;
            state.detail_pane = MessagePane::Response;
        }
    }

    fn intruder_detail(&mut self, ui: &mut egui::Ui) {
        let state = &mut self.intruder;
        let Some(sequence) = state.selected else {
            empty_state(
                ui,
                "Select a result",
                "Click a row to inspect its request and response.",
            );
            return;
        };
        let Some(result) = state.results.iter().find(|row| row.sequence == sequence) else {
            empty_state(
                ui,
                "Result no longer retained",
                "The selected row was removed by the result-history limit or cleared.",
            );
            return;
        };

        let mut pane = state.detail_pane;
        let mut view_mode = state.detail_view;
        ui.horizontal(|ui| {
            ui.label(RichText::new("Selected result").strong().color(theme::TEXT));
            ui.label(
                RichText::new(format!("#{}", result.sequence))
                    .small()
                    .color(theme::TEXT_MUTED),
            );
            match &result.result {
                RepeaterResult::Response { status, reason, .. } => {
                    ui.colored_label(
                        view::status_class_color(*status),
                        RichText::new(if reason.is_empty() {
                            status.to_string()
                        } else {
                            format!("{status} {reason}")
                        })
                        .strong(),
                    );
                }
                RepeaterResult::Failed { .. } => {
                    ui.colored_label(theme::RED, RichText::new("request failed").strong());
                }
            }
            ui.label(
                RichText::new(view::format_duration(result.duration))
                    .small()
                    .color(theme::TEXT_MUTED),
            );
        });
        if !result.payloads.is_empty() {
            ui.add(
                Label::new(
                    RichText::new(format!("Payloads: {}", result.payloads.join(" · ")))
                        .small()
                        .color(theme::TEXT_MUTED),
                )
                .truncate(),
            )
            .on_hover_text(result.payloads.join("\n"));
        }

        ui.horizontal(|ui| {
            if message_mode_tab(ui, pane == MessagePane::Request, "Request").clicked() {
                pane = MessagePane::Request;
            }
            if message_mode_tab(ui, pane == MessagePane::Response, "Response").clicked() {
                pane = MessagePane::Response;
            }
            ui.separator();
            if message_mode_tab(ui, view_mode == MessageView::Pretty, "Pretty").clicked() {
                view_mode = MessageView::Pretty;
            }
            if message_mode_tab(ui, view_mode == MessageView::Raw, "Raw").clicked() {
                view_mode = MessageView::Raw;
            }
            if message_mode_tab(ui, view_mode == MessageView::Hex, "Hex").clicked() {
                view_mode = MessageView::Hex;
            }
        });
        state.detail_pane = pane;
        state.detail_view = view_mode;

        if let Some(detail) = state.detail.as_ref()
            && pane == MessagePane::Response
        {
            let notes = detail.notes();
            if !notes.is_empty() {
                ui.label(RichText::new(notes.join(" · ")).small().color(theme::AMBER));
            }
        }

        ui.add_space(2.0);
        ui.separator();
        ui.add_space(2.0);

        let Some(detail) = state.detail.as_ref() else {
            empty_state(
                ui,
                "No message details",
                "The selected result has no retained data.",
            );
            return;
        };
        let views = match pane {
            MessagePane::Request => detail.request.as_ref(),
            MessagePane::Response => detail.response.as_ref(),
        };
        match views {
            Some(views) => {
                let text = message_view_text(views, view_mode);
                message_text(
                    ui,
                    RESULT_MESSAGE_ID_BASE + result.sequence,
                    pane,
                    view_mode,
                    text,
                );
            }
            None if pane == MessagePane::Request => {
                empty_state(
                    ui,
                    "Request not available",
                    "The template used for this attack is no longer in memory.",
                );
            }
            None => {
                let error = detail
                    .response_error
                    .as_deref()
                    .unwrap_or("The request failed before a response was received.");
                empty_state(ui, "Request failed", error);
            }
        }
    }
}

impl IntruderState {
    pub(super) fn select_request(&mut self, ctx: &egui::Context, range: Range<usize>) {
        let len = self.request.len();
        let start = range.start.min(len);
        let end = range.end.max(start).min(len);
        self.selection = (start != end).then_some(start..end);
        self.cursor = start;

        let start_char = self.request[..start].chars().count();
        let end_char = self.request[..end].chars().count();
        let editor_id = egui::Id::unique(INTRUDER_EDITOR_ID);
        let mut state = TextEditState::load(ctx, editor_id).unwrap_or_default();
        state.cursor.set_char_range(Some(CCursorRange::two(
            CCursor::new(start_char),
            CCursor::new(end_char),
        )));
        state.store(ctx, editor_id);
        ctx.memory_mut(|memory| memory.request_focus(editor_id));
    }
}

/// Build the request editor layout while keeping `$$` visible and marking the
/// enclosed position. The text is appended verbatim so selection, copying, and
/// editing still operate on the raw request.
fn request_layout_job(text: &str, wrap_width: f32, font: &FontId) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    job.wrap.max_width = wrap_width;
    job.wrap.break_anywhere = true;
    job.keep_trailing_whitespace = true;

    let marker = crate::proxy::MARKER;
    let mut cursor = 0;
    while let Some(relative_open) = text[cursor..].find(marker) {
        let open = cursor + relative_open;
        append_text(
            &mut job,
            &text[cursor..open],
            font,
            theme::TEXT,
            Color32::TRANSPARENT,
        );
        let content_start = open + marker.len();
        let Some(relative_close) = text[content_start..].find(marker) else {
            append_text(
                &mut job,
                &text[open..],
                font,
                theme::RED,
                Color32::TRANSPARENT,
            );
            return job;
        };
        let close = content_start + relative_close;
        append_text(
            &mut job,
            marker,
            font,
            theme::ACCENT_DARK,
            theme::ACCENT_SOFT,
        );
        append_text(
            &mut job,
            &text[content_start..close],
            font,
            theme::TEXT,
            theme::SELECTED,
        );
        append_text(
            &mut job,
            marker,
            font,
            theme::ACCENT_DARK,
            theme::ACCENT_SOFT,
        );
        cursor = close + marker.len();
    }
    append_text(
        &mut job,
        &text[cursor..],
        font,
        theme::TEXT,
        Color32::TRANSPARENT,
    );
    job
}

fn append_text(
    job: &mut egui::text::LayoutJob,
    text: &str,
    font: &FontId,
    color: Color32,
    background: Color32,
) {
    if text.is_empty() {
        return;
    }
    job.append(
        text,
        0.0,
        egui::TextFormat {
            font_id: font.clone(),
            color,
            background,
            ..Default::default()
        },
    );
}

fn byte_offset_for_char(text: &str, char_index: usize) -> usize {
    text.char_indices()
        .nth(char_index)
        .map_or(text.len(), |(offset, _)| offset)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_layout_preserves_every_byte_and_marks_positions() {
        let font = FontId::monospace(crate::highlight::MESSAGE_FONT_SIZE);
        let cases = [
            "GET /?id=$$1$$ HTTP/1.1\nHost: example.test\n\n",
            "POST / HTTP/1.1\r\n\r\nuser=$$admin$$",
            "GET /?q=$$unclosed HTTP/1.1\n\n",
            "GET / HTTP/1.1\n\n",
        ];
        for text in cases {
            let job = request_layout_job(text, 200.0, &font);
            assert_eq!(job.text, text);
        }
        let job = request_layout_job(cases[0], 200.0, &font);
        assert!(
            job.sections
                .iter()
                .any(|section| section.format.background == theme::SELECTED),
            "the payload position should receive a background highlight"
        );
    }

    #[test]
    fn byte_offsets_are_utf8_character_boundaries() {
        let text = "Привет $$мир$$";
        let start = byte_offset_for_char(text, 9);
        let end = byte_offset_for_char(text, 12);
        assert_eq!(&text[start..end], "мир");
        assert_eq!(byte_offset_for_char(text, 10_000), text.len());
    }
}
