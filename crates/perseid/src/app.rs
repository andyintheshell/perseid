//! The egui application: a Burp-style workspace over the captured traffic.
//!
//! The navigation intentionally mirrors the tools Burp users already know:
//! Target, Proxy, Intruder, Repeater, and Decoder. Proxy history, editable
//! interception, the Target site map, scope enforcement, match/replace rules,
//! the direct Repeater, and Decoder's encode/decode/hash chain are live;
//! Intruder remains a clearly marked preview until its engine lands.
//!
//! This module holds the application state, the window shell, and the shared
//! top-level panels. The tabs and supporting pieces are split by area:
//! [`decoder`], [`intercept`], [`proxy`], [`repeater`], [`rules`], [`target`],
//! [`previews`], [`windows`], [`widgets`], [`message_view`], [`util`], and
//! [`scale`].

mod decoder;
mod intercept;
mod message_view;
mod previews;
mod proxy;
mod repeater;
mod rules;
mod scale;
#[cfg(feature = "screenshots")]
mod screenshots;
mod target;
mod tls;
mod util;
mod widgets;
mod windows;

use std::collections::{BTreeSet, VecDeque};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crate::proxy::{
    CertificateAuthority, InterceptKey, MatchReplaceRule, Proxy, ProxyConfig, ProxySettings,
    Repeater, TlsConfig,
};
use egui::{Color32, Key, RichText, Stroke};

use self::intercept::InterceptEntry;

use crate::Args;
use crate::branding;
use crate::ca_store;
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
const FILTER_ID: &str = "perseid.filter";
const PROXY_DETAIL_ID: &str = "perseid.proxy.detail";
const PROXY_REQUEST_ID: &str = "perseid.proxy.request";
const TARGET_TREE_ID: &str = "perseid.target.tree";

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
const TAB_DECODER_SHORTCUT: egui::KeyboardShortcut = egui::KeyboardShortcut::new(
    egui::Modifiers::COMMAND.plus(egui::Modifiers::SHIFT),
    Key::D,
);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum MainTab {
    Target,
    #[default]
    Proxy,
    Intruder,
    Repeater,
    Decoder,
}

impl MainTab {
    /// Burp's tool order, so muscle memory carries over.
    const ALL: [(Self, &'static str); 5] = [
        (Self::Target, "Target"),
        (Self::Proxy, "Proxy"),
        (Self::Intruder, "Intruder"),
        (Self::Repeater, "Repeater"),
        (Self::Decoder, "Decoder"),
    ];
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum ProxySection {
    Intercept,
    #[default]
    HttpHistory,
    MatchReplace,
    Tls,
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
    Pretty,
    Raw,
    Hex,
}

#[derive(Clone, Copy)]
enum WindowControl {
    Minimize,
    Maximize,
    Close,
}

pub struct PerseidApp {
    proxy: Option<Proxy>,
    proxy_error: Option<String>,
    notify: Arc<dyn Fn() + Send + Sync>,
    listen: SocketAddr,
    capture_limit: usize,
    intercept_limit: usize,
    history: History,
    dropped_seen: u64,
    paused: bool,
    intercept_enabled: bool,
    /// Whether server responses are also held. Burp leaves response
    /// interception off by default and exposes it as a separate rule set.
    intercept_responses: bool,
    intercept_queue: VecDeque<InterceptEntry>,
    intercept_selected: BTreeSet<InterceptKey>,
    intercept_active: Option<InterceptKey>,
    intercept_anchor: Option<InterceptKey>,
    intercept_scroll_to_active: bool,
    scope_enforce: bool,
    match_replace: Vec<MatchReplaceRule>,
    rules_error: Option<String>,
    filter: String,
    visible: Vec<u64>,
    visible_key: Option<(String, u64)>,
    selected: Option<u64>,
    scroll_to_selected: bool,
    detail: Option<DetailCache>,
    main_tab: MainTab,
    proxy_section: ProxySection,
    target_section: TargetSection,
    request_view: MessageView,
    response_view: MessageView,
    /// Direct replay engine, started on the first Send so a proxy-off run can
    /// still use Repeater.
    repeater: Option<Repeater>,
    repeater_tabs: Vec<repeater::RepeaterTab>,
    repeater_active: usize,
    repeater_next_tab: u64,
    decoder: decoder::DecoderState,
    site_map: Option<(u64, SiteMap)>,
    scope: BTreeSet<String>,
    scope_enabled: bool,
    scope_input: String,
    /// Where the TLS interception CA lives on disk.
    ca_dir: PathBuf,
    /// Loaded CA, if the user created one. `None` keeps HTTPS on passthrough.
    tls_ca: Option<Arc<CertificateAuthority>>,
    /// Why the stored CA could not be loaded, if it failed.
    tls_ca_error: Option<String>,
    /// Whether CONNECT targets are decrypted (subject to the pass-through list).
    tls_intercept: bool,
    /// Show the first-run certificate setup window.
    tls_setup_open: bool,
    tls_passthrough: Vec<String>,
    tls_passthrough_input: String,
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
    #[cfg(feature = "screenshots")]
    screenshots: Option<screenshots::ScreenshotDriver>,
}

/// Formatted request/response text for the selected exchange, rebuilt only
/// when the selection or its lifecycle changes.
struct DetailCache {
    id: u64,
    request: MessageViews,
    response: MessageViews,
}

/// Draw one custom title-bar window control.
fn window_control(ui: &mut egui::Ui, control: WindowControl, maximized: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(24.0, 20.0), egui::Sense::click());
    let hovered = response.hovered();
    let (fill, color) = match control {
        WindowControl::Close if hovered => (theme::RED, Color32::WHITE),
        _ if hovered => (theme::ACCENT_WASH, theme::TEXT),
        _ => (Color32::TRANSPARENT, theme::TEXT),
    };
    if fill != Color32::TRANSPARENT {
        ui.painter().rect_filled(rect, 3.0, fill);
    }

    let center = rect.center();
    let stroke = Stroke::new(1.0, color);
    match control {
        WindowControl::Minimize => {
            ui.painter().line_segment(
                [
                    egui::pos2(center.x - 4.0, center.y + 3.0),
                    egui::pos2(center.x + 4.0, center.y + 3.0),
                ],
                stroke,
            );
        }
        WindowControl::Maximize if maximized => {
            let back = egui::Rect::from_min_size(
                egui::pos2(center.x - 2.0, center.y - 4.0),
                egui::vec2(6.0, 6.0),
            );
            let front = egui::Rect::from_min_size(
                egui::pos2(center.x - 4.0, center.y - 2.0),
                egui::vec2(6.0, 6.0),
            );
            ui.painter()
                .rect_stroke(back, 0.0, stroke, egui::StrokeKind::Inside);
            ui.painter()
                .rect_stroke(front, 0.0, stroke, egui::StrokeKind::Inside);
        }
        WindowControl::Maximize => {
            let square = egui::Rect::from_center_size(center, egui::vec2(7.0, 7.0));
            ui.painter()
                .rect_stroke(square, 0.0, stroke, egui::StrokeKind::Inside);
        }
        WindowControl::Close => {
            ui.painter().line_segment(
                [
                    egui::pos2(center.x - 3.5, center.y - 3.5),
                    egui::pos2(center.x + 3.5, center.y + 3.5),
                ],
                stroke,
            );
            ui.painter().line_segment(
                [
                    egui::pos2(center.x + 3.5, center.y - 3.5),
                    egui::pos2(center.x - 3.5, center.y + 3.5),
                ],
                stroke,
            );
        }
    }

    response
        .on_hover_text(match control {
            WindowControl::Minimize => "Minimize",
            WindowControl::Maximize if maximized => "Restore",
            WindowControl::Maximize => "Maximize",
            WindowControl::Close => "Close",
        })
        .on_hover_cursor(egui::CursorIcon::PointingHand)
}

impl PerseidApp {
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

        let screenshot_mode = {
            #[cfg(feature = "screenshots")]
            {
                args.screenshot_dir.is_some()
            }
            #[cfg(not(feature = "screenshots"))]
            {
                false
            }
        };
        let ca_dir = args.ca_dir.clone().unwrap_or_else(ca_store::default_dir);
        // Create the CA once, on first launch, and reuse it forever after. The
        // screenshot driver supplies its own in-memory CA so documentation
        // builds never touch the user's real trust material.
        let (tls_ca, tls_ca_error, ca_created) = if screenshot_mode {
            (None, None, false)
        } else {
            match ca_store::ensure(&ca_dir) {
                Ok((ca, created)) => {
                    if created {
                        log::info!(
                            "created the local TLS certificate authority at {}",
                            ca_dir.display()
                        );
                    }
                    (Some(ca), None, created)
                }
                Err(err) => {
                    log::warn!("could not prepare the TLS certificate authority: {err}");
                    (None, Some(err), false)
                }
            }
        };
        if args.tls_intercept {
            log::warn!("--tls-intercept is deprecated; HTTPS interception is enabled by default");
        }
        let tls_intercept = !args.no_tls_intercept && tls_ca.is_some();
        let tls_setup_open = ca_created && tls_intercept;

        let mut app = Self {
            proxy: None,
            proxy_error: None,
            notify,
            listen: args.listen,
            capture_limit: args.capture_limit,
            intercept_limit: args.intercept_limit,
            history: History::new(args.history_limit, args.history_bytes),
            dropped_seen: 0,
            paused: false,
            intercept_enabled: false,
            intercept_responses: false,
            intercept_queue: VecDeque::new(),
            intercept_selected: BTreeSet::new(),
            intercept_active: None,
            intercept_anchor: None,
            intercept_scroll_to_active: false,
            scope_enforce: false,
            match_replace: Vec::new(),
            rules_error: None,
            filter: String::new(),
            visible: Vec::new(),
            visible_key: None,
            selected: None,
            scroll_to_selected: false,
            detail: None,
            main_tab: MainTab::default(),
            proxy_section: ProxySection::default(),
            target_section: TargetSection::default(),
            request_view: MessageView::default(),
            response_view: MessageView::default(),
            repeater: None,
            repeater_tabs: Vec::new(),
            repeater_active: 0,
            repeater_next_tab: 1,
            decoder: decoder::DecoderState::new(),
            site_map: None,
            scope: BTreeSet::new(),
            scope_enabled: false,
            scope_input: String::new(),
            ca_dir,
            tls_ca,
            tls_ca_error,
            tls_intercept,
            tls_setup_open,
            tls_passthrough: args.tls_passthrough.clone(),
            tls_passthrough_input: String::new(),
            native_ui_scale,
            base_ui_scale,
            mark,
            lockup: None,
            memory: MemoryMeter::new(),
            memory_open: false,
            about_open: false,
            #[cfg(feature = "screenshots")]
            screenshots: None,
        };
        #[cfg(feature = "screenshots")]
        if let Some(dir) = args.screenshot_dir.clone() {
            match screenshots::ScreenshotDriver::new(dir) {
                Ok(driver) => {
                    app.screenshots = Some(driver);
                }
                Err(err) => {
                    log::error!("screenshot mode: {err}");
                    std::process::exit(1);
                }
            }
        }
        if !args.no_proxy {
            let notify = app.notify.clone();
            app.start_proxy(notify);
        }
        #[cfg(feature = "screenshots")]
        if app.screenshots.is_some() && !args.no_proxy && app.proxy.is_none() {
            // Keep the demo runnable on a desktop where port 8080 is busy.
            app.listen = SocketAddr::from(([127, 0, 0, 1], 0));
            let notify = app.notify.clone();
            app.start_proxy(notify);
        }
        app
    }

    /// URL of the built-in certificate page for the running proxy, if any.
    ///
    /// Loopback aliases are used when the proxy binds a wildcard address so
    /// the link is clickable from the same machine.
    pub(super) fn certificate_page_url(&self) -> Option<String> {
        self.proxy.as_ref().map(|proxy| {
            let addr = proxy.local_addr();
            let host = match addr.ip() {
                std::net::IpAddr::V4(ip) if ip.is_unspecified() => "127.0.0.1".to_string(),
                std::net::IpAddr::V6(ip) if ip.is_unspecified() => "[::1]".to_string(),
                std::net::IpAddr::V4(ip) => ip.to_string(),
                std::net::IpAddr::V6(ip) => format!("[{ip}]"),
            };
            format!("http://{host}:{}/", addr.port())
        })
    }

    fn start_proxy(&mut self, notify: Arc<dyn Fn() + Send + Sync>) {
        let (settings, rule_error) = self.sanitized_proxy_settings();
        self.rules_error = rule_error;
        let config = ProxyConfig {
            listen: self.listen,
            capture_limit: self.capture_limit,
            intercept_limit: self.intercept_limit,
            settings,
            tls: TlsConfig {
                authority: self.tls_ca.clone(),
                extra_roots_pem: Vec::new(),
            },
            notify: Some(notify),
            ..ProxyConfig::default()
        };
        match Proxy::start(config) {
            Ok(proxy) => {
                log::info!("proxy listening on {}", proxy.local_addr());
                proxy.set_paused(self.paused);
                self.proxy = Some(proxy);
                self.proxy_error = None;
                self.dropped_seen = 0;
                self.clear_intercept_queue();
            }
            Err(err) => {
                log::error!("could not start the proxy: {err}");
                self.proxy = None;
                self.proxy_error = Some(err.to_string());
            }
        }
    }

    fn proxy_settings(&self) -> ProxySettings {
        ProxySettings {
            // Burp's master toggle arms interception; requests are held by
            // default, while responses need the separate response rule set.
            intercept_requests: self.intercept_enabled,
            intercept_responses: self.intercept_enabled && self.intercept_responses,
            scope_enforce: self.scope_enforce,
            scope_rules: self.scope.iter().cloned().collect(),
            tls_intercept: self.tls_intercept,
            tls_passthrough: self.tls_passthrough.clone(),
            match_replace: self.match_replace.clone(),
        }
    }

    /// Settings sent to the proxy: enabled rules that compile. A broken regex
    /// stays visible in the editor but never becomes part of the live policy.
    fn sanitized_proxy_settings(&self) -> (ProxySettings, Option<String>) {
        let mut settings = self.proxy_settings();
        let error = settings.validate().err().map(|err| err.to_string());
        if error.is_some() {
            settings
                .match_replace
                .retain(|rule| rule.validate().is_ok());
        }
        (settings, error)
    }

    /// Push interception, scope, and rewrite settings to the running proxy.
    /// Invalid regular expressions stay visible in the UI while valid rules
    /// and toggles remain active.
    fn apply_proxy_settings(&mut self) {
        let (settings, rule_error) = self.sanitized_proxy_settings();
        let applied = if let Some(proxy) = &self.proxy {
            proxy.set_settings(settings)
        } else {
            Ok(())
        };
        self.rules_error = match (rule_error, applied) {
            (Some(err), _) => Some(err),
            (None, Ok(())) => None,
            (None, Err(err)) => Some(err.to_string()),
        };
    }

    fn drain_events(&mut self, ctx: &egui::Context) {
        // Repeater is independent of the proxy, so drain it before the proxy
        // early return. A result may arrive while the proxy is stopped.
        self.drain_repeater_results();
        if self
            .repeater_tabs
            .iter()
            .any(repeater::RepeaterTab::is_pending)
        {
            ctx.request_repaint_after(Duration::from_millis(100));
        }

        let Some(proxy) = self.proxy.as_mut() else {
            return;
        };
        let dropped = proxy.dropped_events();
        if dropped > self.dropped_seen {
            self.dropped_seen = dropped;
            self.history.interrupt_in_flight(
                "capture events were dropped; the exchange result is unavailable",
            );
        }
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
        // Interception has its own bounded, backpressured channel. Draining it
        // here keeps held requests moving even when the history event queue is
        // full or capture is paused.
        let mut received_intercept = false;
        for _ in 0..MAX_EVENTS_PER_FRAME {
            let Some(item) = proxy.try_recv_intercept() else {
                break;
            };
            self.intercept_queue.push_back(InterceptEntry::new(item));
            received_intercept = true;
        }
        // Drain what the proxy queued before declaring it stopped, then
        // resolve any exchange whose terminal event never made it through.
        let stopped = !proxy.is_running();
        if stopped {
            self.proxy = None;
            self.proxy_error = Some("the proxy stopped unexpectedly".to_string());
            self.history
                .interrupt_in_flight("proxy stopped before the exchange finished");
            self.clear_intercept_queue();
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
        if received_intercept {
            self.intercept_scroll_to_active = true;
        }
        if received_intercept || !self.intercept_queue.is_empty() {
            self.sync_intercept_selection();
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

        let (target, proxy, intruder, repeater, decoder) = ctx.input_mut(|input| {
            (
                input.consume_shortcut(&TAB_TARGET_SHORTCUT),
                input.consume_shortcut(&TAB_PROXY_SHORTCUT),
                input.consume_shortcut(&TAB_INTRUDER_SHORTCUT),
                input.consume_shortcut(&TAB_REPEATER_SHORTCUT),
                input.consume_shortcut(&TAB_DECODER_SHORTCUT),
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

        // Ctrl+Enter is the Burp-style send gesture. Consume it before
        // `egui_wants_keyboard_input` so it works while the request editor has
        // focus.
        if self.main_tab == MainTab::Repeater
            && ctx.input_mut(|input| input.consume_key(egui::Modifiers::COMMAND, Key::Enter))
        {
            self.send_active_repeater();
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

    fn title_bar(&mut self, ui: &mut egui::Ui) {
        let maximized = ui
            .ctx()
            .input(|input| input.viewport().maximized.unwrap_or(false));

        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(2.0, 2.0);
            ui.spacing_mut().button_padding = egui::vec2(5.0, 1.0);

            if let Some(mark) = &self.mark {
                let response = ui
                    .add(
                        egui::Image::from_texture(mark)
                            .fit_to_exact_size(egui::vec2(17.0, 17.0))
                            .corner_radius(4.0)
                            .sense(egui::Sense::click()),
                    )
                    .on_hover_text("About Perseid")
                    .on_hover_cursor(egui::CursorIcon::PointingHand);
                if response.clicked() {
                    self.about_open = true;
                }
            }
            ui.add_space(3.0);

            ui.scope(|ui| {
                let widgets = &mut ui.style_mut().visuals.widgets;
                widgets.inactive.weak_bg_fill = Color32::TRANSPARENT;
                widgets.inactive.bg_stroke = Stroke::NONE;
                widgets.inactive.fg_stroke = Stroke::new(1.0, theme::TEXT);
                widgets.hovered.weak_bg_fill = theme::ACCENT_WASH;
                widgets.hovered.bg_stroke = Stroke::NONE;
                widgets.hovered.fg_stroke = Stroke::new(1.0, theme::TEXT);
                widgets.active.weak_bg_fill = theme::ACCENT_SOFT;
                widgets.active.bg_stroke = Stroke::NONE;
                widgets.active.fg_stroke = Stroke::new(1.0, theme::TEXT);

                ui.menu_button("Perseid", |ui| {
                    if ui.button("About Perseid").clicked() {
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
                    if ui.button("About Perseid").clicked() {
                        self.about_open = true;
                        ui.close();
                    }
                    ui.hyperlink_to(
                        "Project on GitHub",
                        "https://github.com/andyintheshell/perseid",
                    );
                });
            });

            // The empty part of the title bar drags the window, like Burp's
            // menu bar. Reserve room on the right for the window controls.
            let drag_height = ui.spacing().interact_size.y.max(18.0);
            let drag_width = (ui.available_width() - 3.0 * 24.0 - 8.0).max(0.0);
            let (drag_rect, drag_response) = ui.allocate_exact_size(
                egui::vec2(drag_width, drag_height),
                egui::Sense::click_and_drag(),
            );
            if drag_response.drag_started() {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
            }
            if drag_response.double_clicked() {
                ui.ctx()
                    .send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
            }
            if drag_rect.width() > 0.0 {
                ui.painter().text(
                    drag_rect.center(),
                    egui::Align2::CENTER_CENTER,
                    "Perseid",
                    egui::FontId::proportional(10.5),
                    theme::TEXT_MUTED,
                );
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if window_control(ui, WindowControl::Close, maximized).clicked() {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
                if window_control(ui, WindowControl::Maximize, maximized).clicked() {
                    ui.ctx()
                        .send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
                }
                if window_control(ui, WindowControl::Minimize, maximized).clicked() {
                    ui.ctx()
                        .send_viewport_cmd(egui::ViewportCommand::Minimized(true));
                }
            });
        });
    }

    fn main_tabs(&mut self, ui: &mut egui::Ui) {
        let shortcuts = [
            ui.ctx().format_shortcut(&TAB_TARGET_SHORTCUT),
            ui.ctx().format_shortcut(&TAB_PROXY_SHORTCUT),
            ui.ctx().format_shortcut(&TAB_INTRUDER_SHORTCUT),
            ui.ctx().format_shortcut(&TAB_REPEATER_SHORTCUT),
            ui.ctx().format_shortcut(&TAB_DECODER_SHORTCUT),
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
            )
            .on_hover_text(format!(
                "{} of {} captured body bytes retained",
                view::human_bytes(self.history.retained_bytes() as u64),
                view::human_bytes(self.history.byte_limit() as u64)
            ));

            let in_flight = self.history.in_flight();
            if in_flight > 0 {
                ui.separator();
                ui.colored_label(theme::BLUE, format!("{in_flight} in flight"));
            }

            let pending = self.intercept_queue.len();
            if pending > 0 {
                ui.separator();
                ui.colored_label(theme::AMBER, format!("{pending} awaiting intercept"));
            } else if self.proxy.is_some() && self.intercept_enabled {
                ui.separator();
                ui.label(
                    RichText::new("intercept armed")
                        .small()
                        .color(theme::ACCENT_DARK),
                );
            }

            let repeater_pending = self
                .repeater_tabs
                .iter()
                .filter(|tab| tab.is_pending())
                .count();
            if repeater_pending > 0 {
                ui.separator();
                ui.colored_label(theme::BLUE, format!("{repeater_pending} Repeater pending"));
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
                let rejected = proxy.rejected_connections() + proxy.rejected_tunnels();
                if rejected > 0 {
                    ui.separator();
                    ui.colored_label(
                        theme::AMBER,
                        format!("{rejected} rejected by connection limits"),
                    );
                }
                let blocked = proxy.blocked_by_scope();
                if blocked > 0 {
                    ui.separator();
                    ui.colored_label(theme::AMBER, format!("{blocked} blocked by scope"));
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

            if self.paused {
                ui.separator();
                ui.colored_label(theme::AMBER, "capture paused");
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                self.memory_indicator(ui);
            });
        });
    }

    /// The window is undecorated so the menu can share the title-bar row with
    /// the window controls. Draw its resize border ourselves.
    fn resize_handles(&self, ui: &mut egui::Ui) {
        let rect = ui.ctx().content_rect();
        let edge = 4.0;
        let corner = 10.0;
        let handles = [
            (
                egui::Rect::from_min_max(
                    egui::pos2(rect.left() + corner, rect.top()),
                    egui::pos2(rect.right() - corner, rect.top() + edge),
                ),
                egui::ResizeDirection::North,
                egui::CursorIcon::ResizeVertical,
            ),
            (
                egui::Rect::from_min_max(
                    egui::pos2(rect.left() + corner, rect.bottom() - edge),
                    egui::pos2(rect.right() - corner, rect.bottom()),
                ),
                egui::ResizeDirection::South,
                egui::CursorIcon::ResizeVertical,
            ),
            (
                egui::Rect::from_min_max(
                    egui::pos2(rect.left(), rect.top() + corner),
                    egui::pos2(rect.left() + edge, rect.bottom() - corner),
                ),
                egui::ResizeDirection::West,
                egui::CursorIcon::ResizeHorizontal,
            ),
            (
                egui::Rect::from_min_max(
                    egui::pos2(rect.right() - edge, rect.top() + corner),
                    egui::pos2(rect.right(), rect.bottom() - corner),
                ),
                egui::ResizeDirection::East,
                egui::CursorIcon::ResizeHorizontal,
            ),
            (
                egui::Rect::from_min_max(
                    egui::pos2(rect.left(), rect.top()),
                    egui::pos2(rect.left() + corner, rect.top() + corner),
                ),
                egui::ResizeDirection::NorthWest,
                egui::CursorIcon::ResizeNwSe,
            ),
            (
                egui::Rect::from_min_max(
                    egui::pos2(rect.right() - corner, rect.top()),
                    egui::pos2(rect.right(), rect.top() + corner),
                ),
                egui::ResizeDirection::NorthEast,
                egui::CursorIcon::ResizeNeSw,
            ),
            (
                egui::Rect::from_min_max(
                    egui::pos2(rect.left(), rect.bottom() - corner),
                    egui::pos2(rect.left() + corner, rect.bottom()),
                ),
                egui::ResizeDirection::SouthWest,
                egui::CursorIcon::ResizeNeSw,
            ),
            (
                egui::Rect::from_min_max(
                    egui::pos2(rect.right() - corner, rect.bottom() - corner),
                    egui::pos2(rect.right(), rect.bottom()),
                ),
                egui::ResizeDirection::SouthEast,
                egui::CursorIcon::ResizeNwSe,
            ),
        ];

        for (index, (rect, direction, cursor)) in handles.into_iter().enumerate() {
            let response = ui
                .interact(
                    rect,
                    egui::Id::unique(("perseid.window.resize", index)),
                    egui::Sense::drag(),
                )
                .on_hover_cursor(cursor);
            if response.drag_started() {
                ui.ctx()
                    .send_viewport_cmd(egui::ViewportCommand::BeginResize(direction));
            }
        }
    }

    fn main_content(&mut self, ui: &mut egui::Ui) {
        match self.main_tab {
            MainTab::Target => self.target_tab(ui),
            MainTab::Proxy => self.proxy_tab(ui),
            MainTab::Intruder => self.intruder_view(ui),
            MainTab::Repeater => self.repeater_view(ui),
            MainTab::Decoder => self.decoder_view(ui),
        }
    }
}

impl eframe::App for PerseidApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.memory.sample_if_due();
        self.drain_events(&ctx);
        self.keyboard(&ctx);
        self.rebuild_visible();

        egui::Panel::top("perseid.titlebar")
            .frame(
                egui::Frame::NONE
                    .fill(theme::SURFACE)
                    .inner_margin(egui::Margin {
                        left: 8,
                        right: 0,
                        top: 2,
                        bottom: 2,
                    }),
            )
            .show(ui, |ui| self.title_bar(ui));

        egui::Panel::top("perseid.toolbar")
            .frame(
                egui::Frame::NONE
                    .fill(theme::SURFACE)
                    .inner_margin(egui::Margin::symmetric(8, 3)),
            )
            .show(ui, |ui| self.main_tabs(ui));

        egui::Panel::bottom("perseid.statusbar")
            .frame(
                egui::Frame::NONE
                    .fill(theme::SURFACE_ALT)
                    .inner_margin(egui::Margin::symmetric(8, 2)),
            )
            .show(ui, |ui| self.status_bar(ui));

        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(theme::CANVAS))
            .show(ui, |ui| self.main_content(ui));

        self.memory_window(&ctx);
        self.about_window(&ctx);
        self.tls_setup_window(&ctx);
        self.resize_handles(ui);

        #[cfg(feature = "screenshots")]
        self.drive_screenshots(&ctx);

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
                MainTab::Repeater,
                MainTab::Decoder
            ]
        );
    }
}
