//! Small, session-only settings window.
use super::*;

#[derive(Default)]
pub(super) struct SettingsState {
    pub open: bool,
    url: String,
    cookie: String,
    error: Option<String>,
}

impl PerseidApp {
    pub(super) fn settings_window(&mut self, ctx: &egui::Context) {
        if !self.settings.open {
            return;
        }
        let mut open = self.settings.open;
        egui::Window::new("Settings").open(&mut open).default_width(720.0).resizable(true).show(ctx, |ui| {
            ui.heading("Cookie jar");
            ui.label("Stored in memory until Perseid closes; not saved with projects. Existing captured requests may still contain cookies.");
            let Ok(mut jar) = self.cookie_jar.lock() else { ui.label("Cookie jar unavailable"); return; };
            ui.checkbox(&mut jar.update_proxy, "Update jar from Proxy requests and responses");
            ui.checkbox(&mut jar.apply, "Apply jar cookies to Repeater and Intruder");
            ui.checkbox(&mut jar.update, "Update jar from Repeater and Intruder responses");
            ui.label("Repeater: matching jar names replace request cookies. Intruder: request cookies take precedence, preserving payloads.");
            ui.label("Parallel requests can race when the server rotates cookies. Only traffic through Proxy is observed.");
            ui.label("Request editors show the original request, not jar changes made at send time.");
            ui.separator();
            ui.label("Add or replace a cookie using its response URL and a Set-Cookie value:");
            ui.horizontal(|ui| { ui.label("URL"); ui.text_edit_singleline(&mut self.settings.url); });
            ui.horizontal(|ui| { ui.label("Set-Cookie"); ui.text_edit_singleline(&mut self.settings.cookie); });
            ui.horizontal(|ui| {
                if ui.button("Add / update").clicked() {
                    match jar.insert(&self.settings.url, &self.settings.cookie) {
                        Ok(()) => { self.settings.error = None; self.settings.cookie.clear(); }
                        Err(e) => self.settings.error = Some(e.into()),
                    }
                }
                if ui.button("Clear jar").clicked() { jar.clear(); }
            });
            if let Some(error) = &self.settings.error { ui.colored_label(theme::RED, error); }
            ui.separator();
            egui::ScrollArea::vertical().max_height(300.0).show(ui, |ui| {
                for (domain, path, name, value, flags) in jar.entries() {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(format!("{domain} {path} {name}={value} {flags}"));
                        if ui.small_button("Edit").clicked() && let Some((url, cookie)) = jar.edit_source(&domain, &path, &name) {
                            self.settings.url = url;
                            self.settings.cookie = cookie;
                        }
                        if ui.small_button("Delete").clicked() { jar.remove(&domain, &path, &name); }
                    });
                }
            });
        });
        self.settings.open = open;
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
}
