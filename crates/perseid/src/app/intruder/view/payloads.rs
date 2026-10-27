//! The payload-set pane: position selection, payload editing, and dedup.

use std::collections::BTreeSet;

use egui::{RichText, TextEdit};

use crate::proxy::marker_ranges;
use crate::theme;

use super::super::super::PerseidApp;
use super::super::super::file_dialog::{self, FileDialog, FileDialogAction};
use super::super::state::PayloadFileDialog;
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
                        ui.horizontal_wrapped(|ui| {
                            if ui.button("Load from file").on_hover_text("Append payloads from a UTF-8 text file").clicked() {
                                let directory = std::env::current_dir().unwrap_or_default();
                                state.payload_file_dialog = Some(PayloadFileDialog { browser: FileDialog::open_any_file(directory), error: None });
                            }
                        });
                        let set = state.active_set_mut();
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

    pub(super) fn intruder_payload_file_window(&mut self, ctx: &egui::Context) {
        let Some(mut dialog) = self.intruder.payload_file_dialog.take() else {
            return;
        };
        let mut action = FileDialogAction::None;
        egui::Modal::new(egui::Id::unique("perseid.intruder.payload_file"))
            .backdrop_color(egui::Color32::from_black_alpha(42))
            .frame(file_dialog::modal_frame())
            .show(ctx, |ui| {
                ui.set_width(file_dialog::MODAL_WIDTH);
                if file_dialog::title_bar(ui, "Load payloads from file", None) {
                    action = FileDialogAction::Cancel;
                } else {
                    egui::Frame::NONE
                        .inner_margin(egui::Margin::symmetric(16, 12))
                        .show(ui, |ui| {
                            ui.set_width(file_dialog::MODAL_WIDTH - 32.0);
                            action = dialog.browser.ui(ui, "Load", &mut dialog.error);
                        });
                }
            });
        if ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
            || matches!(action, FileDialogAction::Cancel)
        {
            return;
        }
        if matches!(action, FileDialogAction::Submit) {
            let result = dialog
                .browser
                .chosen_path()
                .and_then(|path| self.intruder.append_payload_file(&path));
            match result {
                Ok(count) => {
                    self.intruder.notice = Some(format!("Loaded {count} payload(s) from file."));
                    return;
                }
                Err(error) => dialog.error = Some(error),
            }
        }
        self.intruder.payload_file_dialog = Some(dialog);
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

    fn frame(
        ctx: &egui::Context,
        app: &mut PerseidApp,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1400.0, 900.0),
                )),
                events,
                ..Default::default()
            },
            |ui| app.intruder_view(ui),
        )
    }

    fn text_pos(shape: &egui::Shape, label: &str) -> Option<egui::Pos2> {
        match shape {
            egui::Shape::Text(text) if text.galley.text() == label => {
                Some(text.pos + text.galley.size() * 0.5)
            }
            egui::Shape::Vec(shapes) => shapes.iter().find_map(|shape| text_pos(shape, label)),
            _ => None,
        }
    }

    fn click(ctx: &egui::Context, app: &mut PerseidApp, label: &str) {
        frame(ctx, app, vec![]).drop_without_applying_deltas();
        let output = frame(ctx, app, vec![]);
        let pos = output
            .shapes
            .iter()
            .find_map(|shape| text_pos(&shape.shape, label))
            .unwrap_or_else(|| panic!("missing {label}"));
        output.drop_without_applying_deltas();
        for event in [
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            },
        ] {
            frame(ctx, app, vec![event]).drop_without_applying_deltas();
        }
    }

    #[test]
    fn payload_file_dialog_loads_and_appends_selected_file() {
        let root =
            std::env::temp_dir().join(format!("perseid-payload-ui-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("words.txt"), b"new\nsecond\n").unwrap();
        let ctx = egui::Context::default();
        ctx.all_styles_mut(|style| style.animation_time = 0.0);
        let mut app = PerseidApp::test_app();
        app.intruder.payload_sets[0].text = "old".into();
        click(&ctx, &mut app, "Load from file");
        let dialog = app
            .intruder
            .payload_file_dialog
            .as_mut()
            .expect("dialog opened");
        dialog.browser = FileDialog::open_any_file(root.clone());
        click(&ctx, &mut app, "words.txt");
        click(&ctx, &mut app, "Load");
        assert!(app.intruder.payload_file_dialog.is_none());
        assert_eq!(
            payloads(&app.intruder.payload_sets[0].text),
            ["old", "new", "second"]
        );
        assert_eq!(
            app.intruder.notice.as_deref(),
            Some("Loaded 2 payload(s) from file.")
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn payload_file_dialog_keeps_errors_and_cancel_preserves_payloads() {
        let ctx = egui::Context::default();
        ctx.all_styles_mut(|style| style.animation_time = 0.0);
        let mut app = PerseidApp::test_app();
        app.intruder.payload_sets[0].text = "old".into();
        click(&ctx, &mut app, "Load from file");
        click(&ctx, &mut app, "Load");
        assert_eq!(
            app.intruder
                .payload_file_dialog
                .as_ref()
                .unwrap()
                .error
                .as_deref(),
            Some("Enter a file name.")
        );
        assert_eq!(app.intruder.payload_sets[0].text, "old");
        app.intruder
            .payload_file_dialog
            .as_mut()
            .unwrap()
            .browser
            .set_path(&std::env::temp_dir().join(format!("missing-{}", uuid::Uuid::new_v4())));
        click(&ctx, &mut app, "Load");
        assert!(
            app.intruder
                .payload_file_dialog
                .as_ref()
                .unwrap()
                .error
                .as_deref()
                .unwrap()
                .contains("Could not inspect")
        );
        assert_eq!(app.intruder.payload_sets[0].text, "old");
        click(&ctx, &mut app, "Cancel");
        assert!(app.intruder.payload_file_dialog.is_none());
        assert_eq!(app.intruder.payload_sets[0].text, "old");
        click(&ctx, &mut app, "Load from file");
        let escape = egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        frame(&ctx, &mut app, vec![escape]).drop_without_applying_deltas();
        assert!(app.intruder.payload_file_dialog.is_none());
    }

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
