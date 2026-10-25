//! The attack toolbar: attack type, target, concurrency, and start/stop.

use egui::{Color32, RichText, Stroke, TextEdit};

use crate::proxy::{AttackType, MAX_INTRUDER_CONCURRENCY, update_host_header};
use crate::theme;

use super::super::super::PerseidApp;
use super::INTRUDER_TOOLBAR_ID;

impl PerseidApp {
    pub(super) fn intruder_toolbar(&mut self, ui: &mut egui::Ui) {
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
}
