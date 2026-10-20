//! Warm, low-contrast light theme used by the desktop UI.
//!
//! The palette stays close to white, with soft orange-tinted surfaces for
//! hierarchy and the Perseid coral-orange accent for selection and focus. It
//! is meant to feel closer to a modern document tool than to a dark terminal
//! theme.

use egui::{Color32, CornerRadius, FontId, Stroke, TextStyle};

pub const CANVAS: Color32 = Color32::from_rgb(0xfb, 0xf8, 0xf4);
pub const SURFACE: Color32 = Color32::from_rgb(0xff, 0xfd, 0xfa);
pub const SURFACE_ALT: Color32 = Color32::from_rgb(0xf8, 0xf1, 0xe9);
pub const SURFACE_TINT: Color32 = Color32::from_rgb(0xfc, 0xf6, 0xef);
pub const BORDER: Color32 = Color32::from_rgb(0xe8, 0xdd, 0xd1);
pub const TEXT: Color32 = Color32::from_rgb(0x2e, 0x2a, 0x26);
pub const TEXT_MUTED: Color32 = Color32::from_rgb(0x7c, 0x70, 0x65);
pub const ACCENT: Color32 = Color32::from_rgb(0xff, 0x4d, 0x2b);
pub const ACCENT_DARK: Color32 = Color32::from_rgb(0xc9, 0x3a, 0x1c);
pub const ACCENT_SOFT: Color32 = Color32::from_rgb(0xff, 0xdd, 0xd3);
pub const ACCENT_WASH: Color32 = Color32::from_rgb(0xff, 0xf3, 0xef);
pub const SELECTED: Color32 = Color32::from_rgb(0xfb, 0xd9, 0xcd);
pub const GREEN: Color32 = Color32::from_rgb(0x2f, 0x7d, 0x4f);
pub const BLUE: Color32 = Color32::from_rgb(0x2f, 0x6f, 0x9f);
pub const AMBER: Color32 = Color32::from_rgb(0xa9, 0x6a, 0x12);
pub const RED: Color32 = Color32::from_rgb(0xb3, 0x3a, 0x2e);
pub const GRAY: Color32 = Color32::from_rgb(0x8a, 0x81, 0x78);

pub fn apply(ctx: &egui::Context) {
    ctx.set_theme(egui::Theme::Light);

    let mut style = (*ctx.style_of(egui::Theme::Light)).clone();
    style.visuals = visuals();
    style.spacing.item_spacing = egui::vec2(8.0, 6.0);
    style.spacing.button_padding = egui::vec2(8.0, 4.0);
    style
        .text_styles
        .insert(TextStyle::Heading, FontId::proportional(20.0));
    style
        .text_styles
        .insert(TextStyle::Body, FontId::proportional(14.0));
    style
        .text_styles
        .insert(TextStyle::Button, FontId::proportional(14.0));
    style
        .text_styles
        .insert(TextStyle::Small, FontId::proportional(12.0));
    style
        .text_styles
        .insert(TextStyle::Monospace, FontId::monospace(13.0));
    ctx.set_style_of(egui::Theme::Light, style);
}

fn visuals() -> egui::Visuals {
    let mut visuals = egui::Visuals::light();
    visuals.dark_mode = false;
    visuals.panel_fill = CANVAS;
    visuals.window_fill = SURFACE;
    visuals.extreme_bg_color = Color32::WHITE;
    visuals.text_edit_bg_color = Some(Color32::WHITE);
    visuals.faint_bg_color = SURFACE_TINT;
    visuals.code_bg_color = SURFACE_ALT;
    visuals.hyperlink_color = ACCENT_DARK;
    visuals.warn_fg_color = AMBER;
    visuals.error_fg_color = RED;
    visuals.weak_text_color = Some(TEXT_MUTED);
    visuals.window_stroke = Stroke::new(1.0, BORDER);
    visuals.window_corner_radius = CornerRadius::same(8);
    visuals.menu_corner_radius = CornerRadius::same(6);
    visuals.selection.bg_fill = SELECTED;
    visuals.selection.stroke = Stroke::new(1.0, ACCENT);
    visuals.interact_cursor = Some(egui::CursorIcon::PointingHand);
    visuals.indent_has_left_vline = false;
    visuals.collapsing_header_frame = false;
    visuals.striped = true;

    let widgets = &mut visuals.widgets;
    widgets.noninteractive.bg_fill = SURFACE;
    widgets.noninteractive.weak_bg_fill = SURFACE;
    widgets.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
    widgets.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
    widgets.noninteractive.corner_radius = CornerRadius::same(4);

    widgets.inactive.bg_fill = SURFACE;
    widgets.inactive.weak_bg_fill = SURFACE_ALT;
    widgets.inactive.bg_stroke = Stroke::new(1.0, BORDER);
    widgets.inactive.fg_stroke = Stroke::new(1.0, TEXT);
    widgets.inactive.corner_radius = CornerRadius::same(4);

    widgets.hovered.bg_fill = ACCENT_WASH;
    widgets.hovered.weak_bg_fill = ACCENT_WASH;
    widgets.hovered.bg_stroke = Stroke::new(1.0, ACCENT);
    widgets.hovered.fg_stroke = Stroke::new(1.0, ACCENT_DARK);
    widgets.hovered.corner_radius = CornerRadius::same(4);

    widgets.active.bg_fill = ACCENT_SOFT;
    widgets.active.weak_bg_fill = ACCENT_SOFT;
    widgets.active.bg_stroke = Stroke::new(1.0, ACCENT);
    widgets.active.fg_stroke = Stroke::new(1.0, ACCENT_DARK);
    widgets.active.corner_radius = CornerRadius::same(4);

    widgets.open.bg_fill = ACCENT_SOFT;
    widgets.open.weak_bg_fill = ACCENT_SOFT;
    widgets.open.bg_stroke = Stroke::new(1.0, ACCENT);
    widgets.open.fg_stroke = Stroke::new(1.0, ACCENT_DARK);
    widgets.open.corner_radius = CornerRadius::same(4);

    visuals
}
