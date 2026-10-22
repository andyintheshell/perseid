//! Decoder: stacked panels for encoding, decoding, hashing, and smart decode.
//!
//! The layout follows Burp's Decoder. Every transformation produces a new
//! editable panel, and the operation that produced it is shown as a colored
//! chip. Text selections are transformed in place within a new panel; applying
//! an operation to an earlier panel replaces the downstream chain.
//!
//! The byte-level transforms live in [`crate::decoder`]. This module owns the
//! panel state, selection handling, coloring, and the egui layout.

use std::ops::Range;

use egui::text::{CCursor, CCursorRange, LayoutJob};
use egui::text_edit::TextEditState;
use egui::{
    Align, Color32, FontId, Label, Layout, RichText, Stroke, TextEdit, TextFormat, TextStyle,
};

use super::PerseidApp;
use crate::decoder as engine;
use crate::theme;
use crate::view;

const DECODER_TOOLBAR_ID: &str = "perseid.decoder.toolbar";
const DECODER_PANELS_ID: &str = "perseid.decoder.panels";
const PANEL_HEIGHT: f32 = 126.0;
const EDITOR_HEIGHT: f32 = 104.0;
const HEX_BYTES_PER_ROW: usize = 8;
const HEX_PREVIEW_LIMIT: usize = 64 * 1024;

/// State for the Decoder tab.
pub(super) struct DecoderState {
    panels: Vec<Panel>,
    next_id: u64,
    scroll_to_end: bool,
}

impl DecoderState {
    pub(super) fn new() -> Self {
        let mut state = Self {
            panels: Vec::new(),
            next_id: 1,
            scroll_to_end: false,
        };
        state.reset();
        state
    }

    /// Seed the documentation screenshot with a small, readable pipeline.
    #[cfg(feature = "screenshots")]
    pub(super) fn seed_screenshot(&mut self) {
        self.panels.clear();
        let id = self.allocate_id();
        self.panels.push(Panel::new(
            id,
            b"alice:correct horse battery staple".to_vec(),
            Vec::new(),
            None,
        ));
        self.apply(0, engine::Transform::Encode(engine::Codec::Base64), None);
        self.apply(
            1,
            engine::Transform::Hash(engine::HashAlgorithm::Sha256),
            None,
        );
        self.scroll_to_end = false;
    }

    pub(super) fn ui(&mut self, ui: &mut egui::Ui) {
        self.toolbar(ui);

        let scroll_to_end = self.scroll_to_end;
        let mut action = None;
        egui::ScrollArea::vertical()
            .id_salt(DECODER_PANELS_ID)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.add_space(8.0);
                for index in 0..self.panels.len() {
                    if index > 0 {
                        ui.add_space(8.0);
                    }
                    let pending = draw_panel(ui, &mut self.panels[index], index);
                    if action.is_none() {
                        action = pending;
                    }
                }
                ui.add_space(8.0);
                if scroll_to_end {
                    ui.scroll_to_cursor(Some(Align::BOTTOM));
                }
            });
        self.scroll_to_end = false;

        if let Some(action) = action {
            self.run_action(action);
        }
    }

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        let mut clear = false;
        let mut copy = false;
        egui::Panel::top(DECODER_TOOLBAR_ID)
            .frame(
                egui::Frame::NONE
                    .fill(theme::SURFACE)
                    .inner_margin(egui::Margin::symmetric(10, 4)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let layers = self.panels.len().saturating_sub(1);
                    ui.label(
                        RichText::new(if layers == 1 {
                            "1 layer".to_string()
                        } else {
                            format!("{layers} layers")
                        })
                        .small()
                        .color(theme::TEXT_MUTED),
                    );
                    ui.separator();
                    ui.label(
                        RichText::new(
                            "Select text to transform only that part; every operation adds a panel.",
                        )
                        .small()
                        .color(theme::TEXT_MUTED),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui
                            .button("Clear")
                            .on_hover_text("Reset the Decoder chain")
                            .clicked()
                        {
                            clear = true;
                        }
                        if ui
                            .button("Copy output")
                            .on_hover_text("Copy the last panel as text or hex")
                            .clicked()
                        {
                            copy = true;
                        }
                    });
                });
            });

        if clear {
            self.reset();
            ui.ctx().memory_mut(|memory| memory.stop_text_input());
        } else if copy && let Some(panel) = self.panels.last() {
            ui.ctx().copy_text(panel.copy_value());
        }
    }

    fn reset(&mut self) {
        self.panels.clear();
        let id = self.allocate_id();
        self.panels
            .push(Panel::new(id, Vec::new(), Vec::new(), None));
        self.scroll_to_end = false;
    }

    fn allocate_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    fn run_action(&mut self, action: Action) {
        match action {
            Action::Transform {
                index,
                operation,
                selection,
            } => self.apply(index, operation, selection),
            Action::Smart { index, selection } => self.smart(index, selection),
        }
    }

    fn apply(
        &mut self,
        index: usize,
        operation: engine::Transform,
        selection: Option<Range<usize>>,
    ) {
        let result = {
            let Some(source) = self.panels.get(index) else {
                return;
            };
            engine::transform(&source.data, selection, operation)
        };
        self.panels[index].error = None;
        self.panels[index].notice = None;
        match result {
            Ok(transformed) => {
                let id = self.allocate_id();
                let panel = Panel::new(id, transformed.data, transformed.spans, Some(operation));
                self.panels.truncate(index + 1);
                self.panels.push(panel);
                self.scroll_to_end = true;
            }
            Err(error) => {
                self.panels[index].error = Some(error.to_string());
            }
        }
    }

    fn smart(&mut self, index: usize, selection: Option<Range<usize>>) {
        let (prefix, selected, suffix) = {
            let Some(source) = self.panels.get(index) else {
                return;
            };
            let range = clamp_selection(source.data.len(), selection);
            (
                source.data[..range.start].to_vec(),
                source.data[range.clone()].to_vec(),
                source.data[range.end..].to_vec(),
            )
        };
        self.panels[index].error = None;
        self.panels[index].notice = None;
        let steps = engine::smart_decode(&selected);
        if steps.is_empty() {
            self.panels[index].notice = Some("No recognizable encoding found.".to_string());
            return;
        }

        let mut panels = Vec::with_capacity(steps.len());
        for step in steps {
            let id = self.allocate_id();
            let mut data = Vec::with_capacity(prefix.len() + step.data.len() + suffix.len());
            data.extend_from_slice(&prefix);
            let start = data.len();
            data.extend_from_slice(&step.data);
            let end = data.len();
            data.extend_from_slice(&suffix);
            let spans = if start == end {
                Vec::new()
            } else {
                vec![engine::Span {
                    start,
                    end,
                    tag: engine::ColorTag::Codec(step.codec),
                }]
            };
            panels.push(Panel::new(
                id,
                data,
                spans,
                Some(engine::Transform::Decode(step.codec)),
            ));
        }
        self.panels.truncate(index + 1);
        self.panels.extend(panels);
        self.scroll_to_end = true;
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum ViewMode {
    #[default]
    Text,
    Hex,
}

struct Panel {
    id: u64,
    data: Vec<u8>,
    text: String,
    text_valid: bool,
    spans: Vec<engine::Span>,
    operation: Option<engine::Transform>,
    view: ViewMode,
    selection: Option<Range<usize>>,
    context_menu_open: bool,
    error: Option<String>,
    notice: Option<String>,
}

impl Panel {
    fn new(
        id: u64,
        data: Vec<u8>,
        spans: Vec<engine::Span>,
        operation: Option<engine::Transform>,
    ) -> Self {
        let text_valid = std::str::from_utf8(&data).is_ok();
        let text = String::from_utf8_lossy(&data).into_owned();
        let view = if text_valid && engine::is_probably_text(&data) {
            ViewMode::Text
        } else {
            ViewMode::Hex
        };
        Self {
            id,
            data,
            text,
            text_valid,
            spans,
            operation,
            view,
            selection: None,
            context_menu_open: false,
            error: None,
            notice: None,
        }
    }

    fn editor_id(&self) -> egui::Id {
        egui::Id::unique(("perseid.decoder.editor", self.id))
    }

    fn sync_text(&mut self) {
        self.data = self.text.as_bytes().to_vec();
        self.text_valid = true;
        self.spans.clear();
        self.selection = None;
        self.error = None;
        self.notice = None;
    }

    fn copy_value(&self) -> String {
        if self.view == ViewMode::Hex || !self.text_valid || !engine::is_probably_text(&self.data) {
            hex_copy(&self.data)
        } else {
            self.text.clone()
        }
    }

    fn binary(&self) -> bool {
        !self.text_valid || !engine::is_probably_text(&self.data)
    }
}

enum Action {
    Transform {
        index: usize,
        operation: engine::Transform,
        selection: Option<Range<usize>>,
    },
    Smart {
        index: usize,
        selection: Option<Range<usize>>,
    },
}

fn draw_panel(ui: &mut egui::Ui, panel: &mut Panel, index: usize) -> Option<Action> {
    let width = ui.available_width();
    let sidebar_width = 220.0_f32.min((width * 0.44).max(180.0));
    let editor_width = (width - sidebar_width - 56.0).max(150.0);
    let mut action = None;

    egui::Frame::NONE
        .fill(theme::SURFACE)
        .stroke(Stroke::new(1.0, theme::BORDER))
        .corner_radius(4.0)
        .inner_margin(egui::Margin::same(7))
        .show(ui, |ui| {
            ui.horizontal_top(|ui| {
                let edited = ui
                    .allocate_ui_with_layout(
                        egui::vec2(editor_width, PANEL_HEIGHT),
                        Layout::top_down(Align::Min),
                        |ui| draw_editor(ui, panel),
                    )
                    .inner;
                if edited {
                    panel.sync_text();
                }

                ui.add_space(10.0);
                let (divider, _) =
                    ui.allocate_exact_size(egui::vec2(1.0, PANEL_HEIGHT), egui::Sense::hover());
                ui.painter().vline(
                    divider.center().x,
                    divider.y_range(),
                    Stroke::new(1.0, theme::BORDER),
                );
                ui.add_space(10.0);

                action = ui
                    .allocate_ui_with_layout(
                        egui::vec2(sidebar_width, PANEL_HEIGHT),
                        Layout::top_down(Align::Min),
                        |ui| draw_controls(ui, panel, index),
                    )
                    .inner;
            });
        });

    action
}

fn draw_editor(ui: &mut egui::Ui, panel: &mut Panel) -> bool {
    let mut edited = false;
    let mut editor_focused = false;
    let mut focused_selection = None;
    let menu_was_open = panel.context_menu_open;
    let secondary_pressed =
        ui.input(|input| input.pointer.button_pressed(egui::PointerButton::Secondary));
    let editor_id = panel.editor_id();
    let frame = egui::Frame::NONE
        .fill(Color32::WHITE)
        .stroke(Stroke::new(1.0, theme::BORDER))
        .corner_radius(4.0)
        .inner_margin(egui::Margin::same(4))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.set_min_height(EDITOR_HEIGHT);
            match panel.view {
                ViewMode::Text => {
                    egui::ScrollArea::vertical()
                        .id_salt(("perseid.decoder.text", panel.id))
                        .max_height(EDITOR_HEIGHT)
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            let Panel { text, spans, .. } = panel;
                            let mut layouter =
                                |ui: &egui::Ui, buffer: &dyn egui::TextBuffer, wrap_width: f32| {
                                    let job =
                                        text_layout_job(ui, buffer.as_str(), spans, wrap_width);
                                    ui.fonts_mut(|fonts| fonts.layout_job(job))
                                };
                            let output = TextEdit::multiline(text)
                                .id(editor_id)
                                .code_editor()
                                .frame(egui::Frame::NONE)
                                .desired_width(f32::INFINITY)
                                .min_size(egui::vec2(ui.available_width(), EDITOR_HEIGHT - 4.0))
                                .hint_text("Paste or type data, then choose an operation")
                                .layouter(&mut layouter)
                                .show(ui);
                            if output.response.has_focus() {
                                editor_focused = true;
                                if let Some(range) = output.state.cursor.char_range() {
                                    let range = range.as_sorted_char_range();
                                    focused_selection =
                                        char_range_to_byte_range(text, range.start.0, range.end.0);
                                }
                            }
                            edited = output.response.changed();
                        });
                }
                ViewMode::Hex => hex_view(ui, panel),
            }
        });
    // A secondary click collapses the editor's selection but should still make
    // the selection available to the context menu. Keep the last non-empty
    // selection until the menu closes.
    let preserve_selection =
        menu_was_open || (secondary_pressed && frame.response.contains_pointer());
    if editor_focused {
        if focused_selection.is_some() {
            panel.selection = focused_selection;
        } else if !preserve_selection {
            panel.selection = None;
        }
    }

    let context_menu = if panel.view == ViewMode::Text {
        frame.response.container_context_menu(|ui| {
            let selected = panel
                .selection
                .clone()
                .and_then(|selection| panel.text.get(selection))
                .map(str::to_owned);
            let has_selection = selected.is_some();
            if ui
                .add_enabled(has_selection, egui::Button::new("Cut"))
                .clicked()
            {
                focus_editor_with_selection(ui.ctx(), editor_id, panel);
                ui.ctx()
                    .send_viewport_cmd(egui::ViewportCommand::RequestCut);
                ui.close();
            }
            if ui
                .add_enabled(has_selection, egui::Button::new("Copy"))
                .clicked()
                && let Some(selected) = selected.as_deref()
            {
                ui.ctx().copy_text(selected.to_owned());
                ui.close();
            }
            if ui.button("Paste").clicked() {
                focus_editor_with_selection(ui.ctx(), editor_id, panel);
                ui.ctx()
                    .send_viewport_cmd(egui::ViewportCommand::RequestPaste);
                ui.close();
            }
            ui.separator();
            if ui.button("Select all").clicked() {
                set_editor_selection(
                    ui.ctx(),
                    editor_id,
                    Some(CCursorRange::select_all_str(&panel.text)),
                );
                panel.selection = Some(0..panel.text.len());
                ui.close();
            }
        })
    } else {
        frame.response.container_context_menu(|ui| {
            if ui.button("Copy hex").clicked() {
                ui.ctx().copy_text(panel.copy_value());
                ui.close();
            }
        })
    };
    panel.context_menu_open = context_menu.is_some();

    edited
}

/// Convert a byte range in `text` to the character range used by egui.
fn char_range_for_selection(text: &str, selection: &Range<usize>) -> Option<CCursorRange> {
    text.get(selection.clone())?;
    let start = text[..selection.start].chars().count();
    let end = text[..selection.end].chars().count();
    Some(CCursorRange::two(CCursor::new(start), CCursor::new(end)))
}

fn focus_editor_with_selection(ctx: &egui::Context, editor_id: egui::Id, panel: &Panel) {
    if let Some(selection) = panel
        .selection
        .as_ref()
        .and_then(|selection| char_range_for_selection(&panel.text, selection))
    {
        set_editor_selection(ctx, editor_id, Some(selection));
    }
    ctx.memory_mut(|memory| memory.request_focus(editor_id));
}

fn set_editor_selection(ctx: &egui::Context, editor_id: egui::Id, range: Option<CCursorRange>) {
    let mut state = TextEditState::load(ctx, editor_id).unwrap_or_default();
    state.cursor.set_char_range(range);
    state.store(ctx, editor_id);
    ctx.memory_mut(|memory| memory.request_focus(editor_id));
}

fn draw_controls(ui: &mut egui::Ui, panel: &mut Panel, index: usize) -> Option<Action> {
    let mut action = None;
    ui.spacing_mut().item_spacing = egui::vec2(8.0, 4.0);
    ui.spacing_mut().button_padding = egui::vec2(6.0, 3.0);
    ui.spacing_mut().interact_size.y = 24.0;

    let mut view_changed = false;
    ui.horizontal(|ui| {
        operation_chip(ui, panel.operation);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if ui
                .small_button("Copy")
                .on_hover_text("Copy this panel as text or hex")
                .clicked()
            {
                ui.ctx().copy_text(panel.copy_value());
            }
        });
    });
    ui.horizontal(|ui| {
        view_changed |= ui
            .radio_value(&mut panel.view, ViewMode::Text, "Text")
            .changed();
        view_changed |= ui
            .radio_value(&mut panel.view, ViewMode::Hex, "Hex")
            .on_hover_text("Hex view is read-only; to enter bytes, decode ASCII hex in Text view")
            .changed();
    });
    if view_changed {
        panel.selection = None;
    }

    if let Some(choice) = codec_combo(ui, ("perseid.decoder.decode", panel.id), "Decode as…") {
        let operation = match choice {
            CodecChoice::Plain => engine::Transform::Plain,
            CodecChoice::Codec(codec) => engine::Transform::Decode(codec),
        };
        action = Some(Action::Transform {
            index,
            operation,
            selection: selection_for(panel),
        });
    }

    if let Some(choice) = codec_combo(ui, ("perseid.decoder.encode", panel.id), "Encode as…") {
        let operation = match choice {
            CodecChoice::Plain => engine::Transform::Plain,
            CodecChoice::Codec(codec) => engine::Transform::Encode(codec),
        };
        action = Some(Action::Transform {
            index,
            operation,
            selection: selection_for(panel),
        });
    }

    if let Some(algorithm) = hash_combo(ui, ("perseid.decoder.hash", panel.id)) {
        action = Some(Action::Transform {
            index,
            operation: engine::Transform::Hash(algorithm),
            selection: selection_for(panel),
        });
    }

    let smart = egui::Button::new(RichText::new("Smart decode").strong())
        .min_size(egui::vec2(ui.available_width(), 23.0));
    if ui
        .add(smart)
        .on_hover_text("Detect and decode URL, HTML, Base64, ASCII hex, and Gzip layers")
        .clicked()
    {
        action = Some(Action::Smart {
            index,
            selection: selection_for(panel),
        });
    }

    if let Some(error) = &panel.error {
        ui.add(
            Label::new(RichText::new(error).small().color(theme::RED))
                .wrap()
                .selectable(false),
        );
    } else if let Some(notice) = &panel.notice {
        ui.add(
            Label::new(RichText::new(notice).small().color(theme::AMBER))
                .wrap()
                .selectable(false),
        );
    } else if panel.binary() && panel.view == ViewMode::Text {
        ui.add(
            Label::new(
                RichText::new("Binary data. Editing replaces it with UTF-8 text.")
                    .small()
                    .color(theme::AMBER),
            )
            .wrap()
            .selectable(false),
        );
    } else if panel.view == ViewMode::Hex && panel.data.len() > HEX_PREVIEW_LIMIT {
        ui.add(
            Label::new(
                RichText::new("Hex preview is limited to 64 KiB; Copy exports all bytes.")
                    .small()
                    .color(theme::TEXT_MUTED),
            )
            .wrap()
            .selectable(false),
        );
    }

    if action.is_some() {
        panel.error = None;
        panel.notice = None;
    }
    action
}

enum CodecChoice {
    Plain,
    Codec(engine::Codec),
}

fn codec_combo(
    ui: &mut egui::Ui,
    id_salt: impl std::hash::Hash + std::fmt::Debug,
    title: &str,
) -> Option<CodecChoice> {
    let mut choice = None;
    egui::ComboBox::from_id_salt(id_salt)
        .width(panel_control_width(ui))
        .selected_text(title)
        .show_ui(ui, |ui| {
            ui.set_min_width(190.0);
            ui.spacing_mut().item_spacing.y = 3.0;
            ui.spacing_mut().interact_size.y = 24.0;
            let menu_height = (ui.ctx().viewport_rect().height() * 0.5).clamp(180.0, 360.0);
            egui::ScrollArea::vertical()
                .id_salt(("perseid.decoder.codec.menu", title))
                .max_height(menu_height)
                .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible)
                .show(ui, |ui| {
                    if ui.selectable_label(false, "Plain").clicked() {
                        choice = Some(CodecChoice::Plain);
                        ui.close();
                    }
                    ui.separator();
                    for codec in engine::Codec::ALL {
                        let label = RichText::new(codec.label()).color(codec_color(codec));
                        if ui.selectable_label(false, label).clicked() {
                            choice = Some(CodecChoice::Codec(codec));
                            ui.close();
                        }
                    }
                });
        });
    choice
}

fn hash_combo(
    ui: &mut egui::Ui,
    id_salt: impl std::hash::Hash + std::fmt::Debug,
) -> Option<engine::HashAlgorithm> {
    let mut choice = None;
    egui::ComboBox::from_id_salt(id_salt)
        .width(panel_control_width(ui))
        .selected_text("Hash…")
        .show_ui(ui, |ui| {
            ui.set_min_width(210.0);
            ui.spacing_mut().item_spacing.y = 3.0;
            ui.spacing_mut().interact_size.y = 24.0;
            let menu_height = (ui.ctx().viewport_rect().height() * 0.5).clamp(180.0, 360.0);
            egui::ScrollArea::vertical()
                .id_salt("perseid.decoder.hash.menu")
                .max_height(menu_height)
                .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible)
                .show(ui, |ui| {
                    for algorithm in engine::HashAlgorithm::ALL {
                        if ui.selectable_label(false, algorithm.label()).clicked() {
                            choice = Some(algorithm);
                            ui.close();
                        }
                    }
                });
        });
    choice
}

fn panel_control_width(ui: &egui::Ui) -> f32 {
    ui.available_width().max(160.0)
}

fn operation_chip(ui: &mut egui::Ui, operation: Option<engine::Transform>) {
    let (label, color) = match operation {
        Some(operation) => (operation.label(), transform_color(operation)),
        None => ("Input".to_string(), theme::TEXT_MUTED),
    };
    egui::Frame::NONE
        .fill(color.gamma_multiply(0.10))
        .stroke(Stroke::new(1.0, color.gamma_multiply(0.45)))
        .corner_radius(8.0)
        .inner_margin(egui::Margin::symmetric(8, 2))
        .show(ui, |ui| {
            ui.label(RichText::new(label).size(10.5).strong().color(color));
        });
}

fn selection_for(panel: &Panel) -> Option<Range<usize>> {
    if panel.view != ViewMode::Text || !panel.text_valid {
        return None;
    }
    panel.selection.clone()
}

fn char_range_to_byte_range(text: &str, start: usize, end: usize) -> Option<Range<usize>> {
    if start >= end {
        return None;
    }
    let byte_start = text.char_indices().nth(start)?.0;
    let byte_end = text
        .char_indices()
        .nth(end)
        .map(|(offset, _)| offset)
        .unwrap_or(text.len());
    (byte_start < byte_end).then_some(byte_start..byte_end)
}

fn clamp_selection(length: usize, selection: Option<Range<usize>>) -> Range<usize> {
    match selection {
        Some(range) if range.start < range.end && range.start < length => {
            range.start.min(length)..range.end.min(length)
        }
        _ => 0..length,
    }
}

fn text_layout_job(
    ui: &egui::Ui,
    text: &str,
    spans: &[engine::Span],
    wrap_width: f32,
) -> LayoutJob {
    let font = monospace_font(ui);
    let mut job = LayoutJob::default();
    job.wrap.max_width = wrap_width;
    if text.is_empty() {
        return job;
    }

    let mut run_start = 0;
    let mut current_tag = tag_at(spans, 0);
    for (offset, _) in text.char_indices().skip(1) {
        let tag = tag_at(spans, offset);
        if tag != current_tag {
            append_text_run(&mut job, &text[run_start..offset], current_tag, &font);
            run_start = offset;
            current_tag = tag;
        }
    }
    append_text_run(&mut job, &text[run_start..], current_tag, &font);
    job
}

fn append_text_run(job: &mut LayoutJob, text: &str, tag: Option<engine::ColorTag>, font: &FontId) {
    if text.is_empty() {
        return;
    }
    let (color, background) = match tag {
        Some(tag) => (tag_color(tag), tag_wash(tag)),
        None => (theme::TEXT, Color32::TRANSPARENT),
    };
    job.append(
        text,
        0.0,
        TextFormat {
            font_id: font.clone(),
            color,
            background,
            ..Default::default()
        },
    );
}

fn hex_view(ui: &mut egui::Ui, panel: &Panel) {
    if panel.data.is_empty() {
        ui.label(RichText::new("No data").italics().color(theme::TEXT_MUTED));
        return;
    }

    let shown = panel.data.len().min(HEX_PREVIEW_LIMIT);
    let rows = shown.div_ceil(HEX_BYTES_PER_ROW);
    let row_height = ui.text_style_height(&TextStyle::Monospace) + 2.0;
    egui::ScrollArea::both()
        .id_salt(("perseid.decoder.hex", panel.id))
        .max_height(EDITOR_HEIGHT)
        .auto_shrink([false, false])
        .show_rows(ui, row_height, rows, |ui, row_range| {
            for row in row_range {
                let job = hex_row_job(ui, panel, row, shown);
                ui.add(Label::new(job).extend().selectable(true));
            }
        });
    if panel.data.len() > shown {
        ui.label(
            RichText::new(format!(
                "Showing first {} of {} bytes",
                view::human_bytes(shown as u64),
                view::human_bytes(panel.data.len() as u64)
            ))
            .small()
            .color(theme::TEXT_MUTED),
        );
    }
}

fn hex_row_job(ui: &egui::Ui, panel: &Panel, row: usize, shown: usize) -> LayoutJob {
    let font = monospace_font(ui);
    let mut job = LayoutJob::default();
    job.wrap.max_width = f32::INFINITY;
    let start = row * HEX_BYTES_PER_ROW;
    let end = (start + HEX_BYTES_PER_ROW).min(shown);

    append_hex_run(
        &mut job,
        &format!("{start:08x}  "),
        theme::TEXT_MUTED,
        &font,
    );

    for offset in start..start + HEX_BYTES_PER_ROW {
        if offset < end {
            let color = tag_at(&panel.spans, offset)
                .map(tag_color)
                .unwrap_or(theme::TEXT);
            append_hex_run(
                &mut job,
                &format!("{:02x} ", panel.data[offset]),
                color,
                &font,
            );
        } else {
            append_hex_run(&mut job, "   ", theme::TEXT, &font);
        }
    }

    append_hex_run(&mut job, " |", theme::TEXT_MUTED, &font);
    for offset in start..end {
        let byte = panel.data[offset];
        let character = if (0x20..=0x7e).contains(&byte) {
            char::from(byte)
        } else {
            '.'
        };
        let color = tag_at(&panel.spans, offset)
            .map(tag_color)
            .unwrap_or(theme::TEXT_MUTED);
        append_hex_run(&mut job, &character.to_string(), color, &font);
    }
    append_hex_run(&mut job, "|", theme::TEXT_MUTED, &font);
    job
}

fn append_hex_run(job: &mut LayoutJob, text: &str, color: Color32, font: &FontId) {
    job.append(
        text,
        0.0,
        TextFormat {
            font_id: font.clone(),
            color,
            ..Default::default()
        },
    );
}

fn monospace_font(ui: &egui::Ui) -> FontId {
    ui.style()
        .text_styles
        .get(&TextStyle::Monospace)
        .cloned()
        .unwrap_or_else(|| FontId::monospace(13.0))
}

fn tag_at(spans: &[engine::Span], offset: usize) -> Option<engine::ColorTag> {
    spans
        .iter()
        .find(|span| span.start <= offset && offset < span.end)
        .map(|span| span.tag)
}

fn transform_color(transform: engine::Transform) -> Color32 {
    match transform {
        engine::Transform::Plain => theme::TEXT_MUTED,
        engine::Transform::Decode(codec) | engine::Transform::Encode(codec) => codec_color(codec),
        engine::Transform::Hash(_) => hash_color(),
    }
}

fn tag_color(tag: engine::ColorTag) -> Color32 {
    match tag {
        engine::ColorTag::Codec(codec) => codec_color(codec),
        engine::ColorTag::Hash => hash_color(),
        engine::ColorTag::Plain => theme::TEXT,
    }
}

fn tag_wash(tag: engine::ColorTag) -> Color32 {
    match tag {
        engine::ColorTag::Codec(codec) => codec_wash(codec),
        engine::ColorTag::Hash => Color32::from_rgb(0xf1, 0xed, 0xf9),
        engine::ColorTag::Plain => Color32::TRANSPARENT,
    }
}

fn codec_color(codec: engine::Codec) -> Color32 {
    match codec {
        engine::Codec::Url => Color32::from_rgb(0xc0, 0x39, 0x2b),
        engine::Codec::Html => Color32::from_rgb(0xc2, 0x6a, 0x00),
        engine::Codec::Base64 => Color32::from_rgb(0x8c, 0x86, 0x00),
        engine::Codec::AsciiHex => Color32::from_rgb(0x2f, 0x7d, 0x4f),
        engine::Codec::Hex => Color32::from_rgb(0x2f, 0x6f, 0x9f),
        engine::Codec::Octal => Color32::from_rgb(0x00, 0x83, 0x8a),
        engine::Codec::Binary => Color32::from_rgb(0xa3, 0x2a, 0xa3),
        engine::Codec::Gzip => Color32::from_rgb(0x8a, 0x4b, 0x1f),
    }
}

fn codec_wash(codec: engine::Codec) -> Color32 {
    match codec {
        engine::Codec::Url => Color32::from_rgb(0xfd, 0xee, 0xec),
        engine::Codec::Html => Color32::from_rgb(0xfd, 0xf2, 0xe3),
        engine::Codec::Base64 => Color32::from_rgb(0xfa, 0xf8, 0xdc),
        engine::Codec::AsciiHex => Color32::from_rgb(0xec, 0xf6, 0xef),
        engine::Codec::Hex => Color32::from_rgb(0xec, 0xf1, 0xf9),
        engine::Codec::Octal => Color32::from_rgb(0xe7, 0xf5, 0xf5),
        engine::Codec::Binary => Color32::from_rgb(0xf7, 0xec, 0xf7),
        engine::Codec::Gzip => Color32::from_rgb(0xf4, 0xed, 0xe8),
    }
}

fn hash_color() -> Color32 {
    Color32::from_rgb(0x6b, 0x4a, 0xa8)
}

fn hex_copy(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut output = String::with_capacity(bytes.len().saturating_mul(3));
    for (index, byte) in bytes.iter().enumerate() {
        if index > 0 {
            output.push(' ');
        }
        let _ = write!(output, "{byte:02x}");
    }
    output
}

impl PerseidApp {
    pub(super) fn decoder_view(&mut self, ui: &mut egui::Ui) {
        self.decoder.ui(ui);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set_root(state: &mut DecoderState, bytes: &[u8]) {
        let panel = &mut state.panels[0];
        panel.data = bytes.to_vec();
        panel.text = String::from_utf8_lossy(bytes).into_owned();
        panel.text_valid = std::str::from_utf8(bytes).is_ok();
        panel.spans.clear();
        panel.view = ViewMode::Text;
    }

    #[test]
    fn char_ranges_convert_to_utf8_byte_ranges() {
        let text = "aé中z";
        assert_eq!(char_range_to_byte_range(text, 0, 1), Some(0..1));
        assert_eq!(char_range_to_byte_range(text, 1, 2), Some(1..3));
        assert_eq!(char_range_to_byte_range(text, 2, 3), Some(3..6));
        assert_eq!(char_range_to_byte_range(text, 3, 4), Some(6..7));
        assert_eq!(char_range_to_byte_range(text, 2, 2), None);
    }

    #[test]
    fn context_selection_uses_character_offsets() {
        let range = char_range_for_selection("héllo", &(1..3)).expect("valid selection");
        let range = range.as_sorted_char_range();
        assert_eq!((range.start.0, range.end.0), (1, 2));
        assert!(char_range_for_selection("héllo", &(0..2)).is_none());
    }

    #[test]
    fn operations_replace_the_downstream_chain() {
        let mut state = DecoderState::new();
        set_root(&mut state, b"a%20b");
        state.apply(0, engine::Transform::Decode(engine::Codec::Url), None);
        assert_eq!(state.panels.len(), 2);
        assert_eq!(state.panels[1].data, b"a b");
        assert_eq!(
            state.panels[1].operation,
            Some(engine::Transform::Decode(engine::Codec::Url))
        );

        state.apply(0, engine::Transform::Encode(engine::Codec::Html), None);
        assert_eq!(state.panels.len(), 2);
        assert_eq!(state.panels[1].data, b"&#x61;&#x25;&#x32;&#x30;&#x62;");
    }

    #[test]
    fn selections_are_transformed_without_replacing_the_rest() {
        let mut state = DecoderState::new();
        set_root(&mut state, b"hello world");
        state.apply(
            0,
            engine::Transform::Encode(engine::Codec::Url),
            Some(6..11),
        );
        assert_eq!(state.panels.len(), 2);
        assert_eq!(state.panels[1].data, b"hello %77%6f%72%6c%64");
        assert_eq!(state.panels[1].spans[0].start, 6);
    }

    #[test]
    fn smart_decode_adds_one_panel_per_layer() {
        let mut state = DecoderState::new();
        set_root(&mut state, b"%253Cb%253E");
        state.smart(0, None);
        assert_eq!(state.panels.len(), 3);
        assert_eq!(state.panels[1].data, b"%3Cb%3E");
        assert_eq!(state.panels[2].data, b"<b>");
        assert_eq!(
            state.panels[2].operation,
            Some(engine::Transform::Decode(engine::Codec::Url))
        );
    }

    #[test]
    fn smart_decode_reports_when_nothing_is_recognized() {
        let mut state = DecoderState::new();
        set_root(&mut state, b"hello world");
        state.smart(0, None);
        assert_eq!(state.panels.len(), 1);
        assert!(state.panels[0].notice.is_some());
    }

    #[test]
    fn editing_text_updates_the_canonical_bytes() {
        let mut state = DecoderState::new();
        state.panels[0].text = "updated payload".to_string();
        state.panels[0].sync_text();
        assert_eq!(state.panels[0].data, b"updated payload");
        assert!(state.panels[0].spans.is_empty());
        assert!(state.panels[0].selection.is_none());
    }

    #[test]
    fn saved_selection_is_used_only_for_text_panels() {
        let mut state = DecoderState::new();
        set_root(&mut state, b"hello world");
        state.panels[0].selection = Some(6..11);
        assert_eq!(selection_for(&state.panels[0]), Some(6..11));

        state.panels[0].view = ViewMode::Hex;
        assert_eq!(selection_for(&state.panels[0]), None);

        state.panels[0].view = ViewMode::Text;
        state.panels[0].text_valid = false;
        assert_eq!(selection_for(&state.panels[0]), None);
    }
}
