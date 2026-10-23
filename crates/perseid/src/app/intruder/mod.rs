//! Editable Intruder: request positions, payload sets, attacks, and results.
//!
//! The workspace mirrors Burp's Intruder layout. The attack type, target, and
//! position controls sit in the top bars; the request template is on the left
//! and the payload list on the right. Starting an attack opens a resizable
//! result table with the selected response beside it.
//!
//! Positions are written as `$$value$$` in the raw request. The markers are
//! part of the editable text, so copying, pasting, and normal editing keep
//! working. Intruder reuses the direct Repeater transport and sends traffic
//! straight to the target instead of through the proxy listener. When scope
//! enforcement is on, the workspace checks the first effective target before
//! starting and the engine checks every generated request before sending.

mod controller;
mod state;
mod view;

pub(super) use state::IntruderState;
