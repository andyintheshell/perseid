//! Keyboard shortcuts and UI zoom handling.

use super::scale::stepped_scale;
use super::*;

impl PerseidApp {
    pub(super) fn keyboard(&mut self, ctx: &egui::Context) {
        self.zoom_shortcuts(ctx);

        #[cfg(target_os = "macos")]
        if ctx.input_mut(|input| input.consume_shortcut(&FULL_SCREEN_SHORTCUT)) {
            let fullscreen = ctx.input(|input| input.viewport().fullscreen.unwrap_or(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(!fullscreen));
        }

        if ctx.input_mut(|input| input.consume_shortcut(&SEARCH_SHORTCUT)) {
            self.search.focus_or_open();
        }

        let (target, proxy, intruder, repeater, sequencer, decoder, comparer) =
            ctx.input_mut(|input| {
                (
                    input.consume_shortcut(&TAB_TARGET_SHORTCUT),
                    input.consume_shortcut(&TAB_PROXY_SHORTCUT),
                    input.consume_shortcut(&TAB_INTRUDER_SHORTCUT),
                    input.consume_shortcut(&TAB_REPEATER_SHORTCUT),
                    input.consume_shortcut(&TAB_SEQUENCER_SHORTCUT),
                    input.consume_shortcut(&TAB_DECODER_SHORTCUT),
                    input.consume_shortcut(&TAB_COMPARER_SHORTCUT),
                )
            });
        if target {
            self.main_tab = MainTab::Target;
        }
        if proxy {
            self.main_tab = MainTab::Proxy;
        }
        if intruder {
            self.main_tab = MainTab::Intruder;
        }
        if repeater {
            self.main_tab = MainTab::Repeater;
        }
        if sequencer {
            self.main_tab = MainTab::Sequencer;
        }
        if decoder {
            self.main_tab = MainTab::Decoder;
        }
        if comparer {
            self.main_tab = MainTab::Comparer;
        }

        // Ctrl+Enter is the Burp-style send gesture. Consume it before
        // `egui_wants_keyboard_input` so it works while an editor has focus.
        if self.main_tab == MainTab::Intruder
            && ctx.input_mut(|input| input.consume_key(egui::Modifiers::COMMAND, Key::Enter))
        {
            self.toggle_intruder_attack();
        } else if self.main_tab == MainTab::Repeater
            && ctx.input_mut(|input| input.consume_key(egui::Modifiers::COMMAND, Key::Enter))
        {
            self.send_active_repeater();
        }

        let on_http_history =
            self.main_tab == MainTab::Proxy && self.proxy_section == ProxySection::HttpHistory;
        let on_websocket_history = self.main_tab == MainTab::Proxy
            && self.proxy_section == ProxySection::WebSocketsHistory;
        let on_history = on_http_history || on_websocket_history;
        let on_site_map = self.main_tab == MainTab::Target
            && self.target_section == TargetSection::SiteMap
            && self.site_map_ui.on_contents()
            && !self.site_map_ui.windows_open();
        if on_site_map {
            let search_id = egui::Id::unique(super::target::SITE_MAP_SEARCH_ID);
            if ctx.input_mut(|input| input.consume_key(egui::Modifiers::COMMAND, Key::F)) {
                ctx.memory_mut(|memory| memory.request_focus(search_id));
                return;
            }
            if ctx.memory(|memory| memory.focused()) == Some(search_id) {
                if ctx.input(|input| input.key_pressed(Key::Escape)) {
                    ctx.memory_mut(|memory| memory.surrender_focus(search_id));
                }
                return;
            }
        }
        if on_history {
            let filter_id = egui::Id::unique(if on_http_history {
                FILTER_ID
            } else {
                WEBSOCKET_FILTER_ID
            });
            if ctx.input_mut(|input| input.consume_key(egui::Modifiers::COMMAND, Key::F)) {
                ctx.memory_mut(|memory| memory.request_focus(filter_id));
                return;
            }
            if ctx.memory(|memory| memory.focused()) == Some(filter_id) {
                if ctx.input(|input| input.key_pressed(Key::Escape)) {
                    ctx.memory_mut(|memory| memory.surrender_focus(filter_id));
                }
                return;
            }
        }

        if ctx.egui_wants_keyboard_input() || !(on_history || on_site_map) {
            return;
        }

        let down =
            ctx.input(|input| input.key_pressed(Key::ArrowDown) || input.key_pressed(Key::J));
        let up = ctx.input(|input| input.key_pressed(Key::ArrowUp) || input.key_pressed(Key::K));
        let home = ctx.input(|input| input.key_pressed(Key::Home));
        let end = ctx.input(|input| input.key_pressed(Key::End));
        if on_site_map {
            if down {
                self.move_site_map_selection(1);
            }
            if up {
                self.move_site_map_selection(-1);
            }
            if home {
                self.select_site_map_edge(true);
            }
            if end {
                self.select_site_map_edge(false);
            }
            return;
        }
        if on_http_history {
            if down {
                self.move_selection(1);
            }
            if up {
                self.move_selection(-1);
            }
            if home && !self.visible.is_empty() {
                self.select_id(self.visible[0]);
            }
            if end && !self.visible.is_empty() {
                self.select_id(self.visible[self.visible.len() - 1]);
            }
        } else {
            if down {
                self.move_websocket_selection(1);
            }
            if up {
                self.move_websocket_selection(-1);
            }
            if home && !self.websocket_visible.is_empty() {
                self.select_websocket_id(self.websocket_visible[0]);
            }
            if end && !self.websocket_visible.is_empty() {
                self.select_websocket_id(self.websocket_visible[self.websocket_visible.len() - 1]);
            }
        }
    }

    /// Set the total UI scale (system scale times zoom) in physical pixels per
    /// point, clamped to the supported range.
    pub(super) fn set_ui_scale(&self, ctx: &egui::Context, scale: f32) {
        let scale = scale.clamp(MIN_UI_SCALE, MAX_UI_SCALE);
        let zoom = (scale / self.native_ui_scale).clamp(0.2, 5.0);
        ctx.set_zoom_factor(zoom);
    }

    pub(super) fn zoom_shortcuts(&mut self, ctx: &egui::Context) {
        let (zoom_in, zoom_out, reset) = ctx.input_mut(|input| {
            (
                input.consume_shortcut(&ZOOM_IN_SHORTCUT)
                    || input.consume_shortcut(&ZOOM_IN_SECONDARY_SHORTCUT),
                input.consume_shortcut(&ZOOM_OUT_SHORTCUT),
                input.consume_shortcut(&ZOOM_RESET_SHORTCUT),
            )
        });
        if reset {
            self.set_ui_scale(ctx, self.base_ui_scale);
        } else if zoom_in {
            self.set_ui_scale(ctx, stepped_scale(ctx.pixels_per_point(), 0.1));
        } else if zoom_out {
            self.set_ui_scale(ctx, stepped_scale(ctx.pixels_per_point(), -0.1));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(key: Key) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }
    }

    fn shortcut(key: Key) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::COMMAND,
        }
    }

    fn keyboard_frame(ctx: &egui::Context, app: &mut PerseidApp, events: Vec<egui::Event>) {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1400.0, 900.0),
            )),
            events,
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |_ui| app.keyboard(ctx));
        output.textures_delta.clear();
    }

    fn site_map_app() -> PerseidApp {
        let mut app = PerseidApp::test_app();
        app.main_tab = MainTab::Target;
        app.target_section = TargetSection::SiteMap;
        app.site_map_ui.set_visible_ids(vec![10, 20, 30]);
        app
    }

    #[test]
    fn site_map_keyboard_navigates_visible_contents_rows() {
        let mut app = site_map_app();
        let ctx = egui::Context::default();

        keyboard_frame(&ctx, &mut app, vec![key(Key::ArrowDown)]);
        assert_eq!(app.selected, Some(10));
        keyboard_frame(&ctx, &mut app, vec![key(Key::J)]);
        assert_eq!(app.selected, Some(20));
        keyboard_frame(&ctx, &mut app, vec![key(Key::ArrowUp)]);
        assert_eq!(app.selected, Some(10));
        keyboard_frame(&ctx, &mut app, vec![key(Key::End)]);
        assert_eq!(app.selected, Some(30));
        keyboard_frame(&ctx, &mut app, vec![key(Key::Home)]);
        assert_eq!(app.selected, Some(10));

        // The site map only owns the keyboard while Contents is active.
        app.site_map_ui.set_issues_tab();
        keyboard_frame(&ctx, &mut app, vec![key(Key::ArrowDown)]);
        assert_eq!(app.selected, Some(10));

        // Open floating windows also own the keyboard.
        app.site_map_ui.set_contents_tab();
        app.site_map_ui.set_windows_open(true);
        keyboard_frame(&ctx, &mut app, vec![key(Key::ArrowDown)]);
        assert_eq!(app.selected, Some(10));
    }

    #[test]
    fn site_map_search_shortcut_focuses_and_escape_releases_the_field() {
        let mut app = site_map_app();
        let ctx = egui::Context::default();
        let search_id = egui::Id::unique(super::super::target::SITE_MAP_SEARCH_ID);

        keyboard_frame(&ctx, &mut app, vec![shortcut(Key::F)]);
        assert_eq!(ctx.memory(|memory| memory.focused()), Some(search_id));

        // While the search field is focused, site-map navigation keys are
        // routed to the text edit instead of moving the table selection.
        keyboard_frame(&ctx, &mut app, vec![key(Key::ArrowDown)]);
        assert_eq!(app.selected, None);

        keyboard_frame(&ctx, &mut app, vec![key(Key::Escape)]);
        assert_eq!(ctx.memory(|memory| memory.focused()), None);
        keyboard_frame(&ctx, &mut app, vec![key(Key::ArrowDown)]);
        assert_eq!(app.selected, Some(10));
    }

    #[test]
    fn site_map_keyboard_is_inactive_on_other_tabs_and_sections() {
        let mut app = site_map_app();
        app.target_section = TargetSection::Scope;
        let ctx = egui::Context::default();
        keyboard_frame(&ctx, &mut app, vec![key(Key::ArrowDown)]);
        assert_eq!(app.selected, None);

        app.target_section = TargetSection::SiteMap;
        app.main_tab = MainTab::Proxy;
        keyboard_frame(&ctx, &mut app, vec![key(Key::ArrowDown)]);
        assert_eq!(app.selected, None);
    }
}
