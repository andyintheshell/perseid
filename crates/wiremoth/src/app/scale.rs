//! UI scale resolution, zoom stepping, and initial window sizing.

use super::*;

/// The scale the desktop gives GTK/Qt applications, used as a fallback on X11
/// setups where the windowing system reports no HiDPI scaling (no `Xft.dpi`).
pub(super) fn desktop_scale_override() -> Option<f32> {
    ["GDK_SCALE", "QT_SCALE_FACTOR"]
        .iter()
        .filter_map(|name| std::env::var(name).ok()?.trim().parse::<f32>().ok())
        .find(|scale| scale.is_finite() && *scale > 1.0)
        .map(|scale| scale.clamp(MIN_UI_SCALE, MAX_UI_SCALE))
}

pub(super) fn resolve_ui_scale(requested: Option<f32>, native: f32, desktop: Option<f32>) -> f32 {
    requested
        .or_else(|| (native > 1.0).then_some(native))
        .or(desktop)
        .unwrap_or(1.0)
        .clamp(MIN_UI_SCALE, MAX_UI_SCALE)
}

/// One zoom step, rounded to a tenth so repeated steps stay on clean values.
pub(super) fn stepped_scale(current: f32, delta: f32) -> f32 {
    ((current + delta) * 10.0).round() / 10.0
}

/// Keep the same logical content size as at 100% by growing the window when
/// the UI is zoomed at startup. Without this a 2x UI opens in a window that is
/// still 1280x820 physical pixels, leaving only 640x410 points of content.
pub(super) fn resize_initial_window(ctx: &egui::Context, zoom: f32) {
    let mut size = DESIGN_WINDOW_SIZE;
    if let Some(monitor) = ctx.input(|input| input.viewport().monitor_size) {
        // `monitor_size` is in points at the native scale (zoom was 1.0 when
        // it was measured), so divide by the zoom we just applied.
        let max_size = (monitor / zoom - egui::vec2(40.0, 80.0)).max(egui::vec2(320.0, 240.0));
        size = size.min(max_size);
    }
    ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(size));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requested_ui_scale_wins_and_is_clamped() {
        assert_eq!(resolve_ui_scale(Some(1.5), 2.0, Some(2.0)), 1.5);
        assert_eq!(resolve_ui_scale(Some(0.1), 1.0, None), MIN_UI_SCALE);
        assert_eq!(resolve_ui_scale(Some(99.0), 1.0, None), MAX_UI_SCALE);
    }

    #[test]
    fn native_scale_beats_desktop_fallback() {
        assert_eq!(resolve_ui_scale(None, 2.0, Some(1.5)), 2.0);
        assert_eq!(resolve_ui_scale(None, 1.0, Some(2.0)), 2.0);
        assert_eq!(resolve_ui_scale(None, 1.0, None), 1.0);
    }
}
