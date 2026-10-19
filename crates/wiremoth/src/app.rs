//! The egui application: a Burp-style workspace over the captured traffic.
//!
//! The navigation intentionally mirrors the tools Burp users already know:
//! Target, Proxy, Intruder, and Repeater. Proxy history and the Target site map
//! are live today; Intruder and Repeater are clearly marked previews until
//! their engines land.
//!
//! This module holds the application state, the window shell, and the shared
//! top-level panels. The tabs and supporting pieces are split by area:
//! [`proxy`], [`target`], [`previews`], [`windows`], [`widgets`], [`util`],
//! and [`scale`].

mod previews;
mod proxy;
mod scale;
mod target;
mod util;
mod widgets;
mod windows;

use std::collections::BTreeSet;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use egui::{Color32, Key, RichText, Stroke};
use wiremoth_proxy::{Proxy, ProxyConfig};

use crate::Args;
use crate::branding;
use crate::history::History;
use crate::memory::MemoryMeter;
use crate::site_map::{self, SiteMap};
use crate::theme;
use crate::view::{self, MessageViews};

use self::scale::{desktop_scale_override, resize_initial_window, resolve_ui_scale, stepped_scale};
use self::util::matches_filter;
use self::widgets::main_tab_button;

/// Bound on events applied per frame so a flood cannot stall rendering.
const MAX_EVENTS_PER_FRAME: usize = 1024;
const FILTER_ID: &str = "wiremoth.filter";
const PROXY_DETAIL_ID: &str = "wiremoth.proxy.detail";
const TARGET_TREE_ID: &str = "wiremoth.target.tree";
const REPEATER_REQUEST_ID: &str = "wiremoth.repeater.request";

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

const TAB_TARGET_SHORTCUT: egui::KeyboardShortcut = egui::KeyboardShortcut::new(
    egui::Modifiers::COMMAND.plus(egui::Modifiers::SHIFT),
    Key::T,
);
const TAB_PROXY_SHORTCUT: egui::KeyboardShortcut = egui::KeyboardShortcut::new(
    egui::Modifiers::COMMAND.plus(egui::Modifiers::SHIFT),
    Key::P,
);
const TAB_INTRUDER_SHORTCUT: egui::KeyboardShortcut = egui::KeyboardShortcut::new(
    egui::Modifiers::COMMAND.plus(egui::Modifiers::SHIFT),
    Key::I,
);
const TAB_REPEATER_SHORTCUT: egui::KeyboardShortcut = egui::KeyboardShortcut::new(
    egui::Modifiers::COMMAND.plus(egui::Modifiers::SHIFT),
    Key::R,
);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum MainTab {
    Target,
    #[default]
    Proxy,
    Intruder,
    Repeater,
}

impl MainTab {
    /// Burp's tool order, so muscle memory carries over.
    const ALL: [(Self, &'static str); 4] = [
        (Self::Target, "Target"),
        (Self::Proxy, "Proxy"),
        (Self::Intruder, "Intruder"),
        (Self::Repeater, "Repeater"),
    ];
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum ProxySection {
    Intercept,
    #[default]
    HttpHistory,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum TargetSection {
    #[default]
    SiteMap,
    Scope,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum MessagePane {
    #[default]
    Request,
    Response,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum MessageView {
    #[default]
    Raw,
    Headers,
    Hex,
}

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
    main_tab: MainTab,
    proxy_section: ProxySection,
    target_section: TargetSection,
    message_pane: MessagePane,
    message_view: MessageView,
    site_map: Option<(u64, SiteMap)>,
    scope: BTreeSet<String>,
    scope_enabled: bool,
    scope_input: String,
    /// Scale reported by the windowing system (physical pixels per point).
    native_ui_scale: f32,
    /// Scale chosen at startup (CLI flag or auto-detected); Ctrl+0 resets here.
    base_ui_scale: f32,
    /// Toolbar brand mark; the About lockup is decoded on first use.
    mark: Option<egui::TextureHandle>,
    lockup: Option<egui::TextureHandle>,
    memory: MemoryMeter,
    memory_open: bool,
    about_open: bool,
}

/// Formatted request/response text for the selected exchange, rebuilt only
/// when the selection or its lifecycle changes.
struct DetailCache {
    id: u64,
    request: MessageViews,
    response: MessageViews,
}

impl WiremothApp {
    pub fn new(cc: &eframe::CreationContext<'_>, args: Args) -> Self {
        let native_ui_scale = cc
            .egui_ctx
            .native_pixels_per_point()
            .unwrap_or(1.0)
            .max(0.25);
        let base_ui_scale =
            resolve_ui_scale(args.ui_scale, native_ui_scale, desktop_scale_override());

        theme::apply(&cc.egui_ctx);

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
            main_tab: MainTab::default(),
            proxy_section: ProxySection::default(),
            target_section: TargetSection::default(),
            message_pane: MessagePane::default(),
            message_view: MessageView::default(),
            site_map: None,
            scope: BTreeSet::new(),
            scope_enabled: false,
            scope_input: String::new(),
            native_ui_scale,
            base_ui_scale,
            mark,
            lockup: None,
            memory: MemoryMeter::new(),
            memory_open: false,
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
                proxy.set_paused(self.paused);
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
        let mut selected_changed = false;
        for _ in 0..MAX_EVENTS_PER_FRAME {
            let Some(event) = proxy.try_recv() else {
                break;
            };
            let id = self.history.apply(event);
            if self.selected == Some(id) {
                selected_changed = true;
            }
        }
        if selected_changed {
            self.detail = None;
        }
        if self
            .selected
            .is_some_and(|id| self.history.get(id).is_none())
        {
            self.selected = None;
            self.detail = None;
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
        self.visible.clear();

        let needle = self.filter.trim().to_lowercase();
        let scope_active = self.scope_enabled && !self.scope.is_empty();
        for item in self.history.iter() {
            if scope_active && !site_map::host_in_scope(&item.request.host, &self.scope) {
                continue;
            }
            if needle.is_empty() || matches_filter(item, &needle) {
                self.visible.push(item.id);
            }
        }
        self.visible_key = Some(key);
    }

    fn invalidate_views(&mut self) {
        self.visible_key = None;
        self.site_map = None;
    }

    fn ensure_site_map(&mut self) {
        let revision = self.history.revision();
        if self
            .site_map
            .as_ref()
            .is_some_and(|(cached_revision, _)| *cached_revision == revision)
        {
            return;
        }
        let scope = self.scope_enabled.then_some(&self.scope);
        let map = SiteMap::build(&self.history, scope);
        self.site_map = Some((revision, map));
    }

    fn clear_history(&mut self) {
        self.history.clear();
        self.selected = None;
        self.detail = None;
        self.invalidate_views();
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
            Some(position) => {
                (position as isize + delta).clamp(0, self.visible.len() as isize - 1) as usize
            }
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

        let (target, proxy, intruder, repeater) = ctx.input_mut(|input| {
            (
                input.consume_shortcut(&TAB_TARGET_SHORTCUT),
                input.consume_shortcut(&TAB_PROXY_SHORTCUT),
                input.consume_shortcut(&TAB_INTRUDER_SHORTCUT),
                input.consume_shortcut(&TAB_REPEATER_SHORTCUT),
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

        let on_history =
            self.main_tab == MainTab::Proxy && self.proxy_section == ProxySection::HttpHistory;
        if on_history {
            let filter_id = egui::Id::unique(FILTER_ID);
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

        if ctx.input(|input| input.key_pressed(Key::ArrowDown) || input.key_pressed(Key::J)) {
            self.move_selection(1);
        }
        if ctx.input(|input| input.key_pressed(Key::ArrowUp) || input.key_pressed(Key::K)) {
            self.move_selection(-1);
        }
        if ctx.input(|input| input.key_pressed(Key::Home)) && !self.visible.is_empty() {
            self.select_id(self.visible[0]);
        }
        if ctx.input(|input| input.key_pressed(Key::End)) && !self.visible.is_empty() {
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

    fn top_bar(&mut self, ui: &mut egui::Ui) {
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
            ui.label(
                RichText::new("Wiremoth")
                    .size(20.0)
                    .strong()
                    .color(theme::TEXT),
            );
            ui.add_space(6.0);

            ui.scope(|ui| {
                let widgets = &mut ui.style_mut().visuals.widgets;
                widgets.inactive.weak_bg_fill = Color32::TRANSPARENT;
                widgets.inactive.bg_stroke = Stroke::NONE;
                widgets.inactive.fg_stroke = Stroke::new(1.0, theme::TEXT_MUTED);
                widgets.hovered.weak_bg_fill = theme::ACCENT_WASH;
                widgets.hovered.bg_stroke = Stroke::NONE;
                widgets.hovered.fg_stroke = Stroke::new(1.0, theme::ACCENT_DARK);
                widgets.active.weak_bg_fill = theme::ACCENT_SOFT;
                widgets.active.bg_stroke = Stroke::NONE;
                widgets.active.fg_stroke = Stroke::new(1.0, theme::ACCENT_DARK);

                ui.menu_button("Wiremoth", |ui| {
                    if ui.button("About Wiremoth").clicked() {
                        self.about_open = true;
                        ui.close();
                    }
                    ui.separator();
                    if ui.button("Quit").clicked() {
                        ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });
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
                });
                ui.menu_button("Help", |ui| {
                    if ui.button("About Wiremoth").clicked() {
                        self.about_open = true;
                        ui.close();
                    }
                    ui.hyperlink_to(
                        "Project on GitHub",
                        "https://github.com/andyintheshell/wiremoth",
                    );
                });
            });

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                self.capture_toggle(ui);
                ui.separator();
                if let Some(proxy) = &self.proxy {
                    ui.colored_label(theme::GREEN, format!("proxy on {}", proxy.local_addr()));
                } else {
                    let response = ui.colored_label(theme::RED, "proxy not running");
                    if let Some(error) = &self.proxy_error {
                        response.on_hover_text(error);
                    }
                    if ui.button("Retry").clicked() {
                        let notify = self.notify.clone();
                        self.start_proxy(notify);
                    }
                }
            });
        });
    }

    fn main_tabs(&mut self, ui: &mut egui::Ui) {
        ui.add_space(5.0);
        let shortcuts = [
            ui.ctx().format_shortcut(&TAB_TARGET_SHORTCUT),
            ui.ctx().format_shortcut(&TAB_PROXY_SHORTCUT),
            ui.ctx().format_shortcut(&TAB_INTRUDER_SHORTCUT),
            ui.ctx().format_shortcut(&TAB_REPEATER_SHORTCUT),
        ];
        ui.horizontal(|ui| {
            for (index, (tab, label)) in MainTab::ALL.iter().enumerate() {
                let response = main_tab_button(ui, self.main_tab == *tab, label)
                    .on_hover_text(format!("{} ({})", label, shortcuts[index]));
                if response.clicked() && self.main_tab != *tab {
                    self.main_tab = *tab;
                    ui.ctx().memory_mut(|memory| memory.stop_text_input());
                }
            }
        });
        ui.add_space(5.0);
        ui.separator();
    }

    fn capture_toggle(&mut self, ui: &mut egui::Ui) {
        let mut capturing = !self.paused;
        if ui
            .checkbox(&mut capturing, "Capture")
            .on_hover_text("Record new exchanges in HTTP history")
            .changed()
        {
            self.paused = !capturing;
            if let Some(proxy) = &self.proxy {
                proxy.set_paused(self.paused);
            }
        }
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!("{} exchanges", self.history.len()))
                    .small()
                    .color(theme::TEXT_MUTED),
            );

            let in_flight = self.history.in_flight();
            if in_flight > 0 {
                ui.separator();
                ui.colored_label(theme::BLUE, format!("{in_flight} in flight"));
            }

            if let Some(item) = self.selected.and_then(|id| self.history.get(id)) {
                ui.separator();
                ui.label(
                    RichText::new(format!("#{} {}", item.id, item.request.method))
                        .small()
                        .color(theme::TEXT_MUTED),
                );
                ui.colored_label(view::status_color(item), view::status_text(item));
                if let Some(duration) = item.duration() {
                    ui.label(
                        RichText::new(view::format_duration(duration))
                            .small()
                            .color(theme::TEXT_MUTED),
                    );
                }
                ui.label(
                    RichText::new(view::human_bytes(item.transferred()))
                        .small()
                        .color(theme::TEXT_MUTED),
                );
            }

            if let Some(proxy) = &self.proxy {
                let dropped = proxy.dropped_events();
                if dropped > 0 {
                    ui.separator();
                    ui.colored_label(
                        theme::AMBER,
                        format!("{dropped} events dropped (consumer too slow)"),
                    );
                }
            }
            let evicted = self.history.evicted();
            if evicted > 0 {
                ui.separator();
                ui.label(
                    RichText::new(format!("{evicted} evicted by history limit"))
                        .small()
                        .color(theme::TEXT_MUTED),
                );
            }

            ui.separator();
            ui.label(
                RichText::new(format!(
                    "capture limit {}",
                    view::human_bytes(self.capture_limit as u64)
                ))
                .small()
                .color(theme::TEXT_MUTED),
            );

            if self.paused {
                ui.separator();
                ui.colored_label(theme::AMBER, "capture paused");
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                self.memory_indicator(ui);
                ui.separator();
                ui.label(
                    RichText::new(format!("UI {:.0}%", ui.ctx().pixels_per_point() * 100.0))
                        .small()
                        .color(theme::TEXT_MUTED),
                );
                ui.separator();
                ui.label(
                    RichText::new(
                        "HTTPS: CONNECT passthrough - TLS interception not implemented yet",
                    )
                    .small()
                    .color(theme::TEXT_MUTED),
                );
            });
        });
    }

    fn main_content(&mut self, ui: &mut egui::Ui) {
        ui.add_space(2.0);
        match self.main_tab {
            MainTab::Target => self.target_tab(ui),
            MainTab::Proxy => self.proxy_tab(ui),
            MainTab::Intruder => self.intruder_view(ui),
            MainTab::Repeater => self.repeater_view(ui),
        }
    }
}

impl eframe::App for WiremothApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.memory.sample_if_due();
        self.drain_events(&ctx);
        self.keyboard(&ctx);
        self.rebuild_visible();

        egui::Panel::top("wiremoth.top")
            .show_separator_line(false)
            .frame(
                egui::Frame::NONE
                    .fill(theme::SURFACE)
                    .inner_margin(egui::Margin::symmetric(12, 6)),
            )
            .show(ui, |ui| {
                self.top_bar(ui);
                self.main_tabs(ui);
            });

        egui::Panel::bottom("wiremoth.statusbar")
            .frame(
                egui::Frame::NONE
                    .fill(theme::SURFACE_ALT)
                    .inner_margin(egui::Margin::symmetric(12, 4)),
            )
            .show(ui, |ui| self.status_bar(ui));

        egui::CentralPanel::default()
            .frame(
                egui::Frame::NONE
                    .fill(theme::CANVAS)
                    .inner_margin(egui::Margin::same(8)),
            )
            .show(ui, |ui| self.main_content(ui));

        self.memory_window(&ctx);
        self.about_window(&ctx);

        // Keep the fetch loop honest about dropped events even while idle.
        if self
            .proxy
            .as_ref()
            .is_some_and(|proxy| proxy.dropped_events() > 0)
        {
            ctx.request_repaint_after(Duration::from_millis(500));
        }
        ctx.request_repaint_after(self.memory.time_until_next_sample());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tabs_follow_burp_order() {
        let tabs: Vec<MainTab> = MainTab::ALL.iter().map(|(tab, _)| *tab).collect();
        assert_eq!(
            tabs,
            vec![
                MainTab::Target,
                MainTab::Proxy,
                MainTab::Intruder,
                MainTab::Repeater
            ]
        );
    }
}
