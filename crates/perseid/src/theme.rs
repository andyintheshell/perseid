//! Warm, high-contrast light theme used by the desktop UI.
//!
//! The palette stays close to white, with soft orange-tinted surfaces for
//! hierarchy and the Perseid coral-orange accent for selection and focus.
//! Text is deliberately near-black rather than muted grey: Burp's dense
//! workspaces remain readable at small sizes because the type is dark and
//! sharply rendered, not because the font is large.

use std::sync::Arc;

use egui::epaint::text::{HintingTarget, SmoothHinting};
use egui::{
    Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, FontTweak, Stroke,
    TextStyle,
};

/// Segoe UI-compatible regular-weight face. egui's bundled Ubuntu-Light is
/// too thin for 10-12 pt UI text on a light background. Licensed under the
/// SIL OFL; see `assets/Selawik-OFL.txt`.
const UI_FONT: &[u8] = include_bytes!("../assets/Selawik-Regular.ttf");
/// CJK fallback for request/response text and UI labels (SIL OFL).
const CJK_FONT: &[u8] = include_bytes!("../assets/NotoSansCJKsc-Regular.otf");

pub const CANVAS: Color32 = Color32::from_rgb(0xf8, 0xf6, 0xf2);
pub const SURFACE: Color32 = Color32::from_rgb(0xff, 0xfe, 0xfc);
pub const SURFACE_ALT: Color32 = Color32::from_rgb(0xf4, 0xef, 0xe9);
pub const SURFACE_TINT: Color32 = Color32::from_rgb(0xfb, 0xf7, 0xf2);
pub const BORDER: Color32 = Color32::from_rgb(0xde, 0xd5, 0xcb);
pub const TEXT: Color32 = Color32::from_rgb(0x1d, 0x1a, 0x17);
pub const TEXT_MUTED: Color32 = Color32::from_rgb(0x3b, 0x35, 0x2f);
pub const ACCENT: Color32 = Color32::from_rgb(0xff, 0x4d, 0x2b);
pub const ACCENT_DARK: Color32 = Color32::from_rgb(0xad, 0x2f, 0x15);
pub const ACCENT_SOFT: Color32 = Color32::from_rgb(0xff, 0xdd, 0xd3);
pub const ACCENT_WASH: Color32 = Color32::from_rgb(0xff, 0xf3, 0xef);
pub const SELECTED: Color32 = Color32::from_rgb(0xfb, 0xd9, 0xcd);
/// Background for search matches in message text. Warm yellow keeps the
/// syntax colors legible while matching Burp's highlight convention.
pub const SEARCH_MATCH: Color32 = Color32::from_rgb(0xff, 0xe8, 0x94);
pub const GREEN: Color32 = Color32::from_rgb(0x1e, 0x6a, 0x3e);
pub const BLUE: Color32 = Color32::from_rgb(0x1d, 0x5d, 0x90);
pub const AMBER: Color32 = Color32::from_rgb(0x86, 0x50, 0x00);
pub const RED: Color32 = Color32::from_rgb(0xa3, 0x28, 0x1d);
pub const GRAY: Color32 = Color32::from_rgb(0x5b, 0x54, 0x4d);

pub fn apply(ctx: &egui::Context) {
    install_fonts(ctx);
    ctx.set_theme(egui::Theme::Light);

    let mut style = (*ctx.style_of(egui::Theme::Light)).clone();
    style.visuals = visuals();
    // Sub-pixel glyph binning improves kerning at the cost of blurrier text.
    // Burp-style dense text benefits more from crisp stems and dark ink.
    style.visuals.text_options.subpixel_binning = false;
    style.visuals.text_options.font_hinting = true;
    // Burp-style chrome stays compact vertically, but controls need enough
    // padding and separation to scan and click comfortably. These are slightly
    // roomier than egui's dense defaults without wasting editor space.
    style.spacing.item_spacing = egui::vec2(8.0, 4.0);
    style.spacing.button_padding = egui::vec2(8.0, 4.0);
    style.spacing.interact_size.y = 24.0;
    style.spacing.menu_margin = egui::Margin::same(5);
    style
        .text_styles
        .insert(TextStyle::Heading, FontId::proportional(16.0));
    style
        .text_styles
        .insert(TextStyle::Body, FontId::proportional(12.5));
    style
        .text_styles
        .insert(TextStyle::Button, FontId::proportional(12.5));
    style
        .text_styles
        .insert(TextStyle::Small, FontId::proportional(11.0));
    style
        .text_styles
        .insert(TextStyle::Monospace, FontId::monospace(11.5));
    ctx.set_style_of(egui::Theme::Light, style);
}

fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let tweak = FontTweak {
        hinting: Some(true),
        // The default preserves linear metrics and renders symmetrically,
        // which leaves horizontal stems soft at small sizes. Prefer sharp
        // grid-fitting for the compact UI.
        hinting_target: HintingTarget::Smooth(SmoothHinting {
            light: false,
            symmetric_rendering: false,
            preserve_linear_metrics: false,
        }),
        subpixel_binning: Some(false),
        ..Default::default()
    };
    fonts.font_data.insert(
        "perseid-ui".to_owned(),
        Arc::new(FontData::from_static(UI_FONT).tweak(tweak)),
    );
    fonts
        .families
        .entry(FontFamily::Proportional)
        .or_default()
        .insert(0, "perseid-ui".to_owned());
    fonts.font_data.insert(
        "perseid-cjk".to_owned(),
        Arc::new(FontData::from_static(CJK_FONT)),
    );
    for family in [FontFamily::Proportional, FontFamily::Monospace] {
        fonts
            .families
            .entry(family)
            .or_default()
            .push("perseid-cjk".to_owned());
    }
    ctx.set_fonts(fonts);
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
    visuals.weak_text_color = Some(GRAY);
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
    widgets.hovered.fg_stroke = Stroke::new(1.0, TEXT);
    widgets.hovered.corner_radius = CornerRadius::same(4);

    widgets.active.bg_fill = ACCENT_SOFT;
    widgets.active.weak_bg_fill = ACCENT_SOFT;
    widgets.active.bg_stroke = Stroke::new(1.0, ACCENT);
    widgets.active.fg_stroke = Stroke::new(1.0, TEXT);
    widgets.active.corner_radius = CornerRadius::same(4);

    widgets.open.bg_fill = ACCENT_SOFT;
    widgets.open.weak_bg_fill = ACCENT_SOFT;
    widgets.open.bg_stroke = Stroke::new(1.0, ACCENT);
    widgets.open.fg_stroke = Stroke::new(1.0, TEXT);
    widgets.open.corner_radius = CornerRadius::same(4);

    visuals
}

#[cfg(test)]
mod font_tests {
    use super::*;

    #[test]
    fn message_and_ui_fonts_cover_cjk_text() {
        let ctx = egui::Context::default();
        install_fonts(&ctx);
        let mut output = ctx.run_pass(egui::RawInput::default(), |ctx| {
            ctx.fonts_mut(|fonts| {
                for font in [FontId::monospace(12.0), FontId::proportional(12.0)] {
                    assert!(fonts.has_glyphs(&font, "中文漢字日本語한국어"));
                    let galley = fonts.layout_no_wrap("中文".to_owned(), font, Color32::BLACK);
                    assert!(!galley.is_empty());
                }
            });
        });
        output.textures_delta.clear();
    }
}
