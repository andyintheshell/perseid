//! The egui application: a Burp-style workspace over the captured traffic.
//!
//! The navigation intentionally mirrors the tools Burp users already know:
//! Target, Proxy, Intruder, Repeater, Sequencer, Decoder, and Comparer. Proxy
//! history, editable interception, the Target site map, scope enforcement,
//! match/replace rules, the direct Repeater, Intruder's payload attacks,
//! Sequencer's token sampling and randomness analysis, Decoder's
//! encode/decode/hash chain, Comparer's word/byte diffs, and the global Search
//! window are live.
//!
//! This module holds the application state and the eframe shell. The tabs,
//! shared top-level panels, and supporting pieces are split by area:
//! [`chrome`], [`comparer`], [`curl`], [`decoder`], [`events`], [`input`],
//! [`inspector`], [`intercept`], [`intruder`], [`listing`], [`project_ui`], [`proxy`],
//! [`proxy_control`], [`repeater`], [`rules`], [`search`], [`selection`],
//! [`sequencer`], [`target`], [`tls`], [`windows`], [`widgets`],
//! [`message_view`], [`util`], [`scale`], and the feature-gated
//! [`screenshots`] driver.

mod chrome;
mod comparer;
mod curl;
mod decoder;
mod events;
mod input;
mod inspector;
mod intercept;
mod intruder;
mod listing;
mod message_view;
mod project_ui;
mod proxy;
mod proxy_control;
mod repeater;
mod rules;
mod scale;
#[cfg(feature = "screenshots")]
mod screenshots;
mod search;
mod selection;
mod sequencer;
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
    CertificateAuthority, InterceptKey, Intruder, MatchReplaceRule, Proxy, ProxyEvent, Repeater,
};
use egui::{Color32, Key, RichText, Stroke};

use self::intercept::InterceptEntry;

use crate::Args;
use crate::branding;
use crate::ca_store;
use crate::history::History;
use crate::memory::MemoryMeter;
use crate::project::{MessagePaneRecord, MessageViewRecord, MessageViewsWorkspace};
use crate::site_map::{self, SiteMap};
use crate::theme;
use crate::view::{self, MessageViews};
use crate::websocket_history::WebSocketHistory;

use self::scale::{desktop_scale_override, resize_initial_window, resolve_ui_scale, stepped_scale};
use self::widgets::{main_tab_button, search_button};

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
const TAB_SEQUENCER_SHORTCUT: egui::KeyboardShortcut = egui::KeyboardShortcut::new(
    egui::Modifiers::COMMAND.plus(egui::Modifiers::SHIFT),
    Key::S,
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
    Sequencer,
    Decoder,
    Comparer,
}

impl MainTab {
    /// Burp's tool order, so muscle memory carries over.
    const ALL: [(Self, &'static str); 7] = [
        (Self::Target, "Target"),
        (Self::Proxy, "Proxy"),
        (Self::Intruder, "Intruder"),
        (Self::Repeater, "Repeater"),
        (Self::Sequencer, "Sequencer"),
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

impl From<MessagePane> for MessagePaneRecord {
    fn from(pane: MessagePane) -> Self {
        match pane {
            MessagePane::Request => Self::Request,
            MessagePane::Response => Self::Response,
        }
    }
}

impl From<MessagePaneRecord> for MessagePane {
    fn from(pane: MessagePaneRecord) -> Self {
        match pane {
            MessagePaneRecord::Request => Self::Request,
            MessagePaneRecord::Response => Self::Response,
        }
    }
}

impl From<MessageView> for MessageViewRecord {
    fn from(view: MessageView) -> Self {
        match view {
            MessageView::Pretty => Self::Pretty,
            MessageView::Raw => Self::Raw,
            MessageView::Hex => Self::Hex,
        }
    }
}

impl From<MessageViewRecord> for MessageView {
    fn from(view: MessageViewRecord) -> Self {
        match view {
            MessageViewRecord::Pretty => Self::Pretty,
            MessageViewRecord::Raw => Self::Raw,
            MessageViewRecord::Hex => Self::Hex,
        }
    }
}

impl From<&MessageViews> for MessageViewsWorkspace {
    fn from(views: &MessageViews) -> Self {
        Self {
            pretty: views.pretty.clone(),
            raw: views.raw.clone(),
            hex: views.hex.clone(),
        }
    }
}

impl From<&MessageViewsWorkspace> for MessageViews {
    fn from(views: &MessageViewsWorkspace) -> Self {
        Self {
            pretty: views.pretty.clone(),
            raw: views.raw.clone(),
            hex: views.hex.clone(),
        }
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
    sequencer: sequencer::SequencerState,
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
            sequencer: sequencer::SequencerState::new(),
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

    fn main_content(&mut self, ui: &mut egui::Ui) {
        match self.main_tab {
            MainTab::Target => self.target_tab(ui),
            MainTab::Proxy => self.proxy_tab(ui),
            MainTab::Intruder => self.intruder_view(ui),
            MainTab::Repeater => self.repeater_view(ui),
            MainTab::Sequencer => self.sequencer_view(ui),
            MainTab::Decoder => self.decoder_view(ui),
            MainTab::Comparer => self.comparer_view(ui),
        }
    }
}

#[cfg(test)]
impl PerseidApp {
    /// Minimal app state for tests that exercise tab controllers without
    /// constructing the eframe shell, proxy, or TLS material.
    pub(super) fn test_app() -> Self {
        Self {
            proxy: None,
            proxy_error: None,
            notify: Arc::new(|| {}),
            listen: SocketAddr::from(([127, 0, 0, 1], 0)),
            capture_limit: crate::proxy::DEFAULT_CAPTURE_LIMIT,
            intercept_limit: crate::proxy::DEFAULT_INTERCEPT_LIMIT,
            history: History::new(10, 1024),
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
            websocket_history: WebSocketHistory::new(10, 1024),
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
            sequencer: sequencer::SequencerState::new(),
            project: project_ui::ProjectUi::new(10, 1024),
            site_map: None,
            scope: BTreeSet::new(),
            scope_enabled: false,
            scope_input: String::new(),
            ca_dir: PathBuf::new(),
            tls_ca: None,
            tls_ca_error: None,
            tls_intercept: false,
            tls_setup_open: false,
            tls_passthrough: Vec::new(),
            tls_passthrough_input: String::new(),
            native_ui_scale: 1.0,
            base_ui_scale: 1.0,
            mark: None,
            lockup: None,
            memory: MemoryMeter::new(),
            memory_open: false,
            about_open: false,
            #[cfg(feature = "screenshots")]
            screenshots: None,
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
        self.poll_sequencer(&ctx);
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
        self.sequencer_windows(&ctx);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(mut run: impl FnMut(&mut egui::Ui)) -> egui::FullOutput {
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1400.0, 900.0),
            )),
            ..Default::default()
        };
        let mut output = ctx.run_ui(input.clone(), |ui| run(ui));
        output.textures_delta.clear();
        let mut output = ctx.run_ui(input, |ui| run(ui));
        output.textures_delta.clear();
        output
    }

    #[test]
    fn sequencer_shortcut_selects_the_tab_and_chrome_renders_its_status() {
        let mut app = PerseidApp::test_app();
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1400.0, 900.0),
            )),
            events: vec![egui::Event::Key {
                key: Key::S,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::COMMAND.plus(egui::Modifiers::SHIFT),
            }],
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |_ui| app.keyboard(&ctx));
        output.textures_delta.clear();
        assert_eq!(app.main_tab, MainTab::Sequencer);

        let output = frame(|ui| {
            app.main_tabs(ui);
            app.main_content(ui);
        });
        assert!(!output.shapes.is_empty());
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
                MainTab::Repeater,
                MainTab::Sequencer,
                MainTab::Decoder,
                MainTab::Comparer
            ]
        );
    }
}
