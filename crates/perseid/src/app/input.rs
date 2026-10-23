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

        let (target, proxy, intruder, repeater, decoder, comparer) = ctx.input_mut(|input| {
            (
                input.consume_shortcut(&TAB_TARGET_SHORTCUT),
                input.consume_shortcut(&TAB_PROXY_SHORTCUT),
                input.consume_shortcut(&TAB_INTRUDER_SHORTCUT),
                input.consume_shortcut(&TAB_REPEATER_SHORTCUT),
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

        if ctx.egui_wants_keyboard_input() || !on_history {
            return;
        }

        let down =
            ctx.input(|input| input.key_pressed(Key::ArrowDown) || input.key_pressed(Key::J));
        let up = ctx.input(|input| input.key_pressed(Key::ArrowUp) || input.key_pressed(Key::K));
        let home = ctx.input(|input| input.key_pressed(Key::Home));
        let end = ctx.input(|input| input.key_pressed(Key::End));
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
