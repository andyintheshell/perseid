//! Embedded brand assets. The source files live in the repository's
//! `branding/` directory; the app embeds only the PNG exports it needs.

use egui::{ColorImage, Color32, Context, IconData, TextureHandle, TextureOptions};

/// Signal orange from the brand palette.
pub const SIGNAL_ORANGE: Color32 = Color32::from_rgb(0xff, 0x75, 0x42);
/// Ivory from the brand palette.
pub const IVORY: Color32 = Color32::from_rgb(0xf7, 0xf3, 0xec);

const ICON_PNG: &[u8] = include_bytes!("../../../branding/wiremoth-icon-128.png");
const LOCKUP_PNG: &[u8] = include_bytes!("../../../branding/wiremoth-logo-dark.png");

/// Window/task-bar icon, decoded from the 128 px app icon.
pub fn window_icon() -> Option<IconData> {
    let image = image::load_from_memory(ICON_PNG).ok()?.into_rgba8();
    let (width, height) = image.dimensions();
    Some(IconData {
        rgba: image.into_raw(),
        width,
        height,
    })
}

/// Small transparent-tile mark used in the toolbar.
pub fn mark_texture(ctx: &Context) -> Option<TextureHandle> {
    load_texture(ctx, "wiremoth.mark", ICON_PNG)
}

/// Full logo lockup (moth, wordmark, and tagline) used in the About window.
pub fn lockup_texture(ctx: &Context) -> Option<TextureHandle> {
    load_texture(ctx, "wiremoth.lockup", LOCKUP_PNG)
}

fn load_texture(ctx: &Context, name: &str, png: &[u8]) -> Option<TextureHandle> {
    let image = match image::load_from_memory(png) {
        Ok(image) => image.into_rgba8(),
        Err(err) => {
            log::warn!("could not decode {name}: {err}");
            return None;
        }
    };
    let size = [image.width() as usize, image.height() as usize];
    let color = ColorImage::from_rgba_unmultiplied(size, image.as_raw());
    Some(ctx.load_texture(name, color, TextureOptions::LINEAR))
}
