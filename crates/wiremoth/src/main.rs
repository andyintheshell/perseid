//! Wiremoth: a lightweight, keyboard-first workbench for inspecting HTTP traffic.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod history;
mod view;

use std::net::SocketAddr;

use clap::Parser;

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

    /// Maximum number of exchanges kept in the in-memory history.
    #[arg(long, value_name = "N", default_value_t = 2000)]
    pub history_limit: usize,

    /// Start without the proxy (UI only).
    #[arg(long)]
    pub no_proxy: bool,
}

fn main() -> eframe::Result {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let args = Args::parse();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Wiremoth")
            .with_inner_size([1280.0, 820.0])
            .with_min_inner_size([720.0, 480.0]),
        ..Default::default()
    };

    eframe::run_native(
        "Wiremoth",
        options,
        Box::new(move |cc| Ok(Box::new(app::WiremothApp::new(cc, args)))),
    )
}
