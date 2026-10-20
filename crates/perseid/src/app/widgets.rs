//! Small shared egui widgets: tabs, message viewers, cards, badges, and empty
//! states.

use egui::text_selection::LabelSelectionState;
use egui::{Align2, FontId, Label, RichText, Sense};

use super::*;
use crate::highlight;
use crate::history::HistoryItem;

#[derive(Clone, Copy)]
struct TabStyle {
    font_size: f32,
    height: f32,
    min_width: f32,
    padding_x: f32,
    corner_radius: u8,
}

const MAIN_TAB_STYLE: TabStyle = TabStyle {
    font_size: 15.0,
    height: 28.0,
    min_width: 84.0,
    padding_x: 12.0,
    corner_radius: 5,
};

const SECTION_TAB_STYLE: TabStyle = TabStyle {
    font_size: 13.0,
    height: 26.0,
    min_width: 0.0,
    padding_x: 9.0,
    corner_radius: 4,
};

const MESSAGE_TAB_STYLE: TabStyle = TabStyle {
    font_size: 12.5,
    height: 21.0,
    min_width: 0.0,
    padding_x: 7.0,
    corner_radius: 4,
};

pub(super) fn main_tab_button(ui: &mut egui::Ui, selected: bool, label: &str) -> egui::Response {
    tab_button(ui, selected, label, MAIN_TAB_STYLE)
}

pub(super) fn section_tab(ui: &mut egui::Ui, selected: bool, label: &str) -> egui::Response {
    tab_button(ui, selected, label, SECTION_TAB_STYLE)
}

pub(super) fn message_mode_tab(ui: &mut egui::Ui, selected: bool, label: &str) -> egui::Response {
    tab_button(ui, selected, label, MESSAGE_TAB_STYLE)
}

/// Draw a section tab row directly under the primary toolbar.
///
/// A nested top panel gives the row a full-width separator line without
/// introducing another background colour or a gap between the two bars.
pub(super) fn section_bar(ui: &mut egui::Ui, id_salt: &str, add_tabs: impl FnOnce(&mut egui::Ui)) {
    egui::Panel::top(id_salt)
        .frame(
            egui::Frame::NONE
                .fill(theme::SURFACE)
                .inner_margin(egui::Margin::symmetric(12, 4)),
        )
        .show(ui, |ui| {
            ui.horizontal(add_tabs);
        });
}

fn tab_button(ui: &mut egui::Ui, selected: bool, label: &str, style: TabStyle) -> egui::Response {
    let font = egui::FontId::proportional(style.font_size);
    let measured = ui
        .painter()
        .layout_no_wrap(label.to_owned(), font.clone(), theme::TEXT_MUTED);
    let desired = egui::vec2(
        (measured.size().x + 2.0 * style.padding_x).max(style.min_width),
        style.height,
    );
    let (rect, response) = ui.allocate_exact_size(desired, egui::Sense::click());

    if ui.is_rect_visible(rect) {
        let hovered = response.hovered();
        let (fill, stroke, text_color) = if selected {
            let fill = if hovered {
                theme::SELECTED
            } else {
                theme::ACCENT_SOFT
            };
            (fill, Stroke::new(1.0, theme::ACCENT), theme::ACCENT_DARK)
        } else if hovered {
            (
                theme::ACCENT_WASH,
                Stroke::new(1.0, theme::ACCENT_SOFT),
                theme::ACCENT_DARK,
            )
        } else {
            (Color32::TRANSPARENT, Stroke::NONE, theme::TEXT_MUTED)
        };

        let painter = ui.painter();
        let corner_radius = egui::CornerRadius::same(style.corner_radius);
        if fill != Color32::TRANSPARENT || stroke != Stroke::NONE {
            painter.rect(rect, corner_radius, fill, stroke, egui::StrokeKind::Inside);
        }
        if response.has_focus() {
            painter.rect_stroke(
                rect.shrink(1.0),
                corner_radius,
                Stroke::new(1.0, theme::ACCENT_DARK),
                egui::StrokeKind::Inside,
            );
        }

        let galley = painter.layout_no_wrap(label.to_owned(), font, text_color);
        painter.galley(rect.center() - galley.size() * 0.5, galley, text_color);
    }

    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::Role::Button, ui.is_enabled(), selected, label)
    });
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Add the standard padding around card-based page content.
pub(super) fn page_pad<R>(ui: &mut egui::Ui, add_contents: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::NONE
        .inner_margin(egui::Margin::symmetric(12, 10))
        .show(ui, add_contents)
        .inner
}

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
    ui.add_space(2.0);
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
    ui.add_space(2.0);
}

pub(super) fn message_text(
    ui: &mut egui::Ui,
    id: u64,
    pane: MessagePane,
    mode: MessageView,
    text: &str,
) {
    egui::Frame::NONE
        .fill(Color32::WHITE)
        .stroke(Stroke::new(1.0, theme::BORDER))
        .corner_radius(4.0)
        .inner_margin(egui::Margin::same(8))
        .show(ui, |ui| {
            ui.set_min_size(ui.available_size());
            if mode == MessageView::Hex {
                // A single text galley for a 64 KiB hex dump is surprisingly
                // expensive to lay out. Virtualize the lines so memory stays
                // proportional to the visible page rather than the whole dump.
                message_hex(ui, id, pane, mode, text);
            } else {
                message_numbered(ui, id, pane, mode, text);
            }
        });
}

/// Render the Pretty and Raw views as virtualized, numbered, highlighted rows.
///
/// Splitting the message into rows is what makes both the gutter and the
/// per-line highlighting cheap: only the visible page is laid out, even when
/// the body preview is at its 256 KiB limit.
fn message_numbered(ui: &mut egui::Ui, id: u64, pane: MessagePane, mode: MessageView, text: &str) {
    let lines: Vec<&str> = text.lines().collect();
    let syntax = highlight::Syntax::from_lines(&lines);
    let font = message_font();
    // `show_rows` uses this as the row pitch, so it must match the height each
    // row actually occupies. Zero the outer spacing before the estimate is
    // taken, then do the same inside the scroll area.
    let row_height = ui.fonts_mut(|fonts| fonts.row_height(&font));
    let digits = lines.len().max(1).to_string().len();
    let number_width = ui.fonts_mut(|fonts| {
        fonts
            .layout_no_wrap("0".repeat(digits), font.clone(), theme::GRAY)
            .size()
            .x
    });
    let gutter_width = number_width + 10.0;
    let total_rows = total_message_rows(ui, lines.len(), row_height);
    let line_id_base = ui.make_persistent_id(("perseid.message.line", id, pane as u8, mode as u8));

    ui.spacing_mut().item_spacing.y = 0.0;
    egui::ScrollArea::both()
        .id_salt(("perseid.message", id, pane as u8, mode as u8))
        .auto_shrink([false, false])
        .show_rows(ui, row_height, total_rows, |ui, range| {
            ui.spacing_mut().item_spacing.y = 0.0;
            for index in range {
                let numbered = index < lines.len();
                let line = lines.get(index).copied().unwrap_or_default();
                let role = if numbered {
                    syntax.role(index)
                } else {
                    highlight::LineRole::Blank
                };
                let galley = ui.fonts_mut(|fonts| {
                    fonts.layout_job(highlight::line_job(line, role, syntax.language, &font))
                });
                selectable_message_line(
                    ui,
                    line_id_base.with(index),
                    galley,
                    numbered.then_some(index + 1),
                    &font,
                    gutter_width,
                    row_height,
                );
            }
        });
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

pub(super) fn preview_pane(ui: &mut egui::Ui, title: &str, subtitle: &str, text: &str, salt: &str) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(title).strong().color(theme::TEXT));
        ui.label(RichText::new(subtitle).small().color(theme::TEXT_MUTED));
    });
    ui.separator();
    egui::ScrollArea::both()
        .id_salt(("perseid.preview", salt))
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.add(
                Label::new(RichText::new(text).monospace().color(theme::TEXT))
                    .selectable(true)
                    .wrap_mode(egui::TextWrapMode::Extend),
            );
        });
}

pub(super) fn card<R>(ui: &mut egui::Ui, add_contents: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::NONE
        .fill(theme::SURFACE)
        .stroke(Stroke::new(1.0, theme::BORDER))
        .corner_radius(6.0)
        .inner_margin(egui::Margin::same(12))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            add_contents(ui)
        })
        .inner
}

pub(super) fn badge(ui: &mut egui::Ui, text: &str) {
    egui::Frame::NONE
        .fill(theme::ACCENT_WASH)
        .stroke(Stroke::new(1.0, theme::ACCENT_SOFT))
        .corner_radius(10.0)
        .inner_margin(egui::Margin::symmetric(7, 2))
        .show(ui, |ui| {
            ui.label(
                RichText::new(text)
                    .size(10.5)
                    .strong()
                    .color(theme::ACCENT_DARK),
            );
        });
}

pub(super) fn empty_state(ui: &mut egui::Ui, title: &str, body: &str) {
    ui.add_space(28.0);
    ui.vertical_centered(|ui| {
        ui.label(
            RichText::new(title)
                .size(16.0)
                .strong()
                .color(theme::TEXT_MUTED),
        );
        ui.add_space(4.0);
        ui.label(RichText::new(body).color(theme::TEXT_MUTED));
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Context, Event, Modifiers, PointerButton, Pos2, RawInput, Rect};

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
