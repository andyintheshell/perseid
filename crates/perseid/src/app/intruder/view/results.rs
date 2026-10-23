//! The result table and the selected-result detail pane.

use egui::{Label, RichText};
use egui_extras::{Column, TableBuilder};

use crate::proxy::{IntruderResult, RepeaterResult};
use crate::theme;
use crate::view;

use super::super::super::message_view::{message_text, message_view_text};
use super::super::super::widgets::{empty_state, message_mode_tab, table_frame};
use super::super::super::{MessagePane, MessageView, PerseidApp};
use super::{
    INTRUDER_DETAIL_ID, INTRUDER_RESULTS_HEADER_ID, INTRUDER_RESULTS_ID, RESULT_MESSAGE_ID_BASE,
};

impl PerseidApp {
    pub(super) fn intruder_results(&mut self, ui: &mut egui::Ui) {
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

                        let response = row.response().on_hover_text(result_hover(result));
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

/// Hover summary for one result row.
fn result_hover(result: &IntruderResult) -> String {
    let payloads = result.payloads.join(" · ");
    match &result.result {
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
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use crate::proxy::CapturedBody;

    use super::*;

    fn result(payloads: &[&str], result: RepeaterResult) -> IntruderResult {
        IntruderResult {
            attack_id: 1,
            sequence: 2,
            payloads: payloads.iter().map(|payload| payload.to_string()).collect(),
            duration: Duration::from_millis(5),
            result,
        }
    }

    #[test]
    fn hover_text_reports_payloads_status_and_length() {
        let response = result(
            &["admin", "1"],
            RepeaterResult::Response {
                version: "HTTP/1.1".to_string(),
                status: 201,
                reason: "Created".to_string(),
                headers: Vec::new(),
                body: CapturedBody::from_bytes(b"hello".to_vec()),
                body_error: None,
            },
        );
        let hover = result_hover(&response);
        assert!(hover.contains("Payloads: admin · 1"));
        assert!(hover.contains("Status: 201 Created"));
        assert!(hover.contains("Length: 5 B"));
    }

    #[test]
    fn hover_text_reports_failed_requests_and_empty_payloads() {
        let failed = result(
            &[],
            RepeaterResult::Failed {
                error: "timeout".to_string(),
            },
        );
        let hover = result_hover(&failed);
        assert!(hover.contains("Payloads: \n"));
        assert!(hover.contains("Error: timeout"));
    }
}
