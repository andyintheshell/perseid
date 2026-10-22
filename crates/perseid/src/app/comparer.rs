//! Comparer: two item lists over one shared collection, plus a result window.
//!
//! The item tables deliberately show the same collection twice. Burp's
//! Comparer works this way: send an item once, then select any two rows—one in
//! each table—to compare them. Paste, Load, Remove, and Clear therefore all
//! operate on the shared list.
//!
//! The comparison itself lives in [`crate::comparer`]. This module builds the
//! item list, sends history messages here, and renders the result with paired
//! text or hex panes. Text and hex views are virtualized by line so a large
//! captured message does not have to be laid out as one galley.

use std::ops::Range;
use std::path::PathBuf;

use egui::text::LayoutJob;
use egui::{
    Align, Align2, Button, Color32, FontId, Key, Label, Layout, Margin, Rect, RichText, ScrollArea,
    Sense, Stroke, TextEdit, TextFormat, TextStyle, Vec2,
};
use egui_extras::{Column, TableBuilder};

use super::*;
use crate::comparer as engine;
use crate::comparer::{ChangeKind, CompareMode, DiffSpan, DisplayText, MAX_ITEM_BYTES, MAX_ITEMS};
use crate::theme;
use crate::view;

const ITEM_TABLE_ROW_HEIGHT: f32 = 19.0;
const PREVIEW_BYTES: usize = 220;
const CONTROLS_WIDTH: f32 = 108.0;
const RESULT_WINDOW_ID: &str = "perseid.comparer.result";
const HEX_BYTES_PER_ROW: usize = 16;
const MODIFIED_BG: Color32 = Color32::from_rgb(0xcf, 0xe3, 0xf7);
const DELETED_BG: Color32 = Color32::from_rgb(0xf8, 0xd8, 0xd2);
const ADDED_BG: Color32 = Color32::from_rgb(0xd7, 0xf0, 0xdc);

/// One entry in the shared item collection.
struct ComparableItem {
    data: Vec<u8>,
}

impl ComparableItem {
    fn new(data: Vec<u8>) -> Self {
        Self { data }
    }

    fn preview(&self) -> String {
        let mut out = String::with_capacity(PREVIEW_BYTES + 1);
        for &byte in self.data.iter().take(PREVIEW_BYTES) {
            let character = match byte {
                b'\r' | b'\n' | b'\t' => ' ',
                0x20..=0x7e => char::from(byte),
                _ => '·',
            };
            out.push(character);
        }
        if self.data.len() > PREVIEW_BYTES {
            out.push('…');
        }
        out
    }
}

struct Feedback {
    error: bool,
    text: String,
}

struct LoadDialog {
    target: usize,
    path: String,
    error: Option<String>,
}

struct PasteState {
    target: usize,
    frames_left: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ResultView {
    Text,
    Hex,
}

#[derive(Clone, Debug)]
struct Highlight {
    range: Range<usize>,
    kind: ChangeKind,
}

struct CompareResult {
    old_index: usize,
    new_index: usize,
    /// Title shown in the result window.
    title: String,
    old_data: Vec<u8>,
    new_data: Vec<u8>,
    old_text: DisplayText,
    new_text: DisplayText,
    text_highlights: [Vec<Vec<Highlight>>; 2],
    byte_kinds: [Vec<Option<ChangeKind>>; 2],
    spans: Vec<DiffSpan>,
    differences: usize,
}

impl CompareResult {
    fn new(
        mode: CompareMode,
        old_index: usize,
        new_index: usize,
        old_data: Vec<u8>,
        new_data: Vec<u8>,
        comparison: engine::Comparison,
    ) -> Self {
        let old_text = DisplayText::new(&old_data);
        let new_text = DisplayText::new(&new_data);
        let text_highlights = [
            build_text_highlights(&old_text, &comparison.spans, 0),
            build_text_highlights(&new_text, &comparison.spans, 1),
        ];
        let byte_kinds = [
            build_byte_kinds(old_data.len(), &comparison.spans, 0),
            build_byte_kinds(new_data.len(), &comparison.spans, 1),
        ];
        let difference_word = if comparison.differences == 1 {
            "difference"
        } else {
            "differences"
        };
        let title = format!(
            "{} compare of #{} and #{} ({} {difference_word})",
            mode.label(),
            old_index + 1,
            new_index + 1,
            comparison.differences
        );
        Self {
            old_index,
            new_index,
            title,
            old_data,
            new_data,
            old_text,
            new_text,
            text_highlights,
            byte_kinds,
            spans: comparison.spans,
            differences: comparison.differences,
        }
    }

    fn data(&self, side: usize) -> &[u8] {
        if side == 0 {
            &self.old_data
        } else {
            &self.new_data
        }
    }

    fn display(&self, side: usize) -> &DisplayText {
        if side == 0 {
            &self.old_text
        } else {
            &self.new_text
        }
    }

    fn text_highlights(&self, side: usize) -> &[Vec<Highlight>] {
        &self.text_highlights[side]
    }

    fn byte_kinds(&self, side: usize) -> &[Option<ChangeKind>] {
        &self.byte_kinds[side]
    }

    fn side_label(&self, side: usize) -> String {
        format!(
            "#{}",
            if side == 0 {
                self.old_index + 1
            } else {
                self.new_index + 1
            }
        )
    }
}

/// State for the Comparer tab and its result window.
pub(super) struct ComparerState {
    items: Vec<ComparableItem>,
    selected: [Option<usize>; 2],
    active: usize,
    feedback: Option<Feedback>,
    load: Option<LoadDialog>,
    paste: Option<PasteState>,
    result: Option<CompareResult>,
    result_open: bool,
    result_views: [ResultView; 2],
    sync_views: bool,
    diff_cursor: usize,
    result_scroll: [Vec2; 2],
    result_pane_rects: [Option<Rect>; 2],
    result_row_height: f32,
}

impl ComparerState {
    pub(super) fn new() -> Self {
        Self {
            items: Vec::new(),
            selected: [None, None],
            active: 0,
            feedback: None,
            load: None,
            paste: None,
            result: None,
            result_open: false,
            result_views: [ResultView::Text, ResultView::Text],
            sync_views: false,
            diff_cursor: 0,
            result_scroll: [Vec2::ZERO, Vec2::ZERO],
            result_pane_rects: [None, None],
            result_row_height: 14.0,
        }
    }

    pub(super) fn ui(&mut self, ui: &mut egui::Ui) {
        widgets::page_pad(ui, |ui| self.page(ui));
    }

    /// Consume a clipboard paste requested by the Paste button.
    pub(super) fn handle_input(&mut self, ctx: &egui::Context) {
        let Some(mut pending) = self.paste.take() else {
            return;
        };
        let pasted = ctx.input(|input| {
            input.events.iter().find_map(|event| match event {
                egui::Event::Paste(text) if !text.is_empty() => Some(text.clone()),
                _ => None,
            })
        });
        if let Some(text) = pasted {
            if let Err(error) = self.insert_item(text.into_bytes(), pending.target) {
                self.feedback = Some(Feedback {
                    error: true,
                    text: error,
                });
            }
            return;
        }
        pending.frames_left = pending.frames_left.saturating_sub(1);
        if pending.frames_left > 0 {
            self.paste = Some(pending);
        }
    }

    fn page(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("Comparer")
                    .size(15.0)
                    .strong()
                    .color(theme::TEXT),
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(
                    RichText::new("Select one item in each table and compare.")
                        .small()
                        .color(theme::TEXT_MUTED),
                );
            });
        });
        ui.label(
            RichText::new(
                "Right-click a row in Proxy HTTP history to send a request or response, or paste and load items here. Both tables share one collection.",
            )
            .small()
            .color(theme::TEXT_MUTED),
        );
        if let Some(feedback) = &self.feedback {
            ui.add_space(3.0);
            let color = if feedback.error {
                theme::RED
            } else {
                theme::GREEN
            };
            ui.label(RichText::new(&feedback.text).small().color(color));
        }
        ui.add_space(6.0);

        let body_height = ui.available_height().max(180.0);
        let controls_width = CONTROLS_WIDTH;
        let gap = 10.0;
        let table_width = (ui.available_width() - controls_width - gap).max(280.0);
        ui.horizontal_top(|ui| {
            ui.allocate_ui_with_layout(
                Vec2::new(table_width, body_height),
                Layout::top_down(Align::Min),
                |ui| {
                    let section_height = (body_height - 7.0).max(80.0) * 0.5;
                    self.item_table(ui, 0, section_height);
                    ui.add_space(7.0);
                    self.item_table(ui, 1, section_height);
                },
            );
            ui.add_space(gap);
            self.controls(ui, body_height);
        });
    }

    fn item_table(&mut self, ui: &mut egui::Ui, which: usize, height: f32) {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!("Select item {}:", which + 1))
                    .strong()
                    .color(theme::TEXT),
            );
            ui.label(
                RichText::new(format!("{} available", self.items.len()))
                    .small()
                    .color(theme::TEXT_MUTED),
            );
        });
        let table_height = (height - 24.0).max(60.0);
        let mut clicked = None;
        egui::Frame::NONE
            .fill(theme::SURFACE)
            .stroke(Stroke::new(1.0, theme::BORDER))
            .corner_radius(3.0)
            .inner_margin(Margin::symmetric(1, 1))
            .show(ui, |ui| {
                ui.set_min_height(table_height);
                ui.set_max_height(table_height);
                let table = TableBuilder::new(ui)
                    .id_salt(("perseid.comparer.items", which))
                    .striped(true)
                    .resizable(true)
                    .sense(Sense::click())
                    .cell_layout(Layout::left_to_right(Align::Center))
                    .column(Column::exact(38.0))
                    .column(Column::exact(70.0))
                    .column(Column::remainder().at_least(100.0))
                    .min_scrolled_height(table_height)
                    .max_scroll_height(table_height)
                    .auto_shrink([false, false]);
                table
                    .header(20.0, |mut header| {
                        for title in ["#", "Length", "Data"] {
                            header.col(|ui| {
                                ui.label(RichText::new(title).strong().small().color(theme::TEXT));
                            });
                        }
                    })
                    .body(|body| {
                        body.rows(ITEM_TABLE_ROW_HEIGHT, self.items.len(), |mut row| {
                            let index = row.index();
                            row.set_selected(self.selected[which] == Some(index));
                            row.col(|ui| {
                                ui.label(
                                    RichText::new((index + 1).to_string())
                                        .small()
                                        .color(theme::TEXT),
                                );
                            });
                            row.col(|ui| {
                                ui.label(
                                    RichText::new(view::human_bytes(
                                        self.items[index].data.len() as u64
                                    ))
                                    .small()
                                    .color(theme::TEXT),
                                );
                            });
                            let preview = self.items[index].preview();
                            row.col(|ui| {
                                ui.add(
                                    Label::new(RichText::new(preview).small().color(theme::TEXT))
                                        .truncate(),
                                );
                            });
                            if row.response().clicked() {
                                clicked = Some(index);
                            }
                        });
                    });
            });
        if let Some(index) = clicked {
            self.selected[which] = Some(index);
            self.active = which;
        }
    }

    fn controls(&mut self, ui: &mut egui::Ui, height: f32) {
        let mut action = None;
        let can_remove = self.selected[self.active].is_some();
        let can_compare = self.selected[0].is_some() && self.selected[1].is_some();
        ui.allocate_ui_with_layout(
            Vec2::new(CONTROLS_WIDTH, height),
            Layout::top_down(Align::Center),
            |ui| {
                let button_width = CONTROLS_WIDTH - 4.0;
                if ui
                    .add_sized([button_width, 24.0], Button::new("Paste"))
                    .on_hover_text("Paste text from the clipboard as a new item")
                    .clicked()
                {
                    action = Some(ControlAction::Paste);
                }
                if ui
                    .add_sized([button_width, 24.0], Button::new("Load"))
                    .on_hover_text("Load a message from a file")
                    .clicked()
                {
                    action = Some(ControlAction::Load);
                }
                if ui
                    .add_enabled(
                        can_remove,
                        Button::new("Remove").min_size(Vec2::new(button_width, 24.0)),
                    )
                    .on_hover_text("Remove the item selected in the active table")
                    .on_disabled_hover_text("Select an item to remove")
                    .clicked()
                {
                    action = Some(ControlAction::Remove);
                }
                if ui
                    .add_enabled(
                        !self.items.is_empty(),
                        Button::new("Clear").min_size(Vec2::new(button_width, 24.0)),
                    )
                    .on_hover_text("Remove every item")
                    .on_disabled_hover_text("The item list is already empty")
                    .clicked()
                {
                    action = Some(ControlAction::Clear);
                }

                let compare_space = (height - 230.0).max(18.0);
                ui.add_space(compare_space);
                ui.separator();
                ui.add_space(4.0);
                ui.label(
                    RichText::new("Compare selected")
                        .small()
                        .color(theme::TEXT_MUTED),
                );
                ui.add_space(3.0);
                if ui
                    .add_enabled(
                        can_compare,
                        Button::new("Words").min_size(Vec2::new(button_width, 24.0)),
                    )
                    .on_hover_text(format!(
                        "Compare whitespace-delimited words (up to {} per item)",
                        view::human_bytes(engine::MAX_WORD_COMPARE_BYTES as u64)
                    ))
                    .on_disabled_hover_text("Select an item in both tables")
                    .clicked()
                {
                    action = Some(ControlAction::Compare(CompareMode::Words));
                }
                if ui
                    .add_enabled(
                        can_compare,
                        Button::new("Bytes").min_size(Vec2::new(button_width, 24.0)),
                    )
                    .on_hover_text(format!(
                        "Compare raw bytes (up to {} per item)",
                        view::human_bytes(engine::MAX_BYTE_COMPARE_BYTES as u64)
                    ))
                    .on_disabled_hover_text("Select an item in both tables")
                    .clicked()
                {
                    action = Some(ControlAction::Compare(CompareMode::Bytes));
                }
            },
        );

        match action {
            Some(ControlAction::Paste) => self.request_paste(ui.ctx()),
            Some(ControlAction::Load) => {
                self.load = Some(LoadDialog {
                    target: self.active,
                    path: String::new(),
                    error: None,
                });
            }
            Some(ControlAction::Remove) => self.remove_active(),
            Some(ControlAction::Clear) => self.clear_items(),
            Some(ControlAction::Compare(mode)) => self.compare(mode),
            None => {}
        }
    }

    fn request_paste(&mut self, ctx: &egui::Context) {
        self.paste = Some(PasteState {
            target: self.active,
            frames_left: 3,
        });
        ctx.memory_mut(|memory| memory.stop_text_input());
        ctx.send_viewport_cmd(egui::ViewportCommand::RequestPaste);
        ctx.request_repaint();
    }

    fn insert_item(&mut self, data: Vec<u8>, target: usize) -> Result<usize, String> {
        if data.len() > MAX_ITEM_BYTES {
            return Err(format!(
                "item is {}; the Comparer accepts items up to {}",
                view::human_bytes(data.len() as u64),
                view::human_bytes(MAX_ITEM_BYTES as u64)
            ));
        }
        if self.items.len() >= MAX_ITEMS {
            return Err(format!(
                "the Comparer holds at most {MAX_ITEMS} items; remove one before adding another"
            ));
        }
        self.items.push(ComparableItem::new(data));
        let index = self.items.len() - 1;
        self.selected[target] = Some(index);
        self.active = target;
        self.feedback = None;
        Ok(index)
    }

    fn remove_active(&mut self) {
        let Some(index) = self.selected[self.active] else {
            return;
        };
        if index >= self.items.len() {
            self.selected = [None, None];
            return;
        }
        self.items.remove(index);
        for selected in &mut self.selected {
            *selected = match *selected {
                Some(value) if value == index => None,
                Some(value) if value > index => Some(value - 1),
                other => other,
            };
        }
    }

    fn clear_items(&mut self) {
        self.items.clear();
        self.selected = [None, None];
        self.result = None;
        self.result_open = false;
        self.feedback = None;
    }

    fn compare(&mut self, mode: CompareMode) {
        let (Some(old_index), Some(new_index)) = (self.selected[0], self.selected[1]) else {
            self.feedback = Some(Feedback {
                error: true,
                text: "select one item in each table before comparing".to_string(),
            });
            return;
        };
        let (Some(old), Some(new)) = (self.items.get(old_index), self.items.get(new_index)) else {
            self.selected = [None, None];
            return;
        };
        match engine::compare(mode, &old.data, &new.data) {
            Ok(comparison) => {
                let result = CompareResult::new(
                    mode,
                    old_index,
                    new_index,
                    old.data.clone(),
                    new.data.clone(),
                    comparison,
                );
                self.result = Some(result);
                self.result_open = true;
                self.diff_cursor = 0;
                self.result_views = [ResultView::Text, ResultView::Text];
                self.result_scroll = [Vec2::ZERO, Vec2::ZERO];
                self.result_pane_rects = [None, None];
                self.feedback = None;
            }
            Err(error) => {
                self.feedback = Some(Feedback {
                    error: true,
                    text: error.to_string(),
                });
            }
        }
    }

    pub(super) fn result_window(&mut self, ctx: &egui::Context) {
        if !self.result_open {
            return;
        }
        if ctx.input(|input| input.key_pressed(Key::Escape)) {
            self.result_open = false;
            self.result = None;
            return;
        }
        let Some(mut result) = self.result.take() else {
            self.result_open = false;
            return;
        };

        let mut open = true;
        let title = result.title.clone();
        egui::Window::new(title)
            .id(egui::Id::unique(RESULT_WINDOW_ID))
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_pos(egui::pos2(60.0, 78.0))
            .default_size(egui::vec2(1160.0, 640.0))
            .min_width(760.0)
            .min_height(380.0)
            .show(ctx, |ui| self.draw_result(ui, &mut result));

        if open {
            self.result = Some(result);
        } else {
            self.result_open = false;
        }
    }

    fn draw_result(&mut self, ui: &mut egui::Ui, result: &mut CompareResult) {
        let font = result_font(ui);
        let row_height = ui.fonts_mut(|fonts| fonts.row_height(&font)).max(1.0);
        self.result_row_height = row_height;

        self.handle_result_wheel(ui);
        let before = self.result_scroll;
        let legend_height = 28.0;
        let pane_height = (ui.available_height() - legend_height).max(160.0);
        let gap = 8.0;
        let pane_width = ((ui.available_width() - gap) * 0.5).max(260.0);
        let mut navigation = 0;
        ui.horizontal_top(|ui| {
            navigation = self.result_pane(ui, 0, result, pane_width, pane_height, &font);
            ui.add_space(gap);
            let right = self.result_pane(ui, 1, result, pane_width, pane_height, &font);
            if navigation == 0 {
                navigation = right;
            }
        });
        if self.sync_views {
            let left_changed = (self.result_scroll[0].y - before[0].y).abs() > 0.5;
            let right_changed = (self.result_scroll[1].y - before[1].y).abs() > 0.5;
            if left_changed && !right_changed {
                self.result_scroll[1].y = self.result_scroll[0].y;
            } else if right_changed && !left_changed {
                self.result_scroll[0].y = self.result_scroll[1].y;
            }
        }

        ui.add_space(2.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("Key:").small().color(theme::TEXT_MUTED));
            legend_chip(ui, "Modified", MODIFIED_BG);
            legend_chip(ui, "Deleted", DELETED_BG);
            legend_chip(ui, "Added", ADDED_BG);
            ui.add_space(10.0);
            ui.label(
                RichText::new(format!(
                    "{} / {} differences",
                    self.diff_cursor.saturating_add(1).min(result.differences),
                    result.differences
                ))
                .small()
                .color(theme::TEXT_MUTED),
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let mut sync = self.sync_views;
                if ui.checkbox(&mut sync, "Sync views").changed() {
                    self.sync_views = sync;
                    if sync {
                        self.result_scroll[1].y = self.result_scroll[0].y;
                    }
                }
            });
        });

        if navigation != 0 {
            self.navigate_result(result, navigation, row_height);
        }
    }

    fn result_pane(
        &mut self,
        ui: &mut egui::Ui,
        side: usize,
        result: &CompareResult,
        width: f32,
        height: f32,
        font: &FontId,
    ) -> i32 {
        let mut view = self.result_views[side];
        let mut toggle_sync = false;
        let mut navigation = 0;
        ui.allocate_ui_with_layout(
            Vec2::new(width, height),
            Layout::top_down(Align::Min),
            |ui| {
                ui.horizontal(|ui| {
                    if widgets::section_tab(ui, view == ResultView::Text, "Text").clicked() {
                        view = ResultView::Text;
                    }
                    if widgets::section_tab(ui, view == ResultView::Hex, "Hex").clicked() {
                        view = ResultView::Hex;
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let response = ui
                            .add(Button::new("Sync").small())
                            .on_hover_text("Sync scrolling between the two views");
                        if response.clicked() {
                            toggle_sync = true;
                        }
                        ui.label(
                            RichText::new(result.side_label(side))
                                .small()
                                .color(theme::TEXT_MUTED),
                        );
                    });
                });
                let footer_height = 25.0;
                let body_height = (ui.available_height() - footer_height).max(80.0);
                let body_response = egui::Frame::NONE
                    .fill(theme::SURFACE)
                    .stroke(Stroke::new(1.0, theme::BORDER))
                    .corner_radius(3.0)
                    .inner_margin(Margin::same(2))
                    .show(ui, |ui| {
                        ui.set_min_size(Vec2::new(ui.available_width(), body_height - 4.0));
                        self.result_body(ui, side, result, view, body_height - 4.0, font);
                    })
                    .response;
                self.result_pane_rects[side] = Some(body_response.rect);

                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format!("Length: {}", result.display(side).raw_len()))
                            .small()
                            .color(theme::TEXT),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let can_move = result.differences > 0;
                        if ui
                            .add_enabled(can_move, Button::new(">").small())
                            .on_hover_text("Next difference")
                            .on_disabled_hover_text("The items are identical")
                            .clicked()
                        {
                            navigation = 1;
                        }
                        if ui
                            .add_enabled(can_move, Button::new("<").small())
                            .on_hover_text("Previous difference")
                            .on_disabled_hover_text("The items are identical")
                            .clicked()
                        {
                            navigation = -1;
                        }
                    });
                });
            },
        );

        if self.result_views[side] != view {
            self.result_views[side] = view;
            self.result_scroll[side] = Vec2::ZERO;
        }
        if toggle_sync {
            self.sync_views = !self.sync_views;
            if self.sync_views {
                self.result_scroll[1].y = self.result_scroll[0].y;
            }
        }
        navigation
    }

    fn result_body(
        &mut self,
        ui: &mut egui::Ui,
        side: usize,
        result: &CompareResult,
        view: ResultView,
        height: f32,
        font: &FontId,
    ) {
        let rows = match view {
            ResultView::Text => result.display(side).line_count(),
            ResultView::Hex => result.data(side).len().div_ceil(HEX_BYTES_PER_ROW).max(1),
        };
        let offset = self.result_scroll[side];
        let row_height = self.result_row_height;
        let id = egui::Id::unique(("perseid.comparer.result.view", side, view as u8));
        let output = ui
            .scope(|ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                ScrollArea::both()
                    .id_salt(id)
                    .scroll_source(egui::scroll_area::ScrollSource::SCROLL_BAR)
                    .auto_shrink([false, false])
                    .max_height(height)
                    .vertical_scroll_offset(offset.y)
                    .horizontal_scroll_offset(offset.x)
                    .show_rows(ui, row_height, rows, |ui, range| {
                        for row in range {
                            let job = match view {
                                ResultView::Text => {
                                    let line = result.display(side).line(row);
                                    text_line_job(
                                        line,
                                        result
                                            .text_highlights(side)
                                            .get(row)
                                            .map_or(&[][..], Vec::as_slice),
                                        font,
                                    )
                                }
                                ResultView::Hex => hex_line_job(
                                    result.data(side),
                                    result.byte_kinds(side),
                                    row,
                                    font,
                                ),
                            };
                            ui.add(
                                Label::new(job)
                                    .selectable(true)
                                    .wrap_mode(egui::TextWrapMode::Extend),
                            );
                        }
                    })
            })
            .inner;
        self.result_scroll[side] = output.state.clamped_offset();
    }

    fn handle_result_wheel(&mut self, ui: &egui::Ui) {
        let delta = ui.input(|input| input.smooth_scroll_delta);
        if delta == Vec2::ZERO {
            return;
        }
        let Some(pointer) = ui.input(|input| input.pointer.hover_pos()) else {
            return;
        };
        let Some(side) = self
            .result_pane_rects
            .iter()
            .position(|rect| rect.is_some_and(|rect| rect.contains(pointer)))
        else {
            return;
        };

        // Scroll areas use an inverted wheel delta. Keep scrollbars enabled so
        // dragging still works, but own the wheel so Sync views can mirror it.
        self.result_scroll[side].x = (self.result_scroll[side].x - delta.x).max(0.0);
        if self.sync_views {
            for scroll in &mut self.result_scroll {
                scroll.y = (scroll.y - delta.y).max(0.0);
            }
        } else {
            self.result_scroll[side].y = (self.result_scroll[side].y - delta.y).max(0.0);
        }
        ui.ctx()
            .input_mut(|input| input.smooth_scroll_delta = Vec2::ZERO);
        ui.ctx().request_repaint();
    }

    fn navigate_result(&mut self, result: &CompareResult, direction: i32, row_height: f32) {
        if result.spans.is_empty() {
            return;
        }
        let length = result.spans.len();
        if direction > 0 {
            self.diff_cursor = (self.diff_cursor + 1) % length;
        } else {
            self.diff_cursor = (self.diff_cursor + length - 1) % length;
        }
        let span = &result.spans[self.diff_cursor];
        self.result_scroll[0].y =
            visible_line(result.display(0), &span.old, self.result_views[0]) as f32 * row_height;
        self.result_scroll[1].y =
            visible_line(result.display(1), &span.new, self.result_views[1]) as f32 * row_height;
        self.result_scroll[0].y = (self.result_scroll[0].y - row_height * 2.0).max(0.0);
        self.result_scroll[1].y = (self.result_scroll[1].y - row_height * 2.0).max(0.0);
    }

    pub(super) fn load_window(&mut self, ctx: &egui::Context) {
        let Some(mut dialog) = self.load.take() else {
            return;
        };
        if ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, Key::Escape)) {
            return;
        }
        if let Some(path) = ctx.input(|input| {
            input
                .raw
                .dropped_files
                .iter()
                .map(|file| file.path().to_path_buf())
                .next()
        }) {
            dialog.path = path.display().to_string();
        }

        let mut open = true;
        let mut load = false;
        let mut cancel = false;
        egui::Window::new("Load item from file")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
            .default_width(520.0)
            .show(ctx, |ui| {
                ui.label(RichText::new("File path").small().color(theme::TEXT_MUTED));
                let response = ui.add(
                    TextEdit::singleline(&mut dialog.path)
                        .desired_width(f32::INFINITY)
                        .hint_text("/path/to/message"),
                );
                if response.lost_focus() && ui.input(|input| input.key_pressed(Key::Enter)) {
                    load = true;
                }
                if let Some(error) = &dialog.error {
                    ui.label(RichText::new(error).small().color(theme::RED));
                }
                ui.horizontal(|ui| {
                    if ui.button("Load").clicked() {
                        load = true;
                    }
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                    ui.label(
                        RichText::new("You can also drop a file on this window.")
                            .small()
                            .color(theme::TEXT_MUTED),
                    );
                });
            });
        if cancel || !open {
            return;
        }
        if load {
            match read_comparer_file(dialog.path.trim()) {
                Ok(data) => match self.insert_item(data, dialog.target) {
                    Ok(_) => return,
                    Err(error) => dialog.error = Some(error),
                },
                Err(error) => dialog.error = Some(error),
            }
        }
        self.load = Some(dialog);
    }

    #[cfg(feature = "screenshots")]
    pub(super) fn seed_screenshot(&mut self) {
        self.items.clear();
        self.items.push(ComparableItem::new(
            b"GET / HTTP/1.1\r\nHost: example.com\r\nUser-Agent: Mozilla/5.0 (X11; Linux x86_64; rv:153.0) Gecko/20100101 Firefox/153.0\r\nAccept: text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8\r\nAccept-Language: en-US,en;q=0.9\r\nAccept-Encoding: gzip, deflate, br\r\nUpgrade-Insecure-Requests: 1\r\nSec-Fetch-Dest: document\r\nSec-Fetch-Mode: navigate\r\nSec-Fetch-Site: none\r\nSec-Fetch-User: ?1\r\nIf-Modified-Since: Wed, 21 Oct 2026 19:59:02 GMT\r\nPriority: u=0, i\r\nTe: trailers\r\nConnection: keep-alive\r\n\r\n"
                .to_vec(),
        ));
        self.items.push(ComparableItem::new(
            b"HTTP/2 304 Not Modified\r\nDate: Thu, 22 Oct 2026 21:14:56 GMT\r\nAllow: GET, HEAD\r\nAge: 6562\r\nServer: cloudflare\r\nLast-Modified: Wed, 21 Oct 2026 19:59:02 GMT\r\nEtag: \"6ad91986-241\"\r\nCf-Cache-Status: HIT\r\nCf-Ray: a4eb83b53f65e3bd-LIS\r\nAlt-Svc: h3=\":443\"; ma=86400\r\n\r\n"
                .to_vec(),
        ));
        self.selected = [Some(0), Some(1)];
        self.active = 1;
        self.feedback = None;
        self.result = None;
        self.result_open = false;
        self.result_views = [ResultView::Text, ResultView::Text];
        self.sync_views = false;
        self.diff_cursor = 0;
        self.result_scroll = [Vec2::ZERO, Vec2::ZERO];
        self.result_pane_rects = [None, None];
    }

    #[cfg(feature = "screenshots")]
    pub(super) fn open_screenshot_result(&mut self) {
        self.compare(CompareMode::Words);
    }
}

#[derive(Clone, Copy)]
enum ControlAction {
    Paste,
    Load,
    Remove,
    Clear,
    Compare(CompareMode),
}

impl PerseidApp {
    pub(super) fn comparer_view(&mut self, ui: &mut egui::Ui) {
        self.comparer.ui(ui);
    }

    pub(super) fn comparer_windows(&mut self, ctx: &egui::Context) {
        self.comparer.result_window(ctx);
        self.comparer.load_window(ctx);
    }

    /// Add the selected history message to the shared Comparer collection.
    /// Requests target table 1 and responses target table 2, so the common
    /// request/response comparison is a two-click operation.
    pub(super) fn send_selected_to_comparer(&mut self, pane: MessagePane) {
        let Some(item) = self
            .selected
            .and_then(|id| self.history.get(id))
            .filter(|item| !item.is_tunnel())
        else {
            return;
        };
        let target = match pane {
            MessagePane::Request => 0,
            MessagePane::Response => 1,
        };
        let raw = match pane {
            MessagePane::Request => view::request_views(item).raw,
            MessagePane::Response => {
                if item.response.is_none() {
                    return;
                }
                view::response_views(item).raw
            }
        };
        if let Err(error) = self.comparer.insert_item(raw.into_bytes(), target) {
            self.comparer.feedback = Some(Feedback {
                error: true,
                text: error,
            });
        }
    }
}

fn read_comparer_file(path: &str) -> Result<Vec<u8>, String> {
    if path.is_empty() {
        return Err("enter a file path".to_string());
    }
    let path = PathBuf::from(path);
    let metadata = std::fs::metadata(&path)
        .map_err(|error| format!("could not read {}: {error}", path.display()))?;
    if metadata.len() > MAX_ITEM_BYTES as u64 {
        return Err(format!(
            "file is {}; the Comparer accepts items up to {}",
            view::human_bytes(metadata.len()),
            view::human_bytes(MAX_ITEM_BYTES as u64)
        ));
    }
    std::fs::read(&path).map_err(|error| format!("could not read {}: {error}", path.display()))
}

fn build_text_highlights(
    display: &DisplayText,
    spans: &[DiffSpan],
    side: usize,
) -> Vec<Vec<Highlight>> {
    let mut lines = vec![Vec::new(); display.line_count()];
    for span in spans {
        let raw = if side == 0 { &span.old } else { &span.new };
        if raw.is_empty() {
            continue;
        }
        let highlighted = display.display_range(raw.clone());
        if highlighted.is_empty() {
            continue;
        }
        let first_line = display.line_for_display(highlighted.start);
        let last_line = display.line_for_display(highlighted.end.saturating_sub(1));
        for (line, line_highlights) in lines
            .iter_mut()
            .enumerate()
            .take(last_line + 1)
            .skip(first_line)
        {
            let line_range = display.line_range(line);
            let start = highlighted.start.max(line_range.start);
            let end = highlighted.end.min(line_range.end);
            if start < end {
                line_highlights.push(Highlight {
                    range: start - line_range.start..end - line_range.start,
                    kind: span.kind,
                });
            }
        }
    }
    for line in &mut lines {
        line.sort_by_key(|highlight| highlight.range.start);
    }
    lines
}

fn build_byte_kinds(length: usize, spans: &[DiffSpan], side: usize) -> Vec<Option<ChangeKind>> {
    let mut kinds = vec![None; length];
    for span in spans {
        let raw = if side == 0 { &span.old } else { &span.new };
        if raw.start < raw.end && raw.end <= length {
            kinds[raw.clone()].fill(Some(span.kind));
        }
    }
    kinds
}

fn visible_line(display: &DisplayText, raw: &Range<usize>, view: ResultView) -> usize {
    match view {
        ResultView::Text => display.line_for_raw(raw.start),
        ResultView::Hex => raw.start / HEX_BYTES_PER_ROW,
    }
}

fn result_font(ui: &egui::Ui) -> FontId {
    ui.style()
        .text_styles
        .get(&TextStyle::Monospace)
        .cloned()
        .unwrap_or_else(|| FontId::monospace(11.5))
}

fn format(font: &FontId, color: Color32, background: Color32) -> TextFormat {
    TextFormat {
        font_id: font.clone(),
        color,
        background,
        ..Default::default()
    }
}

fn change_background(kind: ChangeKind) -> Color32 {
    match kind {
        ChangeKind::Modified => MODIFIED_BG,
        ChangeKind::Deleted => DELETED_BG,
        ChangeKind::Added => ADDED_BG,
    }
}

fn text_line_job(line: &str, highlights: &[Highlight], font: &FontId) -> LayoutJob {
    let plain = format(font, theme::TEXT, Color32::TRANSPARENT);
    let mut job = LayoutJob::default();
    job.wrap.max_width = f32::INFINITY;
    job.keep_trailing_whitespace = true;
    let mut cursor = 0;
    for highlight in highlights {
        let start = highlight.range.start.min(line.len()).max(cursor);
        let end = highlight.range.end.min(line.len());
        if start > cursor {
            job.append(&line[cursor..start], 0.0, plain.clone());
        }
        if end > start {
            job.append(
                &line[start..end],
                0.0,
                format(font, theme::TEXT, change_background(highlight.kind)),
            );
        }
        cursor = cursor.max(end);
    }
    if cursor < line.len() {
        job.append(&line[cursor..], 0.0, plain.clone());
    }
    if line.is_empty() {
        job.append(" ", 0.0, plain);
    }
    job
}

fn hex_line_job(data: &[u8], kinds: &[Option<ChangeKind>], row: usize, font: &FontId) -> LayoutJob {
    let plain = format(font, theme::TEXT, Color32::TRANSPARENT);
    let muted = format(font, theme::TEXT_MUTED, Color32::TRANSPARENT);
    let start = row * HEX_BYTES_PER_ROW;
    let mut job = LayoutJob::default();
    job.wrap.max_width = f32::INFINITY;
    job.keep_trailing_whitespace = true;
    job.append(&format!("{start:08x}  "), 0.0, muted.clone());
    for column in 0..HEX_BYTES_PER_ROW {
        let index = start + column;
        let byte_format = kinds.get(index).and_then(|kind| *kind).map_or_else(
            || plain.clone(),
            |kind| format(font, theme::TEXT, change_background(kind)),
        );
        if let Some(byte) = data.get(index) {
            job.append(&format!("{byte:02x} "), 0.0, byte_format);
        } else {
            job.append("   ", 0.0, plain.clone());
        }
        if column == 7 {
            job.append(" ", 0.0, plain.clone());
        }
    }
    job.append(" |", 0.0, muted.clone());
    for column in 0..HEX_BYTES_PER_ROW {
        let index = start + column;
        let byte_format = kinds.get(index).and_then(|kind| *kind).map_or_else(
            || plain.clone(),
            |kind| format(font, theme::TEXT, change_background(kind)),
        );
        if let Some(byte) = data.get(index) {
            let character = if byte.is_ascii_graphic() || *byte == b' ' {
                char::from(*byte)
            } else {
                '.'
            };
            job.append(&character.to_string(), 0.0, byte_format);
        } else {
            job.append(" ", 0.0, plain.clone());
        }
    }
    job.append("|", 0.0, muted);
    job
}

fn legend_chip(ui: &mut egui::Ui, label: &str, color: Color32) {
    egui::Frame::NONE
        .fill(color)
        .corner_radius(2.0)
        .inner_margin(Margin::symmetric(4, 1))
        .show(ui, |ui| {
            ui.label(RichText::new(label).small().color(theme::TEXT));
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn result(mode: CompareMode, old: &[u8], new: &[u8]) -> CompareResult {
        let comparison = engine::compare(mode, old, new).unwrap();
        CompareResult::new(mode, 2, 4, old.to_vec(), new.to_vec(), comparison)
    }

    #[test]
    fn word_result_highlights_only_the_changed_word() {
        let result = result(CompareMode::Words, b"GET / HTTP/1.1", b"POST / HTTP/1.1");
        assert_eq!(result.differences, 1);
        let line = &result.text_highlights[0][0];
        assert_eq!(line.len(), 1);
        assert_eq!(&result.old_text.as_str()[line[0].range.clone()], "GET");
        let line = &result.text_highlights[1][0];
        assert_eq!(&result.new_text.as_str()[line[0].range.clone()], "POST");
    }

    #[test]
    fn byte_result_highlights_hex_bytes_and_text() {
        let result = result(CompareMode::Bytes, b"cat", b"cut");
        assert_eq!(result.byte_kinds[0][1], Some(ChangeKind::Modified));
        assert_eq!(result.byte_kinds[1][1], Some(ChangeKind::Modified));
        let line = &result.text_highlights[0][0];
        assert_eq!(&result.old_text.as_str()[line[0].range.clone()], "a");
    }

    #[test]
    fn result_titles_use_one_based_row_numbers() {
        let result = result(CompareMode::Words, b"a", b"b");
        assert_eq!(result.title, "Word compare of #3 and #5 (1 difference)");
    }

    #[test]
    fn removing_an_item_adjusts_both_selections() {
        let mut state = ComparerState::new();
        state.insert_item(b"one".to_vec(), 0).unwrap();
        state.insert_item(b"two".to_vec(), 1).unwrap();
        state.insert_item(b"three".to_vec(), 0).unwrap();
        assert_eq!(state.selected, [Some(2), Some(1)]);
        state.active = 1;
        state.remove_active();
        assert_eq!(state.selected, [Some(1), None]);
        state.active = 0;
        state.remove_active();
        assert_eq!(state.selected, [None, None]);
        assert_eq!(state.items.len(), 1);
    }

    #[test]
    fn clearing_items_closes_the_result() {
        let mut state = ComparerState::new();
        state.insert_item(b"one".to_vec(), 0).unwrap();
        state.insert_item(b"two".to_vec(), 1).unwrap();
        state.compare(CompareMode::Words);
        assert!(state.result_open);
        state.clear_items();
        assert!(!state.result_open);
        assert!(state.result.is_none());
    }

    #[test]
    fn previews_stay_single_line_and_bounded() {
        let item = ComparableItem::new(b"a\r\nb\x00c".to_vec());
        assert_eq!(item.preview(), "a  b·c");
    }
}
