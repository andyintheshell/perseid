//! Perseid desktop binary.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

/// Track live heap bytes for the status bar memory figure.
#[global_allocator]
static GLOBAL_ALLOCATOR: perseid::TrackingAllocator = perseid::TrackingAllocator;

fn main() -> eframe::Result {
    perseid::run()
}
