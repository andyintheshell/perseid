//! Native utility viewports sharing the main app's state (no cloned models).

use egui::{Key, ViewportBuilder, ViewportCommand, ViewportId};
use serde::{Deserialize, Serialize};

use crate::theme;

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
struct Geometry {
    position: Option<[f32; 2]>,
    position_pixels_per_point: Option<f32>,
    size: Option<[f32; 2]>,
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::BTreeMap;
    use std::rc::Rc;

    use eframe::Storage as _;

    use super::*;

    #[derive(Default)]
    struct Storage(BTreeMap<String, String>);

    impl eframe::Storage for Storage {
        fn get_string(&self, key: &str) -> Option<String> {
            self.0.get(key).cloned()
        }
        fn set_string(&mut self, key: &str, value: String) {
            self.0.insert(key.to_owned(), value);
        }
        fn remove_string(&mut self, key: &str) {
            self.0.remove(key);
        }
        fn flush(&mut self) {}
    }

    #[test]
    fn app_settings_save_only_geometry_not_the_shared_workspace() {
        let mut app = crate::app::PerseidApp::test_app();
        app.search.query = "sensitive captured message".to_owned();
        let mut storage = Storage::default();
        eframe::App::save(&mut app, &mut storage);
        assert_eq!(storage.0.len(), 2);
        assert!(storage.0.contains_key("perseid.search.geometry"));
        assert!(storage.0.contains_key("perseid.comparer.geometry"));
        assert!(storage.0.values().all(|value| !value.contains("sensitive")));
        assert!(!eframe::App::persist_egui_memory(&app));
    }

    #[test]
    fn geometry_round_trips_separately_for_each_window() {
        let mut search = UtilityWindow::default();
        search.observe(
            &egui::ViewportInfo {
                outer_rect: Some(egui::Rect::from_min_size(
                    egui::pos2(-1200.0, 80.0),
                    egui::vec2(900.0, 700.0),
                )),
                ..Default::default()
            },
            egui::vec2(900.0, 670.0),
            1.0,
        );
        let mut storage = Storage::default();
        search.save(&mut storage, "search");
        UtilityWindow::default().save(&mut storage, "comparer");
        let mut restored = UtilityWindow::default();
        restored.load(&storage, "search");
        assert_eq!(restored.geometry, search.geometry);
        assert!(!restored.visible);
        restored.load(&storage, "comparer");
        assert_eq!(restored.geometry, Geometry::default());
        storage.set_string("search", "invalid".to_owned());
        restored.load(&storage, "search");
        assert_eq!(restored.geometry, Geometry::default());
    }

    #[test]
    fn wayland_size_is_remembered_without_a_global_position() {
        let mut window = UtilityWindow::default();
        window.observe(
            &egui::ViewportInfo::default(),
            egui::vec2(880.0, 620.0),
            1.0,
        );
        assert_eq!(window.geometry.position, None);
        assert_eq!(window.geometry.size, Some([880.0, 620.0]));
        let original = window.geometry;
        for info in [
            egui::ViewportInfo {
                minimized: Some(true),
                ..Default::default()
            },
            egui::ViewportInfo {
                maximized: Some(true),
                ..Default::default()
            },
            egui::ViewportInfo {
                fullscreen: Some(true),
                ..Default::default()
            },
        ] {
            window.observe(&info, egui::vec2(1920.0, 1080.0), 1.0);
            assert_eq!(window.geometry, original);
        }
        window.observe(
            &egui::ViewportInfo::default(),
            egui::vec2(f32::NAN, 0.0),
            1.0,
        );
        assert_eq!(window.geometry, original);
    }

    #[test]
    fn invalid_saved_geometry_cannot_create_an_invalid_native_window() {
        let window = UtilityWindow {
            spawn_geometry: Geometry {
                position: Some([f32::NAN, 0.0]),
                size: Some([-1.0, 0.0]),
                ..Default::default()
            },
            ..Default::default()
        };
        let builder = window.builder(
            "Search",
            egui::vec2(1000.0, 660.0),
            egui::vec2(680.0, 430.0),
        );
        assert_eq!(builder.position, None);
        assert_eq!(builder.inner_size, Some(egui::vec2(1000.0, 660.0)));
        assert_eq!(builder.decorations, Some(true));
        assert_eq!(builder.resizable, Some(true));
    }

    #[test]
    fn position_restoration_preserves_desktop_pixels_across_dpi_changes() {
        let window = UtilityWindow {
            spawn_geometry: Geometry {
                position: Some([-1200.0, 80.0]),
                position_pixels_per_point: Some(2.0),
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(
            window.restore_position(1.0),
            Some(egui::pos2(-2400.0, 160.0))
        );
        assert_eq!(
            window.restore_position(2.0),
            Some(egui::pos2(-1200.0, 80.0))
        );
        assert_eq!(window.restore_position(0.0), None);
    }

    #[derive(Default)]
    struct NativeHarness {
        info: egui::ViewportInfo,
        events: Vec<egui::Event>,
        builders: Vec<(ViewportId, ViewportBuilder)>,
        root_focused: bool,
    }

    fn native_context() -> (egui::Context, Rc<RefCell<NativeHarness>>) {
        let ctx = egui::Context::default();
        ctx.set_embed_viewports(false);
        let harness = Rc::new(RefCell::new(NativeHarness::default()));
        let shared = harness.clone();
        egui::Context::set_immediate_viewport_renderer(move |ctx, mut viewport| {
            let mut input = egui::RawInput {
                viewport_id: viewport.ids.this,
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1200.0, 800.0),
                )),
                ..Default::default()
            };
            {
                let mut harness = shared.borrow_mut();
                let mut info = harness.info.clone();
                info.parent = Some(viewport.ids.parent);
                input.viewports.insert(viewport.ids.this, info);
                input.events = harness.events.clone();
                harness
                    .builders
                    .push((viewport.ids.this, viewport.builder.clone()));
            }
            let mut output = ctx.run_ui(input, |ui| (viewport.viewport_ui_cb)(ui));
            output.textures_delta.clear();
            if output
                .viewport_output
                .get(&ViewportId::ROOT)
                .is_some_and(|output| output.commands.contains(&ViewportCommand::Focus))
            {
                shared.borrow_mut().root_focused = true;
            }
        });
        (ctx, harness)
    }

    fn render(
        ctx: &egui::Context,
        window: &mut UtilityWindow,
        open: &mut bool,
    ) -> egui::FullOutput {
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            window.show(
                ui.ctx(),
                "test.utility",
                "Utility",
                open,
                [egui::vec2(1000.0, 660.0), egui::vec2(680.0, 430.0)],
                |ui| {
                    ui.label("Shared state");
                },
            );
        });
        output.textures_delta.clear();
        output
    }

    #[test]
    fn native_window_is_independent_and_geometry_only_reapplies_on_reopen() {
        let (ctx, harness) = native_context();
        let mut window = UtilityWindow::default();
        let mut open = true;
        render(&ctx, &mut window, &mut open);
        harness.borrow_mut().info.outer_rect = Some(egui::Rect::from_min_size(
            egui::pos2(1900.0, 100.0),
            egui::vec2(1400.0, 900.0),
        ));
        render(&ctx, &mut window, &mut open);
        render(&ctx, &mut window, &mut open);
        assert_eq!(
            harness.borrow().builders[0].0,
            ViewportId::from_hash_of("test.utility")
        );
        assert_ne!(harness.borrow().builders[0].0, ViewportId::ROOT);
        assert!(
            harness
                .borrow()
                .builders
                .iter()
                .all(|(_, builder)| builder.position.is_none())
        );
        open = false;
        render(&ctx, &mut window, &mut open);
        open = true;
        render(&ctx, &mut window, &mut open);
        assert_eq!(
            harness.borrow().builders.last().unwrap().1.position,
            Some(egui::pos2(1900.0, 100.0))
        );
        assert_eq!(
            harness.borrow().builders.last().unwrap().1.inner_size,
            Some(egui::vec2(1200.0, 800.0))
        );
    }

    #[test]
    fn only_the_focused_child_handles_escape_and_native_close_restores_parent_focus() {
        for native_close in [false, true] {
            let (ctx, harness) = native_context();
            let mut window = UtilityWindow::default();
            let mut open = true;
            harness.borrow_mut().events = vec![egui::Event::Key {
                key: Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }];
            harness.borrow_mut().info.focused = Some(false);
            render(&ctx, &mut window, &mut open);
            assert!(open, "unfocused child must ignore Escape");
            harness.borrow_mut().events.clear();
            render(&ctx, &mut window, &mut open);
            if native_close {
                harness.borrow_mut().info.events = vec![egui::ViewportEvent::Close];
            } else {
                harness.borrow_mut().info.focused = Some(true);
                harness.borrow_mut().events = vec![egui::Event::Key {
                    key: Key::Escape,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }];
            }
            let output = render(&ctx, &mut window, &mut open);
            assert!(!open);
            assert!(!window.visible);
            assert!(
                harness.borrow().root_focused
                    || output.viewport_output[&ViewportId::ROOT]
                        .commands
                        .contains(&ViewportCommand::Focus)
            );
            assert!(
                !output.viewport_output[&ViewportId::ROOT]
                    .commands
                    .contains(&ViewportCommand::Close)
            );
        }
    }
}

#[derive(Default)]
pub(super) struct UtilityWindow {
    geometry: Geometry,
    /// Keep the builder stable while open: otherwise observed moves/resizes
    /// are sent back to the window manager as new positioning commands.
    spawn_geometry: Geometry,
    visible: bool,
    focus_requested: bool,
}

impl UtilityWindow {
    pub(super) fn load(&mut self, storage: &dyn eframe::Storage, key: &str) {
        self.geometry = eframe::get_value(storage, key).unwrap_or_default();
    }

    pub(super) fn save(&self, storage: &mut dyn eframe::Storage, key: &str) {
        eframe::set_value(storage, key, &self.geometry);
    }

    pub(super) fn request_focus(&mut self) {
        self.focus_requested = true;
    }

    pub(super) fn hide(&mut self) {
        self.visible = false;
        self.focus_requested = false;
    }

    pub(super) fn show(
        &mut self,
        ctx: &egui::Context,
        id: &'static str,
        title: &str,
        open: &mut bool,
        sizes: [egui::Vec2; 2],
        mut contents: impl FnMut(&mut egui::Ui),
    ) {
        if !*open {
            self.hide();
            return;
        }
        let [default_size, min_size] = sizes;
        // The embedded path is for headless tests, documentation screenshots,
        // and backends without native multi-viewport support.
        if ctx.embed_viewports() {
            if ctx.input(|input| input.key_pressed(Key::Escape)) {
                *open = false;
            } else {
                egui::Window::new(title)
                    .id(egui::Id::unique(id))
                    .open(open)
                    .collapsible(false)
                    .resizable(true)
                    .default_size(default_size)
                    .min_size(min_size)
                    .show(ctx, |ui| contents(ui));
            }
            return;
        }

        let opening = !self.visible;
        if opening {
            self.spawn_geometry = self.geometry;
            self.focus_requested = true;
        }
        self.visible = true;
        let viewport_id = ViewportId::from_hash_of(id);
        let builder = self.builder(title, default_size, min_size);
        let focus = std::mem::take(&mut self.focus_requested);
        ctx.show_viewport_immediate(viewport_id, builder, |ui, _class| {
            let child = ui.ctx();
            self.observe(
                &child.input(|input| input.viewport().clone()),
                child.content_rect().size(),
                child.pixels_per_point(),
            );
            let close = child.input(|input| {
                input.viewport().close_requested()
                    || (input.viewport().focused == Some(true) && input.key_pressed(Key::Escape))
            });
            if close {
                *open = false;
                // Omitting the viewport next frame destroys the native window.
                // Do not send Close to ROOT: that would quit the application.
                child.send_viewport_cmd_to(child.parent_viewport_id(), ViewportCommand::Focus);
                child.request_repaint_of(child.parent_viewport_id());
            } else {
                if opening && let Some(position) = self.restore_position(child.pixels_per_point()) {
                    // The OS may initially create this window on a monitor
                    // with a different DPI. Correct placement using its real
                    // scale once it exists, as eframe does for the root window.
                    child.send_viewport_cmd(ViewportCommand::OuterPosition(position));
                }
                if focus {
                    child.send_viewport_cmd(ViewportCommand::Focus);
                }
                egui::CentralPanel::default()
                    .frame(egui::Frame::NONE.fill(theme::CANVAS).inner_margin(6))
                    .show(ui, |ui| contents(ui));
            }
        });
        if !*open {
            self.hide();
        }
    }

    fn builder(
        &self,
        title: &str,
        default_size: egui::Vec2,
        min_size: egui::Vec2,
    ) -> ViewportBuilder {
        let size = self
            .spawn_geometry
            .size
            .filter(|size| size.iter().all(|n| n.is_finite() && *n > 0.0))
            .map(egui::Vec2::from)
            .unwrap_or(default_size)
            .max(min_size);
        let mut builder = ViewportBuilder::default()
            .with_title(title)
            .with_decorations(true)
            .with_resizable(true)
            .with_inner_size(size)
            .with_min_inner_size(min_size);
        if let Some(pos) = self
            .spawn_geometry
            .position
            .filter(|pos| pos.iter().all(|n| n.is_finite()))
        {
            builder = builder.with_position(pos);
        }
        builder
    }

    fn restore_position(&self, pixels_per_point: f32) -> Option<egui::Pos2> {
        let position = egui::Pos2::from(self.spawn_geometry.position?);
        let scale = self.spawn_geometry.position_pixels_per_point?;
        (position.is_finite()
            && scale.is_finite()
            && scale > 0.0
            && pixels_per_point.is_finite()
            && pixels_per_point > 0.0)
            .then_some(position * (scale / pixels_per_point))
    }

    fn observe(
        &mut self,
        info: &egui::ViewportInfo,
        content_size: egui::Vec2,
        pixels_per_point: f32,
    ) {
        // Do not replace the normal geometry with a minimized/maximized size.
        if info.minimized == Some(true)
            || info.maximized == Some(true)
            || info.fullscreen == Some(true)
        {
            return;
        }
        let size = info.inner_rect.map_or(content_size, |rect| rect.size());
        if size.is_finite() && size.x > 0.0 && size.y > 0.0 {
            self.geometry.size = Some(size.into());
        }
        // Wayland may not expose global positions; still remember the size.
        if let Some(rect) = info
            .outer_rect
            .filter(|rect| rect.is_finite() && rect.is_positive())
        {
            self.geometry.position = Some(rect.min.into());
            self.geometry.position_pixels_per_point = Some(pixels_per_point);
        }
    }
}
