//! The payload-set pane: position selection, payload editing, and dedup.

use std::collections::BTreeSet;

use egui::{RichText, TextEdit};

use crate::proxy::marker_ranges;
use crate::theme;

use super::super::super::PerseidApp;
use super::super::state::payloads;
use super::INTRUDER_PAYLOADS_ID;

impl PerseidApp {
    pub(super) fn intruder_payloads(&mut self, ui: &mut egui::Ui) {
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
                                set.text = deduplicated_payloads(&set.text);
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
}

/// Remove duplicate payloads while keeping the first occurrence and the
/// existing line-based formatting.
fn deduplicated_payloads(text: &str) -> String {
    let mut seen = BTreeSet::new();
    payloads(text)
        .into_iter()
        .filter(|payload| seen.insert(payload.clone()))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deduplicate_keeps_first_occurrence_and_ignores_blank_lines() {
        assert_eq!(
            deduplicated_payloads("admin\n\nguest\nadmin\r\ntest\nguest\n"),
            "admin\nguest\ntest"
        );
        assert_eq!(deduplicated_payloads("\n\n"), "");
        assert_eq!(deduplicated_payloads("one\r\ntwo\r\n"), "one\ntwo");
    }
}
