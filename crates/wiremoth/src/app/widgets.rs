//! Small shared egui widgets: tabs, message viewers, cards, badges, and empty
//! states.

use egui::Label;

use super::*;
use crate::history::HistoryItem;

pub(super) fn main_tab_button(ui: &mut egui::Ui, selected: bool, label: &str) -> egui::Response {
    let text = RichText::new(label).size(15.0).strong().color(if selected {
        theme::ACCENT_DARK
    } else {
        theme::TEXT_MUTED
    });
    ui.add(
        egui::Button::new(text)
            .fill(if selected {
                theme::ACCENT_SOFT
            } else {
                Color32::TRANSPARENT
            })
            .stroke(if selected {
                Stroke::new(1.0, theme::ACCENT)
            } else {
                Stroke::NONE
            })
            .corner_radius(5.0)
            .min_size(egui::vec2(84.0, 28.0)),
    )
}

pub(super) fn section_tab(ui: &mut egui::Ui, selected: bool, label: &str) -> egui::Response {
    let text = RichText::new(label).size(13.0).color(if selected {
        theme::ACCENT_DARK
    } else {
        theme::TEXT_MUTED
    });
    let text = if selected { text.strong() } else { text };
    ui.add(
        egui::Button::new(text)
            .fill(if selected {
                theme::ACCENT_WASH
            } else {
                Color32::TRANSPARENT
            })
            .stroke(if selected {
                Stroke::new(1.0, theme::ACCENT)
            } else {
                Stroke::NONE
            })
            .corner_radius(4.0)
            .min_size(egui::vec2(0.0, 24.0)),
    )
}

pub(super) fn message_tab(ui: &mut egui::Ui, selected: bool, label: &str) -> egui::Response {
    let text = RichText::new(label).size(13.5).color(if selected {
        theme::ACCENT_DARK
    } else {
        theme::TEXT_MUTED
    });
    let text = if selected { text.strong() } else { text };
    ui.add(
        egui::Button::new(text)
            .fill(if selected {
                theme::ACCENT_WASH
            } else {
                Color32::TRANSPARENT
            })
            .stroke(if selected {
                Stroke::new(1.0, theme::ACCENT)
            } else {
                Stroke::NONE
            })
            .corner_radius(4.0)
            .min_size(egui::vec2(88.0, 24.0)),
    )
}

pub(super) fn message_mode_tab(ui: &mut egui::Ui, selected: bool, label: &str) -> egui::Response {
    let text = RichText::new(label).size(12.5).color(if selected {
        theme::ACCENT_DARK
    } else {
        theme::TEXT_MUTED
    });
    let text = if selected { text.strong() } else { text };
    ui.add(
        egui::Button::new(text)
            .fill(if selected {
                theme::ACCENT_WASH
            } else {
                Color32::TRANSPARENT
            })
            .stroke(if selected {
                Stroke::new(1.0, theme::ACCENT)
            } else {
                Stroke::NONE
            })
            .corner_radius(4.0)
            .min_size(egui::vec2(0.0, 21.0)),
    )
}

pub(super) fn message_tab_bar(
    ui: &mut egui::Ui,
    pane: &mut MessagePane,
    mode: &mut MessageView,
    item: Option<&HistoryItem>,
) {
    ui.horizontal(|ui| {
        if message_tab(ui, *pane == MessagePane::Request, "Request").clicked() {
            *pane = MessagePane::Request;
        }
        if message_tab(ui, *pane == MessagePane::Response, "Response").clicked() {
            *pane = MessagePane::Response;
        }
        if let Some(item) = item {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
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
                ui.colored_label(view::status_color(item), view::status_text(item));
                ui.label(
                    RichText::new(format!("#{} {}", item.id, item.request.method))
                        .small()
                        .color(theme::TEXT_MUTED),
                );
            });
        }
    });
    ui.add_space(2.0);
    ui.horizontal(|ui| {
        if message_mode_tab(ui, *mode == MessageView::Raw, "Raw").clicked() {
            *mode = MessageView::Raw;
        }
        if message_mode_tab(ui, *mode == MessageView::Headers, "Headers").clicked() {
            *mode = MessageView::Headers;
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
            ui.set_min_height(ui.available_height());
            if mode == MessageView::Hex {
                // A single text galley for a 64 KiB hex dump is surprisingly
                // expensive to lay out. Virtualize the lines so memory stays
                // proportional to the visible page rather than the whole dump.
                message_hex(ui, id, pane, mode, text);
            } else {
                egui::ScrollArea::both()
                    .id_salt(("wiremoth.message", id, pane as u8, mode as u8))
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.add(
                            Label::new(RichText::new(text).monospace().color(theme::TEXT))
                                .selectable(true)
                                .wrap_mode(egui::TextWrapMode::Extend),
                        );
                    });
            }
        });
}

pub(super) fn message_hex(
    ui: &mut egui::Ui,
    id: u64,
    pane: MessagePane,
    mode: MessageView,
    text: &str,
) {
    let lines: Vec<&str> = text.lines().collect();
    let row_height = ui.text_style_height(&egui::TextStyle::Monospace) + 2.0;

    egui::ScrollArea::vertical()
        .id_salt(("wiremoth.hex", id, pane as u8, mode as u8))
        .auto_shrink([false, false])
        .show_rows(ui, row_height, lines.len(), |ui, range| {
            ui.spacing_mut().item_spacing.y = 0.0;
            for line in &lines[range] {
                ui.add(
                    Label::new(RichText::new(*line).monospace().color(theme::TEXT))
                        .selectable(true)
                        .wrap_mode(egui::TextWrapMode::Extend),
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
        .id_salt(("wiremoth.preview", salt))
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
