//! Proxy tab: match/replace rules applied after interception.

use crate::proxy::{MatchReplaceRule, RuleDirection, RulePart};
use egui::TextEdit;

use super::widgets::{card, empty_state, page_pad};
use super::*;

fn direction_label(direction: RuleDirection) -> &'static str {
    match direction {
        RuleDirection::Request => "Request",
        RuleDirection::Response => "Response",
    }
}

fn part_label(part: RulePart) -> &'static str {
    match part {
        RulePart::Head => "Head",
        RulePart::Body => "Body",
        RulePart::All => "Head + body",
    }
}

impl PerseidApp {
    pub(super) fn match_replace_view(&mut self, ui: &mut egui::Ui) {
        let mut dirty = false;
        let mut add = None;
        let mut remove = None;

        page_pad(ui, |ui| {
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.set_max_width(980.0);
                card(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new("Match/replace")
                                .size(17.0)
                                .strong()
                                .color(theme::TEXT),
                        );
                        ui.label(
                            RichText::new(format!("{} rules", self.match_replace.len()))
                                .small()
                                .color(theme::TEXT_MUTED),
                        );
                        ui.with_layout(
                            egui::Layout::right_to_left(egui::Align::Center),
                            |ui| {
                                if ui.button("Add response rule").clicked() {
                                    add = Some(RuleDirection::Response);
                                }
                                if ui.button("Add request rule").clicked() {
                                    add = Some(RuleDirection::Request);
                                }
                            },
                        );
                    });
                    ui.add_space(3.0);
                    ui.label(
                        "Rules run in order after interception and before the message is sent. \
                         Body rules apply only to buffered UTF-8 bodies; binary and streaming \
                         bodies are left untouched.",
                    );
                    if let Some(error) = &self.rules_error {
                        ui.add_space(4.0);
                        ui.label(RichText::new(error).small().color(theme::RED));
                    }
                });

                ui.add_space(8.0);
                if self.match_replace.is_empty() {
                    empty_state(
                        ui,
                        "No rewrite rules",
                        "Add a request or response rule to replace tokens in heads or buffered bodies.",
                    );
                    return;
                }

                for (index, rule) in self.match_replace.iter_mut().enumerate() {
                    ui.push_id(("perseid.match-replace", index), |ui| {
                        card(ui, |ui| {
                            ui.horizontal(|ui| {
                                if ui
                                    .checkbox(&mut rule.enabled, "Enabled")
                                    .changed()
                                {
                                    dirty = true;
                                }
                                ui.separator();

                                let mut direction = rule.direction;
                                egui::ComboBox::from_id_salt((
                                    "perseid.rule.direction",
                                    index,
                                ))
                                .selected_text(direction_label(direction))
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(
                                        &mut direction,
                                        RuleDirection::Request,
                                        "Request",
                                    );
                                    ui.selectable_value(
                                        &mut direction,
                                        RuleDirection::Response,
                                        "Response",
                                    );
                                });
                                if direction != rule.direction {
                                    rule.direction = direction;
                                    dirty = true;
                                }

                                let mut part = rule.part;
                                egui::ComboBox::from_id_salt(("perseid.rule.part", index))
                                    .selected_text(part_label(part))
                                    .show_ui(ui, |ui| {
                                        ui.selectable_value(
                                            &mut part,
                                            RulePart::Head,
                                            "Head",
                                        );
                                        ui.selectable_value(
                                            &mut part,
                                            RulePart::Body,
                                            "Body",
                                        );
                                        ui.selectable_value(
                                            &mut part,
                                            RulePart::All,
                                            "Head + body",
                                        );
                                    });
                                if part != rule.part {
                                    rule.part = part;
                                    dirty = true;
                                }

                                if ui
                                    .checkbox(&mut rule.regex, "Regex")
                                    .on_hover_text(
                                        "Use a Rust regex; $1-style captures work in the replacement",
                                    )
                                    .changed()
                                {
                                    dirty = true;
                                }

                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        if ui.small_button("Remove").clicked() {
                                            remove = Some(index);
                                        }
                                    },
                                );
                            });

                            ui.add_space(4.0);
                            ui.horizontal(|ui| {
                                ui.label(RichText::new("Match").color(theme::TEXT_MUTED));
                                if ui
                                    .add(
                                        TextEdit::singleline(&mut rule.pattern)
                                            .hint_text(if rule.regex {
                                                "regular expression"
                                            } else {
                                                "literal text"
                                            })
                                            .desired_width(280.0),
                                    )
                                    .changed()
                                {
                                    dirty = true;
                                }
                                ui.label(
                                    RichText::new("Replace with").color(theme::TEXT_MUTED),
                                );
                                if ui
                                    .add(
                                        TextEdit::singleline(&mut rule.replacement)
                                            .hint_text("replacement")
                                            .desired_width(280.0),
                                    )
                                    .changed()
                                {
                                    dirty = true;
                                }
                            });
                        });
                    });
                    ui.add_space(6.0);
                }

                ui.add_space(4.0);
                ui.label(
                    RichText::new(
                        "Empty patterns are ignored. Disabled rules stay in the list but do not \
                         affect traffic.",
                    )
                    .small()
                    .color(theme::TEXT_MUTED),
                );
            });
        });

        if let Some(direction) = add {
            self.match_replace.push(MatchReplaceRule {
                enabled: true,
                direction,
                part: RulePart::Head,
                pattern: String::new(),
                replacement: String::new(),
                regex: false,
            });
            dirty = true;
        }
        if let Some(index) = remove {
            if index < self.match_replace.len() {
                self.match_replace.remove(index);
            }
            dirty = true;
        }
        if dirty {
            self.apply_proxy_settings();
        }
    }
}
