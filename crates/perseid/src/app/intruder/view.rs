//! Intruder workspace drawing: toolbar, positions, editor, payloads, results,
//! and the selected-result detail pane.
//!
//! The drawing code is split by area: [`toolbar`], [`positions`], [`editor`],
//! [`payloads`], [`results`], and the request-editor [`layout`] helper.

use crate::proxy::marker_ranges;

use super::super::PerseidApp;

mod editor;
mod layout;
mod payloads;
mod positions;
mod results;
mod toolbar;

#[cfg(test)]
mod tests;

const INTRUDER_TOOLBAR_ID: &str = "perseid.intruder.toolbar";
const INTRUDER_POSITIONS_ID: &str = "perseid.intruder.positions";
const INTRUDER_PAYLOADS_ID: &str = "perseid.intruder.payloads";
const INTRUDER_RESULTS_ID: &str = "perseid.intruder.results";
const INTRUDER_RESULTS_HEADER_ID: &str = "perseid.intruder.results.header";
const INTRUDER_DETAIL_ID: &str = "perseid.intruder.detail";
const INTRUDER_EDITOR_ID: &str = "perseid.intruder.request.editor";

/// Offset message-view ids so a result sequence cannot collide with a history
/// exchange that happens to have the same number.
const RESULT_MESSAGE_ID_BASE: u64 = 1_000_000_000;

impl PerseidApp {
    pub(in crate::app) fn intruder_view(&mut self, ui: &mut egui::Ui) {
        self.intruder_toolbar(ui);

        let positions = marker_ranges(&self.intruder.request)
            .map(|ranges| ranges.len())
            .unwrap_or(0);
        self.intruder.sync_payload_sets(positions);
        self.intruder_positions(ui);

        let positions = marker_ranges(&self.intruder.request)
            .map(|ranges| ranges.len())
            .unwrap_or(0);
        self.intruder.sync_payload_sets(positions);
        self.intruder_results(ui);
        self.intruder_payloads(ui);
        self.intruder_editor(ui);
        self.intruder_payload_file_window(ui.ctx());
    }
}
