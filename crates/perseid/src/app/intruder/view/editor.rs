//! The raw request editor and its selection handling.

use std::ops::Range;

use egui::text::{CCursor, CCursorRange};
use egui::text_edit::TextEditState;
use egui::{Color32, FontId, RichText, Stroke, TextEdit};

use crate::theme;

use super::super::super::{MessagePane, PerseidApp, message_view};
use super::super::state::IntruderState;
use super::INTRUDER_EDITOR_ID;
use super::layout::{byte_offset_for_char, request_layout_job};

impl PerseidApp {
    pub(super) fn intruder_editor(&mut self, ui: &mut egui::Ui) {
        egui::CentralPanel::default()
            .frame(
                egui::Frame::NONE
                    .fill(theme::SURFACE)
                    .inner_margin(egui::Margin::same(5)),
            )
            .show(ui, |ui| {
                let state = &mut self.intruder;
                let before = state.body_incomplete.then(|| state.request.clone());
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Request").strong().color(theme::TEXT));
                    ui.label(
                        RichText::new(if state.body_escaped {
                            "editable raw message · body: \\xNN bytes, \\\\ backslash"
                        } else {
                            "editable raw message"
                        })
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
                let show_invisibles = message_view::invisible_toggle(ui, true);
                let wrap = message_view::wrap_lines(ui);

                egui::Frame::NONE
                    .fill(Color32::WHITE)
                    .stroke(Stroke::new(1.0, theme::BORDER))
                    .corner_radius(4.0)
                    .inner_margin(egui::Margin::same(8))
                    .show(ui, |ui| {
                        message_view::searchable_view(
                            ui,
                            MessagePane::Request,
                            &mut state.request,
                            None,
                            |ui, text, search| {
                                let output = request_text(ui, text, search, wrap, show_invisibles);
                                if output.response.has_focus()
                                    && let Some(range) = output.state.cursor.char_range()
                                {
                                    let range = range.as_sorted_char_range();
                                    let start = byte_offset_for_char(text.as_str(), range.start.0);
                                    let end = byte_offset_for_char(text.as_str(), range.end.0);
                                    state.cursor = start;
                                    state.selection = (start != end).then_some(start..end);
                                }
                                if output.response.changed() {
                                    state.error = None;
                                }
                            },
                        );
                    });
                if let Some(before) = before {
                    crate::app::replay_body::edited(
                        &before,
                        &state.request,
                        &mut state.body_incomplete,
                    );
                    if !state.body_incomplete {
                        state.legacy_body = None;
                    }
                }
            });
    }
}

fn request_text(
    ui: &mut egui::Ui,
    text: &mut dyn egui::TextBuffer,
    search: &message_view::MessageSearch,
    wrap: bool,
    show_invisibles: bool,
) -> egui::text_edit::TextEditOutput {
    let min_height = ui.available_height().max(160.0);
    let font = FontId::monospace(crate::highlight::MESSAGE_FONT_SIZE);
    let mut layouter = |ui: &egui::Ui, buffer: &dyn egui::TextBuffer, wrap_width: f32| {
        let width = if wrap { wrap_width } else { f32::INFINITY };
        let mut job = request_layout_job(buffer.as_str(), width, &font);
        search.paint(&mut job);
        ui.fonts_mut(|fonts| fonts.layout_job(job))
    };
    egui::ScrollArea::new([!wrap, true])
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let output = TextEdit::multiline(text)
                .id(egui::Id::unique(INTRUDER_EDITOR_ID))
                .code_editor()
                .desired_width(f32::INFINITY)
                .min_size(egui::vec2(0.0, min_height))
                .hint_text("Paste or edit a raw HTTP request, then wrap values in §")
                .layouter(&mut layouter)
                .show(ui);
            if show_invisibles {
                message_view::paint_invisibles(ui, &output.galley, output.galley_pos, &font);
            }
            search.reveal(ui, &output.galley, output.galley_pos);
            output
        })
        .inner
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binary_and_incomplete_drafts_have_no_preserved_body_controls() {
        let ctx = egui::Context::default();
        let mut app = PerseidApp::test_app();
        app.intruder.request =
            "POST / HTTP/1.1\r\nContent-Encoding: gzip\r\nContent-Length: 1\r\n\r\n\\xFF".into();
        app.intruder.body_escaped = true;
        app.intruder.body_incomplete = true;
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1200.0, 600.0),
            )),
            ..Default::default()
        };
        let output = ctx.run_ui(input.clone(), |ui| app.intruder_editor(ui));
        assert!(!output.shapes.iter().any(|shape| matches!(&shape.shape,
            egui::Shape::Text(text) if text.galley.job.text.contains("Preserved captured body")
                || text.galley.job.text.contains("Replace captured body")
                || text.galley.job.text.contains("Use captured prefix")
        )));
        assert!(output.shapes.iter().any(|shape| matches!(&shape.shape,
            egui::Shape::Text(text) if text.galley.job.text.contains("\\xFF")
        )));
        output.drop_without_applying_deltas();
        assert!(app.intruder.body_incomplete);
        let end = app.intruder.request.len();
        app.intruder.select_request(&ctx, end..end);
        ctx.run_ui(
            egui::RawInput {
                events: vec![egui::Event::Text("edited".into())],
                ..input
            },
            |ui| app.intruder_editor(ui),
        )
        .drop_without_applying_deltas();
        assert!(!app.intruder.body_incomplete);
        assert_eq!(
            crate::app::replay_body::bytes(&app.intruder.request, true, false).unwrap(),
            b"\xffedited"
        );
    }

    #[test]
    fn short_panels_preserve_the_request_editor_minimum_height() {
        let ctx = egui::Context::default();
        for wrap in [true, false] {
            ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(400.0, 100.0),
                    )),
                    ..Default::default()
                },
                |ui| {
                    assert!(ui.available_height() < 160.0);
                    let output = request_text(
                        ui,
                        &mut String::new(),
                        &message_view::MessageSearch::default(),
                        wrap,
                        false,
                    );
                    assert!(output.response.rect.height() >= 160.0);
                },
            )
            .drop_without_applying_deltas();
        }
    }

    #[test]
    fn searching_and_wrapping_preserve_intruder_markers_and_unicode() {
        let ctx = egui::Context::default();
        let original = format!("POST / HTTP/1.1\r\n\r\n§é中文§ {}needle", "x".repeat(200));
        let mut text = original.clone();
        let mut search = message_view::MessageSearch::default();
        search.query = "中文".into();
        search.refresh(&text, None);
        search.advance(false);
        let mut heights = Vec::new();
        for wrap in [true, false] {
            let output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(400.0, 300.0),
                    )),
                    ..Default::default()
                },
                |ui| {
                    let output = request_text(ui, &mut text, &search, wrap, true);
                    assert_eq!(output.galley.job.text, original);
                    assert!(!output.response.changed());
                    assert!(
                        output
                            .galley
                            .job
                            .sections
                            .iter()
                            .any(|section| section.format.background == theme::SEARCH_ACTIVE)
                    );
                    assert!(
                        output
                            .galley
                            .job
                            .sections
                            .iter()
                            .any(|section| section.format.background == theme::ACCENT_SOFT)
                    );
                    heights.push(output.galley.size().y);
                },
            );
            output.drop_without_applying_deltas();
        }
        assert_eq!(text, original);
        assert!(heights[0] > heights[1]);
    }

    #[test]
    fn marker_selection_is_preserved_in_editor_character_offsets() {
        let ctx = egui::Context::default();
        let mut state = IntruderState::new();
        state.request = "😀中文".to_owned();
        for target in [4..4, 4..state.request.len()] {
            let (request, range) = crate::proxy::toggle_marker(&state.request, target.clone());
            state.request = request;
            state.select_request(&ctx, range.clone());
            assert_eq!(state.cursor, range.start);
            assert_eq!(
                state.selection,
                (range.start != range.end).then_some(range.clone())
            );
            let editor = TextEditState::load(&ctx, egui::Id::unique(INTRUDER_EDITOR_ID)).unwrap();
            let chars = editor.cursor.char_range().unwrap().as_sorted_char_range();
            assert_eq!(chars.start.0, state.request[..range.start].chars().count());
            assert_eq!(chars.end.0, state.request[..range.end].chars().count());
            let (request, restored) = crate::proxy::toggle_marker(&state.request, range);
            state.request = request;
            assert_eq!(restored, target);
        }
    }
}
