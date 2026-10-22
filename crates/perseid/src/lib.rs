//! Perseid: a lightweight web security workbench for inspecting HTTP traffic.
//!
//! The desktop application is started with [`run`]. The interception proxy is
//! embedded in this crate and exposed as a library through [`proxy`].

mod app;
mod branding;
mod ca_store;
mod decode;
mod decoder;
mod highlight;
mod history;
mod memory;
mod pretty;
mod site_map;
mod theme;
mod view;

pub mod proxy;

pub use memory::TrackingAllocator;

use std::net::SocketAddr;

use clap::Parser;

use crate::history::default_history_bytes;

/// A lightweight web security workbench for inspecting, replaying, and
/// testing HTTP traffic.
#[derive(Debug, Parser)]
#[command(name = "perseid", version, about, long_about = None)]
pub(crate) struct Args {
    /// Address for the interception proxy to listen on (keep it on loopback).
    #[arg(long, value_name = "ADDR", default_value = crate::proxy::DEFAULT_LISTEN)]
    pub listen: SocketAddr,

    /// Maximum bytes of each request/response body to keep in memory.
    #[arg(long, value_name = "BYTES", default_value_t = crate::proxy::DEFAULT_CAPTURE_LIMIT)]
    pub capture_limit: usize,

    /// Maximum body bytes buffered for interception and match/replace editing.
    #[arg(long, value_name = "BYTES", default_value_t = crate::proxy::DEFAULT_INTERCEPT_LIMIT)]
    pub intercept_limit: usize,

    /// Maximum number of exchanges kept in the in-memory history.
    #[arg(long, value_name = "N", default_value_t = 2000)]
    pub history_limit: usize,

    /// Maximum total request/response body bytes retained by history
    /// (default: half of physical memory).
    #[arg(long, value_name = "BYTES", default_value_t = default_history_bytes())]
    pub history_bytes: usize,

    /// Start without the proxy (UI only).
    #[arg(long)]
    pub no_proxy: bool,

    /// Directory holding the TLS interception certificate authority
    /// (`perseid-ca.pem` and its private key). Defaults to
    /// `$PERSEID_CA_DIR`, then the platform data directory.
    #[arg(long, value_name = "DIR")]
    pub ca_dir: Option<std::path::PathBuf>,

    /// Start without decrypting HTTPS. The local CA is still created and
    /// available for installation.
    #[arg(long)]
    pub no_tls_intercept: bool,

    /// Deprecated: HTTPS interception is enabled by default.
    #[arg(long, hide = true)]
    pub tls_intercept: bool,

    /// Host that is never TLS-intercepted; repeat the flag for more hosts.
    #[arg(long, value_name = "HOST")]
    pub tls_passthrough: Vec<String>,

    /// Scale for the whole UI, e.g. 2 on a HiDPI display. By default the
    /// system scale is used; on desktops that report no scaling to the
    /// windowing system (common on X11), GDK_SCALE / QT_SCALE_FACTOR are
    /// honored instead.
    #[arg(long, value_name = "FACTOR")]
    pub ui_scale: Option<f32>,

    /// Render the documentation screenshots into DIR, then exit.
    ///
    /// Only available when built with the `screenshots` feature; see
    /// `scripts/screenshots.sh`.
    #[cfg(feature = "screenshots")]
    #[arg(long, value_name = "DIR", hide = true)]
    pub screenshot_dir: Option<std::path::PathBuf>,
}

/// Start the Perseid desktop application.
pub fn run() -> eframe::Result {
    tune_allocator();
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let args = Args::parse();

    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Perseid")
        .with_inner_size([1280.0, 820.0])
        .with_min_inner_size([720.0, 480.0])
        // The menu shares the title-bar row with the window controls, Burp
        // style. The app draws the drag area and resize border itself.
        .with_decorations(false);
    if let Some(icon) = branding::window_icon() {
        viewport = viewport.with_icon(icon);
    } else {
        log::warn!("could not decode the window icon");
    }

    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    eframe::run_native(
        "Perseid",
        options,
        Box::new(move |cc| Ok(Box::new(app::PerseidApp::new(cc, args)))),
    )
}

/// Keep glibc from creating a separate malloc arena for every proxy/UI thread.
///
/// Perseid is a small process with a few threads. The default arena policy
/// was worth about 8 MiB of idle RSS on the reference VM, while two arenas
/// leave enough room for concurrent allocation.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn tune_allocator() {
    // SAFETY: this only changes glibc malloc configuration, before the UI and
    // proxy threads are started.
    unsafe {
        libc::mallopt(libc::M_ARENA_MAX, 2);
    }
}

#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
fn tune_allocator() {}
