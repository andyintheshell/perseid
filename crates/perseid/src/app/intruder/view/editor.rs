//! The raw request editor and its selection handling.

use std::ops::Range;

use egui::text::{CCursor, CCursorRange};
use egui::text_edit::TextEditState;
use egui::{Color32, FontId, RichText, Stroke, TextEdit};

use crate::theme;

use super::super::super::PerseidApp;
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
