//! Sequencer: token sampling, live capture, and randomness analysis.
//!
//! The tab follows Burp's three-page layout: **Live capture** watches new proxy
//! responses for token matches, **Manual load** accepts a pasted or loaded
//! sample, and **Sequencer settings** controls the capture expression and the
//! statistical tests. The analysis itself lives in [`crate::sequencer`]; this
//! module owns the sample, the floating result window, and the history
//! integration.
//!
//! [`state`] holds the token collection and capture state, while [`view`] draws
//! the tab, token tables, chart, and result tabs.

mod state;
mod view;

pub(super) use state::SequencerState;

use super::*;

impl PerseidApp {
    pub(super) fn sequencer_view(&mut self, ui: &mut egui::Ui) {
        self.sequencer.ui(ui, &self.history);
    }

    pub(super) fn sequencer_windows(&mut self, ctx: &egui::Context) {
        self.sequencer.result_window(ctx);
        self.sequencer.load_window(ctx);
    }

    /// Poll live capture and consume a clipboard paste requested by the UI.
    pub(super) fn poll_sequencer(&mut self, ctx: &egui::Context) {
        self.sequencer.handle_input(ctx);
        self.sequencer.poll_capture(&self.history);
        if self.sequencer.capture_active() {
            // A response can complete without any other visible change, so
            // keep sampling while capture is armed.
            ctx.request_repaint_after(Duration::from_millis(500));
        }
    }

    /// Add tokens from the highlighted Proxy history response.
    pub(super) fn send_selected_to_sequencer(&mut self) {
        let Some(item) = self
            .selected
            .and_then(|id| self.history.get(id))
            .filter(|item| !item.is_tunnel())
        else {
            return;
        };
        let number = item.number;
        match self.sequencer.capture_from_history(item) {
            Ok(added) => {
                self.main_tab = MainTab::Sequencer;
                self.sequencer.section = state::SequencerSection::ManualLoad;
                self.sequencer.feedback = Some(state::Feedback {
                    error: false,
                    text: format!("Added {added} token(s) from exchange #{number}."),
                });
            }
            Err(error) => {
                self.main_tab = MainTab::Sequencer;
                self.sequencer.feedback = Some(state::Feedback {
                    error: true,
                    text: error,
                });
            }
        }
    }
}
