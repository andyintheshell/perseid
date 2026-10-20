//! Previews for tools whose engines have not landed yet: Intruder and
//! Repeater.

use egui::Label;

use super::widgets::{badge, card, empty_state, page_pad, preview_pane};
use super::*;

impl PerseidApp {
    pub(super) fn intruder_view(&mut self, ui: &mut egui::Ui) {
        page_pad(ui, |ui| {
            let preview = self
                .selected
                .and_then(|id| self.history.get(id))
                .filter(|item| !item.is_tunnel())
                .map(view::format_request);

            card(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("Intruder")
                            .size(17.0)
                            .strong()
                            .color(theme::TEXT),
                    );
                    badge(ui, "MILESTONE 4");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.add_enabled(false, egui::Button::new("Start attack"))
                            .on_disabled_hover_text("Intruder execution is part of Milestone 4");
                    });
                });
                ui.label(
                    "Payload-based request fuzzing with positions, payload sets, and live results.",
                );
                ui.add_space(6.0);
                ui.separator();
                ui.add_space(6.0);

                ui.columns(2, |columns| {
                    columns[0].label(
                        RichText::new("Planned workspace")
                            .strong()
                            .color(theme::TEXT),
                    );
                    columns[0].add_space(3.0);
                    for line in [
                        "Sniper, battering ram, pitchfork, and cluster bomb attacks.",
                        "Simple lists, numbers, dates, brute-force, and runtime file payloads.",
                        "Position marking in raw requests and a live result table.",
                        "Scope-aware, user-controlled concurrency.",
                    ] {
                        columns[0].label(format!("- {line}"));
                    }

                    columns[1].label(
                        RichText::new("Request template")
                            .strong()
                            .color(theme::TEXT),
                    );
                    columns[1].add_space(3.0);
                    match &preview {
                        Some(text) => {
                            egui::ScrollArea::both()
                                .id_salt("perseid.intruder.preview")
                                .max_height(240.0)
                                .auto_shrink([false, false])
                                .show(&mut columns[1], |ui| {
                                    ui.add(
                                        Label::new(
                                            RichText::new(text).monospace().color(theme::TEXT),
                                        )
                                        .selectable(true)
                                        .wrap_mode(egui::TextWrapMode::Extend),
                                    );
                                });
                        }
                        None => {
                            columns[1].label(
                            RichText::new(
                                "Select an HTTP exchange in Proxy history to use it as a template.",
                            )
                            .small()
                            .color(theme::TEXT_MUTED),
                        );
                        }
                    }
                });
            });
            ui.add_space(8.0);
            ui.label(
                RichText::new(
                    "No artificial rate limit: Perseid will run attacks at the concurrency and pace you configure.",
                )
                .small()
                .color(theme::ACCENT_DARK),
            );
        });
    }

    pub(super) fn repeater_view(&mut self, ui: &mut egui::Ui) {
        page_pad(ui, |ui| {
            card(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("Repeater")
                            .size(17.0)
                            .strong()
                            .color(theme::TEXT),
                    );
                    badge(ui, "PREVIEW");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.add_enabled(false, egui::Button::new("Send"))
                            .on_disabled_hover_text("Sending arrives in Milestone 3");
                    });
                });
                ui.label(
                    "A read-only preview of the selected exchange. Editing and resending requests \
                 arrive in Milestone 3.",
                );
            });
            ui.add_space(8.0);

            let Some(id) = self.selected else {
                empty_state(
                    ui,
                    "No request selected",
                    "Select an exchange in Proxy history to preview it here.",
                );
                return;
            };
            self.ensure_detail(id);
            let Some(cache) = &self.detail else {
                empty_state(
                    ui,
                    "Exchange no longer in history",
                    "The selected exchange was evicted or cleared.",
                );
                return;
            };

            egui::Panel::left(REPEATER_REQUEST_ID)
                .resizable(true)
                .default_size((ui.available_width() / 2.0).max(240.0))
                .min_size(180.0)
                .frame(
                    egui::Frame::NONE
                        .fill(theme::SURFACE)
                        .inner_margin(egui::Margin::same(10)),
                )
                .show(ui, |ui| {
                    preview_pane(ui, "Request", "as captured", &cache.request.raw, "request");
                });

            egui::CentralPanel::default()
                .frame(
                    egui::Frame::NONE
                        .fill(theme::SURFACE)
                        .inner_margin(egui::Margin::same(10)),
                )
                .show(ui, |ui| {
                    preview_pane(
                        ui,
                        "Response",
                        "as captured",
                        &cache.response.raw,
                        "response",
                    );
                });
        });
    }
}
