//! Small, session-only settings window.
use super::*;

#[derive(Default)]
pub(super) struct SettingsState {
    pub open: bool,
    pub(super) jar_open: bool,
    editor_open: bool,
    selected: Option<(String, String, String)>,
    url: String,
    cookie: String,
    error: Option<String>,
}

impl PerseidApp {
    pub(super) fn settings_window(&mut self, ctx: &egui::Context) {
        let mut open = self.settings.open;
        if open {
            egui::Window::new("Settings").open(&mut open).default_width(640.0).resizable(true).show(ctx, |ui| {
            ui.heading("Sessions");
            ui.separator();
            ui.add_space(8.0);
            ui.strong("Session handling");
            ui.label("Use stored cookies when sending requests from replay tools.");
            let Ok(mut jar) = self.cookie_jar.lock() else { ui.label("Cookie jar unavailable"); return; };
            ui.checkbox(&mut jar.apply, "Apply jar cookies to Repeater and Intruder");
            ui.small("Repeater replaces matching names. Intruder preserves template cookies and payloads.")
                .on_hover_text("Request editors show the original request, not changes made at send time. Parallel requests can race when cookies rotate.");
            ui.add_space(12.0);
            ui.separator();
            ui.add_space(8.0);
            ui.heading("Cookie jar");
            ui.label("Choose which traffic updates the cookie jar.");
            ui.horizontal(|ui| {
                ui.checkbox(&mut jar.update_proxy, "Proxy").on_hover_text("Observe requests and responses, even when history capture is paused.");
                ui.checkbox(&mut jar.update, "Repeater and Intruder").on_hover_text("Observe replay responses.");
            });
            ui.add_space(8.0);
            if ui.button("Open cookie jar").clicked() { self.settings.jar_open = true; }
            ui.add_space(12.0);
            ui.separator();
            ui.small("Session-only storage · Shared across projects · Never saved with projects")
                .on_hover_text("Existing captured requests may still contain cookies. Only proxied browser traffic is observed; HTTPS requires TLS interception.");
        });
        }
        self.settings.open = open;
        self.cookie_jar_window(ctx);
    }

    fn cookie_jar_window(&mut self, ctx: &egui::Context) {
        let mut open = self.settings.jar_open;
        if open {
            egui::Window::new("Cookie jar viewer")
                .open(&mut open)
                .default_size([900.0, 420.0])
                .show(ctx, |ui| {
                    let Ok(mut jar) = self.cookie_jar.lock() else {
                        ui.label("Cookie jar unavailable");
                        return;
                    };
                    let entries = jar.entries();
                    if !entries.iter().any(|e| {
                        self.settings.selected.as_ref()
                            == Some(&(e.0.clone(), e.1.clone(), e.2.clone()))
                    }) {
                        self.settings.selected = None;
                    }
                    ui.horizontal_top(|ui| {
                        let width = (ui.available_width() - 150.0).max(300.0);
                        ui.allocate_ui_with_layout(
                            egui::vec2(width, 340.0),
                            egui::Layout::top_down(egui::Align::Min),
                            |ui| {
                                use egui_extras::{Column, TableBuilder};
                                TableBuilder::new(ui)
                                    .striped(true)
                                    .resizable(true)
                                    .sense(egui::Sense::click())
                                    .column(Column::initial(115.0))
                                    .column(Column::initial(55.0))
                                    .column(Column::initial(100.0))
                                    .column(Column::remainder().at_least(80.0))
                                    .column(Column::initial(48.0))
                                    .column(Column::initial(65.0))
                                    .column(Column::initial(85.0))
                                    .min_scrolled_height(320.0)
                                    .max_scroll_height(320.0)
                                    .header(24.0, |mut row| {
                                        for title in [
                                            "Domain",
                                            "Path",
                                            "Name",
                                            "Value",
                                            "Secure",
                                            "HTTP only",
                                            "Expires",
                                        ] {
                                            row.col(|ui| {
                                                ui.strong(title);
                                            });
                                        }
                                    })
                                    .body(|body| {
                                        body.rows(22.0, entries.len(), |mut row| {
                                            let (domain, path, name, value, flags) =
                                                &entries[row.index()];
                                            let key = (domain.clone(), path.clone(), name.clone());
                                            row.set_selected(
                                                self.settings.selected.as_ref() == Some(&key),
                                            );
                                            for text in [
                                                domain.as_str(),
                                                path.as_str(),
                                                name.as_str(),
                                                value.as_str(),
                                                if flags.contains("Secure") { "Yes" } else { "" },
                                                if flags.contains("HttpOnly") {
                                                    "Yes"
                                                } else {
                                                    ""
                                                },
                                            ] {
                                                row.col(|ui| {
                                                    ui.add(egui::Label::new(text).truncate())
                                                        .on_hover_text(text);
                                                });
                                            }
                                            row.col(|ui| {
                                                ui.label(jar.expiration_label(domain, path, name));
                                            });
                                            if row.response().clicked() {
                                                self.settings.selected = Some(key);
                                            }
                                        });
                                    });
                            },
                        );
                        ui.vertical(|ui| {
                            ui.set_min_width(130.0);
                            if ui.button("Add cookie").clicked() {
                                self.settings.url.clear();
                                self.settings.cookie.clear();
                                self.settings.error = None;
                                self.settings.editor_open = true;
                            }
                            let selected = self.settings.selected.clone();
                            if ui
                                .add_enabled(selected.is_some(), egui::Button::new("Edit cookie"))
                                .clicked()
                                && let Some((d, p, n)) = &selected
                                && let Some((url, cookie)) = jar.edit_source(d, p, n)
                            {
                                self.settings.url = url;
                                self.settings.cookie = cookie;
                                self.settings.error = None;
                                self.settings.editor_open = true;
                            }
                            if ui
                                .add_enabled(selected.is_some(), egui::Button::new("Remove cookie"))
                                .clicked()
                                && let Some((d, p, n)) = selected
                            {
                                jar.remove(&d, &p, &n);
                                self.settings.selected = None;
                            }
                            ui.add_space(8.0);
                            if ui
                                .add_enabled(
                                    !entries.is_empty(),
                                    egui::Button::new("Empty cookie jar"),
                                )
                                .clicked()
                            {
                                jar.clear();
                                self.settings.selected = None;
                            }
                        });
                    });
                    ui.separator();
                    ui.label(format!("{} cookies · In memory only", entries.len()));
                });
        }
        self.settings.jar_open = open;
        let mut editor_open = self.settings.editor_open;
        let mut saved = false;
        if editor_open {
            egui::Window::new("Cookie editor")
                .open(&mut editor_open)
                .default_width(560.0)
                .show(ctx, |ui| {
                    ui.label("Response URL");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.settings.url)
                            .desired_width(f32::INFINITY),
                    );
                    ui.label("Set-Cookie");
                    ui.add(
                        egui::TextEdit::multiline(&mut self.settings.cookie)
                            .desired_rows(3)
                            .desired_width(f32::INFINITY),
                    );
                    ui.small("Include attributes such as Path, Domain, Secure and HttpOnly.");
                    if let Some(error) = &self.settings.error {
                        ui.colored_label(theme::RED, error);
                    }
                    if ui.button("Save cookie").clicked()
                        && let Ok(mut jar) = self.cookie_jar.lock()
                    {
                        match jar.insert(&self.settings.url, &self.settings.cookie) {
                            Ok(()) => {
                                self.settings.error = None;
                                saved = true;
                            }
                            Err(e) => self.settings.error = Some(e.into()),
                        }
                    }
                });
        }
        self.settings.editor_open = editor_open && !saved;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_window_renders_with_a_cookie() {
        let mut app = PerseidApp::test_app();
        app.settings.open = true;
        app.cookie_jar
            .lock()
            .unwrap()
            .insert("https://example.test/", "session=secret; Path=/")
            .unwrap();
        let ctx = egui::Context::default();
        let mut output = ctx.run_ui(egui::RawInput::default(), |_| app.settings_window(&ctx));
        assert!(!output.shapes.is_empty());
        output.textures_delta.clear();
        assert!(app.settings.open);
    }

    #[test]
    fn cookie_viewer_and_editor_render_without_settings_window() {
        let mut app = PerseidApp::test_app();
        app.settings.jar_open = true;
        app.settings.editor_open = true;
        app.cookie_jar
            .lock()
            .unwrap()
            .insert(
                "https://example.test/",
                "sid=value; Secure; HttpOnly; Path=/",
            )
            .unwrap();
        app.settings.selected = Some(("example.test".into(), "/".into(), "sid".into()));
        let ctx = egui::Context::default();
        let mut output = ctx.run_ui(egui::RawInput::default(), |_| app.settings_window(&ctx));
        output.textures_delta.clear();
        assert!(app.settings.selected.is_some());
        assert!(app.settings.jar_open && app.settings.editor_open);
        assert!(!app.settings.open);
        app.cookie_jar.lock().unwrap().clear();
        let mut output = ctx.run_ui(egui::RawInput::default(), |_| app.settings_window(&ctx));
        output.textures_delta.clear();
        assert!(app.settings.selected.is_none());
    }
}
