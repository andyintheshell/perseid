//! Deterministic documentation screenshots.
//!
//! This module is compiled only with the `screenshots` feature and is driven
//! by `scripts/screenshots.sh`. It seeds a representative HTTP history, walks
//! every documented tab, asks egui for a framebuffer capture, saves the PNGs,
//! and exits. No window manager, mouse coordinates, or screenshot utility is
//! involved.
//!
//! The history is intentionally synthetic: screenshots should be reproducible
//! in CI and must not depend on a demo server or network access. The memory
//! meter's current/peak labels remain real allocator measurements; only the
//! rolling graph is seeded so the run does not have to wait three minutes.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use crate::proxy::{
    CapturedBody, CertificateAuthority, ExchangeEnd, ExchangeResult, InterceptItem,
    MatchReplaceRule, ProxyEvent, RequestInfo, ResponseHead, RuleDirection, RulePart,
    WebSocketConnectionInfo, WebSocketDirection, WebSocketOpcode,
};

use super::comparer::ComparerState;
use super::decoder::DecoderState;
use super::intercept::InterceptEntry;
use super::intruder::IntruderState;
use super::repeater::RepeaterTab;
use super::*;
use crate::history::History;
use crate::websocket_history::WebSocketHistory;

const DESIGN_WIDTH: f32 = 1280.0;
const DESIGN_HEIGHT: f32 = 820.0;
const MAIN_FILE: &str = "perseid-main.png";
const MEMORY_FILE: &str = "perseid-memory.png";
// The screenshots are captured at 2x (1280x820 points). Keeping the native
// framebuffer width gives GitHub and HiDPI readers a full-resolution source
// instead of an extra Lanczos downscale.
const MAIN_WIDTH: u32 = 2560;
const MEMORY_WIDTH: u32 = 970;
const CROP_MARGIN_POINTS: f32 = 0.0;
const TIMEOUT: Duration = Duration::from_secs(150);

/// Every UI tab captured for the documentation and pull-request preview.
/// `perseid-main.png` is the Proxy > HTTP history scene used by the README.
/// Keep this in sync with `scripts/check_screenshots.py` and
/// `docs/screenshots.md`, and add a label in the Screenshots workflow.
const SCENE_FILES: &[&str] = &[
    MAIN_FILE,
    "perseid-target-site-map.png",
    "perseid-target-scope.png",
    "perseid-proxy-intercept.png",
    "perseid-proxy-match-replace.png",
    "perseid-proxy-tls.png",
    "perseid-intruder.png",
    "perseid-repeater.png",
    "perseid-decoder.png",
    "perseid-comparer.png",
    "perseid-comparer-result.png",
    "perseid-proxy-websockets.png",
];

const CURL_UA: &str = "curl/8.22.0";
const DEMO_SERVER: &str = "DemoServer/1.0 Python/3.14.7";

const HOME: &[u8] = b"<!DOCTYPE html><html><head><title>Demo shop</title></head><body><h1>Demo shop</h1><p>Products and prices.</p></body></html>";
const API_JSON: &[u8] = br#"{"items":[{"id":1,"name":"Router"},{"id":2,"name":"Cable"}]}"#;
const CSS: &[u8] = b"body { font-family: sans-serif; }";
const MISSING: &[u8] = b"<!DOCTYPE html><html><body>Not found</body></html>";
const PRODUCTS: &[u8] = b"<!DOCTYPE html><html><body><h1>Products</h1></body></html>";
const NOT_IMPLEMENTED: &[u8] = b"<!DOCTYPE html><html><body>Not implemented</body></html>";
const PRODUCTS_HTML: &[u8] =
    b"<!DOCTYPE html><html><body><h1>Products</h1><ul><li>Router</li><li>Cable</li></ul></body></html>";

type ShotResult = Arc<Mutex<Option<Result<(), String>>>>;

pub(super) struct ScreenshotDriver {
    out_dir: PathBuf,
    phase: Phase,
    memory_rect: Option<egui::Rect>,
    result: ShotResult,
    started: Instant,
}

enum Phase {
    /// Wait for the zoom-driven window resize to settle, then capture.
    Warmup(u32),
    /// A scene has been prepared; let its scroll areas and tables lay out.
    SceneSettle { index: usize, frames_left: u32 },
    /// Capture requested for a scene; waiting for the paint callback.
    SceneCapture { index: usize },
    /// Memory window opened; let it lay out before capturing.
    MemoryWait(u32),
    /// Capture requested; waiting for the paint callback.
    Memory,
}

impl ScreenshotDriver {
    pub(super) fn new(out_dir: PathBuf) -> Result<Self, String> {
        std::fs::create_dir_all(&out_dir)
            .map_err(|err| format!("could not create {}: {err}", out_dir.display()))?;
        Ok(Self {
            out_dir,
            phase: Phase::Warmup(2),
            memory_rect: None,
            result: Arc::new(Mutex::new(None)),
            started: Instant::now(),
        })
    }

    pub(super) fn record_memory_rect(&mut self, rect: egui::Rect) {
        self.memory_rect = Some(rect);
    }

    /// Advance the capture state machine once per rendered frame.
    pub(super) fn drive(&mut self, app: &mut PerseidApp, ctx: &egui::Context) {
        if self.started.elapsed() > TIMEOUT {
            self.fail("timed out while rendering the screenshots");
        }

        let settled = {
            let size = ctx.content_rect().size();
            (size.x - DESIGN_WIDTH).abs() < 2.0 && (size.y - DESIGN_HEIGHT).abs() < 2.0
        };

        match self.phase {
            Phase::Warmup(frames_left) => {
                if !settled {
                    // The window starts at the native scale and is resized for
                    // the requested zoom; do not capture the intermediate size.
                    ctx.request_repaint();
                    return;
                }
                if frames_left > 0 {
                    self.phase = Phase::Warmup(frames_left - 1);
                    ctx.request_repaint();
                    return;
                }
                if let Err(err) = app.prepare_screenshot_base() {
                    self.fail(&err);
                }
                if let Err(err) = app.prepare_screenshot_scene(0) {
                    self.fail(&err);
                }
                self.phase = Phase::SceneSettle {
                    index: 0,
                    frames_left: 3,
                };
                ctx.request_repaint();
            }
            Phase::SceneSettle { index, frames_left } => {
                if frames_left > 0 {
                    self.phase = Phase::SceneSettle {
                        index,
                        frames_left: frames_left - 1,
                    };
                    ctx.request_repaint();
                    return;
                }
                self.request_scene(ctx, index);
                self.phase = Phase::SceneCapture { index };
            }
            Phase::SceneCapture { index } => {
                if let Some(result) = self.take_result() {
                    if let Err(err) = result {
                        self.fail(&err);
                    }
                    if index + 1 < SCENE_FILES.len() {
                        if let Err(err) = app.prepare_screenshot_scene(index + 1) {
                            self.fail(&err);
                        }
                        self.phase = Phase::SceneSettle {
                            index: index + 1,
                            frames_left: 3,
                        };
                    } else {
                        // The result window is independent of the tab, so
                        // reset the scene before capturing the memory window.
                        if let Err(err) = app.prepare_screenshot_scene(0) {
                            self.fail(&err);
                        }
                        crate::memory::reset_peak_for_screenshot();
                        app.memory.seed_presentation_samples();
                        app.memory_open = true;
                        self.phase = Phase::MemoryWait(3);
                    }
                }
                ctx.request_repaint();
            }
            Phase::MemoryWait(frames_left) => {
                if frames_left > 0 {
                    self.phase = Phase::MemoryWait(frames_left - 1);
                    ctx.request_repaint();
                    return;
                }
                let Some(rect) = self.memory_rect else {
                    self.fail("the memory window was not laid out");
                };
                self.request_memory(ctx, rect);
                self.phase = Phase::Memory;
            }
            Phase::Memory => {
                if let Some(result) = self.take_result() {
                    if let Err(err) = result {
                        self.fail(&err);
                    }
                    log::info!("screenshots written to {}", self.out_dir.display());
                    std::process::exit(0);
                }
                ctx.request_repaint();
            }
        }
    }

    fn request_scene(&self, ctx: &egui::Context, index: usize) {
        let Some(file) = SCENE_FILES.get(index).copied() else {
            self.fail("screenshot scene index is out of range");
        };
        let result = Arc::clone(&self.result);
        let out_dir = self.out_dir.clone();
        ctx.request_screenshot(move |image| {
            let outcome = save_scene(&image, file, &out_dir);
            *result.lock().expect("screenshot result mutex") = Some(outcome);
        });
    }

    fn request_memory(&self, ctx: &egui::Context, rect: egui::Rect) {
        let result = Arc::clone(&self.result);
        let out_dir = self.out_dir.clone();
        let screen = ctx.content_rect();
        ctx.request_screenshot(move |image| {
            let outcome = save_memory(&image, screen, rect, &out_dir);
            *result.lock().expect("screenshot result mutex") = Some(outcome);
        });
    }

    fn take_result(&self) -> Option<Result<(), String>> {
        self.result.lock().expect("screenshot result mutex").take()
    }

    fn fail(&self, message: &str) -> ! {
        log::error!("screenshot mode: {message}");
        eprintln!("screenshot mode: {message}");
        std::process::exit(1);
    }
}

impl PerseidApp {
    pub(super) fn drive_screenshots(&mut self, ctx: &egui::Context) {
        let Some(mut driver) = self.screenshots.take() else {
            return;
        };
        driver.drive(self, ctx);
        self.screenshots = Some(driver);
    }

    /// Seed the demo history and a reusable in-memory CA once before the scene
    /// matrix. Per-tab state is reset by [`Self::prepare_screenshot_scene`].
    pub(super) fn prepare_screenshot_base(&mut self) -> Result<(), String> {
        self.clear_history();
        self.clear_websocket_history();
        self.seed_screenshot_history();
        self.seed_screenshot_websockets();
        // Keep documentation deterministic and avoid exposing the developer's
        // home directory in the TLS scene.
        self.ca_dir = PathBuf::from("~/.local/share/perseid");
        if self.tls_ca.is_none() {
            let ca = CertificateAuthority::generate()
                .map_err(|err| format!("could not create the demo certificate authority: {err}"))?;
            self.tls_ca = Some(Arc::new(ca));
        }
        self.tls_ca_error = None;
        Ok(())
    }

    /// Reset transient tab state and prepare one screenshot scene.
    pub(super) fn prepare_screenshot_scene(&mut self, index: usize) -> Result<(), String> {
        self.main_tab = MainTab::Proxy;
        self.proxy_section = ProxySection::HttpHistory;
        self.target_section = TargetSection::SiteMap;
        self.request_view = MessageView::Pretty;
        self.response_view = MessageView::Pretty;
        self.inspector.reset();
        self.filter.clear();
        self.visible_key = None;
        self.selected = Some(2);
        self.scroll_to_selected = false;
        self.detail = None;
        self.websocket_filter.clear();
        self.websocket_visible_key = None;
        self.websocket_selected = None;
        self.websocket_scroll_to_selected = false;
        self.websocket_detail = None;
        self.websocket_view = MessageView::Pretty;
        self.memory_open = false;
        self.about_open = false;
        self.tls_setup_open = false;
        self.intercept_enabled = false;
        self.intercept_responses = false;
        self.clear_intercept_queue();
        self.match_replace.clear();
        self.scope.clear();
        self.scope_enabled = false;
        self.scope_enforce = false;
        self.tls_intercept = true;
        self.tls_passthrough.clear();
        self.repeater_tabs.clear();
        self.repeater_active = 0;
        self.intruder = IntruderState::new();
        self.decoder = DecoderState::new();
        self.comparer = ComparerState::new();

        match index {
            // Proxy > HTTP history (the README scene).
            0 => {}
            // Target > Site map.
            1 => {
                self.main_tab = MainTab::Target;
                self.target_section = TargetSection::SiteMap;
            }
            // Target > Scope.
            2 => {
                self.main_tab = MainTab::Target;
                self.target_section = TargetSection::Scope;
                self.scope_enabled = true;
                self.scope_enforce = true;
                self.scope.insert("example.com".to_string());
                self.scope.insert("127.0.0.1:8090".to_string());
            }
            // Proxy > Intercept with a small queue of held messages.
            3 => {
                self.main_tab = MainTab::Proxy;
                self.proxy_section = ProxySection::Intercept;
                self.seed_screenshot_intercepts();
            }
            // Proxy > Match/replace.
            4 => {
                self.main_tab = MainTab::Proxy;
                self.proxy_section = ProxySection::MatchReplace;
                self.match_replace = vec![
                    MatchReplaceRule {
                        enabled: true,
                        direction: RuleDirection::Request,
                        part: RulePart::Head,
                        pattern: format!("User-Agent: {CURL_UA}"),
                        replacement: "User-Agent: Perseid/0.1".to_string(),
                        regex: false,
                    },
                    MatchReplaceRule {
                        enabled: true,
                        direction: RuleDirection::Response,
                        part: RulePart::Body,
                        pattern: "Demo shop".to_string(),
                        replacement: "Perseid shop".to_string(),
                        regex: false,
                    },
                    MatchReplaceRule {
                        enabled: false,
                        direction: RuleDirection::Request,
                        part: RulePart::All,
                        pattern: r"session=[0-9a-f]+".to_string(),
                        replacement: "session=redacted".to_string(),
                        regex: true,
                    },
                ];
            }
            // Proxy > TLS.
            5 => {
                self.main_tab = MainTab::Proxy;
                self.proxy_section = ProxySection::Tls;
                self.tls_intercept = true;
                self.tls_passthrough.push("banking.example.com".to_string());
            }
            // Intruder with a populated template, payload set, and results.
            6 => {
                self.main_tab = MainTab::Intruder;
                self.intruder.seed_screenshot();
            }
            // Repeater with two seeded history requests.
            7 => {
                self.main_tab = MainTab::Repeater;
                for (tab_id, history_id) in [(1, 2), (2, 5)] {
                    let item = self.history.get(history_id).ok_or_else(|| {
                        format!("screenshot history is missing exchange {history_id}")
                    })?;
                    self.repeater_tabs
                        .push(RepeaterTab::from_history(tab_id, item));
                }
                self.repeater_active = 0;
                if let Some(tab) = self.repeater_tabs.first_mut() {
                    tab.seed_screenshot_response();
                }
            }
            // Decoder with Base64 and SHA-256 layers.
            8 => {
                self.main_tab = MainTab::Decoder;
                self.decoder.seed_screenshot();
            }
            // Comparer with a request and response selected.
            9 => {
                self.main_tab = MainTab::Comparer;
                self.comparer.seed_screenshot();
            }
            // Comparer result window for the same pair.
            10 => {
                self.main_tab = MainTab::Comparer;
                self.comparer.seed_screenshot();
                self.comparer.open_screenshot_result();
            }
            // Proxy > WebSockets history with a JSON message selected.
            11 => {
                self.main_tab = MainTab::Proxy;
                self.proxy_section = ProxySection::WebSocketsHistory;
                self.websocket_selected = Some(23);
                self.websocket_view = MessageView::Pretty;
            }
            _ => return Err(format!("unknown screenshot scene {index}")),
        }

        Ok(())
    }

    fn seed_screenshot_intercepts(&mut self) {
        let host = "127.0.0.1:8090";
        let requests = [
            ("GET", "/admin", None, None),
            (
                "POST",
                "/api/orders",
                Some(br#"{"sku":"RB-01","quantity":2}"#.as_slice()),
                Some("application/json"),
            ),
            ("GET", "/account/settings", None, None),
        ];

        for (index, (method, path, body, content_type)) in requests.into_iter().enumerate() {
            let mut head = format!(
                "{method} {path} HTTP/1.1\r\nHost: {host}\r\nUser-Agent: {CURL_UA}\r\nAccept: */*\r\n"
            );
            if let Some(body) = body {
                if let Some(content_type) = content_type {
                    head.push_str(&format!("Content-Type: {content_type}\r\n"));
                }
                head.push_str(&format!("Content-Length: {}\r\n", body.len()));
            }
            head.push_str("\r\n");

            let request = RequestInfo {
                method: method.to_string(),
                uri: format!("http://{host}{path}"),
                scheme: "http".to_string(),
                host: host.to_string(),
                path: path.to_string(),
                version: "HTTP/1.1".to_string(),
                headers: proxy_headers(host),
            };
            let (responder, _receiver) = tokio::sync::oneshot::channel();
            let item = InterceptItem::new_request(
                900 + index as u64,
                request,
                head,
                body.map(|bytes| bytes.to_vec()),
                responder,
            );
            self.intercept_queue.push_back(InterceptEntry::new(item));
        }

        self.intercept_enabled = true;
        let keys: Vec<_> = self
            .intercept_queue
            .iter()
            .map(InterceptEntry::key)
            .collect();
        if let Some(key) = keys.get(1).copied() {
            self.intercept_selected.insert(key);
            self.intercept_active = Some(key);
        }
        self.sync_intercept_selection();
    }

    fn seed_screenshot_history(&mut self) {
        let now = SystemTime::now();
        let base = now.checked_sub(Duration::from_secs(10)).unwrap_or(now);
        let later = base + Duration::from_secs(1);
        let date = chrono::Utc::now()
            .format("%a, %d %b %Y %H:%M:%S GMT")
            .to_string();
        let host = "127.0.0.1:8090";

        demo_http(
            &mut self.history,
            1,
            base,
            "GET",
            &format!("http://{host}/"),
            host,
            "/",
            proxy_headers(host),
            Vec::new(),
            200,
            "text/html",
            HOME.to_vec(),
            2,
            &date,
        );
        demo_http(
            &mut self.history,
            2,
            base,
            "GET",
            &format!("http://{host}/api.json"),
            host,
            "/api.json",
            proxy_headers(host),
            Vec::new(),
            200,
            "application/json",
            API_JSON.to_vec(),
            3,
            &date,
        );
        demo_http(
            &mut self.history,
            3,
            base,
            "GET",
            &format!("http://{host}/assets/site.css"),
            host,
            "/assets/site.css",
            proxy_headers(host),
            Vec::new(),
            200,
            "text/css",
            CSS.to_vec(),
            1,
            &date,
        );
        demo_http(
            &mut self.history,
            4,
            base,
            "GET",
            &format!("http://{host}/missing"),
            host,
            "/missing",
            proxy_headers(host),
            Vec::new(),
            404,
            "text/html",
            MISSING.to_vec(),
            1,
            &date,
        );
        demo_http(
            &mut self.history,
            5,
            later,
            "GET",
            &format!("http://{host}/products?sort=price"),
            host,
            "/products?sort=price",
            proxy_headers(host),
            Vec::new(),
            200,
            "text/html",
            PRODUCTS.to_vec(),
            4,
            &date,
        );

        let mut post_headers = proxy_headers(host);
        post_headers.push(("content-type".to_owned(), "application/json".to_owned()));
        post_headers.push(("content-length".to_owned(), "8".to_owned()));
        demo_http(
            &mut self.history,
            6,
            later,
            "POST",
            &format!("http://{host}/api.json"),
            host,
            "/api.json",
            post_headers,
            br#"{"id":3}"#.to_vec(),
            501,
            "text/html",
            NOT_IMPLEMENTED.to_vec(),
            2,
            &date,
        );
        demo_http(
            &mut self.history,
            7,
            later,
            "GET",
            &format!("http://{host}/products.html"),
            host,
            "/products.html",
            proxy_headers(host),
            Vec::new(),
            200,
            "text/html",
            PRODUCTS_HTML.to_vec(),
            3,
            &date,
        );

        // Only the decrypted HTTP request gets a history row. The CONNECT
        // tunnel that carries it is transport plumbing, as in Burp.
        let example_headers = vec![
            ("host".to_owned(), "example.com".to_owned()),
            ("user-agent".to_owned(), CURL_UA.to_owned()),
            ("accept".to_owned(), "*/*".to_owned()),
        ];
        demo_http(
            &mut self.history,
            8,
            later,
            "GET",
            "https://example.com/",
            "example.com",
            "/",
            example_headers,
            Vec::new(),
            200,
            "text/html",
            example_body(),
            118,
            &date,
        );
    }

    fn seed_screenshot_websockets(&mut self) {
        let now = SystemTime::now();
        let base = now.checked_sub(Duration::from_secs(8)).unwrap_or(now);
        let secure = WebSocketConnectionInfo {
            id: 4,
            url: "wss://127.0.0.1:8090/socket".to_string(),
            host: "127.0.0.1:8090".to_string(),
            tls: true,
            listener_port: 8080,
            protocol: Some("chat".to_string()),
        };
        let plain = WebSocketConnectionInfo {
            id: 7,
            url: "ws://127.0.0.1:8090/events".to_string(),
            host: "127.0.0.1:8090".to_string(),
            tls: false,
            listener_port: 8080,
            protocol: None,
        };

        demo_websocket(
            &mut self.websocket_history,
            21,
            base,
            &secure,
            WebSocketDirection::ToServer,
            WebSocketOpcode::Text,
            br#"{"type":"subscribe","channel":"orders"}"#,
        );
        demo_websocket(
            &mut self.websocket_history,
            22,
            base + Duration::from_millis(120),
            &secure,
            WebSocketDirection::ToClient,
            WebSocketOpcode::Text,
            br#"{"type":"subscribed","channel":"orders","ok":true}"#,
        );
        demo_websocket(
            &mut self.websocket_history,
            23,
            base + Duration::from_secs(1),
            &secure,
            WebSocketDirection::ToClient,
            WebSocketOpcode::Text,
            br#"{"type":"order","id":1042,"customer":"demo","total":79.9,"lines":[{"sku":"RB-01","quantity":2},{"sku":"CB-09","quantity":1}]}"#,
        );
        demo_websocket(
            &mut self.websocket_history,
            24,
            base + Duration::from_secs(2),
            &plain,
            WebSocketDirection::ToServer,
            WebSocketOpcode::Text,
            br#"{"type":"ping","at":1760000000}"#,
        );
        demo_websocket(
            &mut self.websocket_history,
            25,
            base + Duration::from_secs(2),
            &plain,
            WebSocketDirection::ToClient,
            WebSocketOpcode::Binary,
            &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15],
        );
    }
}

fn demo_websocket(
    history: &mut WebSocketHistory,
    id: u64,
    timestamp: SystemTime,
    connection: &WebSocketConnectionInfo,
    direction: WebSocketDirection,
    opcode: WebSocketOpcode,
    payload: &[u8],
) {
    history.apply(ProxyEvent::WebSocketMessage {
        id,
        timestamp,
        connection: connection.clone(),
        direction,
        opcode,
        payload: CapturedBody::from_bytes(payload.to_vec()),
    });
}

fn proxy_headers(host: &str) -> Vec<(String, String)> {
    vec![
        ("host".to_owned(), host.to_owned()),
        ("user-agent".to_owned(), CURL_UA.to_owned()),
        ("accept".to_owned(), "*/*".to_owned()),
        ("proxy-connection".to_owned(), "Keep-Alive".to_owned()),
    ]
}

#[allow(clippy::too_many_arguments)]
fn demo_http(
    history: &mut History,
    id: u64,
    timestamp: SystemTime,
    method: &str,
    url: &str,
    host: &str,
    path: &str,
    request_headers: Vec<(String, String)>,
    request_body: Vec<u8>,
    status: u16,
    content_type: &str,
    response_body: Vec<u8>,
    duration_ms: u64,
    date: &str,
) {
    let scheme = if url.starts_with("https://") {
        "https"
    } else {
        "http"
    };
    history.apply(ProxyEvent::Started {
        id,
        timestamp,
        request: RequestInfo {
            method: method.to_owned(),
            uri: url.to_owned(),
            scheme: scheme.to_owned(),
            host: host.to_owned(),
            path: path.to_owned(),
            version: "HTTP/1.1".to_owned(),
            headers: request_headers,
        },
    });
    history.apply(ProxyEvent::ResponseHead {
        id,
        head: ResponseHead {
            version: "HTTP/1.0".to_owned(),
            status,
            reason: String::new(),
            headers: vec![
                ("server".to_owned(), DEMO_SERVER.to_owned()),
                ("date".to_owned(), date.to_owned()),
                ("content-type".to_owned(), content_type.to_owned()),
                ("content-length".to_owned(), response_body.len().to_string()),
            ],
        },
    });
    history.apply(ProxyEvent::Finished {
        id,
        end: ExchangeEnd {
            duration: Duration::from_millis(duration_ms),
            request_body: CapturedBody::from_bytes(request_body),
            result: ExchangeResult::Response {
                body: CapturedBody::from_bytes(response_body),
            },
        },
    });
}

fn example_body() -> Vec<u8> {
    let mut body = b"<!doctype html><html><head><title>Example Domain</title></head><body><h1>Example Domain</h1><p>This domain is for use in illustrative examples in documents.</p></body></html>".to_vec();
    body.resize(577, b' ');
    body
}

fn save_scene(image: &egui::ColorImage, file: &str, out_dir: &Path) -> Result<(), String> {
    let rgba = to_rgba(image)?;
    let resized = if rgba.width() == MAIN_WIDTH {
        rgba
    } else {
        resize_to_width(&rgba, MAIN_WIDTH)
    };
    let path = out_dir.join(file);
    write_png(&path, &resized)?;
    log::info!(
        "wrote {} ({}x{})",
        path.display(),
        resized.width(),
        resized.height()
    );
    Ok(())
}

fn save_memory(
    image: &egui::ColorImage,
    screen: egui::Rect,
    window: egui::Rect,
    out_dir: &Path,
) -> Result<(), String> {
    let rgba = to_rgba(image)?;
    if screen.width() <= 0.0 || screen.height() <= 0.0 {
        return Err("the viewport reported an empty size".to_owned());
    }
    let scale_x = rgba.width() as f32 / screen.width();
    let scale_y = rgba.height() as f32 / screen.height();
    let crop = window.expand(CROP_MARGIN_POINTS);

    let left = ((crop.left() - screen.left()) * scale_x).floor().max(0.0) as u32;
    let top = ((crop.top() - screen.top()) * scale_y).floor().max(0.0) as u32;
    let right = ((crop.right() - screen.left()) * scale_x)
        .ceil()
        .min(rgba.width() as f32) as u32;
    let bottom = ((crop.bottom() - screen.top()) * scale_y)
        .ceil()
        .min(rgba.height() as f32) as u32;
    if right <= left || bottom <= top {
        return Err(format!(
            "the memory window crop is empty ({left},{top})..({right},{bottom})"
        ));
    }

    let cropped =
        image::imageops::crop_imm(&rgba, left, top, right - left, bottom - top).to_image();
    let resized = resize_to_width(&cropped, MEMORY_WIDTH);
    let path = out_dir.join(MEMORY_FILE);
    write_png(&path, &resized)?;
    log::info!(
        "wrote {} ({}x{})",
        path.display(),
        resized.width(),
        resized.height()
    );
    Ok(())
}

fn to_rgba(image: &egui::ColorImage) -> Result<image::RgbaImage, String> {
    let [width, height] = image.size;
    if width == 0 || height == 0 {
        return Err("the renderer returned an empty screenshot".to_owned());
    }
    let mut bytes = Vec::with_capacity(width * height * 4);
    for pixel in &image.pixels {
        bytes.extend_from_slice(&pixel.to_array());
    }
    image::RgbaImage::from_raw(width as u32, height as u32, bytes)
        .ok_or_else(|| "the screenshot buffer had an unexpected length".to_owned())
}

fn resize_to_width(image: &image::RgbaImage, width: u32) -> image::RgbaImage {
    let height = (image.height() as f32 * width as f32 / image.width() as f32)
        .round()
        .max(1.0) as u32;
    image::imageops::resize(image, width, height, image::imageops::FilterType::Lanczos3)
}

fn write_png(path: &Path, image: &image::RgbaImage) -> Result<(), String> {
    image::save_buffer(
        path,
        image.as_raw(),
        image.width(),
        image.height(),
        image::ColorType::Rgba8,
    )
    .map_err(|err| format!("could not save {}: {err}", path.display()))
}
