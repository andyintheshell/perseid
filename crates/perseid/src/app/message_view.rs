//! Wrapped, selectable message viewers with a painted line-number gutter.
//!
//! Pretty and Raw use egui's multiline editor so wrapping, selection, keyboard
//! navigation, and clipboard behavior come from the widget rather than a
//! bespoke text layout. Hex uses a virtualized row list because a large dump is
//! too expensive to lay out as one galley.

use std::sync::Arc;

use egui::text_selection::LabelSelectionState;
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
    ui.add_space(1.0);
}

pub(super) fn message_text(
    ui: &mut egui::Ui,
    id: u64,
    pane: MessagePane,
    mode: MessageView,
    text: &str,
) {
    let selection_key =
        ui.make_persistent_id(("perseid.message.selection", id, pane as u8, mode as u8));
    let secondary_pressed =
        ui.input(|input| input.pointer.button_pressed(egui::PointerButton::Secondary));
    let mut selection = None;
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
                message_hex(ui, id, pane, mode, text);
            } else {
                selection = message_numbered(ui, id, pane, mode, text);
            }
        });

    // egui's text editor collapses a selection when it is pressed or loses
    // focus (for example when a menu item is clicked). Remember the text that
    // was selected when the context menu opened so Copy selection still has
    // something to copy after that happens.
    if secondary_pressed {
        let selected = selection
            .map(|range| range.slice_str(text))
            .filter(|selected| !selected.is_empty())
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
    let copy_all = match pane {
        MessagePane::Request => "Copy request",
        MessagePane::Response => "Copy response",
    };
    frame.response.container_context_menu(|ui| {
        let selected = saved_selection.as_deref().or_else(|| {
            selection
                .map(|range| range.slice_str(text))
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
        if ui.button(copy_all).clicked() {
            ui.ctx().copy_text(text.to_owned());
            ui.close();
        }
    });
}

/// Render the Pretty and Raw views as a wrapped, selectable text editor.
///
/// The editor owns the text buffer, so wrapping, selection, copying, and
/// keyboard navigation all use egui's native text behavior. Line numbers stay
/// in a painted gutter outside the buffer: they align with the editor's galley
/// but can never be selected or copied.
///
/// Returns the current selection, if any, so the message frame can offer a
/// right-click copy action for it.
fn message_numbered(
    ui: &mut egui::Ui,
    id: u64,
    pane: MessagePane,
    mode: MessageView,
    text: &str,
) -> Option<egui::text::CCursorRange> {
    let line_count = text.lines().count() + usize::from(text.ends_with('\n'));
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
        let job = highlight::message_job(buffer.as_str(), wrap_width, &font);
        ui.fonts_mut(|fonts| fonts.layout_job(job))
    };
    let mut readonly = text;

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
            let output = egui::TextEdit::multiline(&mut readonly)
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
            previous_selection.or(output.cursor_range.filter(|range| !range.is_empty()))
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

/// One virtualized message row with a selectable text area and an optional
/// non-selectable line-number gutter.
///
/// The interactive rect spans the full visible row width, not just the text
/// galley. That lets a drag begin to the right of a short line and still anchor
/// at the line's end. The gutter is painted separately and never enters the
/// label-selection state.
fn selectable_message_line(
    ui: &mut egui::Ui,
    id: egui::Id,
    galley: Arc<egui::Galley>,
    number: Option<usize>,
    font: &FontId,
    gutter_width: f32,
    row_height: f32,
) -> egui::Response {
    let text_width = galley.size().x;
    let available_width = ui.available_width();
    let row_width = (gutter_width + text_width).max(if available_width.is_finite() {
        available_width
    } else {
        0.0
    });
    let (_, row_rect) = ui.allocate_space(egui::vec2(row_width, row_height));

    if let Some(number) = number {
        ui.painter().text(
            egui::pos2(row_rect.left() + gutter_width - 8.0, row_rect.top()),
            Align2::RIGHT_TOP,
            number.to_string(),
            font.clone(),
            theme::GRAY,
        );
    }

    let text_left = row_rect.left() + gutter_width;
    let text_rect = egui::Rect::from_min_max(
        egui::pos2(text_left, row_rect.top()),
        egui::pos2(
            row_rect.right().max(text_left + text_width),
            row_rect.bottom(),
        ),
    );
    let response = ui.interact(text_rect, id, Sense::click_and_drag() - Sense::FOCUSABLE);
    if ui.is_rect_visible(row_rect) {
        LabelSelectionState::label_text_selection(
            ui,
            &response,
            egui::pos2(text_left, row_rect.top()),
            galley,
            theme::TEXT,
            Stroke::NONE,
        );
    }
    response
}

pub(super) fn message_hex(
    ui: &mut egui::Ui,
    id: u64,
    pane: MessagePane,
    mode: MessageView,
    text: &str,
) {
    let lines: Vec<&str> = text.lines().collect();
    // `show_rows` estimates both the scrollable content height and the visible
    // row range from this pitch, so it has to match the height the rows
    // actually occupy. Use the plain monospace row height and zero out the
    // spacing on the *outer* ui before the estimate is taken: the old extra
    // padding made the estimate too tall, so long dumps stopped short of the
    // viewport bottom and left a large blank area when scrolled to the end.
    ui.spacing_mut().item_spacing.y = 0.0;
    let font = message_font();
    let row_height = ui.fonts_mut(|fonts| fonts.row_height(&font));
    let total_rows = total_message_rows(ui, lines.len(), row_height);
    let line_id_base = ui.make_persistent_id(("perseid.hex.line", id, pane as u8, mode as u8));

    egui::ScrollArea::both()
        .id_salt(("perseid.hex", id, pane as u8, mode as u8))
        .auto_shrink([false, false])
        .show_rows(ui, row_height, total_rows, |ui, range| {
            ui.spacing_mut().item_spacing.y = 0.0;
            for index in range {
                let line = lines.get(index).copied().unwrap_or_default();
                let galley = ui.fonts_mut(|fonts| {
                    fonts.layout_no_wrap(line.to_owned(), font.clone(), theme::TEXT)
                });
                selectable_message_line(
                    ui,
                    line_id_base.with(index),
                    galley,
                    None,
                    &font,
                    0.0,
                    row_height,
                );
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Context, Event, Key, Modifiers, PointerButton, Pos2, RawInput, Rect};

    fn run_frame(
        ctx: &Context,
        events: Vec<Event>,
        row_rect: &mut Option<Rect>,
    ) -> egui::FullOutput {
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(400.0, 200.0))),
            events,
            ..Default::default()
        };
        ctx.run_ui(input, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            let font = message_font();
            let row_height = ui.fonts_mut(|fonts| fonts.row_height(&font));
            let galley = ui.fonts_mut(|fonts| {
                fonts.layout_no_wrap("short".to_owned(), font.clone(), theme::TEXT)
            });
            let response = selectable_message_line(
                ui,
                egui::Id::unique("perseid.test.message.line"),
                galley,
                Some(123),
                &font,
                40.0,
                row_height,
            );
            *row_rect = Some(response.rect);
        })
    }

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
            let _ = message_numbered(ui, 1, MessagePane::Request, MessageView::Raw, text);
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
                            let _ = message_numbered(
                                ui,
                                1,
                                MessagePane::Request,
                                MessageView::Raw,
                                text,
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

    #[test]
    fn selection_can_start_in_blank_space_without_the_gutter() {
        let ctx = Context::default();
        let mut row_rect = None;
        run_frame(&ctx, vec![], &mut row_rect).drop_without_applying_deltas();
        let rect = row_rect.expect("the message row should be rendered");
        let start = egui::pos2(rect.right() - 4.0, rect.center().y);
        let end = egui::pos2(rect.left() + 1.0, rect.center().y);

        run_frame(&ctx, vec![Event::PointerMoved(start)], &mut row_rect)
            .drop_without_applying_deltas();
        run_frame(&ctx, vec![button(start, true)], &mut row_rect).drop_without_applying_deltas();
        assert!(
            ctx.plugin::<LabelSelectionState>().lock().has_selection(),
            "a drag that starts in the blank area should begin a text selection"
        );

        run_frame(&ctx, vec![Event::PointerMoved(end)], &mut row_rect)
            .drop_without_applying_deltas();
        run_frame(&ctx, vec![button(end, false)], &mut row_rect).drop_without_applying_deltas();

        let output = run_frame(&ctx, vec![Event::Copy], &mut row_rect);
        let copied = output
            .platform_output
            .commands
            .iter()
            .find_map(|command| match command {
                egui::OutputCommand::CopyText(text) => Some(text.as_str()),
                _ => None,
            });
        assert_eq!(copied, Some("short"), "gutter text must not be copied");
    }
}
