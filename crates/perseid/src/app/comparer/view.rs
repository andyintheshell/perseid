//! Comparer rendering: the item tables, controls, and result window.

use egui::text::LayoutJob;
use egui::{
    Align, Align2, Button, Color32, FontId, Key, Label, Layout, Margin, RichText, ScrollArea,
    Sense, Stroke, TextEdit, TextFormat, TextStyle, Vec2,
};
use egui_extras::{Column, TableBuilder};

use super::highlight::{
    ADDED_BG, CompareResult, DELETED_BG, HEX_BYTES_PER_ROW, Highlight, MODIFIED_BG,
    change_background,
};
#[cfg(feature = "screenshots")]
use super::state::ComparableItem;
use super::state::{
    ComparerState, Feedback, LoadDialog, PasteState, ResultView, read_comparer_file,
};
use crate::app::widgets;
use crate::comparer as engine;
use crate::comparer::{ChangeKind, CompareMode};
use crate::theme;
use crate::view;

const ITEM_TABLE_ROW_HEIGHT: f32 = 19.0;
const CONTROLS_WIDTH: f32 = 108.0;
const RESULT_WINDOW_ID: &str = "perseid.comparer.result";

#[derive(Clone, Copy)]
enum ControlAction {
    Paste,
    Load,
    Remove,
    Clear,
    Compare(CompareMode),
}

impl ComparerState {
    pub(super) fn ui(&mut self, ui: &mut egui::Ui) {
        widgets::page_pad(ui, |ui| self.page(ui));
    }

    /// Consume a clipboard paste requested by the Paste button.
    pub(in crate::app) fn handle_input(&mut self, ctx: &egui::Context) {
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
        let title = result.title().to_string();
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
                    self.diff_cursor.saturating_add(1).min(result.differences()),
                    result.differences()
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
                        let can_move = result.differences() > 0;
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
    pub(in crate::app) fn seed_screenshot(&mut self) {
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
    pub(in crate::app) fn open_screenshot_result(&mut self) {
        self.compare(CompareMode::Words);
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
    use crate::comparer::ChangeKind;

    #[test]
    fn text_line_job_preserves_text_and_highlights() {
        let font = FontId::monospace(11.5);
        let plain = text_line_job("abcd", &[], &font);
        assert_eq!(plain.text, "abcd");

        let highlights = [Highlight {
            range: 1..3,
            kind: ChangeKind::Modified,
        }];
        let job = text_line_job("abcd", &highlights, &font);
        assert_eq!(job.text, "abcd");
        assert_eq!(job.sections.len(), 3);
        assert_eq!(job.sections[1].format.background, MODIFIED_BG);

        // An empty line still gets one space so the row keeps its height.
        assert_eq!(text_line_job("", &[], &font).text, " ");
    }

    #[test]
    fn hex_line_job_formats_full_and_partial_rows() {
        let font = FontId::monospace(11.5);
        let kinds = [
            None,
            Some(ChangeKind::Added),
            None,
            None,
            None,
            None,
            None,
            None,
        ];
        let job = hex_line_job(b"hello", &kinds, 0, &font);
        assert!(job.text.starts_with("00000000  68 65 6c 6c 6f"));
        assert!(job.text.contains("|hello"));
        assert!(
            job.sections
                .iter()
                .any(|section| section.format.background == ADDED_BG)
        );

        let job = hex_line_job(b"hello", &kinds, 1, &font);
        assert!(job.text.starts_with("00000010"));
        assert!(job.text.ends_with('|'));
    }
}
