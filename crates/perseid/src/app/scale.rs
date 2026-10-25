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

    #[test]
    fn a_native_scale_at_or_below_one_does_not_count_as_hidpi() {
        assert_eq!(resolve_ui_scale(None, 0.5, None), 1.0);
        assert_eq!(resolve_ui_scale(None, 1.0, Some(1.25)), 1.25);
        // A non-finite native scale must not leak into the UI scale.
        assert_eq!(resolve_ui_scale(None, f32::NAN, Some(2.0)), 2.0);
        assert_eq!(resolve_ui_scale(None, f32::NAN, None), 1.0);
    }

    #[test]
    fn desktop_fallback_is_clamped_like_the_other_sources() {
        assert_eq!(resolve_ui_scale(None, 1.0, Some(0.1)), MIN_UI_SCALE);
        assert_eq!(resolve_ui_scale(None, 1.0, Some(99.0)), MAX_UI_SCALE);
    }

    #[test]
    fn stepped_scale_lands_on_clean_tenths() {
        assert_eq!(stepped_scale(1.0, 0.1), 1.1);
        assert_eq!(stepped_scale(1.0, -0.1), 0.9);
        assert_eq!(stepped_scale(1.04, 0.0), 1.0);
        assert_eq!(stepped_scale(1.06, 0.0), 1.1);
        assert_eq!(stepped_scale(1.11, 0.0), 1.1);
    }

    #[test]
    fn stepped_scale_does_not_clamp_itself() {
        // The caller clamps the result against MIN_UI_SCALE/MAX_UI_SCALE.
        assert_eq!(stepped_scale(MIN_UI_SCALE, -0.1), 0.4);
    }

    #[test]
    fn desktop_override_is_a_sane_scale_when_the_environment_provides_one() {
        // The environment is process-wide, so nothing is mutated here: the
        // test only pins the invariants of whatever GDK_SCALE/QT_SCALE_FACTOR
        // (if any) is already set.
        if let Some(scale) = desktop_scale_override() {
            assert!(scale.is_finite() && scale > 1.0);
            assert!((MIN_UI_SCALE..=MAX_UI_SCALE).contains(&scale));
        }
    }

    fn inner_size_command(ctx: &egui::Context) -> Option<egui::Vec2> {
        let output = ctx.run_pass(egui::RawInput::default(), |_| {});
        let size = output
            .viewport_output
            .get(&egui::ViewportId::ROOT)
            .and_then(|viewport| {
                viewport.commands.iter().find_map(|command| match command {
                    egui::ViewportCommand::InnerSize(size) => Some(*size),
                    _ => None,
                })
            });
        output.drop_without_applying_deltas();
        size
    }

    /// A context that has seen one frame reporting the given monitor size.
    fn context_with_monitor(monitor: egui::Vec2) -> egui::Context {
        let ctx = egui::Context::default();
        let mut input = egui::RawInput::default();
        input
            .viewports
            .get_mut(&egui::ViewportId::ROOT)
            .expect("the root viewport is present in default input")
            .monitor_size = Some(monitor);
        let output = ctx.run_pass(input, |_| {});
        output.drop_without_applying_deltas();
        ctx
    }

    #[test]
    fn resize_initial_window_uses_the_design_size_without_a_monitor() {
        let ctx = egui::Context::default();
        resize_initial_window(&ctx, 1.0);
        assert_eq!(inner_size_command(&ctx), Some(DESIGN_WINDOW_SIZE));
    }

    #[test]
    fn resize_initial_window_keeps_the_content_size_when_zoomed() {
        let ctx = context_with_monitor(egui::vec2(2560.0, 1440.0));
        resize_initial_window(&ctx, 2.0);
        assert_eq!(inner_size_command(&ctx), Some(egui::vec2(1240.0, 640.0)));
    }

    #[test]
    fn resize_initial_window_never_goes_below_the_content_floor() {
        let ctx = context_with_monitor(egui::vec2(100.0, 100.0));
        resize_initial_window(&ctx, 1.0);
        assert_eq!(inner_size_command(&ctx), Some(egui::vec2(320.0, 240.0)));
    }
}
