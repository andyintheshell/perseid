//! Wiremoth: a lightweight, keyboard-first workbench for inspecting HTTP traffic.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod branding;
mod ca_store;
mod decode;
mod history;
mod memory;
mod site_map;
mod theme;
mod view;

use std::net::SocketAddr;

use clap::Parser;

use crate::history::DEFAULT_HISTORY_BYTES;

/// Track live heap bytes for the status bar memory figure.
#[global_allocator]
static GLOBAL_ALLOCATOR: memory::TrackingAllocator = memory::TrackingAllocator;

/// A lightweight, keyboard-first workbench for inspecting, replaying, and
/// testing HTTP traffic.
#[derive(Debug, Parser)]
#[command(name = "wiremoth", version, about, long_about = None)]
pub struct Args {
    /// Address for the interception proxy to listen on (keep it on loopback).
    #[arg(long, value_name = "ADDR", default_value = wiremoth_proxy::DEFAULT_LISTEN)]
    pub listen: SocketAddr,

    /// Maximum bytes of each request/response body to keep in memory.
    #[arg(long, value_name = "BYTES", default_value_t = wiremoth_proxy::DEFAULT_CAPTURE_LIMIT)]
    pub capture_limit: usize,

    /// Maximum body bytes buffered for interception and match/replace editing.
    #[arg(long, value_name = "BYTES", default_value_t = wiremoth_proxy::DEFAULT_INTERCEPT_LIMIT)]
    pub intercept_limit: usize,

    /// Maximum number of exchanges kept in the in-memory history.
    #[arg(long, value_name = "N", default_value_t = 2000)]
    pub history_limit: usize,

    /// Maximum total request/response body bytes retained by history.
    #[arg(long, value_name = "BYTES", default_value_t = DEFAULT_HISTORY_BYTES)]
    pub history_bytes: usize,

    /// Start without the proxy (UI only).
    #[arg(long)]
    pub no_proxy: bool,

    /// Directory holding the TLS interception certificate authority
    /// (`wiremoth-ca.pem` and its private key). Defaults to
    /// `$WIREMOTH_CA_DIR`, then `$XDG_DATA_HOME/wiremoth`.
    #[arg(long, value_name = "DIR")]
    pub ca_dir: Option<std::path::PathBuf>,

    /// Start with HTTPS interception enabled (requires an existing CA).
    #[arg(long)]
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
}

fn main() -> eframe::Result {
    tune_allocator();
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let args = Args::parse();

    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Wiremoth")
        .with_inner_size([1280.0, 820.0])
        .with_min_inner_size([720.0, 480.0]);
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
        "Wiremoth",
        options,
        Box::new(move |cc| Ok(Box::new(app::WiremothApp::new(cc, args)))),
    )
}

/// Keep glibc from creating a separate malloc arena for every proxy/UI thread.
///
/// Wiremoth is a small process with a few threads. The default arena policy
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
