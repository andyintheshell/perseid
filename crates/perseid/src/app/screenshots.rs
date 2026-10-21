//! Deterministic documentation screenshots.
//!
//! This module is compiled only with the `screenshots` feature and is driven
//! by `scripts/screenshots.sh`. It seeds a representative HTTP history, opens
//! the two documented views, asks egui for a framebuffer capture, saves the
//! PNGs, and exits. No window manager, mouse coordinates, or screenshot utility
//! is involved.
//!
//! The history is intentionally synthetic: screenshots should be reproducible
//! in CI and must not depend on a demo server or network access. The memory
//! meter's current/peak labels remain real allocator measurements; only the
//! rolling graph is seeded so the run does not have to wait three minutes.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use crate::proxy::{
    CapturedBody, CertificateAuthority, ExchangeEnd, ExchangeResult, ProxyEvent, RequestInfo,
    ResponseHead,
};

use super::*;
use crate::history::History;

const DESIGN_WIDTH: f32 = 1280.0;
const DESIGN_HEIGHT: f32 = 820.0;
const MAIN_FILE: &str = "perseid-main.png";
const MEMORY_FILE: &str = "perseid-memory.png";
const MAIN_WIDTH: u32 = 1620;
const MEMORY_WIDTH: u32 = 970;
const CROP_MARGIN_POINTS: f32 = 0.0;
const TIMEOUT: Duration = Duration::from_secs(60);

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
    /// Capture requested; waiting for the paint callback.
    Main,
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
                self.request_main(ctx);
                self.phase = Phase::Main;
            }
            Phase::Main => {
                if let Some(result) = self.take_result() {
                    if let Err(err) = result {
                        self.fail(&err);
                    }
                    crate::memory::reset_peak_for_screenshot();
                    app.memory.seed_presentation_samples();
                    app.memory_open = true;
                    self.phase = Phase::MemoryWait(3);
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

    fn request_main(&self, ctx: &egui::Context) {
        let result = Arc::clone(&self.result);
        let out_dir = self.out_dir.clone();
        ctx.request_screenshot(move |image| {
            let outcome = save_main(&image, &out_dir);
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

    /// Seed the documented view: the demo history, the selected API exchange,
    /// and an in-memory CA so the TLS controls read as enabled.
    pub(super) fn prepare_screenshot_scene(&mut self) -> Result<(), String> {
        self.main_tab = MainTab::Proxy;
        self.proxy_section = ProxySection::HttpHistory;
        self.request_view = MessageView::Pretty;
        self.response_view = MessageView::Pretty;
        self.filter.clear();
        self.visible_key = None;
        self.clear_history();
        self.seed_screenshot_history();
        self.selected = Some(2);
        self.scroll_to_selected = false;

        let ca = CertificateAuthority::generate()
            .map_err(|err| format!("could not create the demo certificate authority: {err}"))?;
        self.tls_ca = Some(Arc::new(ca));
        self.tls_intercept = true;
        self.tls_ca_error = None;
        Ok(())
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

fn save_main(image: &egui::ColorImage, out_dir: &Path) -> Result<(), String> {
    let rgba = to_rgba(image)?;
    let resized = resize_to_width(&rgba, MAIN_WIDTH);
    let path = out_dir.join(MAIN_FILE);
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
