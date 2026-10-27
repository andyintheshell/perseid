//! Wrapped, selectable message viewers with a painted line-number gutter.
//!
//! Pretty and Raw use egui's multiline editor so wrapping, selection, keyboard
//! navigation, and clipboard behavior come from the widget rather than a
//! bespoke text layout. Interception makes both editable; history and Hex
//! remain read-only. Hex uses a virtualized row list because a large dump is
//! too expensive to lay out as one galley.

use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::sync::Arc;

use egui::{Align2, FontId, RichText, Sense};

use super::widgets::message_mode_tab;
use super::*;
use crate::highlight;
use crate::history::HistoryItem;

pub(super) fn message_toolbar(
    ui: &mut egui::Ui,
    pane: MessagePane,
    mode: &mut MessageView,
    item: Option<&HistoryItem>,
) {
    ui.horizontal(|ui| {
        let title = match pane {
            MessagePane::Request => "Request",
            MessagePane::Response => "Response",
        };
        ui.label(RichText::new(title).strong().color(theme::TEXT));
        if let Some(item) = item {
            match pane {
                MessagePane::Request => {
                    ui.label(
                        RichText::new(&item.request.method)
                            .small()
                            .color(theme::TEXT_MUTED),
                    );
                }
                MessagePane::Response => {
                    ui.colored_label(view::status_color(item), view::status_text(item));
                }
            }
            ui.with_layout(
                egui::Layout::right_to_left(egui::Align::Center),
                |ui| match pane {
                    MessagePane::Request => {
                        ui.label(
                            RichText::new(format!("#{}", item.number))
                                .small()
                                .color(theme::TEXT_MUTED),
                        );
                    }
                    MessagePane::Response => {
                        if let Some(duration) = item.duration() {
                            ui.label(
                                RichText::new(view::format_duration(duration))
                                    .small()
                                    .color(theme::TEXT_MUTED),
                            );
                        }
                        ui.label(
                            RichText::new(view::human_bytes(item.transferred()))
                                .small()
                                .color(theme::TEXT_MUTED),
                        );
                    }
                },
            );
        }
    });
    ui.add_space(1.0);
    message_tabs(ui, mode);
    ui.add_space(1.0);
}

/// Draw the Pretty/Raw/Hex row shared by the history detail panes and the
/// interception editor.
pub(super) fn message_tabs(ui: &mut egui::Ui, mode: &mut MessageView) {
    ui.horizontal(|ui| {
        if message_mode_tab(ui, *mode == MessageView::Pretty, "Pretty").clicked() {
            *mode = MessageView::Pretty;
        }
        if message_mode_tab(ui, *mode == MessageView::Raw, "Raw").clicked() {
            *mode = MessageView::Raw;
        }
        if message_mode_tab(ui, *mode == MessageView::Hex, "Hex").clicked() {
            *mode = MessageView::Hex;
        }
    });
}

pub(super) fn message_view_text(views: &MessageViews, mode: MessageView) -> &str {
    match mode {
        MessageView::Pretty => &views.pretty,
        MessageView::Raw => &views.raw,
        MessageView::Hex => &views.hex,
    }
}

pub(super) fn message_text(
    ui: &mut egui::Ui,
    id: u64,
    pane: MessagePane,
    mode: MessageView,
    text: &str,
) {
    let copy_label = match pane {
        MessagePane::Request => "Copy request",
        MessagePane::Response => "Copy response",
    };
    message_text_labeled(ui, id, pane, mode, text, copy_label);
}

/// Render read-only message text with a custom context-menu copy label. Used
/// by the WebSocket detail pane, where "request"/"response" would be wrong.
pub(super) fn message_text_labeled(
    ui: &mut egui::Ui,
    id: u64,
    pane: MessagePane,
    mode: MessageView,
    text: &str,
    copy_label: &str,
) {
    let mut readonly = text;
    let _ = message_frame(
        ui,
        id,
        pane,
        mode,
        &mut readonly,
        MessageContext::new(copy_label),
    );
}

/// Render a captured HTTP request pane with the Copy as curl context action.
pub(super) fn request_message_text_with_curl(
    ui: &mut egui::Ui,
    id: u64,
    mode: MessageView,
    item: &HistoryItem,
    text: &str,
) {
    let mut readonly = text;
    let _ = message_frame(
        ui,
        id,
        MessagePane::Request,
        mode,
        &mut readonly,
        MessageContext {
            copy_label: "Copy request",
            matches: None,
            curl_item: Some(item),
        },
    );
}

/// Render read-only message text with search-match backgrounds.
pub(super) fn message_text_with_matches(
    ui: &mut egui::Ui,
    id: u64,
    pane: MessagePane,
    mode: MessageView,
    text: &str,
    matches: &[Range<usize>],
) {
    let copy_label = match pane {
        MessagePane::Request => "Copy request",
        MessagePane::Response => "Copy response",
    };
    let mut readonly = text;
    let _ = message_frame(
        ui,
        id,
        pane,
        mode,
        &mut readonly,
        MessageContext {
            copy_label,
            matches: Some(matches),
            curl_item: None,
        },
    );
}

/// Render the shared message frame with an editable text buffer.
///
/// Interception uses this for both Pretty and Raw. Hex remains a read-only
/// representation of the same draft.
pub(super) fn message_editor(
    ui: &mut egui::Ui,
    id: u64,
    pane: MessagePane,
    mode: MessageView,
    text: &mut String,
) -> bool {
    let copy_label = match pane {
        MessagePane::Request => "Copy request",
        MessagePane::Response => "Copy response",
    };
    message_frame(ui, id, pane, mode, text, MessageContext::new(copy_label))
}

struct MessageContext<'a> {
    copy_label: &'a str,
    matches: Option<&'a [Range<usize>]>,
    curl_item: Option<&'a HistoryItem>,
}

impl<'a> MessageContext<'a> {
    fn new(copy_label: &'a str) -> Self {
        Self {
            copy_label,
            matches: None,
            curl_item: None,
        }
    }
}

fn message_frame(
    ui: &mut egui::Ui,
    id: u64,
    pane: MessagePane,
    mode: MessageView,
    text: &mut dyn egui::TextBuffer,
    context: MessageContext<'_>,
) -> bool {
    let selection_key =
        ui.make_persistent_id(("perseid.message.selection", id, pane as u8, mode as u8));
    let secondary_pressed =
        ui.input(|input| input.pointer.button_pressed(egui::PointerButton::Secondary));
    let mut selection = None;
    let mut hex_selection = None;
    let mut changed = false;
    let frame = egui::Frame::NONE
        .fill(Color32::WHITE)
        .stroke(Stroke::new(1.0, theme::BORDER))
        .corner_radius(4.0)
        .inner_margin(egui::Margin::same(5))
        .show(ui, |ui| {
            ui.set_min_size(ui.available_size());
            if mode == MessageView::Hex {
                // A single text galley for a 64 KiB hex dump is surprisingly
                // expensive to lay out. Virtualize the lines so memory stays
                // proportional to the visible page rather than the whole dump.
                hex_selection = message_hex(ui, id, pane, mode, text.as_str())
                    .map(|bytes| format_hex_selection(&bytes));
            } else {
                let output = message_numbered(ui, id, pane, mode, text, context.matches);
                selection = output.selection;
                changed = output.changed;
            }
        });

    // egui's text editor collapses a selection when it is pressed or loses
    // focus (for example when a menu item is clicked). Remember the text that
    // was selected when the context menu opened so Copy selection still has
    // something to copy after that happens.
    if secondary_pressed {
        let selected = hex_selection
            .as_deref()
            .or_else(|| {
                selection
                    .map(|range| range.slice_str(text.as_str()))
                    .filter(|selected| !selected.is_empty())
            })
            .map(Arc::<str>::from);
        ui.data_mut(|data| {
            if let Some(selected) = selected {
                data.insert_temp(selection_key, selected);
            } else {
                data.remove::<Arc<str>>(selection_key);
            }
        });
    }
    let saved_selection = ui.data(|data| data.get_temp::<Arc<str>>(selection_key));
    frame.response.container_context_menu(|ui| {
        let selected = saved_selection
            .as_deref()
            .or(hex_selection.as_deref())
            .or_else(|| {
                selection
                    .map(|range| range.slice_str(text.as_str()))
                    .filter(|selected| !selected.is_empty())
            });
        // Keep the item in the menu even without a selection so the entries
        // never move around while the menu is open.
        if ui
            .add_enabled(selected.is_some(), egui::Button::new("Copy selection"))
            .clicked()
            && let Some(selected) = selected
        {
            ui.ctx().copy_text(selected.to_owned());
            ui.close();
        }
        if ui.button(context.copy_label).clicked() {
            ui.ctx().copy_text(text.as_str().to_owned());
            ui.close();
        }
        if let Some(item) = context.curl_item {
            super::curl::context_menu(ui, item);
        }
    });
    changed
}

/// Render a wrapped, selectable text editor with a painted line-number gutter.
///
/// The editor owns the text buffer, so wrapping, selection, copying, and
/// keyboard navigation all use egui's native text behavior. Line numbers stay
/// in a painted gutter outside the buffer: they align with the editor's galley
/// but can never be selected or copied.
struct NumberedOutput {
    selection: Option<egui::text::CCursorRange>,
    changed: bool,
}

fn message_numbered(
    ui: &mut egui::Ui,
    id: u64,
    pane: MessagePane,
    mode: MessageView,
    text: &mut dyn egui::TextBuffer,
    matches: Option<&[Range<usize>]>,
) -> NumberedOutput {
    let line_count = text.as_str().lines().count() + usize::from(text.as_str().ends_with('\n'));
    let font = message_font();
    let digits = line_count.max(1).to_string().len();
    let number_width = ui.fonts_mut(|fonts| {
        fonts
            .layout_no_wrap("0".repeat(digits), font.clone(), theme::GRAY)
            .size()
            .x
    });
    let gutter_width = number_width + 10.0;
    let available_height = ui.available_height();
    let min_height = if available_height.is_finite() {
        available_height.max(0.0)
    } else {
        0.0
    };
    let text_id = ui.make_persistent_id(("perseid.message.text", id, pane as u8, mode as u8));
    let frame = egui::Frame::NONE.inner_margin(egui::Margin {
        left: gutter_width.round().clamp(0.0, i8::MAX as f32) as i8,
        right: 0,
        top: 0,
        bottom: 0,
    });
    let mut layouter = |ui: &egui::Ui, buffer: &dyn egui::TextBuffer, wrap_width: f32| {
        let mut job = highlight::message_job(buffer.as_str(), wrap_width, &font);
        if let Some(matches) = matches {
            highlight::paint_matches(&mut job, matches, theme::SEARCH_MATCH);
        }
        ui.fonts_mut(|fonts| fonts.layout_job(job))
    };

    // A right-click is turned into a context menu by the message frame, but
    // egui's text editor treats any pointer press as the start of a selection.
    // Capture the selection before the editor clears it, then restore it so it
    // stays visible while the menu is open.
    let secondary_pressed =
        ui.input(|input| input.pointer.button_pressed(egui::PointerButton::Secondary));
    let previous_selection = if secondary_pressed {
        egui::text_edit::TextEditState::load(ui.ctx(), text_id)
            .and_then(|state| state.cursor.char_range())
            .filter(|range| !range.is_empty())
    } else {
        None
    };
    ui.spacing_mut().item_spacing.y = 0.0;
    egui::ScrollArea::vertical()
        .id_salt(("perseid.message", id, pane as u8, mode as u8))
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            let output = egui::TextEdit::multiline(text)
                .id(text_id)
                .font(egui::FontSelection::FontId(font.clone()))
                .frame(frame)
                .desired_width(f32::INFINITY)
                .min_size(egui::vec2(0.0, min_height))
                .layouter(&mut layouter)
                .show(ui);
            if let Some(range) = previous_selection {
                let mut state = output.state.clone();
                state.cursor.set_char_range(Some(range));
                state.store(ui.ctx(), text_id);
            }
            autoscroll_message_selection(ui, &output.response.response);
            paint_line_numbers(ui, &output.galley, output.galley_pos, &font);
            NumberedOutput {
                selection: previous_selection
                    .or(output.cursor_range.filter(|range| !range.is_empty())),
                changed: output.response.response.changed(),
            }
        })
        .inner
}

fn autoscroll_message_selection(ui: &egui::Ui, response: &egui::Response) {
    if !response.dragged() {
        return;
    }
    let Some(pointer) = ui.input(|input| input.pointer.interact_pos()) else {
        return;
    };
    let viewport = ui.clip_rect();
    const EDGE: f32 = 20.0;
    const SPEED: f32 = 12.0;
    let delta = if pointer.y < viewport.top() + EDGE {
        SPEED
    } else if pointer.y > viewport.bottom() - EDGE {
        -SPEED
    } else {
        return;
    };
    let content = response.rect;
    if (delta < 0.0 && content.bottom() <= viewport.bottom() + 0.5)
        || (delta > 0.0 && content.top() >= viewport.top() - 0.5)
    {
        return;
    }
    ui.scroll_with_delta_animation(egui::vec2(0.0, delta), egui::style::ScrollAnimation::none());
    ui.ctx().request_repaint();
}

fn paint_line_numbers(ui: &egui::Ui, galley: &egui::Galley, galley_pos: egui::Pos2, font: &FontId) {
    let clip = ui.clip_rect();
    let mut number = 1;
    for (index, row) in galley.rows.iter().enumerate() {
        let starts_line = index == 0 || galley.rows[index - 1].ends_with_newline;
        if !starts_line {
            continue;
        }
        let y = galley_pos.y + row.pos.y;
        if y + row.row.size.y >= clip.top() && y <= clip.bottom() {
            ui.painter().text(
                egui::pos2(galley_pos.x - 8.0, y),
                Align2::RIGHT_TOP,
                number.to_string(),
                font.clone(),
                theme::GRAY,
            );
        }
        number += 1;
    }
}

fn message_font() -> FontId {
    FontId::monospace(highlight::MESSAGE_FONT_SIZE)
}

/// Keep short messages filling the viewport so a selection can begin in the
/// blank area below the last line, just like in a regular text editor.
fn total_message_rows(ui: &egui::Ui, content_rows: usize, row_height: f32) -> usize {
    let available = ui.available_height();
    let viewport_rows = if available.is_finite() && row_height > 0.0 {
        (available / row_height).ceil().max(1.0) as usize
    } else {
        1
    };
    content_rows.max(viewport_rows)
}

/// Selection endpoints are byte offsets in the visible dump, inclusive.
#[derive(Clone, Default)]
struct HexSelection {
    fingerprint: u64,
    anchor: Option<usize>,
    focus: Option<usize>,
    dragging: bool,
}

impl HexSelection {
    fn range(&self) -> Option<Range<usize>> {
        let (a, b) = (self.anchor?, self.focus?);
        Some(a.min(b)..a.max(b) + 1)
    }
}

/// Extract only well-formed byte columns. Offset and ASCII columns are never
/// treated as data, and malformed or non-dump lines are ignored.
fn hex_row_bytes(line: &str, row: usize) -> Vec<u8> {
    let Some(offset) = line.get(..8) else {
        return Vec::new();
    };
    if usize::from_str_radix(offset, 16).ok() != Some(row * 16) || line.get(8..10) != Some("  ") {
        return Vec::new();
    }
    let mut bytes = Vec::new();
    for index in 0..16 {
        let start = 10 + index * 3 + usize::from(index >= 8);
        let Some(pair) = line.get(start..start + 2) else {
            break;
        };
        if pair == "  " {
            break;
        }
        let Ok(byte) = u8::from_str_radix(pair, 16) else {
            return Vec::new();
        };
        bytes.push(byte);
    }
    bytes
}

fn hex_column_byte(column: usize, count: usize) -> Option<usize> {
    let hex = (0..count).find(|&index| {
        let start = 10 + index * 3 + usize::from(index >= 8);
        (start..start + 2).contains(&column)
    });
    hex.or_else(|| (61..61 + count).contains(&column).then(|| column - 61))
}

fn selected_hex_bytes(lines: &[&str], range: Range<usize>) -> Vec<u8> {
    let mut selected = Vec::new();
    if range.is_empty() {
        return selected;
    }
    for row in range.start / 16..=(range.end - 1) / 16 {
        let Some(line) = lines.get(row) else { break };
        for (column, byte) in hex_row_bytes(line, row).into_iter().enumerate() {
            if range.contains(&(row * 16 + column)) {
                selected.push(byte);
            }
        }
    }
    selected
}

fn format_hex_selection(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Select whole bytes in either column; clipboard text is spaced hexadecimal.
pub(super) fn message_hex(
    ui: &mut egui::Ui,
    id: u64,
    pane: MessagePane,
    mode: MessageView,
    text: &str,
) -> Option<Vec<u8>> {
    let lines: Vec<&str> = text.lines().collect();
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    let fingerprint = hasher.finish();
    let selection_id = ui.make_persistent_id(("perseid.hex.selection", id, pane as u8, mode as u8));
    let mut selection = ui
        .data(|data| data.get_temp::<HexSelection>(selection_id))
        .unwrap_or_default();
    if selection.fingerprint != fingerprint {
        selection = HexSelection {
            fingerprint,
            ..Default::default()
        };
    }
    ui.spacing_mut().item_spacing.y = 0.0;
    let font = message_font();
    let row_height = ui.fonts_mut(|fonts| fonts.row_height(&font));
    let char_width = ui.fonts_mut(|fonts| {
        fonts
            .layout_no_wrap("0".into(), font.clone(), theme::TEXT)
            .size()
            .x
    });
    let total_rows = total_message_rows(ui, lines.len(), row_height);
    let line_id_base = ui.make_persistent_id(("perseid.hex.line", id, pane as u8, mode as u8));
    let focus_id = selection_id.with("focus");
    ui.interact(
        egui::Rect::NOTHING,
        focus_id,
        Sense::focusable_noninteractive(),
    );
    let (pressed, down, released, pointer) = ui.input(|input| {
        (
            input.pointer.primary_pressed(),
            input.pointer.primary_down(),
            input.pointer.primary_released(),
            input.pointer.interact_pos(),
        )
    });

    egui::ScrollArea::both()
        .id_salt(("perseid.hex", id, pane as u8, mode as u8))
        .auto_shrink([false, false])
        .show_rows(ui, row_height, total_rows, |ui, range| {
            ui.spacing_mut().item_spacing.y = 0.0;
            for index in range {
                let line = lines.get(index).copied().unwrap_or_default();
                let bytes = hex_row_bytes(line, index);
                let galley = ui.fonts_mut(|fonts| {
                    fonts.layout_no_wrap(line.to_owned(), font.clone(), theme::TEXT)
                });
                let available_width = ui.available_width();
                let width = galley.size().x.max(if available_width.is_finite() {
                    available_width
                } else {
                    0.0
                });
                let (_, rect) = ui.allocate_space(egui::vec2(width, row_height));
                ui.interact(
                    rect,
                    line_id_base.with(index),
                    Sense::click_and_drag() - Sense::FOCUSABLE,
                );
                let hit_pos = pointer.map(|pos| {
                    if selection.dragging && down && ui.clip_rect().height() > 1.0 {
                        egui::pos2(
                            pos.x,
                            pos.y
                                .clamp(ui.clip_rect().top() + 0.5, ui.clip_rect().bottom() - 0.5),
                        )
                    } else {
                        pos
                    }
                });
                if let Some(pos) =
                    hit_pos.filter(|pos| rect.contains(*pos) && ui.clip_rect().contains(*pos))
                {
                    let column = ((pos.x - rect.left()) / char_width).floor().max(0.0) as usize;
                    if let Some(byte) = hex_column_byte(column, bytes.len()) {
                        let offset = index * 16 + byte;
                        if pressed {
                            selection.anchor = Some(offset);
                            selection.focus = Some(offset);
                            selection.dragging = true;
                            ui.memory_mut(|memory| memory.request_focus(focus_id));
                        } else if down && selection.dragging {
                            selection.focus = Some(offset);
                        }
                    } else if pressed {
                        selection.anchor = None;
                        selection.focus = None;
                        selection.dragging = false;
                    }
                }
                if ui.is_rect_visible(rect) {
                    if let Some(selected) = selection.range() {
                        for byte in 0..bytes.len() {
                            if selected.contains(&(index * 16 + byte)) {
                                for column in [10 + byte * 3 + usize::from(byte >= 8), 61 + byte] {
                                    let left = rect.left() + column as f32 * char_width;
                                    let width = if column < 61 {
                                        2.0 * char_width
                                    } else {
                                        char_width
                                    };
                                    ui.painter().rect_filled(
                                        egui::Rect::from_min_size(
                                            egui::pos2(left, rect.top()),
                                            egui::vec2(width, row_height),
                                        ),
                                        0.0,
                                        ui.visuals().selection.bg_fill,
                                    );
                                }
                            }
                        }
                    }
                    ui.painter().galley(rect.min, galley, theme::TEXT);
                }
            }
            if selection.dragging
                && down
                && let Some(pos) = pointer
            {
                let clip = ui.clip_rect();
                if pos.y < clip.top() || pos.y > clip.bottom() {
                    let delta = if pos.y < clip.top() { 18.0 } else { -18.0 };
                    ui.scroll_with_delta_animation(
                        egui::vec2(0.0, delta),
                        egui::style::ScrollAnimation::none(),
                    );
                    ui.ctx().request_repaint();
                }
            }
        });
    if released {
        selection.dragging = false;
    }
    let selected = selection
        .range()
        .map(|range| selected_hex_bytes(&lines, range))
        .filter(|bytes| !bytes.is_empty());
    if ui.input(|input| {
        input
            .events
            .iter()
            .any(|event| matches!(event, egui::Event::Copy))
            || (input.key_pressed(egui::Key::C) && input.modifiers.command)
    }) && ui.ctx().memory(|memory| memory.has_focus(focus_id))
        && let Some(value) = &selected
    {
        ui.ctx().copy_text(format_hex_selection(value));
    }
    ui.data_mut(|data| data.insert_temp(selection_id, selection));
    selected
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Context, Event, Key, Modifiers, PointerButton, Pos2, RawInput, Rect};

    fn button(pos: Pos2, pressed: bool) -> Event {
        Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        }
    }

    fn secondary_button(pos: Pos2, pressed: bool) -> Event {
        Event::PointerButton {
            pos,
            button: PointerButton::Secondary,
            pressed,
            modifiers: Modifiers::NONE,
        }
    }

    fn run_message_text_frame(ctx: &Context, events: Vec<Event>, text: &str) -> egui::FullOutput {
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(400.0, 300.0))),
            events,
            ..Default::default()
        };
        ctx.run_ui(input, |ui| {
            message_text(ui, 1, MessagePane::Request, MessageView::Raw, text);
        })
    }

    fn run_curl_request_frame(
        ctx: &Context,
        events: Vec<Event>,
        item: &HistoryItem,
    ) -> egui::FullOutput {
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(400.0, 300.0))),
            events,
            ..Default::default()
        };
        ctx.run_ui(input, |ui| {
            request_message_text_with_curl(
                ui,
                item.id,
                MessageView::Raw,
                item,
                "GET /path?q=1 HTTP/1.1\r\n\r\n",
            );
        })
    }

    fn run_message_frame(
        ctx: &Context,
        events: Vec<Event>,
        text: &str,
    ) -> Vec<egui::OutputCommand> {
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(400.0, 300.0))),
            events,
            ..Default::default()
        };
        let output = ctx.run_ui(input, |ui| {
            let mut readonly = text;
            let _ = message_numbered(
                ui,
                1,
                MessagePane::Request,
                MessageView::Raw,
                &mut readonly,
                None,
            );
        });
        let commands = output.platform_output.commands.clone();
        output.drop_without_applying_deltas();
        commands
    }

    fn settled_selections(commands: &[egui::OutputCommand]) -> Vec<&str> {
        commands
            .iter()
            .filter_map(|command| match command {
                egui::OutputCommand::TextSelectionSettled(text) => Some(text.as_str()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn message_selection_autoscrolls_past_the_viewport() {
        let text = (0..60)
            .map(|index| format!("line-{index:03}"))
            .collect::<Vec<_>>()
            .join("\n");
        let ctx = Context::default();
        let start = egui::pos2(20.0, 5.0);
        let below_viewport = egui::pos2(40.0, 310.0);
        let mut commands = run_message_frame(&ctx, vec![], &text);

        commands.extend(run_message_frame(
            &ctx,
            vec![Event::PointerMoved(start)],
            &text,
        ));
        commands.extend(run_message_frame(&ctx, vec![button(start, true)], &text));
        commands.extend(run_message_frame(
            &ctx,
            vec![Event::PointerMoved(below_viewport)],
            &text,
        ));
        for _ in 0..60 {
            commands.extend(run_message_frame(&ctx, vec![], &text));
        }
        commands.extend(run_message_frame(
            &ctx,
            vec![button(below_viewport, false)],
            &text,
        ));
        commands.extend(run_message_frame(&ctx, vec![], &text));

        let selected = settled_selections(&commands)
            .into_iter()
            .next()
            .expect("selection should settle");
        assert!(
            selected.starts_with("line-000"),
            "selection should start at the first line without the gutter: {selected:?}"
        );
        assert!(
            selected.contains("line-050"),
            "dragging below the viewport should keep scrolling: {selected:?}"
        );
    }

    #[test]
    fn message_selection_can_start_in_blank_space() {
        let text = "line-000\nline-001\nline-002";
        let ctx = Context::default();
        let blank = egui::pos2(40.0, 250.0);
        let first_line = egui::pos2(20.0, 5.0);
        let mut commands = run_message_frame(&ctx, vec![], text);

        commands.extend(run_message_frame(
            &ctx,
            vec![Event::PointerMoved(blank)],
            text,
        ));
        commands.extend(run_message_frame(&ctx, vec![button(blank, true)], text));
        commands.extend(run_message_frame(
            &ctx,
            vec![Event::PointerMoved(first_line)],
            text,
        ));
        commands.extend(run_message_frame(
            &ctx,
            vec![button(first_line, false)],
            text,
        ));
        commands.extend(run_message_frame(&ctx, vec![], text));

        let selected = settled_selections(&commands)
            .into_iter()
            .next()
            .expect("selection should settle");
        assert!(
            selected.starts_with("line-000"),
            "a drag from the blank area should select the message: {selected:?}"
        );
        assert!(selected.contains("line-002"));
    }

    #[test]
    fn message_selection_does_not_leak_into_other_labels() {
        let text = "line-000\nline-001\nline-002";
        let ctx = Context::default();
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(500.0, 300.0))),
            ..Default::default()
        };
        let run = |events| {
            let input = RawInput {
                events,
                ..input.clone()
            };
            let output = ctx.run_ui(input, |ui| {
                ui.horizontal(|ui| {
                    ui.allocate_ui_with_layout(
                        egui::vec2(300.0, 300.0),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            let mut readonly = text;
                            let _ = message_numbered(
                                ui,
                                1,
                                MessagePane::Request,
                                MessageView::Raw,
                                &mut readonly,
                                None,
                            );
                        },
                    );
                    ui.add(egui::Label::new("OUTSIDE").selectable(true));
                });
            });
            let commands = output.platform_output.commands.clone();
            output.drop_without_applying_deltas();
            commands
        };

        let start = egui::pos2(40.0, 5.0);
        let outside = egui::pos2(350.0, 100.0);
        let mut commands = run(vec![]);
        commands.extend(run(vec![Event::PointerMoved(start)]));
        commands.extend(run(vec![button(start, true)]));
        commands.extend(run(vec![Event::PointerMoved(outside)]));
        commands.extend(run(vec![button(outside, false)]));
        commands.extend(run(vec![]));

        let selections = settled_selections(&commands);
        assert!(!selections.is_empty(), "selection should settle");
        assert!(
            selections.iter().all(|text| !text.contains("OUTSIDE")),
            "selection should stay inside the message view: {selections:?}"
        );
    }

    #[test]
    fn hex_byte_columns_ignore_offsets_padding_and_ascii_placeholders() {
        let lines = [
            "00000000  00 ff 41 42 43 44 45 46  47 48 49 4a 4b 4c 4d 4e  |..ABCDEFGHIJKLMN|",
            "00000010  80 20 7f                                          |. .|",
        ];
        assert_eq!(hex_row_bytes(lines[0], 0).len(), 16);
        assert_eq!(hex_row_bytes(lines[1], 1), [0x80, 0x20, 0x7f]);
        assert!(hex_row_bytes(lines[1], 0).is_empty());
        assert_eq!(hex_column_byte(10, 16), Some(0));
        assert_eq!(hex_column_byte(35, 16), Some(8));
        assert_eq!(hex_column_byte(61, 16), Some(0));
        assert_eq!(hex_column_byte(63, 3), Some(2));
        assert_eq!(hex_column_byte(9, 16), None);
        assert_eq!(hex_column_byte(64, 3), None);
        assert_eq!(
            selected_hex_bytes(&lines, 14..19),
            [0x4d, 0x4e, 0x80, 0x20, 0x7f]
        );
        let selection = HexSelection {
            anchor: Some(18),
            focus: Some(14),
            ..Default::default()
        };
        assert_eq!(selection.range(), Some(14..19));
    }

    #[test]
    fn hex_view_drag_selects_binary_bytes_and_context_menu_copies_hex() {
        let text = "00000000  00 ff 41 42 43 44 45 46  47 48 49 4a 4b 4c 4d 4e  |..ABCDEFGHIJKLMN|\n00000010  80 20 7f                                          |. .|\n";
        let ctx = Context::default();
        ctx.all_styles_mut(|style| style.animation_time = 0.0);
        let run = |events| {
            ctx.run_ui(
                RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(700.0, 300.0))),
                    events,
                    ..Default::default()
                },
                |ui| {
                    message_text(ui, 7, MessagePane::Response, MessageView::Hex, text);
                },
            )
        };
        run(vec![]).drop_without_applying_deltas();
        let (width, height) = ctx.fonts_mut(|fonts| {
            let font = message_font();
            (
                fonts
                    .layout_no_wrap("0".into(), font.clone(), theme::TEXT)
                    .size()
                    .x,
                fonts.row_height(&font),
            )
        });
        let start = egui::pos2(5.0 + 13.5 * width, 5.0 + height * 0.5);
        let end = egui::pos2(5.0 + 62.5 * width, 5.0 + height * 1.5);
        for events in [
            vec![Event::PointerMoved(start)],
            vec![button(start, true)],
            vec![Event::PointerMoved(end)],
            vec![button(end, false)],
        ] {
            run(events).drop_without_applying_deltas();
        }
        let output = run(vec![Event::Copy]);
        assert!(
            output
                .platform_output
                .commands
                .iter()
                .any(|command| matches!(
                    command, egui::OutputCommand::CopyText(value)
                        if value == "ff 41 42 43 44 45 46 47 48 49 4a 4b 4c 4d 4e 80 20"
                ))
        );
        output.drop_without_applying_deltas();
        let right = egui::pos2(50.0, 10.0);
        for events in [
            vec![Event::PointerMoved(right)],
            vec![secondary_button(right, true)],
            vec![secondary_button(right, false)],
            vec![],
        ] {
            run(events).drop_without_applying_deltas();
        }
        let copy = egui::pos2(90.0, 27.0);
        for events in [vec![Event::PointerMoved(copy)], vec![button(copy, true)]] {
            run(events).drop_without_applying_deltas();
        }
        let output = run(vec![button(copy, false)]);
        let copied = output
            .platform_output
            .commands
            .iter()
            .find_map(|command| match command {
                egui::OutputCommand::CopyText(value) => Some(value.as_str()),
                _ => None,
            });
        assert_eq!(
            copied,
            Some("ff 41 42 43 44 45 46 47 48 49 4a 4b 4c 4d 4e 80 20")
        );
    }

    #[test]
    fn hex_selection_resets_when_the_preview_changes() {
        let ctx = Context::default();
        let selected = std::cell::RefCell::new(None);
        let run = |text: &str, events: Vec<Event>| {
            let output = ctx.run_ui(
                RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(700.0, 200.0))),
                    events,
                    ..Default::default()
                },
                |ui| {
                    *selected.borrow_mut() =
                        message_hex(ui, 1, MessagePane::Request, MessageView::Hex, text);
                },
            );
            output.drop_without_applying_deltas();
        };
        let first = "00000000  ff                                                |.|\n";
        let second = "00000000  00                                                |.|\n";
        run(first, vec![]);
        let width = ctx.fonts_mut(|fonts| {
            fonts
                .layout_no_wrap("0".into(), message_font(), theme::TEXT)
                .size()
                .x
        });
        let pos = egui::pos2(10.5 * width, 5.0);
        run(first, vec![Event::PointerMoved(pos)]);
        run(first, vec![button(pos, true)]);
        run(first, vec![button(pos, false)]);
        assert_eq!(*selected.borrow(), Some(vec![0xff]));
        run(second, vec![]);
        assert!(
            selected.borrow().is_none(),
            "an updated preview must not retain the previous selection"
        );
    }

    #[test]
    fn hex_selection_autoscrolls_beyond_visible_rows() {
        let mut text = String::new();
        for row in 0..60 {
            text.push_str(&format!("{:08x}  ", row * 16));
            for col in 0..16 {
                text.push_str(&format!("{:02x} ", (row * 16 + col) % 256));
                if col == 7 {
                    text.push(' ');
                }
            }
            text.push_str(" |................|\n");
        }
        let ctx = Context::default();
        let run = |events| {
            ctx.run_ui(
                RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(700.0, 150.0))),
                    events,
                    ..Default::default()
                },
                |ui| {
                    message_text(ui, 8, MessagePane::Request, MessageView::Hex, &text);
                },
            )
        };
        run(vec![]).drop_without_applying_deltas();
        let width = ctx.fonts_mut(|fonts| {
            fonts
                .layout_no_wrap("0".into(), message_font(), theme::TEXT)
                .size()
                .x
        });
        let start = egui::pos2(5.0 + 10.5 * width, 10.0);
        let below = egui::pos2(start.x, 170.0);
        for events in [
            vec![Event::PointerMoved(start)],
            vec![button(start, true)],
            vec![Event::PointerMoved(below)],
        ] {
            run(events).drop_without_applying_deltas();
        }
        for _ in 0..30 {
            run(vec![]).drop_without_applying_deltas();
        }
        run(vec![button(below, false)]).drop_without_applying_deltas();
        let output = run(vec![Event::Copy]);
        let copied = output
            .platform_output
            .commands
            .iter()
            .find_map(|command| match command {
                egui::OutputCommand::CopyText(value) => Some(value),
                _ => None,
            })
            .expect("selection copied");
        assert!(
            copied.split_whitespace().count() > 16 * 10,
            "selection should follow the scroll: {copied}"
        );
    }

    #[test]
    fn right_click_opens_message_context_menu() {
        let text = "alpha\nbeta";
        let ctx = Context::default();
        let pos = egui::pos2(40.0, 10.0);

        run_message_text_frame(&ctx, vec![], text).drop_without_applying_deltas();
        run_message_text_frame(&ctx, vec![Event::PointerMoved(pos)], text)
            .drop_without_applying_deltas();
        run_message_text_frame(&ctx, vec![secondary_button(pos, true)], text)
            .drop_without_applying_deltas();
        run_message_text_frame(&ctx, vec![secondary_button(pos, false)], text)
            .drop_without_applying_deltas();
        // The popup layer is reported by the context on the following frame.
        run_message_text_frame(&ctx, vec![], text).drop_without_applying_deltas();

        assert!(
            ctx.any_popup_open(),
            "right-clicking the message should open its context menu"
        );
    }

    #[test]
    fn request_detail_context_menu_offers_copy_as_curl() {
        use std::time::{Duration, SystemTime};

        use crate::history::Phase;
        use crate::proxy::{CapturedBody, ExchangeResult, RequestInfo};

        let item = HistoryItem {
            id: 1,
            number: 1,
            timestamp: SystemTime::now(),
            request: RequestInfo {
                method: "GET".into(),
                uri: "/path?q=1".into(),
                scheme: "http".into(),
                host: "example.test".into(),
                path: "/path?q=1".into(),
                version: "HTTP/1.1".into(),
                headers: Vec::new(),
            },
            request_body: CapturedBody::from_bytes(Vec::new()),
            response: None,
            phase: Phase::Finished {
                duration: Duration::ZERO,
                result: ExchangeResult::Dropped,
            },
        };
        let ctx = Context::default();
        let right_click = egui::pos2(40.0, 10.0);

        run_curl_request_frame(&ctx, vec![], &item).drop_without_applying_deltas();
        run_curl_request_frame(&ctx, vec![Event::PointerMoved(right_click)], &item)
            .drop_without_applying_deltas();
        run_curl_request_frame(&ctx, vec![secondary_button(right_click, true)], &item)
            .drop_without_applying_deltas();
        run_curl_request_frame(&ctx, vec![secondary_button(right_click, false)], &item)
            .drop_without_applying_deltas();
        run_curl_request_frame(&ctx, vec![], &item).drop_without_applying_deltas();
        assert!(
            ctx.any_popup_open(),
            "right-clicking a captured request should open the menu with Copy as curl"
        );
    }

    #[test]
    fn context_menu_copies_the_selection() {
        let text = "alpha\nbeta";
        let ctx = Context::default();
        let start = egui::pos2(20.0, 5.0);
        let end = egui::pos2(70.0, 5.0);
        let right_click = egui::pos2(40.0, 10.0);
        let copy_selection = egui::pos2(80.0, 27.0);

        run_message_text_frame(&ctx, vec![], text).drop_without_applying_deltas();
        run_message_text_frame(&ctx, vec![Event::PointerMoved(start)], text)
            .drop_without_applying_deltas();
        run_message_text_frame(&ctx, vec![button(start, true)], text)
            .drop_without_applying_deltas();
        run_message_text_frame(&ctx, vec![Event::PointerMoved(end)], text)
            .drop_without_applying_deltas();
        run_message_text_frame(&ctx, vec![button(end, false)], text).drop_without_applying_deltas();
        run_message_text_frame(&ctx, vec![Event::PointerMoved(right_click)], text)
            .drop_without_applying_deltas();
        run_message_text_frame(&ctx, vec![secondary_button(right_click, true)], text)
            .drop_without_applying_deltas();
        run_message_text_frame(&ctx, vec![secondary_button(right_click, false)], text)
            .drop_without_applying_deltas();
        run_message_text_frame(&ctx, vec![], text).drop_without_applying_deltas();
        run_message_text_frame(&ctx, vec![Event::PointerMoved(copy_selection)], text)
            .drop_without_applying_deltas();
        run_message_text_frame(&ctx, vec![button(copy_selection, true)], text)
            .drop_without_applying_deltas();
        let output = run_message_text_frame(&ctx, vec![button(copy_selection, false)], text);

        let copied = output
            .platform_output
            .commands
            .iter()
            .find_map(|command| match command {
                egui::OutputCommand::CopyText(text) => Some(text.as_str()),
                _ => None,
            });
        assert_eq!(
            copied,
            Some("alpha"),
            "the context menu should copy the selection"
        );
    }

    #[test]
    fn context_menu_copies_a_one_character_selection() {
        let text = "7 alpha\nbeta";
        let ctx = Context::default();
        let cursor = egui::pos2(20.0, 5.0);
        let right_click = egui::pos2(40.0, 10.0);
        let copy_selection = egui::pos2(80.0, 27.0);

        run_message_text_frame(&ctx, vec![], text).drop_without_applying_deltas();
        run_message_text_frame(&ctx, vec![Event::PointerMoved(cursor)], text)
            .drop_without_applying_deltas();
        run_message_text_frame(&ctx, vec![button(cursor, true)], text)
            .drop_without_applying_deltas();
        run_message_text_frame(&ctx, vec![button(cursor, false)], text)
            .drop_without_applying_deltas();
        run_message_text_frame(
            &ctx,
            vec![Event::Key {
                key: Key::ArrowRight,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Modifiers::SHIFT,
            }],
            text,
        )
        .drop_without_applying_deltas();
        run_message_text_frame(&ctx, vec![], text).drop_without_applying_deltas();
        run_message_text_frame(&ctx, vec![Event::PointerMoved(right_click)], text)
            .drop_without_applying_deltas();
        run_message_text_frame(&ctx, vec![secondary_button(right_click, true)], text)
            .drop_without_applying_deltas();
        run_message_text_frame(&ctx, vec![secondary_button(right_click, false)], text)
            .drop_without_applying_deltas();
        run_message_text_frame(&ctx, vec![], text).drop_without_applying_deltas();
        run_message_text_frame(&ctx, vec![Event::PointerMoved(copy_selection)], text)
            .drop_without_applying_deltas();
        run_message_text_frame(&ctx, vec![button(copy_selection, true)], text)
            .drop_without_applying_deltas();
        let output = run_message_text_frame(&ctx, vec![button(copy_selection, false)], text);

        let copied = output
            .platform_output
            .commands
            .iter()
            .find_map(|command| match command {
                egui::OutputCommand::CopyText(text) => Some(text.as_str()),
                _ => None,
            });
        assert_eq!(
            copied,
            Some("7"),
            "the context menu should copy a one-character selection"
        );
    }

    #[test]
    fn context_menu_copies_the_full_message() {
        let text = "alpha\nbeta";
        let ctx = Context::default();
        let pos = egui::pos2(40.0, 10.0);
        let copy_all = egui::pos2(80.0, 50.0);

        run_message_text_frame(&ctx, vec![], text).drop_without_applying_deltas();
        run_message_text_frame(&ctx, vec![Event::PointerMoved(pos)], text)
            .drop_without_applying_deltas();
        run_message_text_frame(&ctx, vec![secondary_button(pos, true)], text)
            .drop_without_applying_deltas();
        run_message_text_frame(&ctx, vec![secondary_button(pos, false)], text)
            .drop_without_applying_deltas();
        run_message_text_frame(&ctx, vec![], text).drop_without_applying_deltas();
        run_message_text_frame(&ctx, vec![Event::PointerMoved(copy_all)], text)
            .drop_without_applying_deltas();
        run_message_text_frame(&ctx, vec![button(copy_all, true)], text)
            .drop_without_applying_deltas();
        let output = run_message_text_frame(&ctx, vec![button(copy_all, false)], text);

        let copied = output
            .platform_output
            .commands
            .iter()
            .find_map(|command| match command {
                egui::OutputCommand::CopyText(text) => Some(text.as_str()),
                _ => None,
            });
        assert_eq!(
            copied,
            Some(text),
            "the context menu should copy the message"
        );
    }
}
