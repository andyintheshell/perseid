//! Direct HTTP/HTTPS Intruder engine: marker parsing and payload attacks.
//!
//! Intruder uses the same direct client as Repeater, so attacks work with the
//! proxy stopped and never pass through interception or rewrite policy. The
//! engine applies the optional scope list in [`IntruderPlan::scope`] to every
//! generated request; the desktop workspace fills it when scope enforcement is
//! on and also preflights the first request. A request template uses `$$`
//! around each payload position. Marker contents
//! are the base values: Sniper leaves untargeted positions at their base value,
//! while the other attack types replace every marked position.
//!
//! Work is generated lazily. Only the configured number of requests is in
//! flight, and completed results wait in a bounded channel. The desktop UI
//! drains that channel every frame and keeps its own bounded result history.
//!
//! The module is split into [`plan`], which parses templates and renders
//! requests, and [`engine`], which runs attacks on a small background runtime.

mod engine;
mod plan;

pub use engine::{Intruder, IntruderConfig, IntruderEvent, IntruderResult, IntruderStartError};
pub use plan::{
    AttackType, IntruderError, IntruderPlan, MARKER, MAX_INTRUDER_CONCURRENCY, auto_mark,
    clear_markers, marker_ranges, split_raw_request, toggle_marker, update_host_header,
};

#[cfg(test)]
mod tests;
