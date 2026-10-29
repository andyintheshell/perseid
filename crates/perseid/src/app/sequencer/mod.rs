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
            .and_then(|id| {
                self.history
                    .get(id)
                    .or_else(|| self.site_map_ui.stored.item(id))
            })
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::{HistoryItem, Phase};
    use crate::proxy::{CapturedBody, ExchangeResult, RequestInfo, ResponseHead};
    use std::time::{Duration, SystemTime};

    fn history_item(id: u64, method: &str, body: &[u8], finished: bool) -> HistoryItem {
        HistoryItem {
            id,
            number: id,
            timestamp: SystemTime::now(),
            request: RequestInfo {
                method: method.to_string(),
                uri: "http://example.test/token".to_string(),
                scheme: "http".to_string(),
                host: "example.test".to_string(),
                path: "/token".to_string(),
                version: "HTTP/1.1".to_string(),
                headers: Vec::new(),
            },
            request_body: CapturedBody::from_bytes(Vec::new()),
            response: Some(ResponseHead {
                version: "HTTP/1.1".to_string(),
                status: 200,
                reason: "OK".to_string(),
                headers: Vec::new(),
            }),
            phase: if finished {
                Phase::Finished {
                    duration: Duration::from_millis(1),
                    result: ExchangeResult::Response {
                        body: CapturedBody::from_bytes(body.to_vec()),
                    },
                }
            } else {
                Phase::InFlight
            },
        }
    }

    fn frame(mut run: impl FnMut(&mut egui::Ui)) -> egui::FullOutput {
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1400.0, 900.0),
            )),
            ..Default::default()
        };
        let mut output = ctx.run_ui(input.clone(), |ui| run(ui));
        output.textures_delta.clear();
        let mut output = ctx.run_ui(input, |ui| run(ui));
        output.textures_delta.clear();
        output
    }

    fn rendered_text(output: &egui::FullOutput) -> String {
        fn collect(shape: &egui::Shape, text: &mut String) {
            match shape {
                egui::Shape::Text(shape) => {
                    text.push_str(shape.galley.text());
                    text.push('\n');
                }
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        collect(shape, text);
                    }
                }
                _ => {}
            }
        }
        let mut text = String::new();
        for clipped in &output.shapes {
            collect(&clipped.shape, &mut text);
        }
        text
    }

    #[test]
    fn sequencer_view_and_windows_render_from_the_app() {
        let mut app = PerseidApp::test_app();
        let output = frame(|ui| app.sequencer_view(ui));
        assert!(rendered_text(&output).contains("Manual load"));

        app.sequencer.add_tokens([
            "token=deadbeefcafebabe".to_string(),
            "token=0011223344556677".to_string(),
        ]);
        app.sequencer.analyze_now("unit test");
        app.sequencer.load = Some(state::LoadDialog {
            path: "/tmp/tokens.txt".to_string(),
            error: Some("not found".to_string()),
        });
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1400.0, 900.0),
            )),
            ..Default::default()
        };
        let mut output = ctx.run_ui(input.clone(), |_ui| app.sequencer_windows(&ctx));
        output.textures_delta.clear();
        let mut output = ctx.run_ui(input, |_ui| app.sequencer_windows(&ctx));
        output.textures_delta.clear();
        let text = rendered_text(&output);
        assert!(text.contains("Perseid Sequencer"));
        assert!(text.contains("Load tokens from file"));
    }

    #[test]
    fn poll_sequencer_consumes_pastes_and_polls_capture() {
        let mut app = PerseidApp::test_app();
        let ctx = egui::Context::default();
        app.sequencer.request_paste(&ctx);
        app.poll_sequencer(&ctx);
        assert_eq!(
            app.sequencer.paste.as_ref().map(|paste| paste.frames_left),
            Some(2)
        );

        app.history.load(vec![history_item(
            1,
            "GET",
            b"token=deadbeefcafebabe",
            true,
        )]);
        app.sequencer.settings.capture_regex = r"token=[0-9a-f]+".to_string();
        app.sequencer.start_capture(&app.history);
        assert!(app.sequencer.capture_active());
        app.poll_sequencer(&ctx);
        assert!(app.sequencer.capture_active());
        let output = frame(|ui| app.status_bar(ui));
        assert!(rendered_text(&output).contains("Sequencer capturing"));
    }

    #[test]
    fn send_selected_to_sequencer_adds_tokens_from_the_highlighted_response() {
        let mut app = PerseidApp::test_app();
        app.send_selected_to_sequencer();
        assert!(app.sequencer.feedback.is_none());
        assert_eq!(app.main_tab, MainTab::Proxy);

        app.history.load(vec![history_item(
            1,
            "GET",
            b"token=deadbeefcafebabe",
            true,
        )]);
        app.selected = Some(1);
        app.send_selected_to_sequencer();
        assert_eq!(app.main_tab, MainTab::Sequencer);
        assert_eq!(app.sequencer.section, state::SequencerSection::ManualLoad);
        assert_eq!(app.sequencer.tokens, ["token=deadbeefcafebabe"]);
        let feedback = app.sequencer.feedback.as_ref().unwrap();
        assert!(!feedback.error);
        assert_eq!(feedback.text, "Added 1 token(s) from exchange #1.");
    }

    #[test]
    fn send_selected_to_sequencer_ignores_tunnels_and_reports_unusable_responses() {
        let mut app = PerseidApp::test_app();
        app.history.load(vec![history_item(
            1,
            "CONNECT",
            b"token=deadbeefcafebabe",
            true,
        )]);
        app.selected = Some(1);
        app.send_selected_to_sequencer();
        assert!(app.sequencer.feedback.is_none());
        assert!(app.sequencer.tokens.is_empty());

        app.history.load(vec![history_item(
            2,
            "GET",
            b"token=deadbeefcafebabe",
            false,
        )]);
        app.selected = Some(2);
        app.send_selected_to_sequencer();
        assert_eq!(app.main_tab, MainTab::Sequencer);
        let feedback = app.sequencer.feedback.as_ref().unwrap();
        assert!(feedback.error);
        assert!(feedback.text.contains("no response"));

        app.history
            .load(vec![history_item(3, "GET", b"short", true)]);
        app.selected = Some(3);
        app.send_selected_to_sequencer();
        let feedback = app.sequencer.feedback.as_ref().unwrap();
        assert!(feedback.error);
        assert!(feedback.text.contains("no tokens matched"));
    }
}
