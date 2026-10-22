//! Small shared egui widgets: tabs, cards, badges, and empty states.

use egui::{Color32, RichText, Stroke};

use crate::theme;

#[derive(Clone, Copy)]
struct TabStyle {
    font_size: f32,
    height: f32,
    min_width: f32,
    padding_x: f32,
    corner_radius: u8,
}

const MAIN_TAB_STYLE: TabStyle = TabStyle {
    font_size: 12.5,
    height: 23.0,
    min_width: 64.0,
    padding_x: 9.0,
    corner_radius: 3,
};

const SECTION_TAB_STYLE: TabStyle = TabStyle {
    font_size: 11.5,
    height: 21.0,
    min_width: 0.0,
    padding_x: 7.0,
    corner_radius: 3,
};

const MESSAGE_TAB_STYLE: TabStyle = TabStyle {
    font_size: 11.0,
    height: 18.0,
    min_width: 0.0,
    padding_x: 6.0,
    corner_radius: 3,
};

pub(super) fn main_tab_button(ui: &mut egui::Ui, selected: bool, label: &str) -> egui::Response {
    tab_button(ui, selected, label, MAIN_TAB_STYLE, false)
}

pub(super) fn section_tab(ui: &mut egui::Ui, selected: bool, label: &str) -> egui::Response {
    tab_button(ui, selected, label, SECTION_TAB_STYLE, false)
}

/// A section tab with a small painted gear before its label.
pub(super) fn section_tab_gear(ui: &mut egui::Ui, selected: bool, label: &str) -> egui::Response {
    tab_button(ui, selected, label, SECTION_TAB_STYLE, true)
}

pub(super) fn message_mode_tab(ui: &mut egui::Ui, selected: bool, label: &str) -> egui::Response {
    tab_button(ui, selected, label, MESSAGE_TAB_STYLE, false)
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
                .inner_margin(egui::Margin::symmetric(8, 2)),
        )
        .show(ui, |ui| {
            ui.horizontal(add_tabs);
        });
}

fn tab_button(
    ui: &mut egui::Ui,
    selected: bool,
    label: &str,
    style: TabStyle,
    with_gear: bool,
) -> egui::Response {
    let font = egui::FontId::proportional(style.font_size);
    let measured = ui
        .painter()
        .layout_no_wrap(label.to_owned(), font.clone(), theme::TEXT);
    let icon_width = if with_gear { 13.0 } else { 0.0 };
    let content_width = measured.size().x + icon_width;
    let desired = egui::vec2(
        (content_width + 2.0 * style.padding_x).max(style.min_width),
        style.height,
    );
    let (rect, response) = ui.allocate_exact_size(desired, egui::Sense::click());

    if ui.is_rect_visible(rect) {
        let hovered = response.hovered();
        let fill = if hovered && !selected {
            theme::ACCENT_WASH
        } else {
            Color32::TRANSPARENT
        };
        let text_color = theme::TEXT;

        let painter = ui.painter();
        let corner_radius = egui::CornerRadius::same(style.corner_radius);
        if fill != Color32::TRANSPARENT {
            painter.rect_filled(rect, corner_radius, fill);
        }
        if selected {
            let underline = egui::Rect::from_min_max(
                egui::pos2(rect.left() + 1.0, rect.bottom() - 2.0),
                egui::pos2(rect.right() - 1.0, rect.bottom()),
            );
            painter.rect_filled(underline, egui::CornerRadius::same(1), theme::ACCENT);
        }
        if response.has_focus() {
            painter.rect_stroke(
                rect.shrink(1.0),
                corner_radius,
                Stroke::new(1.0, theme::ACCENT_DARK),
                egui::StrokeKind::Inside,
            );
        }

        let content_left = rect.center().x - content_width * 0.5;
        if with_gear {
            let hole = if fill == Color32::TRANSPARENT {
                theme::SURFACE
            } else {
                fill
            };
            paint_gear(
                painter,
                egui::pos2(content_left + 5.0, rect.center().y),
                text_color,
                hole,
            );
        }
        let galley = painter.layout_no_wrap(label.to_owned(), font, text_color);
        painter.galley(
            egui::pos2(
                content_left + icon_width,
                rect.center().y - galley.size().y * 0.5,
            ),
            galley,
            text_color,
        );
    }

    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::Role::Button, ui.is_enabled(), selected, label)
    });
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A tiny monochrome cog for the Proxy settings tab.
fn paint_gear(painter: &egui::Painter, center: egui::Pos2, color: Color32, hole_color: Color32) {
    let stroke = Stroke::new(2.0, color);
    for index in 0..8 {
        let angle = index as f32 * std::f32::consts::TAU / 8.0;
        let direction = egui::vec2(angle.cos(), angle.sin());
        painter.line_segment([center + direction * 3.0, center + direction * 5.0], stroke);
    }
    painter.circle_filled(center, 3.4, color);
    painter.circle_filled(center, 1.4, hole_color);
}

/// Inset a full-width table from the surrounding panel edges.
///
/// Table rows and the header both need the same horizontal breathing room;
/// padding individual cells leaves the selection and stripe fills pressed
/// against the window edge.
pub(super) fn table_frame<R>(
    ui: &mut egui::Ui,
    add_contents: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    egui::Frame::NONE
        .inner_margin(egui::Margin::symmetric(8, 0))
        .show(ui, add_contents)
        .inner
}

/// Add the standard padding around card-based page content.
pub(super) fn page_pad<R>(ui: &mut egui::Ui, add_contents: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::NONE
        .inner_margin(egui::Margin::symmetric(8, 6))
        .show(ui, add_contents)
        .inner
}

pub(super) fn card<R>(ui: &mut egui::Ui, add_contents: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::NONE
        .fill(theme::SURFACE)
        .stroke(Stroke::new(1.0, theme::BORDER))
        .corner_radius(4.0)
        .inner_margin(egui::Margin::same(8))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            add_contents(ui)
        })
        .inner
}

pub(super) fn empty_state(ui: &mut egui::Ui, title: &str, body: &str) {
    ui.add_space(22.0);
    ui.vertical_centered(|ui| {
        ui.label(RichText::new(title).size(14.0).strong().color(theme::TEXT));
        ui.add_space(3.0);
        ui.label(RichText::new(body).color(theme::TEXT_MUTED));
    });
}
