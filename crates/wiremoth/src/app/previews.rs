//! Previews for tools whose engines have not landed yet: Intercept, Intruder,
//! and Repeater.

use egui::Label;

use super::widgets::{badge, card, empty_state, preview_pane};
use super::*;

impl WiremothApp {
    pub(super) fn intercept_view(&mut self, ui: &mut egui::Ui) {
        let mut open_history = false;
        let mut clear = false;
        let listener = self
            .proxy
            .as_ref()
            .map(Proxy::local_addr)
            .unwrap_or(self.listen);

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_max_width(820.0);
                card(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new("Intercept")
                                .size(17.0)
                                .strong()
                                .color(theme::TEXT),
                        );
                        badge(ui, "MILESTONE 1");
                    });
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new("Requests are forwarded immediately.")
                            .strong()
                            .color(theme::TEXT),
                    );
                    ui.label(
                        "Editable interception - hold, inspect, modify, forward, or drop - is \
                         part of Milestone 1. Wiremoth currently records every exchange in \
                         HTTP history without pausing traffic.",
                    );
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        self.capture_toggle(ui);
                        if ui.button("Open HTTP history").clicked() {
                            open_history = true;
                        }
                        if ui.button("Clear history").clicked() {
                            clear = true;
                        }
                    });
                });

                ui.add_space(10.0);
                card(ui, |ui| {
                    ui.label(
                        RichText::new("Proxy listener")
                            .size(15.0)
                            .strong()
                            .color(theme::TEXT),
                    );
                    ui.add_space(2.0);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(listener.to_string()).monospace());
                        if self.proxy.is_some() {
                            ui.colored_label(theme::GREEN, "listening");
                        } else {
                            ui.colored_label(theme::RED, "not running");
                            if ui.button("Retry").clicked() {
                                let notify = self.notify.clone();
                                self.start_proxy(notify);
                            }
                        }
                    });
                    if let Some(error) = &self.proxy_error {
                        ui.label(RichText::new(error).small().color(theme::RED));
                    }
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new(
                            "HTTPS uses CONNECT passthrough. Wiremoth tunnels TLS but does not \
                             decrypt it yet; history shows the tunnel, not the requests inside.",
                        )
                        .small()
                        .color(theme::TEXT_MUTED),
                    );
                });

                ui.add_space(10.0);
                card(ui, |ui| {
                    ui.label(
                        RichText::new("Coming with interception")
                            .size(15.0)
                            .strong()
                            .color(theme::TEXT),
                    );
                    for line in [
                        "Pause matching requests before they reach the origin.",
                        "Edit headers and bodies in a raw request editor.",
                        "Forward, drop, or send the request to another tool.",
                        "Intercept only in-scope hosts.",
                    ] {
                        ui.label(format!("- {line}"));
                    }
                });
            });

        if open_history {
            self.proxy_section = ProxySection::HttpHistory;
        }
        if clear {
            self.clear_history();
        }
    }

    pub(super) fn intruder_view(&mut self, ui: &mut egui::Ui) {
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
                            .id_salt("wiremoth.intruder.preview")
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
                                "Select a plain-HTTP exchange in Proxy history to use it as a template.",
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
                "No artificial rate limit: Wiremoth will run attacks at the concurrency and pace you configure.",
            )
            .small()
            .color(theme::ACCENT_DARK),
        );
    }

    pub(super) fn repeater_view(&mut self, ui: &mut egui::Ui) {
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
    }
}
