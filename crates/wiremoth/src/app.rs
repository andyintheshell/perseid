//! The egui application: a Burp-style workspace over the captured traffic.
//!
//! The navigation intentionally mirrors the tools Burp users already know:
//! Target, Proxy, Intruder, and Repeater. Proxy history and the Target site map
//! are live today; Intruder and Repeater are clearly marked previews until
//! their engines land.

use std::collections::{BTreeMap, BTreeSet};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use egui::{Align, Color32, Key, Label, RichText, Stroke, TextEdit};
use egui_extras::{Column, TableBuilder};
use wiremoth_proxy::{Proxy, ProxyConfig};

use crate::Args;
use crate::branding;
use crate::history::{History, HistoryItem};
use crate::memory::MemoryMeter;
use crate::site_map::{self, SiteMap};
use crate::theme;
use crate::view::{self, MessageViews};

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

    fn memory_indicator(&mut self, ui: &mut egui::Ui) {
        let current = self.memory.current();
        let label = match current {
            Some(sample) => format!("Memory {}", view::human_bytes(sample.rss)),
            None => "Memory n/a".to_owned(),
        };
        let hover = if let Some(sample) = current {
            format!(
                "Resident memory: {}\nPeak: {}\n\nClick for details",
                view::human_bytes(sample.rss),
                view::human_bytes(sample.peak_rss)
            )
        } else if self.memory.supported() {
            "Waiting for a memory sample. Click for details.".to_owned()
        } else {
            "Process memory measurement is available on Linux only.".to_owned()
        };

        let mut text = RichText::new(label).small().color(if self.memory_open {
            theme::ACCENT_DARK
        } else {
            theme::TEXT_MUTED
        });
        if self.memory_open {
            text = text.underline();
        }
        let response = ui
            .add(Label::new(text).sense(egui::Sense::click()))
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text(hover);
        if response.clicked() {
            self.memory_open = !self.memory_open;
        }
    }

    fn memory_window(&mut self, ctx: &egui::Context) {
        if !self.memory_open {
            return;
        }
        if ctx.input(|input| input.key_pressed(Key::Escape)) {
            self.memory_open = false;
            return;
        }

        let current = self.memory.current();
        let samples: Vec<u64> = self.memory.samples().iter().copied().collect();
        let supported = self.memory.supported();
        let exchanges = self.history.len();
        let body_bytes = self.history.retained_body_bytes();
        let mut open = self.memory_open;

        egui::Window::new("Memory usage")
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_width(470.0)
            .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-16.0, -44.0))
            .show(ctx, |ui| {
                ui.set_min_width(440.0);
                ui.horizontal(|ui| {
                    memory_metric(
                        ui,
                        "Current",
                        current.map(|sample| sample.rss),
                        theme::ACCENT_DARK,
                    );
                    ui.add_space(24.0);
                    memory_metric(
                        ui,
                        "Peak",
                        current.map(|sample| sample.peak_rss),
                        theme::TEXT,
                    );
                    ui.add_space(24.0);
                    memory_metric(ui, "Captured bodies", Some(body_bytes), theme::TEXT);
                });

                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("Resident memory")
                            .small()
                            .color(theme::TEXT_MUTED),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let window = if samples.len() > 1 {
                            format!("last {}s", samples.len() - 1)
                        } else {
                            "collecting...".to_owned()
                        };
                        ui.label(RichText::new(window).small().color(theme::TEXT_MUTED));
                    });
                });
                memory_graph(ui, &samples);

                ui.add_space(6.0);
                ui.label(
                    RichText::new(format!(
                        "{exchanges} exchanges · {} retained in captured bodies",
                        view::human_bytes(body_bytes)
                    ))
                    .small()
                    .color(theme::TEXT_MUTED),
                );
                if supported {
                    ui.label(
                        RichText::new(
                            "Linux process RSS, sampled once per second. Peak is VmHWM. \
                             RSS includes mapped libraries and graphics-driver code; \
                             GPU-resident memory is not included.",
                        )
                        .small()
                        .color(theme::TEXT_MUTED),
                    );
                } else {
                    ui.colored_label(
                        theme::AMBER,
                        "Process memory measurement is available on Linux only.",
                    );
                }
            });
        self.memory_open = open;
    }

    fn about_window(&mut self, ctx: &egui::Context) {
        if !self.about_open {
            return;
        }
        if ctx.input(|input| input.key_pressed(Key::Escape)) {
            self.about_open = false;
            return;
        }
        if self.lockup.is_none() {
            self.lockup = branding::lockup_texture(ctx);
        }
        let mut open = self.about_open;
        let lockup = self.lockup.clone();
        egui::Window::new("About Wiremoth")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
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
                ui.label(
                    RichText::new(format!("Wiremoth {}", env!("CARGO_PKG_VERSION")))
                        .strong()
                        .color(theme::TEXT),
                );
                ui.hyperlink_to(
                    RichText::new("github.com/andyintheshell/wiremoth").color(theme::ACCENT_DARK),
                    "https://github.com/andyintheshell/wiremoth",
                );
                ui.add_space(2.0);
                ui.weak("A lightweight, keyboard-first workbench for inspecting HTTP traffic.");
                ui.weak("MIT OR Apache-2.0");
            });
        self.about_open = open;
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

    fn proxy_tab(&mut self, ui: &mut egui::Ui) {
        egui::Frame::NONE
            .fill(theme::SURFACE_ALT)
            .inner_margin(egui::Margin::symmetric(10, 5))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    if section_tab(
                        ui,
                        self.proxy_section == ProxySection::Intercept,
                        "Intercept",
                    )
                    .clicked()
                    {
                        self.proxy_section = ProxySection::Intercept;
                        ui.ctx().memory_mut(|memory| memory.stop_text_input());
                    }
                    if section_tab(
                        ui,
                        self.proxy_section == ProxySection::HttpHistory,
                        "HTTP history",
                    )
                    .clicked()
                    {
                        self.proxy_section = ProxySection::HttpHistory;
                    }
                });
            });

        match self.proxy_section {
            ProxySection::Intercept => self.intercept_view(ui),
            ProxySection::HttpHistory => self.http_history_view(ui),
        }
    }

    fn http_history_view(&mut self, ui: &mut egui::Ui) {
        egui::Frame::NONE
            .fill(theme::SURFACE)
            .inner_margin(egui::Margin::symmetric(10, 6))
            .show(ui, |ui| self.history_toolbar(ui));
        self.rebuild_visible();
        // The filter or scope can hide the selected exchange. Do not leave its
        // detail visible next to a table where the row no longer exists.
        if self.selected.is_some_and(|id| !self.visible.contains(&id)) {
            self.selected = None;
            self.detail = None;
            self.scroll_to_selected = false;
        }

        egui::Panel::bottom(PROXY_DETAIL_ID)
            .resizable(true)
            .default_size(340.0)
            .min_size(280.0)
            .frame(
                egui::Frame::NONE
                    .fill(theme::SURFACE)
                    .inner_margin(egui::Margin::same(8)),
            )
            .show(ui, |ui| self.detail_panel(ui));

        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(theme::SURFACE))
            .show(ui, |ui| self.history_table(ui));
    }

    fn history_toolbar(&mut self, ui: &mut egui::Ui) {
        let mut clear = false;
        ui.horizontal(|ui| {
            ui.label(RichText::new("HTTP history").strong().color(theme::TEXT));
            ui.separator();
            ui.label(RichText::new("Filter").color(theme::TEXT_MUTED));
            let filter_id = egui::Id::unique(FILTER_ID);
            let response = ui.add(
                TextEdit::singleline(&mut self.filter)
                    .id(filter_id)
                    .hint_text("method, host, path, status")
                    .desired_width(260.0),
            );
            if response.changed() {
                self.visible_key = None;
            }
            let clear_filter = ui
                .add_enabled(!self.filter.is_empty(), egui::Button::new("x"))
                .on_hover_text("Clear the filter (Ctrl+F to focus)")
                .on_disabled_hover_text("No filter is active");
            if clear_filter.clicked() {
                self.filter.clear();
                self.visible_key = None;
            }

            if self.scope_enabled {
                ui.separator();
                if ui
                    .link("scope filter")
                    .on_hover_text(
                        "Only in-scope hosts are shown. Click to edit the scope in Target.",
                    )
                    .clicked()
                {
                    self.main_tab = MainTab::Target;
                    self.target_section = TargetSection::Scope;
                }
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .button("Clear history")
                    .on_hover_text("Discard all captured exchanges")
                    .clicked()
                {
                    clear = true;
                }
                ui.label(
                    RichText::new(format!(
                        "{} shown / {} captured",
                        self.visible.len(),
                        self.history.len()
                    ))
                    .small()
                    .color(theme::TEXT_MUTED),
                );
            });
        });
        if clear {
            self.clear_history();
        }
    }

    fn history_table(&mut self, ui: &mut egui::Ui) {
        let rows = self.visible.len();
        if rows == 0 {
            empty_state(
                ui,
                "No exchanges to show",
                "Start the proxy, point a browser at it, and browse an HTTP site. The filter and scope can also hide the history.",
            );
            return;
        }

        let mut clicked = None;
        let mut table = TableBuilder::new(ui)
            .striped(true)
            .resizable(true)
            .sense(egui::Sense::click())
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::exact(44.0))
            .column(Column::exact(64.0))
            .column(Column::initial(420.0).at_least(180.0))
            .column(Column::exact(56.0))
            .column(Column::exact(76.0))
            .column(Column::initial(130.0).at_least(80.0))
            .column(Column::remainder().at_least(64.0));

        if self.scroll_to_selected {
            if let Some(row) = self.selected_row_index() {
                table = table.scroll_to_row(row, Some(Align::Center));
            }
            self.scroll_to_selected = false;
        }

        table
            .header(22.0, |mut header| {
                for title in ["#", "Method", "URL", "Status", "Length", "MIME type", "Time"] {
                    header.col(|ui| {
                        ui.label(
                            RichText::new(title)
                                .strong()
                                .small()
                                .color(theme::TEXT_MUTED),
                        );
                    });
                }
            })
            .body(|body| {
                body.rows(20.0, rows, |mut row| {
                    let index = row.index();
                    let id = self.visible[index];
                    let Some(item) = self.history.get(id) else {
                        return;
                    };

                    row.set_selected(self.selected == Some(id));
                    row.col(|ui| {
                        ui.label(
                            RichText::new(item.id.to_string())
                                .small()
                                .color(theme::TEXT_MUTED),
                        );
                    });
                    row.col(|ui| {
                        ui.label(RichText::new(&item.request.method).color(theme::TEXT));
                    });
                    let url = request_url(item);
                    row.col(|ui| {
                        ui.add(Label::new(RichText::new(&url).color(theme::TEXT)).truncate())
                            .on_hover_text(&url);
                    });
                    row.col(|ui| {
                        ui.label(
                            RichText::new(view::status_text(item))
                                .color(view::status_color(item)),
                        );
                    });
                    row.col(|ui| {
                        ui.label(
                            RichText::new(view::human_bytes(item.transferred()))
                                .small()
                                .color(theme::TEXT_MUTED),
                        );
                    });
                    row.col(|ui| {
                        let mime = mime_type(item);
                        let color = if item.is_tunnel() {
                            theme::AMBER
                        } else {
                            theme::TEXT_MUTED
                        };
                        ui.add(Label::new(RichText::new(mime).small().color(color)).truncate());
                    });
                    row.col(|ui| {
                        ui.label(
                            RichText::new(view::format_time(item.timestamp))
                                .small()
                                .color(theme::TEXT_MUTED),
                        );
                    });

                    let hover = if item.is_tunnel() {
                        format!(
                            "{url}\nCONNECT tunnel: payload not decrypted until TLS interception lands."
                        )
                    } else {
                        url
                    };
                    if row.response().on_hover_text(hover).clicked() {
                        clicked = Some(id);
                    }
                });
            });

        if let Some(id) = clicked {
            self.select_id(id);
        }
    }

    fn ensure_detail(&mut self, id: u64) {
        if self.detail.as_ref().is_some_and(|cache| cache.id == id)
            && self.history.get(id).is_some()
        {
            return;
        }
        self.detail = self.history.get(id).map(|item| DetailCache {
            id,
            request: view::request_views(item),
            response: view::response_views(item),
        });
    }

    fn detail_panel(&mut self, ui: &mut egui::Ui) {
        let Some(id) = self.selected else {
            empty_state(
                ui,
                "Select an exchange",
                "Click a row, or use j/k and the arrow keys, to inspect the request and response.",
            );
            return;
        };
        self.ensure_detail(id);

        let mut pane = self.message_pane;
        let mut mode = self.message_view;
        let item = self.history.get(id);
        message_tab_bar(ui, &mut pane, &mut mode, item);
        self.message_pane = pane;
        self.message_view = mode;

        let Some(cache) = &self.detail else {
            empty_state(
                ui,
                "Exchange no longer in history",
                "The selected exchange was evicted or cleared.",
            );
            return;
        };
        let text = match (pane, mode) {
            (MessagePane::Request, MessageView::Raw) => &cache.request.raw,
            (MessagePane::Request, MessageView::Headers) => &cache.request.head,
            (MessagePane::Request, MessageView::Hex) => &cache.request.hex,
            (MessagePane::Response, MessageView::Raw) => &cache.response.raw,
            (MessagePane::Response, MessageView::Headers) => &cache.response.head,
            (MessagePane::Response, MessageView::Hex) => &cache.response.hex,
        };
        message_text(ui, id, pane, mode, text);
    }

    fn target_tab(&mut self, ui: &mut egui::Ui) {
        egui::Frame::NONE
            .fill(theme::SURFACE_ALT)
            .inner_margin(egui::Margin::symmetric(10, 5))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    if section_tab(
                        ui,
                        self.target_section == TargetSection::SiteMap,
                        "Site map",
                    )
                    .clicked()
                    {
                        self.target_section = TargetSection::SiteMap;
                        ui.ctx().memory_mut(|memory| memory.stop_text_input());
                    }
                    if section_tab(ui, self.target_section == TargetSection::Scope, "Scope")
                        .clicked()
                    {
                        self.target_section = TargetSection::Scope;
                    }
                });
            });

        match self.target_section {
            TargetSection::SiteMap => self.site_map_view(ui),
            TargetSection::Scope => self.scope_view(ui),
        }
    }

    fn site_map_view(&mut self, ui: &mut egui::Ui) {
        self.ensure_site_map();
        let mut picked = None;
        let selected = self.selected;

        if let Some((_, map)) = &self.site_map {
            if map.is_empty() {
                let body = if self.scope_enabled {
                    "No in-scope HTTP endpoints. Adjust the scope in Target > Scope, or turn off scope filtering."
                } else {
                    "Browse an HTTP site through the proxy to populate the site map. HTTPS traffic is captured as CONNECT tunnels until TLS interception lands."
                };
                empty_state(ui, "No HTTP endpoints", body);
                return;
            }

            egui::Panel::left(TARGET_TREE_ID)
                .resizable(true)
                .default_size(380.0)
                .min_size(220.0)
                .frame(
                    egui::Frame::NONE
                        .fill(theme::SURFACE)
                        .inner_margin(egui::Margin::same(8)),
                )
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Site map").strong().color(theme::TEXT));
                        ui.label(
                            RichText::new(format!(
                                "{} hosts / {} endpoints",
                                map.hosts.len(),
                                map.endpoint_count()
                            ))
                            .small()
                            .color(theme::TEXT_MUTED),
                        );
                    });
                    ui.separator();
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            for host in &map.hosts {
                                let header =
                                    format!("http://{}  ({})", host.host, host.entries.len());
                                egui::CollapsingHeader::new(
                                    RichText::new(header).strong().color(theme::TEXT),
                                )
                                .id_salt(("wiremoth.site-map", host.host.as_str()))
                                .default_open(true)
                                .show(ui, |ui| {
                                    for entry in &host.entries {
                                        let selected_here = selected == Some(entry.id);
                                        let label = if entry.count > 1 {
                                            format!(
                                                "{} {}  x{}",
                                                entry.method, entry.path, entry.count
                                            )
                                        } else {
                                            format!("{} {}", entry.method, entry.path)
                                        };
                                        let text = RichText::new(label).monospace().color(
                                            if selected_here {
                                                theme::ACCENT_DARK
                                            } else {
                                                theme::TEXT
                                            },
                                        );
                                        if ui.selectable_label(selected_here, text).clicked() {
                                            picked = Some(entry.id);
                                        }
                                    }
                                });
                            }
                            ui.add_space(6.0);
                            ui.label(
                                RichText::new(
                                    "HTTP only. CONNECT tunnels are omitted until TLS interception lands.",
                                )
                                .small()
                                .color(theme::TEXT_MUTED),
                            );
                        });
                });
        }

        if let Some(id) = picked {
            self.select_id(id);
        }

        egui::CentralPanel::default()
            .frame(
                egui::Frame::NONE
                    .fill(theme::CANVAS)
                    .inner_margin(egui::Margin::same(8)),
            )
            .show(ui, |ui| self.detail_panel(ui));
    }

    fn scope_view(&mut self, ui: &mut egui::Ui) {
        let captured_hosts = captured_hosts(&self.history);
        let mut dirty = false;
        let mut clear_scope = false;

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_max_width(780.0);
                card(ui, |ui| {
                    ui.label(
                        RichText::new("Target scope")
                            .size(17.0)
                            .strong()
                            .color(theme::TEXT),
                    );
                    ui.label(
                        "Scope currently filters the site map and HTTP history. Capture and \
                         forwarding still apply to every host.",
                    );
                    ui.add_space(8.0);
                    let mut enabled = self.scope_enabled;
                    if ui
                        .checkbox(&mut enabled, "Filter views to the scope")
                        .changed()
                    {
                        self.scope_enabled = enabled;
                        dirty = true;
                    }
                    if self.scope_enabled && self.scope.is_empty() {
                        ui.label(
                            RichText::new(
                                "No rules yet, so every host is considered in scope.",
                            )
                            .small()
                            .color(theme::TEXT_MUTED),
                        );
                    }

                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        ui.label("Include host");
                        let response = ui.add(
                            TextEdit::singleline(&mut self.scope_input)
                                .hint_text("example.com or https://example.com/path")
                                .desired_width(340.0),
                        );
                        let enter =
                            response.lost_focus() && ui.input(|input| input.key_pressed(Key::Enter));
                        if ui.button("Add").clicked() || enter {
                            if let Some(rule) =
                                site_map::normalize_scope_input(&self.scope_input)
                            {
                                if self.scope.insert(rule) {
                                    dirty = true;
                                }
                                self.scope_input.clear();
                            }
                        }
                    });

                    ui.add_space(8.0);
                    if self.scope.is_empty() {
                        ui.label(
                            RichText::new("No explicit scope rules.")
                                .small()
                                .color(theme::TEXT_MUTED),
                        );
                    } else {
                        let rules: Vec<String> = self.scope.iter().cloned().collect();
                        for rule in rules {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(&rule).monospace().color(theme::TEXT));
                                if ui.small_button("Remove").clicked() {
                                    self.scope.remove(&rule);
                                    dirty = true;
                                }
                            });
                        }
                        if ui.button("Clear scope").clicked() {
                            clear_scope = true;
                        }
                    }
                });

                ui.add_space(10.0);
                card(ui, |ui| {
                    ui.label(
                        RichText::new("Captured hosts")
                            .size(15.0)
                            .strong()
                            .color(theme::TEXT),
                    );
                    ui.label(
                        RichText::new(
                            "Add a host from observed traffic. Matching also covers subdomains.",
                        )
                        .small()
                        .color(theme::TEXT_MUTED),
                    );
                    ui.add_space(4.0);
                    if captured_hosts.is_empty() {
                        ui.label(
                            RichText::new("No hosts captured yet.")
                                .small()
                                .color(theme::TEXT_MUTED),
                        );
                    }
                    for (host, count) in &captured_hosts {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(host).monospace().color(theme::TEXT));
                            ui.label(
                                RichText::new(format!("x{count}"))
                                    .small()
                                    .color(theme::TEXT_MUTED),
                            );
                            if self.scope.contains(host) {
                                ui.label(
                                    RichText::new("in scope")
                                        .small()
                                        .color(theme::GREEN),
                                );
                            } else if ui.small_button("Add").clicked() {
                                self.scope.insert(host.clone());
                                dirty = true;
                            }
                        });
                    }
                });

                ui.add_space(8.0);
                ui.label(
                    RichText::new(
                        "Scope is view-only for now; interception and Intruder will honour it in later milestones.",
                    )
                    .small()
                    .color(theme::TEXT_MUTED),
                );
            });

        if clear_scope {
            self.scope.clear();
            dirty = true;
        }
        if dirty {
            self.invalidate_views();
        }
    }

    fn intercept_view(&mut self, ui: &mut egui::Ui) {
        let mut open_history = false;
        let mut clear = false;
        let listener = self
            .proxy
            .as_ref()
            .map(Proxy::local_addr)
            .unwrap_or(self.listen);

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_max_width(820.0);
                card(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new("Intercept")
                                .size(17.0)
                                .strong()
                                .color(theme::TEXT),
                        );
                        badge(ui, "MILESTONE 1");
                    });
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new("Requests are forwarded immediately.")
                            .strong()
                            .color(theme::TEXT),
                    );
                    ui.label(
                        "Editable interception - hold, inspect, modify, forward, or drop - is \
                         part of Milestone 1. Wiremoth currently records every exchange in \
                         HTTP history without pausing traffic.",
                    );
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        self.capture_toggle(ui);
                        if ui.button("Open HTTP history").clicked() {
                            open_history = true;
                        }
                        if ui.button("Clear history").clicked() {
                            clear = true;
                        }
                    });
                });

                ui.add_space(10.0);
                card(ui, |ui| {
                    ui.label(
                        RichText::new("Proxy listener")
                            .size(15.0)
                            .strong()
                            .color(theme::TEXT),
                    );
                    ui.add_space(2.0);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(listener.to_string()).monospace());
                        if self.proxy.is_some() {
                            ui.colored_label(theme::GREEN, "listening");
                        } else {
                            ui.colored_label(theme::RED, "not running");
                            if ui.button("Retry").clicked() {
                                let notify = self.notify.clone();
                                self.start_proxy(notify);
                            }
                        }
                    });
                    if let Some(error) = &self.proxy_error {
                        ui.label(RichText::new(error).small().color(theme::RED));
                    }
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new(
                            "HTTPS uses CONNECT passthrough. Wiremoth tunnels TLS but does not \
                             decrypt it yet; history shows the tunnel, not the requests inside.",
                        )
                        .small()
                        .color(theme::TEXT_MUTED),
                    );
                });

                ui.add_space(10.0);
                card(ui, |ui| {
                    ui.label(
                        RichText::new("Coming with interception")
                            .size(15.0)
                            .strong()
                            .color(theme::TEXT),
                    );
                    for line in [
                        "Pause matching requests before they reach the origin.",
                        "Edit headers and bodies in a raw request editor.",
                        "Forward, drop, or send the request to another tool.",
                        "Intercept only in-scope hosts.",
                    ] {
                        ui.label(format!("- {line}"));
                    }
                });
            });

        if open_history {
            self.proxy_section = ProxySection::HttpHistory;
        }
        if clear {
            self.clear_history();
        }
    }

    fn intruder_view(&mut self, ui: &mut egui::Ui) {
        let preview = self
            .selected
            .and_then(|id| self.history.get(id))
            .filter(|item| !item.is_tunnel())
            .map(view::format_request);

        card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("Intruder")
                        .size(17.0)
                        .strong()
                        .color(theme::TEXT),
                );
                badge(ui, "MILESTONE 4");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add_enabled(false, egui::Button::new("Start attack"))
                        .on_disabled_hover_text("Intruder execution is part of Milestone 4");
                });
            });
            ui.label(
                "Payload-based request fuzzing with positions, payload sets, and live results.",
            );
            ui.add_space(6.0);
            ui.separator();
            ui.add_space(6.0);

            ui.columns(2, |columns| {
                columns[0].label(
                    RichText::new("Planned workspace")
                        .strong()
                        .color(theme::TEXT),
                );
                columns[0].add_space(3.0);
                for line in [
                    "Sniper, battering ram, pitchfork, and cluster bomb attacks.",
                    "Simple lists, numbers, dates, brute-force, and runtime file payloads.",
                    "Position marking in raw requests and a live result table.",
                    "Scope-aware, user-controlled concurrency.",
                ] {
                    columns[0].label(format!("- {line}"));
                }

                columns[1].label(
                    RichText::new("Request template")
                        .strong()
                        .color(theme::TEXT),
                );
                columns[1].add_space(3.0);
                match &preview {
                    Some(text) => {
                        egui::ScrollArea::both()
                            .id_salt("wiremoth.intruder.preview")
                            .max_height(240.0)
                            .auto_shrink([false, false])
                            .show(&mut columns[1], |ui| {
                                ui.add(
                                    Label::new(
                                        RichText::new(text).monospace().color(theme::TEXT),
                                    )
                                    .selectable(true)
                                    .wrap_mode(egui::TextWrapMode::Extend),
                                );
                            });
                    }
                    None => {
                        columns[1].label(
                            RichText::new(
                                "Select a plain-HTTP exchange in Proxy history to use it as a template.",
                            )
                            .small()
                            .color(theme::TEXT_MUTED),
                        );
                    }
                }
            });
        });
        ui.add_space(8.0);
        ui.label(
            RichText::new(
                "No artificial rate limit: Wiremoth will run attacks at the concurrency and pace you configure.",
            )
            .small()
            .color(theme::ACCENT_DARK),
        );
    }

    fn repeater_view(&mut self, ui: &mut egui::Ui) {
        card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("Repeater")
                        .size(17.0)
                        .strong()
                        .color(theme::TEXT),
                );
                badge(ui, "PREVIEW");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add_enabled(false, egui::Button::new("Send"))
                        .on_disabled_hover_text("Sending arrives in Milestone 3");
                });
            });
            ui.label(
                "A read-only preview of the selected exchange. Editing and resending requests \
                 arrive in Milestone 3.",
            );
        });
        ui.add_space(8.0);

        let Some(id) = self.selected else {
            empty_state(
                ui,
                "No request selected",
                "Select an exchange in Proxy history to preview it here.",
            );
            return;
        };
        self.ensure_detail(id);
        let Some(cache) = &self.detail else {
            empty_state(
                ui,
                "Exchange no longer in history",
                "The selected exchange was evicted or cleared.",
            );
            return;
        };

        egui::Panel::left(REPEATER_REQUEST_ID)
            .resizable(true)
            .default_size((ui.available_width() / 2.0).max(240.0))
            .min_size(180.0)
            .frame(
                egui::Frame::NONE
                    .fill(theme::SURFACE)
                    .inner_margin(egui::Margin::same(10)),
            )
            .show(ui, |ui| {
                preview_pane(ui, "Request", "as captured", &cache.request.raw, "request");
            });

        egui::CentralPanel::default()
            .frame(
                egui::Frame::NONE
                    .fill(theme::SURFACE)
                    .inner_margin(egui::Margin::same(10)),
            )
            .show(ui, |ui| {
                preview_pane(
                    ui,
                    "Response",
                    "as captured",
                    &cache.response.raw,
                    "response",
                );
            });
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
        if self.memory.supported() {
            ctx.request_repaint_after(self.memory.time_until_next_sample());
        }
    }
}

fn main_tab_button(ui: &mut egui::Ui, selected: bool, label: &str) -> egui::Response {
    let text = RichText::new(label).size(15.0).strong().color(if selected {
        theme::ACCENT_DARK
    } else {
        theme::TEXT_MUTED
    });
    ui.add(
        egui::Button::new(text)
            .fill(if selected {
                theme::ACCENT_SOFT
            } else {
                Color32::TRANSPARENT
            })
            .stroke(if selected {
                Stroke::new(1.0, theme::ACCENT)
            } else {
                Stroke::NONE
            })
            .corner_radius(5.0)
            .min_size(egui::vec2(84.0, 28.0)),
    )
}

fn section_tab(ui: &mut egui::Ui, selected: bool, label: &str) -> egui::Response {
    let text = RichText::new(label).size(13.0).color(if selected {
        theme::ACCENT_DARK
    } else {
        theme::TEXT_MUTED
    });
    let text = if selected { text.strong() } else { text };
    ui.add(
        egui::Button::new(text)
            .fill(if selected {
                theme::ACCENT_WASH
            } else {
                Color32::TRANSPARENT
            })
            .stroke(if selected {
                Stroke::new(1.0, theme::ACCENT)
            } else {
                Stroke::NONE
            })
            .corner_radius(4.0)
            .min_size(egui::vec2(0.0, 24.0)),
    )
}

fn message_tab(ui: &mut egui::Ui, selected: bool, label: &str) -> egui::Response {
    let text = RichText::new(label).size(13.5).color(if selected {
        theme::ACCENT_DARK
    } else {
        theme::TEXT_MUTED
    });
    let text = if selected { text.strong() } else { text };
    ui.add(
        egui::Button::new(text)
            .fill(if selected {
                theme::ACCENT_WASH
            } else {
                Color32::TRANSPARENT
            })
            .stroke(if selected {
                Stroke::new(1.0, theme::ACCENT)
            } else {
                Stroke::NONE
            })
            .corner_radius(4.0)
            .min_size(egui::vec2(88.0, 24.0)),
    )
}

fn message_mode_tab(ui: &mut egui::Ui, selected: bool, label: &str) -> egui::Response {
    let text = RichText::new(label).size(12.5).color(if selected {
        theme::ACCENT_DARK
    } else {
        theme::TEXT_MUTED
    });
    let text = if selected { text.strong() } else { text };
    ui.add(
        egui::Button::new(text)
            .fill(if selected {
                theme::ACCENT_WASH
            } else {
                Color32::TRANSPARENT
            })
            .stroke(if selected {
                Stroke::new(1.0, theme::ACCENT)
            } else {
                Stroke::NONE
            })
            .corner_radius(4.0)
            .min_size(egui::vec2(0.0, 21.0)),
    )
}

fn message_tab_bar(
    ui: &mut egui::Ui,
    pane: &mut MessagePane,
    mode: &mut MessageView,
    item: Option<&HistoryItem>,
) {
    ui.horizontal(|ui| {
        if message_tab(ui, *pane == MessagePane::Request, "Request").clicked() {
            *pane = MessagePane::Request;
        }
        if message_tab(ui, *pane == MessagePane::Response, "Response").clicked() {
            *pane = MessagePane::Response;
        }
        if let Some(item) = item {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
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
                ui.colored_label(view::status_color(item), view::status_text(item));
                ui.label(
                    RichText::new(format!("#{} {}", item.id, item.request.method))
                        .small()
                        .color(theme::TEXT_MUTED),
                );
            });
        }
    });
    ui.add_space(2.0);
    ui.horizontal(|ui| {
        if message_mode_tab(ui, *mode == MessageView::Raw, "Raw").clicked() {
            *mode = MessageView::Raw;
        }
        if message_mode_tab(ui, *mode == MessageView::Headers, "Headers").clicked() {
            *mode = MessageView::Headers;
        }
        if message_mode_tab(ui, *mode == MessageView::Hex, "Hex").clicked() {
            *mode = MessageView::Hex;
        }
    });
    ui.add_space(2.0);
}

fn message_text(ui: &mut egui::Ui, id: u64, pane: MessagePane, mode: MessageView, text: &str) {
    egui::Frame::NONE
        .fill(Color32::WHITE)
        .stroke(Stroke::new(1.0, theme::BORDER))
        .corner_radius(4.0)
        .inner_margin(egui::Margin::same(8))
        .show(ui, |ui| {
            ui.set_min_height(ui.available_height());
            if mode == MessageView::Hex {
                // A single text galley for a 64 KiB hex dump is surprisingly
                // expensive to lay out. Virtualize the lines so memory stays
                // proportional to the visible page rather than the whole dump.
                message_hex(ui, id, pane, mode, text);
            } else {
                egui::ScrollArea::both()
                    .id_salt(("wiremoth.message", id, pane as u8, mode as u8))
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.add(
                            Label::new(RichText::new(text).monospace().color(theme::TEXT))
                                .selectable(true)
                                .wrap_mode(egui::TextWrapMode::Extend),
                        );
                    });
            }
        });
}

fn message_hex(ui: &mut egui::Ui, id: u64, pane: MessagePane, mode: MessageView, text: &str) {
    let lines: Vec<&str> = text.lines().collect();
    let row_height = ui.text_style_height(&egui::TextStyle::Monospace) + 2.0;

    egui::ScrollArea::vertical()
        .id_salt(("wiremoth.hex", id, pane as u8, mode as u8))
        .auto_shrink([false, false])
        .show_rows(ui, row_height, lines.len(), |ui, range| {
            ui.spacing_mut().item_spacing.y = 0.0;
            for line in &lines[range] {
                ui.add(
                    Label::new(RichText::new(*line).monospace().color(theme::TEXT))
                        .selectable(true)
                        .wrap_mode(egui::TextWrapMode::Extend),
                );
            }
        });
}

fn preview_pane(ui: &mut egui::Ui, title: &str, subtitle: &str, text: &str, salt: &str) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(title).strong().color(theme::TEXT));
        ui.label(RichText::new(subtitle).small().color(theme::TEXT_MUTED));
    });
    ui.separator();
    egui::ScrollArea::both()
        .id_salt(("wiremoth.preview", salt))
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.add(
                Label::new(RichText::new(text).monospace().color(theme::TEXT))
                    .selectable(true)
                    .wrap_mode(egui::TextWrapMode::Extend),
            );
        });
}

fn memory_metric(ui: &mut egui::Ui, label: &str, value: Option<u64>, color: Color32) {
    ui.vertical(|ui| {
        ui.label(RichText::new(label).small().color(theme::TEXT_MUTED));
        ui.label(
            RichText::new(
                value
                    .map(view::human_bytes)
                    .unwrap_or_else(|| "n/a".to_owned()),
            )
            .size(17.0)
            .strong()
            .color(color),
        );
    });
}

fn memory_graph(ui: &mut egui::Ui, samples: &[u64]) {
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), 132.0),
        egui::Sense::hover(),
    );
    if !ui.is_rect_visible(rect) {
        return;
    }

    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, egui::CornerRadius::same(6), theme::SURFACE_TINT);
    painter.rect_stroke(
        rect,
        egui::CornerRadius::same(6),
        Stroke::new(1.0, theme::BORDER),
        egui::StrokeKind::Inside,
    );

    let chart = rect.shrink2(egui::vec2(10.0, 12.0));
    for row in 0..=4 {
        let y = chart.top() + chart.height() * row as f32 / 4.0;
        painter.line_segment(
            [egui::pos2(chart.left(), y), egui::pos2(chart.right(), y)],
            Stroke::new(1.0, theme::BORDER),
        );
    }

    let Some(max) = samples.iter().copied().max().filter(|max| *max > 0) else {
        return;
    };
    let ceiling = (max as f64 * 1.1).max(1.0);
    let denominator = samples.len().saturating_sub(1).max(1) as f32;
    let points: Vec<egui::Pos2> = samples
        .iter()
        .enumerate()
        .map(|(index, rss)| {
            let x = if samples.len() == 1 {
                chart.center().x
            } else {
                chart.left() + chart.width() * index as f32 / denominator
            };
            let y = chart.bottom() - chart.height() * (*rss as f64 / ceiling).min(1.0) as f32;
            egui::pos2(x, y)
        })
        .collect();

    painter.add(egui::Shape::line(
        points.clone(),
        Stroke::new(1.5, theme::ACCENT),
    ));
    if let Some(last) = points.last() {
        painter.circle_filled(*last, 3.0, theme::ACCENT);
    }
}

fn card<R>(ui: &mut egui::Ui, add_contents: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::NONE
        .fill(theme::SURFACE)
        .stroke(Stroke::new(1.0, theme::BORDER))
        .corner_radius(6.0)
        .inner_margin(egui::Margin::same(12))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            add_contents(ui)
        })
        .inner
}

fn badge(ui: &mut egui::Ui, text: &str) {
    egui::Frame::NONE
        .fill(theme::ACCENT_WASH)
        .stroke(Stroke::new(1.0, theme::ACCENT_SOFT))
        .corner_radius(10.0)
        .inner_margin(egui::Margin::symmetric(7, 2))
        .show(ui, |ui| {
            ui.label(
                RichText::new(text)
                    .size(10.5)
                    .strong()
                    .color(theme::ACCENT_DARK),
            );
        });
}

fn empty_state(ui: &mut egui::Ui, title: &str, body: &str) {
    ui.add_space(28.0);
    ui.vertical_centered(|ui| {
        ui.label(
            RichText::new(title)
                .size(16.0)
                .strong()
                .color(theme::TEXT_MUTED),
        );
        ui.add_space(4.0);
        ui.label(RichText::new(body).color(theme::TEXT_MUTED));
    });
}

fn request_url(item: &HistoryItem) -> String {
    if item.is_tunnel() {
        return format!("https://{}", item.request.host);
    }
    if item.request.uri.starts_with("http://") || item.request.uri.starts_with("https://") {
        item.request.uri.clone()
    } else {
        format!("http://{}{}", item.request.host, item.request.path)
    }
}

fn mime_type(item: &HistoryItem) -> String {
    if item.is_tunnel() {
        return "TLS tunnel".to_string();
    }
    item.response
        .as_ref()
        .and_then(|head| {
            head.headers
                .iter()
                .find(|(name, _)| name.eq_ignore_ascii_case("content-type"))
        })
        .map(|(_, value)| value.split(';').next().unwrap_or(value).trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "-".to_string())
}

fn captured_hosts(history: &History) -> Vec<(String, usize)> {
    let mut hosts: BTreeMap<String, usize> = BTreeMap::new();
    for item in history.iter() {
        if !item.request.host.is_empty() {
            *hosts.entry(item.request.host.clone()).or_insert(0) += 1;
        }
    }
    hosts.into_iter().collect()
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
        let max_size = (monitor / zoom - egui::vec2(40.0, 80.0)).max(egui::vec2(320.0, 240.0));
        size = size.min(max_size);
    }
    ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(size));
}

fn matches_filter(item: &HistoryItem, needle: &str) -> bool {
    let status = item
        .status()
        .map(|status| status.to_string())
        .unwrap_or_default();
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
