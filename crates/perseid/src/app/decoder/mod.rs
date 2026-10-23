//! Decoder: stacked panels for encoding, decoding, hashing, and smart decode.
//!
//! The layout follows Burp's Decoder. Every transformation produces a new
//! editable panel, and the operation that produced it is shown as a colored
//! chip. Text selections are transformed in place within a new panel; applying
//! an operation to an earlier panel replaces the downstream chain.
//!
//! The byte-level transforms live in [`crate::decoder`]. This module owns the
//! panel state, selection handling, coloring, and the egui layout.

mod colors;
mod state;
mod view;

use super::PerseidApp;
use crate::project::DecoderPanelWorkspace;

pub(super) use state::DecoderState;

use state::Panel;

impl PerseidApp {
    pub(super) fn decoder_view(&mut self, ui: &mut egui::Ui) {
        self.decoder.ui(ui);
    }

    pub(super) fn decoder_workspace(&self) -> Vec<DecoderPanelWorkspace> {
        self.decoder
            .panels
            .iter()
            .map(|panel| DecoderPanelWorkspace {
                id: panel.id,
                data: panel.data.clone(),
            })
            .collect()
    }

    pub(super) fn apply_decoder_workspace(&mut self, panels: &[DecoderPanelWorkspace]) {
        if panels.is_empty() {
            self.decoder.reset();
            return;
        }
        self.decoder.panels.clear();
        let mut next_id = 1;
        for workspace in panels {
            let id = workspace.id.max(next_id);
            next_id = id.saturating_add(1).max(next_id.saturating_add(1));
            self.decoder
                .panels
                .push(Panel::new(id, workspace.data.clone(), Vec::new(), None));
        }
        self.decoder.next_id = next_id;
        self.decoder.scroll_to_end = false;
    }
}
