//! macOS application menu. Events are forwarded into the egui event loop.
use super::project_ui::{ProjectDialogKind, ProjectSwitch};
use super::*;
use muda::accelerator::{Accelerator, Code, Modifiers};
use muda::{Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use std::sync::mpsc::{self, Receiver};

pub(super) struct NativeMenu {
    _menu: Menu,
    events: Receiver<String>,
    reset: MenuItem,
    fullscreen: MenuItem,
}

impl NativeMenu {
    fn new(ctx: &egui::Context) -> Self {
        let item = |id, label| MenuItem::with_id(id, label, true, None);
        let menu = Menu::new();
        let app = Submenu::with_items(
            "Perseid",
            true,
            &[
                &item("about", "About Perseid"),
                &PredefinedMenuItem::separator(),
                &PredefinedMenuItem::hide(None),
                &PredefinedMenuItem::hide_others(None),
                &PredefinedMenuItem::show_all(None),
                &PredefinedMenuItem::separator(),
                &MenuItem::with_id(
                    "quit",
                    "Quit Perseid",
                    true,
                    Some(Accelerator::new(Modifiers::META, Code::KeyQ)),
                ),
            ],
        )
        .expect("create application menu");
        let file = Submenu::with_items(
            "File",
            true,
            &[
                &item("new", "New Project"),
                &item("open", "Open Project…"),
                &PredefinedMenuItem::separator(),
                &item("save_as", "Save Project As…"),
                &item("save_copy", "Save Project Copy…"),
            ],
        )
        .expect("create File menu");
        let reset = item("reset", "Reset Zoom");
        let fullscreen = item("fullscreen", "Enter Full Screen");
        let view = Submenu::with_items(
            "View",
            true,
            &[
                &item("zoom_in", "Zoom In"),
                &item("zoom_out", "Zoom Out"),
                &reset,
                &PredefinedMenuItem::separator(),
                &fullscreen,
            ],
        )
        .expect("create View menu");
        let help = Submenu::with_items(
            "Help",
            true,
            &[
                &item("about", "About Perseid"),
                &item("github", "Project on GitHub"),
            ],
        )
        .expect("create Help menu");
        menu.append_items(&[&app, &file, &view, &help])
            .expect("create menu bar");
        let (sender, events) = mpsc::channel();
        let ctx = ctx.clone();
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            let _ = sender.send(event.id.0);
            ctx.request_repaint();
        }));
        menu.init_for_nsapp();
        Self {
            _menu: menu,
            events,
            reset,
            fullscreen,
        }
    }
}

impl PerseidApp {
    pub(super) fn poll_native_menu(&mut self, ctx: &egui::Context) {
        let menu = self.native_menu.get_or_insert_with(|| NativeMenu::new(ctx));
        let fullscreen = ctx.input(|i| i.viewport().fullscreen.unwrap_or(false));
        menu.fullscreen.set_text(if fullscreen {
            "Exit Full Screen"
        } else {
            "Enter Full Screen"
        });
        menu.reset
            .set_enabled((ctx.pixels_per_point() - self.base_ui_scale).abs() > 1e-3);
        let events: Vec<_> = menu.events.try_iter().collect();
        for event in events {
            self.handle_native_menu_action(ctx, &event);
        }
    }

    fn handle_native_menu_action(&mut self, ctx: &egui::Context, event: &str) {
        match event {
            "about" => self.about_open = true,
            "quit" => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            "new" => self.request_project_switch(ProjectSwitch::New),
            "open" | "save_as" | "save_copy" => {
                let kind = match event {
                    "open" => ProjectDialogKind::Open,
                    "save_as" => ProjectDialogKind::SaveAs,
                    _ => ProjectDialogKind::SaveCopy,
                };
                let current = self.current_project_path();
                self.project.open_dialog(kind, current.as_deref());
            }
            "zoom_in" => self.set_ui_scale(ctx, stepped_scale(ctx.pixels_per_point(), 0.1)),
            "zoom_out" => self.set_ui_scale(ctx, stepped_scale(ctx.pixels_per_point(), -0.1)),
            "reset" => self.set_ui_scale(ctx, self.base_ui_scale),
            "fullscreen" => {
                let fullscreen = ctx.input(|i| i.viewport().fullscreen.unwrap_or(false));
                ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(!fullscreen))
            }
            "github" => ctx.open_url(egui::OpenUrl::new_tab(branding::REPOSITORY_URL)),
            _ => log::warn!("unknown native menu action: {event}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_menu_actions_reach_application_state() {
        let mut app = PerseidApp::test_app();
        let ctx = egui::Context::default();
        app.handle_native_menu_action(&ctx, "about");
        assert!(app.about_open);
        app.handle_native_menu_action(&ctx, "unknown");
        assert!(app.about_open);
    }
}
