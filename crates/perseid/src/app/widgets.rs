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
