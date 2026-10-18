//! The egui application: proxy history table plus request/response panes.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use egui::{Align, Color32, Key, Label, RichText, TextEdit};
use egui_extras::{Column, TableBuilder};
use wiremoth_proxy::{Proxy, ProxyConfig};

use crate::Args;
use crate::branding;
use crate::history::{History, HistoryItem};
use crate::view;

/// Bound on events applied per frame so a flood cannot stall rendering.
const MAX_EVENTS_PER_FRAME: usize = 1024;
const FILTER_ID: &str = "wiremoth.filter";
const RESPONSE_PANE_ID: &str = "wiremoth.detail.response";
const ORANGE: Color32 = Color32::from_rgb(255, 176, 80);

/// Content size (in egui points) the window aims for at startup.
const DESIGN_WINDOW_SIZE: egui::Vec2 = egui::vec2(1280.0, 820.0);
const MIN_UI_SCALE: f32 = 0.5;
const MAX_UI_SCALE: f32 = 5.0;

const ZOOM_IN_SHORTCUT: egui::KeyboardShortcut =
    egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, Key::Plus);
const ZOOM_IN_SECONDARY_SHORTCUT: egui::KeyboardShortcut =
    egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, Key::Equals);
const ZOOM_OUT_SHORTCUT: egui::KeyboardShortcut =
    egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, Key::Minus);
const ZOOM_RESET_SHORTCUT: egui::KeyboardShortcut =
    egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, Key::Num0);

pub struct WiremothApp {
    proxy: Option<Proxy>,
    proxy_error: Option<String>,
    notify: Arc<dyn Fn() + Send + Sync>,
    listen: SocketAddr,
    capture_limit: usize,
    history: History,
    paused: bool,
    filter: String,
    visible: Vec<u64>,
    visible_key: Option<(String, u64)>,
    selected: Option<u64>,
    scroll_to_selected: bool,
    detail: Option<DetailCache>,
    filter_id: Option<egui::Id>,
    /// Scale reported by the windowing system (physical pixels per point).
    native_ui_scale: f32,
    /// Scale chosen at startup (CLI flag or auto-detected); Ctrl+0 resets here.
    base_ui_scale: f32,
    /// Toolbar brand mark and full logo lockup for the About window.
    mark: Option<egui::TextureHandle>,
    lockup: Option<egui::TextureHandle>,
    about_open: bool,
}

/// Formatted request/response text for the selected exchange, rebuilt only
/// when the selection or the history changes.
struct DetailCache {
    id: u64,
    revision: u64,
    request: String,
    response: String,
}

impl WiremothApp {
    pub fn new(cc: &eframe::CreationContext<'_>, args: Args) -> Self {
        let native_ui_scale = cc.egui_ctx.native_pixels_per_point().unwrap_or(1.0).max(0.25);
        let base_ui_scale =
            resolve_ui_scale(args.ui_scale, native_ui_scale, desktop_scale_override());

        // We handle zoom shortcuts ourselves so Ctrl+0 returns to the detected
        // scale instead of 100%.
        cc.egui_ctx
            .options_mut(|options| options.zoom_with_keyboard = false);
        let zoom = (base_ui_scale / native_ui_scale).clamp(0.2, 5.0);
        cc.egui_ctx.set_zoom_factor(zoom);
        log::info!(
            "UI scale {:.0}% (system {:.0}%)",
            base_ui_scale * 100.0,
            native_ui_scale * 100.0
        );
        if (zoom - 1.0).abs() > 1e-3 {
            resize_initial_window(&cc.egui_ctx, zoom);
        }

        let notify: Arc<dyn Fn() + Send + Sync> = {
            let ctx = cc.egui_ctx.clone();
            Arc::new(move || ctx.request_repaint())
        };

        let mark = branding::mark_texture(&cc.egui_ctx);
        let lockup = branding::lockup_texture(&cc.egui_ctx);

        let mut app = Self {
            proxy: None,
            proxy_error: None,
            notify,
            listen: args.listen,
            capture_limit: args.capture_limit,
            history: History::new(args.history_limit),
            paused: false,
            filter: String::new(),
            visible: Vec::new(),
            visible_key: None,
            selected: None,
            scroll_to_selected: false,
            detail: None,
            filter_id: None,
            native_ui_scale,
            base_ui_scale,
            mark,
            lockup,
            about_open: false,
        };
        if !args.no_proxy {
            let notify = app.notify.clone();
            app.start_proxy(notify);
        }
        app
    }

    fn start_proxy(&mut self, notify: Arc<dyn Fn() + Send + Sync>) {
        let config = ProxyConfig {
            listen: self.listen,
            capture_limit: self.capture_limit,
            notify: Some(notify),
            ..ProxyConfig::default()
        };
        match Proxy::start(config) {
            Ok(proxy) => {
                log::info!("proxy listening on {}", proxy.local_addr());
                self.proxy = Some(proxy);
                self.proxy_error = None;
            }
            Err(err) => {
                log::error!("could not start the proxy: {err}");
                self.proxy = None;
                self.proxy_error = Some(err.to_string());
            }
        }
    }

    fn drain_events(&mut self, ctx: &egui::Context) {
        let Some(proxy) = &self.proxy else {
            return;
        };
        for _ in 0..MAX_EVENTS_PER_FRAME {
            let Some(event) = proxy.try_recv() else {
                break;
            };
            let id = self.history.apply(event);
            if self.selected == Some(id) {
                self.detail = None;
            }
        }
        // Keep durations and in-flight rows ticking while anything is open.
        if self.history.in_flight() > 0 {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }

    fn rebuild_visible(&mut self) {
        let key = (self.filter.clone(), self.history.revision());
        if self.visible_key.as_ref() == Some(&key) {
            return;
        }
        self.visible_key = Some(key);
        self.visible.clear();

        let needle = self.filter.trim().to_lowercase();
        if needle.is_empty() {
            self.visible.extend(self.history.iter().map(|item| item.id));
        } else {
            for item in self.history.iter() {
                if matches_filter(item, &needle) {
                    self.visible.push(item.id);
                }
            }
        }
    }

    fn clear_history(&mut self) {
        self.history.clear();
        self.selected = None;
        self.detail = None;
        self.visible_key = None;
    }

    fn selected_row_index(&self) -> Option<usize> {
        self.selected
            .and_then(|id| self.visible.iter().position(|&visible| visible == id))
    }

    fn move_selection(&mut self, delta: isize) {
        if self.visible.is_empty() {
            self.selected = None;
            return;
        }
        let next = match self.selected_row_index() {
            Some(position) => (position as isize + delta)
                .clamp(0, self.visible.len() as isize - 1) as usize,
            None => 0,
        };
        self.select_id(self.visible[next]);
    }

    fn select_id(&mut self, id: u64) {
        if self.selected != Some(id) {
            self.selected = Some(id);
            self.detail = None;
        }
        self.scroll_to_selected = true;
    }

    fn keyboard(&mut self, ctx: &egui::Context) {
        self.zoom_shortcuts(ctx);

        let Some(filter_id) = self.filter_id else {
            return;
        };

        if ctx.input(|i| i.modifiers.command && i.key_pressed(Key::F)) {
            ctx.memory_mut(|memory| memory.request_focus(filter_id));
            return;
        }

        if ctx.memory(|memory| memory.focused()) == Some(filter_id) {
            if ctx.input(|i| i.key_pressed(Key::Escape)) {
                ctx.memory_mut(|memory| memory.surrender_focus(filter_id));
            }
            return;
        }
        if ctx.egui_wants_keyboard_input() {
            return;
        }

        if ctx.input(|i| i.key_pressed(Key::ArrowDown) || i.key_pressed(Key::J)) {
            self.move_selection(1);
        }
        if ctx.input(|i| i.key_pressed(Key::ArrowUp) || i.key_pressed(Key::K)) {
            self.move_selection(-1);
        }
        if ctx.input(|i| i.key_pressed(Key::Home)) && !self.visible.is_empty() {
            self.select_id(self.visible[0]);
        }
        if ctx.input(|i| i.key_pressed(Key::End)) && !self.visible.is_empty() {
            let last = self.visible[self.visible.len() - 1];
            self.select_id(last);
        }
    }

    /// Set the total UI scale (system scale times zoom) in physical pixels per
    /// point, clamped to the supported range.
    fn set_ui_scale(&self, ctx: &egui::Context, scale: f32) {
        let scale = scale.clamp(MIN_UI_SCALE, MAX_UI_SCALE);
        let zoom = (scale / self.native_ui_scale).clamp(0.2, 5.0);
        ctx.set_zoom_factor(zoom);
    }

    fn zoom_shortcuts(&mut self, ctx: &egui::Context) {
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

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        ui.add_space(3.0);
        ui.horizontal(|ui| {
            if let Some(mark) = &self.mark {
                let response = ui
                    .add(
                        egui::Image::from_texture(mark)
                            .fit_to_exact_size(egui::vec2(24.0, 24.0))
                            .corner_radius(6.0)
                            .sense(egui::Sense::click()),
                    )
                    .on_hover_text("About Wiremoth")
                    .on_hover_cursor(egui::CursorIcon::PointingHand);
                if response.clicked() {
                    self.about_open = true;
                }
            }
            ui.heading(RichText::new("Wiremoth").color(branding::IVORY));
            ui.separator();

            if let Some(proxy) = &self.proxy {
                ui.colored_label(Color32::LIGHT_GREEN, format!("proxy on {}", proxy.local_addr()));
            } else {
                ui.colored_label(Color32::LIGHT_RED, "proxy not running");
                if ui.button("Retry").clicked() {
                    let notify = self.notify.clone();
                    self.start_proxy(notify);
                }
            }
            if let Some(error) = &self.proxy_error {
                ui.label(RichText::new(error).color(ORANGE));
            }

            ui.separator();
            let mut capturing = !self.paused;
            if ui
                .checkbox(&mut capturing, "Capture")
                .on_hover_text("Toggle whether new exchanges are recorded")
                .changed()
            {
                self.paused = !capturing;
                if let Some(proxy) = &self.proxy {
                    proxy.set_paused(self.paused);
                }
            }
            if ui
                .button("Clear")
                .on_hover_text("Discard all captured exchanges")
                .clicked()
            {
                self.clear_history();
            }

            ui.separator();
            ui.menu_button("View", |ui| {
                let ctx = ui.ctx().clone();
                let current = ctx.pixels_per_point();

                let zoom_in = egui::Button::new("Zoom In")
                    .shortcut_text(ctx.format_shortcut(&ZOOM_IN_SHORTCUT));
                if ui.add(zoom_in).clicked() {
                    self.set_ui_scale(&ctx, stepped_scale(current, 0.1));
                }
                let zoom_out = egui::Button::new("Zoom Out")
                    .shortcut_text(ctx.format_shortcut(&ZOOM_OUT_SHORTCUT));
                if ui.add(zoom_out).clicked() {
                    self.set_ui_scale(&ctx, stepped_scale(current, -0.1));
                }
                let reset = egui::Button::new("Reset Zoom")
                    .shortcut_text(ctx.format_shortcut(&ZOOM_RESET_SHORTCUT));
                if ui
                    .add_enabled((current - self.base_ui_scale).abs() > 1e-3, reset)
                    .clicked()
                {
                    self.set_ui_scale(&ctx, self.base_ui_scale);
                }

                ui.separator();
                ui.weak(format!(
                    "UI scale: {:.0}% (system {:.0}%)",
                    current * 100.0,
                    self.native_ui_scale * 100.0,
                ));
                ui.separator();
                if ui.button("About Wiremoth").clicked() {
                    self.about_open = true;
                    ui.close();
                }
            });

            ui.separator();
            ui.label("Filter");
            let filter_id = ui.make_persistent_id(FILTER_ID);
            self.filter_id = Some(filter_id);
            ui.add(
                TextEdit::singleline(&mut self.filter)
                    .id(filter_id)
                    .hint_text("method, host, path, status")
                    .desired_width(220.0),
            );
            if ui
                .button("x")
                .on_hover_text("Clear the filter (Ctrl+F to focus)")
                .clicked()
            {
                self.filter.clear();
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(format!(
                    "{} shown / {} captured",
                    self.visible.len(),
                    self.history.len()
                ));
            });
        });
        ui.add_space(3.0);
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            ui.label(format!("{} exchanges", self.history.len()));

            let in_flight = self.history.in_flight();
            if in_flight > 0 {
                ui.separator();
                ui.colored_label(Color32::LIGHT_BLUE, format!("{in_flight} in flight"));
            }

            if let Some(proxy) = &self.proxy {
                let dropped = proxy.dropped_events();
                if dropped > 0 {
                    ui.separator();
                    ui.colored_label(ORANGE, format!("{dropped} events dropped (consumer too slow)"));
                }
            }
            let evicted = self.history.evicted();
            if evicted > 0 {
                ui.separator();
                ui.label(format!("{evicted} evicted by history limit"));
            }

            ui.separator();
            ui.label(format!("capture limit {}", view::human_bytes(self.capture_limit as u64)));

            if self.paused {
                ui.separator();
                ui.colored_label(ORANGE, "capture paused");
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.weak("HTTPS: CONNECT passthrough — TLS interception not implemented yet");
                ui.separator();
                ui.weak(format!(
                    "UI {:.0}%",
                    ui.ctx().pixels_per_point() * 100.0
                ));
            });
        });
        ui.add_space(2.0);
    }

    fn about_window(&mut self, ctx: &egui::Context) {
        if !self.about_open {
            return;
        }
        let mut open = self.about_open;
        let lockup = self.lockup.clone();
        egui::Window::new("About Wiremoth")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.set_max_width(460.0);
                if let Some(lockup) = &lockup {
                    ui.add(
                        egui::Image::from_texture(lockup)
                            .max_width(420.0)
                            .corner_radius(10.0),
                    );
                }
                ui.add_space(4.0);
                ui.label(format!("Wiremoth {}", env!("CARGO_PKG_VERSION")));
                ui.hyperlink_to(
                    RichText::new("github.com/andyintheshell/wiremoth")
                        .color(branding::SIGNAL_ORANGE),
                    "https://github.com/andyintheshell/wiremoth",
                );
                ui.add_space(2.0);
                ui.weak("A lightweight, keyboard-first workbench for inspecting HTTP traffic.");
            });
        self.about_open = open;
    }

    fn history_table(&mut self, ui: &mut egui::Ui) {
        let mut clicked = None;
        let rows = self.visible.len();

        let mut table = TableBuilder::new(ui)
            .striped(true)
            .resizable(true)
            .sense(egui::Sense::click())
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::exact(48.0))
            .column(Column::exact(62.0))
            .column(Column::exact(66.0))
            .column(Column::exact(52.0))
            .column(Column::initial(170.0).at_least(80.0))
            .column(Column::remainder().at_least(120.0))
            .column(Column::exact(76.0))
            .column(Column::exact(64.0));

        if self.scroll_to_selected {
            if let Some(row) = self.selected_row_index() {
                table = table.scroll_to_row(row, Some(Align::Center));
            }
            self.scroll_to_selected = false;
        }

        table
            .header(20.0, |mut header| {
                for title in ["#", "At", "Method", "Status", "Host", "Path", "Size", "Took"] {
                    header.col(|ui| {
                        ui.strong(title);
                    });
                }
            })
            .body(|body| {
                body.rows(18.0, rows, |mut row| {
                    let index = row.index();
                    let id = self.visible[index];
                    let Some(item) = self.history.get(id) else {
                        return;
                    };

                    row.set_selected(self.selected == Some(id));
                    row.col(|ui| {
                        ui.label(item.id.to_string());
                    });
                    row.col(|ui| {
                        ui.label(view::format_time(item.timestamp));
                    });
                    row.col(|ui| {
                        ui.label(item.request.method.as_str());
                    });
                    row.col(|ui| {
                        ui.label(
                            RichText::new(view::status_text(item)).color(view::status_color(item)),
                        );
                    });
                    row.col(|ui| {
                        ui.add(Label::new(item.request.host.as_str()).truncate());
                    });
                    row.col(|ui| {
                        let path = if item.is_tunnel() {
                            "—"
                        } else {
                            item.request.path.as_str()
                        };
                        ui.add(Label::new(path).truncate());
                    });
                    row.col(|ui| {
                        ui.label(view::human_bytes(item.transferred()));
                    });
                    row.col(|ui| {
                        ui.label(
                            item.duration()
                                .map(view::format_duration)
                                .unwrap_or_else(|| "…".to_string()),
                        );
                    });

                    if row.response().clicked() {
                        clicked = Some(id);
                    }
                });
            });

        if let Some(id) = clicked {
            self.select_id(id);
        }
    }

    fn ensure_detail(&mut self, id: u64) {
        let revision = self.history.revision();
        if let Some(cache) = &self.detail
            && cache.id == id
            && cache.revision == revision
        {
            return;
        }
        self.detail = self.history.get(id).map(|item| DetailCache {
            id,
            revision,
            request: view::format_request(item),
            response: view::format_response(item),
        });
    }

    fn detail_panel(&mut self, ui: &mut egui::Ui) {
        let Some(id) = self.selected else {
            ui.centered_and_justified(|ui| {
                ui.weak("Select an exchange to inspect it (click a row, j/k, or the arrow keys)");
            });
            return;
        };
        self.ensure_detail(id);

        let Some(cache) = &self.detail else {
            ui.centered_and_justified(|ui| {
                ui.weak("This exchange is no longer in history");
            });
            return;
        };

        egui::Panel::right(RESPONSE_PANE_ID)
            .resizable(true)
            .default_size((ui.available_width() / 2.0).max(200.0))
            .show(ui, |ui| {
                text_pane(ui, "Response", &cache.response);
            });
        text_pane(ui, "Request", &cache.request);
    }
}

impl eframe::App for WiremothApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.drain_events(&ctx);
        self.keyboard(&ctx);
        self.rebuild_visible();

        egui::Panel::top("wiremoth.toolbar").show(ui, |ui| self.toolbar(ui));
        egui::Panel::bottom("wiremoth.statusbar").show(ui, |ui| self.status_bar(ui));
        egui::Panel::bottom("wiremoth.detail")
            .resizable(true)
            .default_size(320.0)
            .min_size(120.0)
            .show(ui, |ui| self.detail_panel(ui));
        egui::CentralPanel::default().show(ui, |ui| self.history_table(ui));

        self.about_window(&ctx);

        // Keep the fetch loop honest about dropped events even while idle.
        if self
            .proxy
            .as_ref()
            .is_some_and(|proxy| proxy.dropped_events() > 0)
        {
            ctx.request_repaint_after(Duration::from_millis(500));
        }
    }
}

fn text_pane(ui: &mut egui::Ui, title: &str, text: &str) {
    ui.add_space(3.0);
    ui.horizontal(|ui| {
        ui.strong(title);
    });
    ui.separator();
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.add(Label::new(RichText::new(text).monospace()));
        });
}

/// The scale the desktop gives GTK/Qt applications, used as a fallback on X11
/// setups where the windowing system reports no HiDPI scaling (no `Xft.dpi`).
fn desktop_scale_override() -> Option<f32> {
    ["GDK_SCALE", "QT_SCALE_FACTOR"]
        .iter()
        .filter_map(|name| std::env::var(name).ok()?.trim().parse::<f32>().ok())
        .find(|scale| scale.is_finite() && *scale > 1.0)
        .map(|scale| scale.clamp(MIN_UI_SCALE, MAX_UI_SCALE))
}

fn resolve_ui_scale(requested: Option<f32>, native: f32, desktop: Option<f32>) -> f32 {
    requested
        .or_else(|| (native > 1.0).then_some(native))
        .or(desktop)
        .unwrap_or(1.0)
        .clamp(MIN_UI_SCALE, MAX_UI_SCALE)
}

/// One zoom step, rounded to a tenth so repeated steps stay on clean values.
fn stepped_scale(current: f32, delta: f32) -> f32 {
    ((current + delta) * 10.0).round() / 10.0
}

/// Keep the same logical content size as at 100% by growing the window when
/// the UI is zoomed at startup. Without this a 2x UI opens in a window that is
/// still 1280x820 physical pixels, leaving only 640x410 points of content.
fn resize_initial_window(ctx: &egui::Context, zoom: f32) {
    let mut size = DESIGN_WINDOW_SIZE;
    if let Some(monitor) = ctx.input(|input| input.viewport().monitor_size) {
        // `monitor_size` is in points at the native scale (zoom was 1.0 when
        // it was measured), so divide by the zoom we just applied.
        let max_size =
            (monitor / zoom - egui::vec2(40.0, 80.0)).max(egui::vec2(320.0, 240.0));
        size = size.min(max_size);
    }
    ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(size));
}

fn matches_filter(item: &HistoryItem, needle: &str) -> bool {
    let status = item.status().map(|status| status.to_string()).unwrap_or_default();
    let haystack = format!(
        "{} {} {} {} {}",
        item.request.method, item.request.host, item.request.path, item.request.uri, status
    );
    haystack.to_lowercase().contains(needle)
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
