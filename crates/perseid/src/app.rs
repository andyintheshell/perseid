//! The egui application: a Burp-style workspace over the captured traffic.
//!
//! The navigation intentionally mirrors the tools Burp users already know:
//! Target, Proxy, Intruder, Repeater, Decoder, and Comparer. Proxy history,
//! editable interception, the Target site map, scope enforcement,
//! match/replace rules, the direct Repeater, Intruder's payload attacks,
//! Decoder's encode/decode/hash chain, Comparer's word/byte diffs, and the
//! global Search window are live.
//!
//! This module holds the application state, the window shell, and the shared
//! top-level panels. The tabs and supporting pieces are split by area:
//! [`comparer`], [`decoder`], [`inspector`], [`intercept`], [`intruder`],
//! [`project_ui`], [`proxy`], [`repeater`], [`rules`], [`search`], [`target`],
//! [`tls`], [`windows`], [`widgets`], [`message_view`], [`util`], [`scale`],
//! and the feature-gated [`screenshots`] driver.

mod comparer;
mod decoder;
mod inspector;
mod intercept;
mod intruder;
mod message_view;
mod project_ui;
mod proxy;
mod repeater;
mod rules;
mod scale;
#[cfg(feature = "screenshots")]
mod screenshots;
mod search;
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
    CertificateAuthority, InterceptKey, Intruder, MatchReplaceRule, Proxy, ProxyConfig, ProxyEvent,
    ProxySettings, Repeater, TlsConfig, display_ip,
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
use crate::websocket_history::WebSocketHistory;

use self::scale::{desktop_scale_override, resize_initial_window, resolve_ui_scale, stepped_scale};
use self::util::{matches_filter, websocket_matches_filter};
use self::widgets::{main_tab_button, search_button};

/// Bound on events applied per frame so a flood cannot stall rendering.
const MAX_EVENTS_PER_FRAME: usize = 1024;
const FILTER_ID: &str = "perseid.filter";
const WEBSOCKET_FILTER_ID: &str = "perseid.websocket.filter";
const PROXY_DETAIL_ID: &str = "perseid.proxy.detail";
const PROXY_WEBSOCKET_DETAIL_ID: &str = "perseid.proxy.websocket.detail";
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
const TAB_COMPARER_SHORTCUT: egui::KeyboardShortcut = egui::KeyboardShortcut::new(
    egui::Modifiers::COMMAND.plus(egui::Modifiers::SHIFT),
    Key::C,
);
const SEARCH_SHORTCUT: egui::KeyboardShortcut = egui::KeyboardShortcut::new(
    egui::Modifiers::COMMAND.plus(egui::Modifiers::SHIFT),
    Key::F,
);
#[cfg(target_os = "macos")]
const FULL_SCREEN_SHORTCUT: egui::KeyboardShortcut =
    egui::KeyboardShortcut::new(egui::Modifiers::CTRL.plus(egui::Modifiers::COMMAND), Key::F);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum MainTab {
    Target,
    #[default]
    Proxy,
    Intruder,
    Repeater,
    Decoder,
    Comparer,
}

impl MainTab {
    /// Burp's tool order, so muscle memory carries over.
    const ALL: [(Self, &'static str); 6] = [
        (Self::Target, "Target"),
        (Self::Proxy, "Proxy"),
        (Self::Intruder, "Intruder"),
        (Self::Repeater, "Repeater"),
        (Self::Decoder, "Decoder"),
        (Self::Comparer, "Comparer"),
    ];
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum ProxySection {
    Intercept,
    #[default]
    HttpHistory,
    WebSocketsHistory,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WindowControlAction {
    Fullscreen(bool),
    Maximized(bool),
}

fn window_control_action(
    is_macos: bool,
    fullscreen: bool,
    maximized: bool,
    option_held: bool,
) -> WindowControlAction {
    if !is_macos {
        return WindowControlAction::Maximized(!maximized);
    }

    if fullscreen {
        WindowControlAction::Fullscreen(false)
    } else if option_held {
        WindowControlAction::Maximized(!maximized)
    } else {
        WindowControlAction::Fullscreen(true)
    }
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
    websocket_history: WebSocketHistory,
    websocket_filter: String,
    websocket_visible: Vec<u64>,
    websocket_visible_key: Option<(String, u64)>,
    websocket_selected: Option<u64>,
    websocket_scroll_to_selected: bool,
    websocket_detail: Option<WebSocketDetailCache>,
    websocket_view: MessageView,
    main_tab: MainTab,
    proxy_section: ProxySection,
    target_section: TargetSection,
    request_view: MessageView,
    response_view: MessageView,
    inspector: inspector::InspectorState,
    search: search::SearchState,
    /// Direct replay engine, started on the first Send so a proxy-off run can
    /// still use Repeater.
    repeater: Option<Repeater>,
    repeater_tabs: Vec<repeater::RepeaterTab>,
    repeater_active: usize,
    repeater_next_tab: u64,
    /// Direct attack engine, started on the first Start attack.
    intruder_engine: Option<Intruder>,
    intruder: intruder::IntruderState,
    decoder: decoder::DecoderState,
    comparer: comparer::ComparerState,
    project: project_ui::ProjectUi,
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
    inspector: inspector::InspectorModel,
}

/// Formatted Pretty/Raw/Hex views for one selected WebSocket message.
struct WebSocketDetailCache {
    id: u64,
    views: MessageViews,
}

/// Draw one custom title-bar window control.
fn window_control(
    ui: &mut egui::Ui,
    control: WindowControl,
    maximized: bool,
    fullscreen: bool,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(26.0, 22.0), egui::Sense::click());
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
        WindowControl::Maximize if maximized || fullscreen => {
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
            WindowControl::Maximize if cfg!(target_os = "macos") && fullscreen => {
                "Exit Full Screen"
            }
            WindowControl::Maximize if cfg!(target_os = "macos") && maximized => {
                "Enter Full Screen (Option-click to Restore)"
            }
            WindowControl::Maximize if cfg!(target_os = "macos") => {
                "Enter Full Screen (Option-click to Zoom)"
            }
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
            websocket_history: WebSocketHistory::new(args.history_limit, args.history_bytes),
            websocket_filter: String::new(),
            websocket_visible: Vec::new(),
            websocket_visible_key: None,
            websocket_selected: None,
            websocket_scroll_to_selected: false,
            websocket_detail: None,
            websocket_view: MessageView::default(),
            main_tab: MainTab::default(),
            proxy_section: ProxySection::default(),
            target_section: TargetSection::default(),
            request_view: MessageView::default(),
            response_view: MessageView::default(),
            inspector: inspector::InspectorState::default(),
            search: search::SearchState::default(),
            repeater: None,
            repeater_tabs: Vec::new(),
            repeater_active: 0,
            repeater_next_tab: 1,
            intruder_engine: None,
            intruder: intruder::IntruderState::new(),
            decoder: decoder::DecoderState::new(),
            comparer: comparer::ComparerState::new(),
            project: project_ui::ProjectUi::new(args.history_limit, args.history_bytes),
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
            format!("http://{}:{}/", display_ip(addr.ip()), addr.port())
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
        sanitized_settings(self.proxy_settings())
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
        // Repeater and Intruder are independent of the proxy, so drain them
        // before the proxy early return. Results may arrive while the proxy is
        // stopped.
        self.drain_repeater_results();
        self.drain_intruder_results();
        if self
            .repeater_tabs
            .iter()
            .any(repeater::RepeaterTab::is_pending)
            || self.intruder.is_running()
        {
            ctx.request_repaint_after(Duration::from_millis(100));
        }

        let mut selected_changed = false;
        let mut received_intercept = false;
        let mut queue_drained = false;
        let stopped;
        {
            let Some(proxy) = self.proxy.as_mut() else {
                return;
            };
            // Only discard a stopped runtime after draining a queue sampled
            // after it stopped. Shutdown itself may enqueue final
            // body-completion events.
            let was_stopped = !proxy.is_running();
            let dropped = proxy.dropped_events();
            let selected = self.selected;
            let websocket_selected = self.websocket_selected;
            for _ in 0..MAX_EVENTS_PER_FRAME {
                let Some(event) = proxy.try_recv() else {
                    queue_drained = true;
                    break;
                };
                // Give the project first refusal: it assigns stable IDs and
                // queues the normalized event for the writer. The UI applies
                // the returned event so its rows and the file agree.
                let event = match self.project.handle() {
                    Some(handle) => {
                        let outcome = handle.capture(&event);
                        let Some(event) = outcome.event else {
                            continue;
                        };
                        event
                    }
                    None => event,
                };
                let id = event.id();
                selected_changed |= selected == Some(id) || websocket_selected == Some(id);
                match event {
                    ProxyEvent::WebSocketMessage { .. } => {
                        self.websocket_history.apply(event);
                    }
                    event => {
                        self.history.apply(event);
                    }
                }
            }
            if queue_drained && dropped > self.dropped_seen {
                self.dropped_seen = dropped;
                let interrupted = self.history.interrupt_in_flight(
                    "capture events were dropped; the exchange result is unavailable",
                );
                selected_changed |= interrupted > 0;
                if interrupted > 0
                    && let Some(handle) = self.project.handle()
                {
                    handle.interrupt_in_flight(
                        "capture events were dropped; the exchange result is unavailable",
                    );
                }
            }
            if !queue_drained || proxy.dropped_events() > self.dropped_seen {
                ctx.request_repaint();
            }
            // Interception has its own bounded, backpressured channel.
            // Draining it here keeps held requests moving even when the
            // history event queue is full or capture is paused.
            for _ in 0..MAX_EVENTS_PER_FRAME {
                let Some(item) = proxy.try_recv_intercept() else {
                    break;
                };
                self.intercept_queue.push_back(InterceptEntry::new(item));
                received_intercept = true;
            }
            // Drain what the proxy queued before declaring it stopped, then
            // resolve any exchange whose terminal event never made it through.
            if !was_stopped && !proxy.is_running() {
                ctx.request_repaint();
            }
            stopped = was_stopped && queue_drained;
        }
        if stopped {
            self.proxy = None;
            self.proxy_error = Some("the proxy stopped unexpectedly".to_string());
            self.history
                .interrupt_in_flight("proxy stopped before the exchange finished");
            if let Some(handle) = self.project.handle() {
                handle.interrupt_in_flight("proxy stopped before the exchange finished");
            }
            selected_changed = true;
            self.clear_intercept_queue();
        }
        if selected_changed {
            self.detail = None;
            self.websocket_detail = None;
        }
        if self
            .selected
            .is_some_and(|id| self.history.get(id).is_none())
        {
            self.selected = None;
            self.detail = None;
        }
        if self
            .websocket_selected
            .is_some_and(|id| self.websocket_history.get(id).is_none())
        {
            self.websocket_selected = None;
            self.websocket_detail = None;
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
        self.rebuild_http_visible();
        self.rebuild_websocket_visible();
    }

    fn rebuild_http_visible(&mut self) {
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

    fn rebuild_websocket_visible(&mut self) {
        let key = (
            self.websocket_filter.clone(),
            self.websocket_history.revision(),
        );
        if self.websocket_visible_key.as_ref() == Some(&key) {
            return;
        }
        self.websocket_visible.clear();

        let needle = self.websocket_filter.trim().to_lowercase();
        let scope_active = self.scope_enabled && !self.scope.is_empty();
        for item in self.websocket_history.iter() {
            if scope_active && !site_map::host_in_scope(&item.connection.host, &self.scope) {
                continue;
            }
            if needle.is_empty() || websocket_matches_filter(item, &needle) {
                self.websocket_visible.push(item.id);
            }
        }
        self.websocket_visible_key = Some(key);
    }

    fn invalidate_views(&mut self) {
        self.visible_key = None;
        self.websocket_visible_key = None;
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
        self.inspector.clear_notes();
        self.search.clear_history();
        self.invalidate_views();
        self.project.clear_http_history();
    }

    fn clear_websocket_history(&mut self) {
        self.websocket_history.clear();
        self.websocket_selected = None;
        self.websocket_detail = None;
        self.invalidate_views();
        self.project.clear_ws_history();
    }

    fn selected_row_index(&self) -> Option<usize> {
        self.selected
            .and_then(|id| self.visible.iter().position(|&visible| visible == id))
    }

    fn selected_websocket_row_index(&self) -> Option<usize> {
        self.websocket_selected.and_then(|id| {
            self.websocket_visible
                .iter()
                .position(|&visible| visible == id)
        })
    }

    fn move_selection(&mut self, delta: isize) {
        match moved_row_index(self.visible.len(), self.selected_row_index(), delta) {
            Some(next) => self.select_id(self.visible[next]),
            None => self.selected = None,
        }
    }

    fn move_websocket_selection(&mut self, delta: isize) {
        match moved_row_index(
            self.websocket_visible.len(),
            self.selected_websocket_row_index(),
            delta,
        ) {
            Some(next) => self.select_websocket_id(self.websocket_visible[next]),
            None => self.websocket_selected = None,
        }
    }

    fn select_id(&mut self, id: u64) {
        if self.selected != Some(id) {
            self.selected = Some(id);
            self.detail = None;
        }
        self.scroll_to_selected = true;
    }

    fn select_websocket_id(&mut self, id: u64) {
        if self.websocket_selected != Some(id) {
            self.websocket_selected = Some(id);
            self.websocket_detail = None;
        }
        self.websocket_scroll_to_selected = true;
    }

    fn keyboard(&mut self, ctx: &egui::Context) {
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
        let (maximized, fullscreen) = ui.ctx().input(|input| {
            (
                input.viewport().maximized.unwrap_or(false),
                input.viewport().fullscreen.unwrap_or(false),
            )
        });

        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(3.0, 2.0);
            ui.spacing_mut().button_padding = egui::vec2(7.0, 2.0);

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

                self.project_menu(ui);
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

                    #[cfg(target_os = "macos")]
                    {
                        ui.separator();
                        let fullscreen =
                            ctx.input(|input| input.viewport().fullscreen.unwrap_or(false));
                        let label = if fullscreen {
                            "Exit Full Screen"
                        } else {
                            "Enter Full Screen"
                        };
                        let fullscreen_button = egui::Button::new(label)
                            .shortcut_text(ctx.format_shortcut(&FULL_SCREEN_SHORTCUT));
                        if ui.add(fullscreen_button).clicked() {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(!fullscreen));
                            ui.close();
                        }
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
                    ui.hyperlink_to("Project on GitHub", branding::REPOSITORY_URL);
                });
            });

            // The empty part of the title bar drags the window, like Burp's
            // menu bar. Reserve room on the right for the window controls.
            let drag_height = ui.spacing().interact_size.y.max(20.0);
            let drag_width = (ui.available_width() - 3.0 * 26.0 - 10.0).max(0.0);
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
                if window_control(ui, WindowControl::Close, maximized, fullscreen).clicked() {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
                let maximize_button =
                    window_control(ui, WindowControl::Maximize, maximized, fullscreen);
                if maximize_button.clicked() {
                    let option_held =
                        cfg!(target_os = "macos") && ui.input(|input| input.modifiers.alt);
                    match window_control_action(
                        cfg!(target_os = "macos"),
                        fullscreen,
                        maximized,
                        option_held,
                    ) {
                        WindowControlAction::Fullscreen(fullscreen) => {
                            ui.ctx()
                                .send_viewport_cmd(egui::ViewportCommand::Fullscreen(fullscreen));
                        }
                        WindowControlAction::Maximized(maximized) => {
                            ui.ctx()
                                .send_viewport_cmd(egui::ViewportCommand::Maximized(maximized));
                        }
                    }
                }
                if window_control(ui, WindowControl::Minimize, maximized, fullscreen).clicked() {
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
            ui.ctx().format_shortcut(&TAB_COMPARER_SHORTCUT),
        ];
        let search_shortcut = ui.ctx().format_shortcut(&SEARCH_SHORTCUT);
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
                if search_button(ui, self.search.open)
                    .on_hover_text(format!("Search ({search_shortcut})"))
                    .clicked()
                {
                    self.search.toggle();
                }
                ui.separator();
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

            self.project_status_widget(ui);

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

            if let Some(summary) = self.intruder.status_summary() {
                ui.separator();
                ui.colored_label(theme::BLUE, summary);
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
        if ui
            .ctx()
            .input(|input| input.viewport().fullscreen.unwrap_or(false))
        {
            return;
        }

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
            MainTab::Comparer => self.comparer_view(ui),
        }
    }
}

impl eframe::App for PerseidApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.memory.sample_if_due();
        self.poll_project();
        self.drain_events(&ctx);
        self.comparer.handle_input(&ctx);
        self.keyboard(&ctx);
        self.rebuild_visible();

        egui::Panel::top("perseid.titlebar")
            .frame(
                egui::Frame::NONE
                    .fill(theme::SURFACE)
                    .inner_margin(egui::Margin {
                        left: 10,
                        right: 0,
                        top: 3,
                        bottom: 3,
                    }),
            )
            .show(ui, |ui| self.title_bar(ui));

        egui::Panel::top("perseid.toolbar")
            .frame(
                egui::Frame::NONE
                    .fill(theme::SURFACE)
                    .inner_margin(egui::Margin::symmetric(10, 5)),
            )
            .show(ui, |ui| self.main_tabs(ui));

        egui::Panel::bottom("perseid.statusbar")
            .frame(
                egui::Frame::NONE
                    .fill(theme::SURFACE_ALT)
                    .inner_margin(egui::Margin::symmetric(10, 3)),
            )
            .show(ui, |ui| self.status_bar(ui));

        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(theme::CANVAS))
            .show(ui, |ui| self.main_content(ui));

        self.memory_window(&ctx);
        self.about_window(&ctx);
        self.tls_setup_window(&ctx);
        self.project_windows(&ctx);
        self.comparer_windows(&ctx);
        self.search_window(&ctx);
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

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.shutdown_project();
    }
}

/// Settings sent to the proxy: enabled rules that compile.
///
/// A broken regex stays visible in the editor but never becomes part of the
/// live policy: every rule that fails validation is dropped and the first
/// validation error is reported.
fn sanitized_settings(mut settings: ProxySettings) -> (ProxySettings, Option<String>) {
    let error = settings.validate().err().map(|err| err.to_string());
    if error.is_some() {
        settings
            .match_replace
            .retain(|rule| rule.validate().is_ok());
    }
    (settings, error)
}

/// Row index after moving the selection by `delta` within `len` rows.
///
/// Without a selection the first row becomes selected, regardless of the
/// direction; otherwise the target index clamps to the ends and never wraps.
/// `None` means there are no rows.
fn moved_row_index(len: usize, current: Option<usize>, delta: isize) -> Option<usize> {
    if len == 0 {
        return None;
    }
    let next = match current {
        Some(position) => position as isize + delta,
        None => 0,
    };
    Some(next.clamp(0, len as isize - 1) as usize)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proxy::{RuleDirection, RulePart};

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
                MainTab::Decoder,
                MainTab::Comparer
            ]
        );
    }

    #[test]
    fn sanitized_settings_keep_only_rules_that_compile() {
        let rule = |pattern: &str, regex: bool| MatchReplaceRule {
            enabled: true,
            direction: RuleDirection::Request,
            part: RulePart::All,
            pattern: pattern.to_string(),
            replacement: String::new(),
            regex,
        };

        let settings = ProxySettings {
            match_replace: vec![rule("(", true), rule("ok", false)],
            ..ProxySettings::default()
        };
        let (sanitized, error) = sanitized_settings(settings);
        assert!(error.is_some());
        assert_eq!(sanitized.match_replace.len(), 1);
        assert_eq!(sanitized.match_replace[0].pattern, "ok");

        let (clean, error) = sanitized_settings(ProxySettings {
            match_replace: vec![rule("ok", false)],
            ..ProxySettings::default()
        });
        assert!(error.is_none());
        assert_eq!(clean.match_replace.len(), 1);
    }

    #[test]
    fn move_selection_starts_at_the_first_row_and_clamps() {
        assert_eq!(moved_row_index(0, None, 1), None);
        assert_eq!(moved_row_index(5, None, 1), Some(0));
        assert_eq!(moved_row_index(5, None, -1), Some(0));
        assert_eq!(moved_row_index(5, Some(2), 10), Some(4));
        assert_eq!(moved_row_index(5, Some(2), -10), Some(0));
        assert_eq!(moved_row_index(5, Some(0), -1), Some(0));
        assert_eq!(moved_row_index(5, Some(4), 1), Some(4));
    }

    #[test]
    fn macos_green_control_enters_fullscreen_and_option_click_zooms() {
        assert_eq!(
            window_control_action(true, false, false, false),
            WindowControlAction::Fullscreen(true)
        );
        assert_eq!(
            window_control_action(true, false, false, true),
            WindowControlAction::Maximized(true)
        );
        assert_eq!(
            window_control_action(true, false, true, true),
            WindowControlAction::Maximized(false)
        );
    }

    #[test]
    fn macos_green_control_exits_fullscreen() {
        assert_eq!(
            window_control_action(true, true, false, false),
            WindowControlAction::Fullscreen(false)
        );
        assert_eq!(
            window_control_action(true, true, false, true),
            WindowControlAction::Fullscreen(false)
        );
    }

    #[test]
    fn other_platforms_keep_maximize_restore_behavior() {
        assert_eq!(
            window_control_action(false, false, false, false),
            WindowControlAction::Maximized(true)
        );
        assert_eq!(
            window_control_action(false, true, true, true),
            WindowControlAction::Maximized(false)
        );
    }
}
