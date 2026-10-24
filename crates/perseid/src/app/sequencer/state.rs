//! Sequencer state: token samples, capture settings, and the result window.

use std::collections::{BTreeSet, HashSet};
use std::path::PathBuf;

use egui::Context;
use regex::Regex;

use crate::history::{History, HistoryItem};
use crate::sequencer::{self, Analysis, AnalysisSettings, MAX_TOKENS, token_length};
use crate::view;

/// Default capture expression: a reasonably long opaque token. It is a
/// starting point, not a universal session-token detector; users are expected
/// to narrow it for the target.
pub(super) const DEFAULT_CAPTURE_REGEX: &str = r"[A-Za-z0-9+/=_-]{16,}";
/// Maximum size of a token file loaded through the in-app path dialog.
pub(super) const MAX_TOKEN_FILE_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum SequencerSection {
    LiveCapture,
    #[default]
    ManualLoad,
    Settings,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum CapturePart {
    #[default]
    Body,
    Headers,
    Both,
}

impl CapturePart {
    pub(super) const ALL: [Self; 3] = [Self::Body, Self::Headers, Self::Both];

    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Body => "Response body",
            Self::Headers => "Response headers",
            Self::Both => "Headers and body",
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct SequencerSettings {
    pub(super) analysis: AnalysisSettings,
    pub(super) capture_regex: String,
    pub(super) capture_part: CapturePart,
    pub(super) deduplicate: bool,
}

impl Default for SequencerSettings {
    fn default() -> Self {
        Self {
            analysis: AnalysisSettings::default(),
            capture_regex: DEFAULT_CAPTURE_REGEX.to_string(),
            capture_part: CapturePart::default(),
            deduplicate: true,
        }
    }
}

impl SequencerSettings {
    pub(super) fn regex(&self) -> Result<Regex, String> {
        if self.capture_regex.trim().is_empty() {
            return Err("enter a capture regex".to_string());
        }
        Regex::new(&self.capture_regex).map_err(|error| format!("invalid capture regex: {error}"))
    }
}

pub(super) struct Feedback {
    pub(super) error: bool,
    pub(super) text: String,
}

pub(super) struct PasteState {
    pub(super) frames_left: u8,
}

pub(super) struct LoadDialog {
    pub(super) path: String,
    pub(super) error: Option<String>,
}

#[derive(Default)]
pub(super) struct CaptureState {
    pub(super) active: bool,
    pub(super) seen: BTreeSet<u64>,
    pub(super) last_revision: u64,
    pub(super) responses_scanned: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum ResultTab {
    #[default]
    Summary,
    Character,
    Bit,
    Settings,
}

/// State for the Sequencer tab and its floating result window.
pub(in crate::app) struct SequencerState {
    pub(super) section: SequencerSection,
    pub(super) tokens: Vec<String>,
    seen_tokens: HashSet<String>,
    pub(super) feedback: Option<Feedback>,
    pub(super) paste: Option<PasteState>,
    pub(super) load: Option<LoadDialog>,
    pub(super) settings: SequencerSettings,
    pub(super) capture: CaptureState,
    pub(super) result: Option<Analysis>,
    pub(super) result_open: bool,
    pub(super) result_tab: ResultTab,
    pub(super) result_title: String,
    pub(super) analysis_count: u64,
}

impl SequencerState {
    pub(in crate::app) fn new() -> Self {
        Self {
            section: SequencerSection::default(),
            tokens: Vec::new(),
            seen_tokens: HashSet::new(),
            feedback: None,
            paste: None,
            load: None,
            settings: SequencerSettings::default(),
            capture: CaptureState::default(),
            result: None,
            result_open: false,
            result_tab: ResultTab::default(),
            result_title: String::new(),
            analysis_count: 0,
        }
    }

    pub(super) fn token_count(&self) -> usize {
        self.tokens.len()
    }

    pub(super) fn capture_active(&self) -> bool {
        self.capture.active
    }

    /// Compact status-bar text while live capture is armed.
    pub(in crate::app) fn status_summary(&self) -> Option<String> {
        self.capture
            .active
            .then(|| format!("Sequencer capturing ({} tokens)", self.tokens.len()))
    }

    pub(super) fn token_stats(&self) -> (Option<usize>, Option<usize>) {
        let shortest = self.tokens.iter().map(|token| token_length(token)).min();
        let longest = self.tokens.iter().map(|token| token_length(token)).max();
        (shortest, longest)
    }

    pub(super) fn add_tokens(&mut self, tokens: impl IntoIterator<Item = String>) -> usize {
        let mut added = 0;
        for token in tokens {
            let token = token.trim().to_string();
            if token.is_empty() || token_length(&token) > sequencer::MAX_TOKEN_CHARS {
                continue;
            }
            if self.settings.deduplicate && self.seen_tokens.contains(&token) {
                continue;
            }
            if self.tokens.len() >= MAX_TOKENS {
                self.feedback = Some(Feedback {
                    error: true,
                    text: format!("the Sequencer holds at most {MAX_TOKENS} tokens"),
                });
                break;
            }
            self.seen_tokens.insert(token.clone());
            self.tokens.push(token);
            added += 1;
        }
        added
    }

    pub(super) fn clear_tokens(&mut self) {
        self.tokens.clear();
        self.seen_tokens.clear();
        self.feedback = None;
        self.result = None;
        self.result_open = false;
    }

    pub(super) fn request_paste(&mut self, ctx: &Context) {
        self.paste = Some(PasteState { frames_left: 3 });
        ctx.memory_mut(|memory| memory.stop_text_input());
        ctx.send_viewport_cmd(egui::ViewportCommand::RequestPaste);
        ctx.request_repaint();
    }

    /// Consume a clipboard paste requested by the Paste button.
    pub(in crate::app) fn handle_input(&mut self, ctx: &Context) {
        let Some(mut pending) = self.paste.take() else {
            return;
        };
        let pasted = ctx.input(|input| {
            input.events.iter().find_map(|event| match event {
                egui::Event::Paste(text) if !text.is_empty() => Some(text.clone()),
                _ => None,
            })
        });
        if let Some(text) = pasted {
            let tokens = sequencer::parse_tokens(&text);
            let added = self.add_tokens(tokens);
            self.feedback = Some(if added > 0 {
                Feedback {
                    error: false,
                    text: format!("Pasted {added} token(s)."),
                }
            } else {
                Feedback {
                    error: true,
                    text: "No new tokens were found in the clipboard text.".to_string(),
                }
            });
            return;
        }
        pending.frames_left = pending.frames_left.saturating_sub(1);
        if pending.frames_left > 0 {
            self.paste = Some(pending);
        }
    }

    pub(super) fn start_capture(&mut self, history: &History) {
        if self.tokens.len() >= MAX_TOKENS {
            self.feedback = Some(Feedback {
                error: true,
                text: format!("the token limit ({MAX_TOKENS}) is full; clear the sample first"),
            });
            return;
        }
        if let Err(error) = self.settings.regex() {
            self.feedback = Some(Feedback {
                error: true,
                text: error,
            });
            return;
        }
        self.capture.active = true;
        // Existing completed exchanges are old sample data. An exchange that
        // is already in flight is left unseen so its completed response is
        // captured: it is still the next response to arrive.
        self.capture.seen = history
            .iter()
            .filter(|item| !item.is_in_flight())
            .map(|item| item.id)
            .collect();
        self.capture.last_revision = history.revision();
        self.capture.responses_scanned = 0;
        self.feedback = Some(Feedback {
            error: false,
            text: "Live capture started; waiting for new proxy responses.".to_string(),
        });
    }

    pub(super) fn stop_capture(&mut self) {
        self.capture.active = false;
        self.feedback = Some(Feedback {
            error: false,
            text: format!(
                "Live capture stopped after scanning {} response(s).",
                self.capture.responses_scanned
            ),
        });
    }

    /// Scan newly completed proxy responses while live capture is armed.
    pub(in crate::app) fn poll_capture(&mut self, history: &History) {
        if !self.capture.active {
            return;
        }
        if history.revision() == self.capture.last_revision {
            return;
        }
        self.capture.last_revision = history.revision();
        let regex = match self.settings.regex() {
            Ok(regex) => regex,
            Err(error) => {
                self.capture.active = false;
                self.feedback = Some(Feedback {
                    error: true,
                    text: error,
                });
                return;
            }
        };
        if history.len() == 0 {
            self.capture.seen.clear();
        }

        let mut captured = Vec::new();
        let mut scanned = 0;
        for item in history.iter() {
            if self.capture.seen.contains(&item.id) {
                continue;
            }
            if item.is_tunnel() {
                self.capture.seen.insert(item.id);
                continue;
            }
            if item.is_in_flight() || item.response.is_none() {
                // Wait for the terminal event so a response body is available.
                continue;
            }
            self.capture.seen.insert(item.id);
            scanned += 1;
            if let Some(text) = capture_text(item, self.settings.capture_part) {
                captured.extend(sequencer::extract_tokens(&text, &regex));
            }
        }
        self.capture.responses_scanned += scanned;

        if self.capture.seen.len() > history.len().saturating_mul(2) + 64 {
            let live: BTreeSet<u64> = history.iter().map(|item| item.id).collect();
            self.capture.seen.retain(|id| live.contains(id));
        }

        let added = self.add_tokens(captured);
        if self.tokens.len() >= MAX_TOKENS {
            self.capture.active = false;
            self.feedback = Some(Feedback {
                error: true,
                text: format!("the token limit ({MAX_TOKENS}) is full; live capture stopped"),
            });
        } else if scanned > 0 {
            self.feedback = Some(Feedback {
                error: false,
                text: if added > 0 {
                    format!("Captured {added} token(s) from {scanned} new response(s).")
                } else {
                    format!("Scanned {scanned} new response(s); no new tokens matched.")
                },
            });
        }
    }

    /// Extract tokens from one selected history response. Used by the history
    /// row context menu.
    pub(super) fn capture_from_history(&mut self, item: &HistoryItem) -> Result<usize, String> {
        let regex = self.settings.regex()?;
        let text = capture_text(item, self.settings.capture_part)
            .ok_or_else(|| "the selected exchange has no response to scan".to_string())?;
        let matches = sequencer::extract_tokens(&text, &regex);
        if matches.is_empty() {
            return Err("no tokens matched the capture regex in the selected response".to_string());
        }
        let added = self.add_tokens(matches);
        if added == 0 {
            return Err(
                "all matched tokens are already loaded or the token limit is full".to_string(),
            );
        }
        Ok(added)
    }

    pub(super) fn analyze_now(&mut self, source: &str) {
        if self.tokens.len() < 2 {
            self.feedback = Some(Feedback {
                error: true,
                text: "load at least two tokens before analyzing".to_string(),
            });
            return;
        }
        match sequencer::analyze(&self.tokens, self.settings.analysis, source) {
            Ok(analysis) => {
                self.analysis_count += 1;
                self.result_title = format!(
                    "Perseid Sequencer [{source} analysis #{}]",
                    self.analysis_count
                );
                self.result = Some(analysis);
                self.result_open = true;
                self.result_tab = ResultTab::Summary;
                self.feedback = Some(Feedback {
                    error: false,
                    text: format!("Analysis complete: {} token(s).", self.tokens.len()),
                });
            }
            Err(error) => {
                self.feedback = Some(Feedback {
                    error: true,
                    text: error,
                });
            }
        }
    }

    #[cfg(feature = "screenshots")]
    pub(in crate::app) fn seed_screenshot(&mut self) {
        *self = Self::new();
        self.section = SequencerSection::ManualLoad;
        let mut state = 0x243f_6a88_85a3_08d3u64;
        for _ in 0..100 {
            let mut bytes = [0u8; 12];
            for byte in &mut bytes {
                state = state
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
                *byte = (state >> 33) as u8;
            }
            // Four fixed bits make the documentation chart show a visible
            // entropy gap while leaving the sample overwhelmingly random.
            bytes[0] = (bytes[0] & 0x0f) | 0xf0;
            let token = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
            self.tokens.push(token);
        }
        self.seen_tokens = self.tokens.iter().cloned().collect();
    }

    #[cfg(feature = "screenshots")]
    pub(in crate::app) fn open_screenshot_result(&mut self) {
        // Match the reference's "analysis #3" title without opening two
        // throwaway result windows.
        self.analysis_count = 2;
        self.analyze_now("manual load");
        self.result_tab = ResultTab::Summary;
    }
}

/// Read a token file through the in-app path dialog.
pub(super) fn read_token_file(path: &str) -> Result<String, String> {
    if path.is_empty() {
        return Err("enter a file path".to_string());
    }
    let path = PathBuf::from(path);
    let metadata = std::fs::metadata(&path)
        .map_err(|error| format!("could not read {}: {error}", path.display()))?;
    if metadata.len() > MAX_TOKEN_FILE_BYTES {
        return Err(format!(
            "file is {}; token files are limited to {}",
            view::human_bytes(metadata.len()),
            view::human_bytes(MAX_TOKEN_FILE_BYTES)
        ));
    }
    let bytes = std::fs::read(&path)
        .map_err(|error| format!("could not read {}: {error}", path.display()))?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// Response text used by live capture and the history context-menu action.
pub(super) fn capture_text(item: &HistoryItem, part: CapturePart) -> Option<String> {
    item.response.as_ref()?;
    match part {
        CapturePart::Headers => Some(view::response_head(item)),
        CapturePart::Body => {
            item.response_body()?;
            let body = view::response_body_for_search(item);
            Some(String::from_utf8_lossy(body.as_ref()).into_owned())
        }
        CapturePart::Both => {
            let mut text = view::response_head(item);
            if item.response_body().is_some() {
                let body = view::response_body_for_search(item);
                text.push('\n');
                text.push_str(&String::from_utf8_lossy(body.as_ref()));
            }
            Some(text)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::{HistoryItem, Phase};
    use crate::proxy::{CapturedBody, ExchangeResult, RequestInfo, ResponseHead};
    use std::time::{Duration, SystemTime};

    fn history_item(id: u64, body: &[u8]) -> HistoryItem {
        let request = RequestInfo {
            method: "GET".to_string(),
            uri: "http://example.test/token".to_string(),
            scheme: "http".to_string(),
            host: "example.test".to_string(),
            path: "/token".to_string(),
            version: "HTTP/1.1".to_string(),
            headers: Vec::new(),
        };
        let response = ResponseHead {
            version: "HTTP/1.1".to_string(),
            status: 200,
            reason: "OK".to_string(),
            headers: vec![("content-type".to_string(), "text/plain".to_string())],
        };
        HistoryItem {
            id,
            number: id,
            timestamp: SystemTime::now(),
            request,
            request_body: CapturedBody::from_bytes(Vec::new()),
            response: Some(response),
            phase: Phase::Finished {
                duration: Duration::from_millis(1),
                result: ExchangeResult::Response {
                    body: CapturedBody::from_bytes(body.to_vec()),
                },
            },
        }
    }

    #[test]
    fn adds_tokens_deduplicates_and_reports_stats() {
        let mut state = SequencerState::new();
        assert_eq!(
            state.add_tokens(vec!["abc".to_string(), "abc".to_string(), "de".to_string()]),
            2
        );
        assert_eq!(state.token_count(), 2);
        assert_eq!(state.token_stats(), (Some(2), Some(3)));
        state.settings.deduplicate = false;
        assert_eq!(state.add_tokens(vec!["abc".to_string()]), 1);
        assert_eq!(state.token_count(), 3);
    }

    #[test]
    fn clearing_tokens_closes_the_result() {
        let mut state = SequencerState::new();
        state.add_tokens(vec!["abcd".to_string(), "ef01".to_string()]);
        state.analyze_now("manual load");
        assert!(state.result_open);
        state.clear_tokens();
        assert!(!state.result_open);
        assert!(state.result.is_none());
    }

    #[test]
    fn capture_text_respects_the_selected_part() {
        let item = history_item(1, b"session=deadbeef");
        assert_eq!(
            capture_text(&item, CapturePart::Body).unwrap(),
            "session=deadbeef"
        );
        let headers = capture_text(&item, CapturePart::Headers).unwrap();
        assert!(headers.contains("HTTP/1.1 200 OK"));
        let both = capture_text(&item, CapturePart::Both).unwrap();
        assert!(both.contains("HTTP/1.1 200 OK"));
        assert!(both.contains("session=deadbeef"));
    }

    #[test]
    fn context_menu_capture_uses_the_configured_regex() {
        let item = history_item(1, b"csrf=deadbeef&other=1234");
        let mut state = SequencerState::new();
        state.settings.capture_regex = r"csrf=([0-9a-f]+)".to_string();
        state.settings.capture_part = CapturePart::Body;
        let added = state.capture_from_history(&item).unwrap();
        assert_eq!(added, 1);
        assert_eq!(state.tokens, ["csrf=deadbeef"]);
    }

    #[test]
    fn invalid_capture_regex_is_reported() {
        let mut state = SequencerState::new();
        state.settings.capture_regex = "(".to_string();
        let error = state.settings.regex().unwrap_err();
        assert!(error.contains("invalid capture regex"));
    }

    #[test]
    fn analysis_opens_a_numbered_result_window() {
        let mut state = SequencerState::new();
        state.add_tokens(vec![
            "deadbeef".to_string(),
            "00112233".to_string(),
            "cafebabe".to_string(),
        ]);
        state.analyze_now("live capture");
        assert_eq!(state.analysis_count, 1);
        assert_eq!(
            state.result_title,
            "Perseid Sequencer [live capture analysis #1]"
        );
        assert!(state.result_open);
        assert_eq!(state.result_tab, ResultTab::Summary);
    }

    #[test]
    fn live_capture_scans_only_new_completed_responses() {
        let mut history = History::new(10, 1024 * 1024);
        let item = history_item(1, b"token=deadbeef");
        history.load(vec![item]);
        let mut state = SequencerState::new();
        state.settings.capture_regex = r"token=[0-9a-f]+".to_string();
        state.start_capture(&history);
        assert!(state.capture_active());
        // Existing responses are not part of the new capture.
        state.poll_capture(&history);
        assert!(state.tokens.is_empty());

        let mut next = history_item(2, b"token=cafebabe");
        next.number = 2;
        history.load(vec![history_item(1, b"token=deadbeef"), next]);
        state.poll_capture(&history);
        assert_eq!(state.tokens, ["token=cafebabe"]);
        assert_eq!(state.capture.responses_scanned, 1);
    }

    #[test]
    fn live_capture_includes_responses_already_in_flight() {
        let mut history = History::new(10, 1024 * 1024);
        let mut pending = history_item(1, b"");
        pending.phase = Phase::InFlight;
        pending.response = None;
        history.load(vec![pending]);

        let mut state = SequencerState::new();
        state.settings.capture_regex = r"token=[0-9a-f]+".to_string();
        state.start_capture(&history);

        let mut finished = history_item(1, b"token=deadbeef");
        finished.response = Some(ResponseHead {
            version: "HTTP/1.1".to_string(),
            status: 200,
            reason: "OK".to_string(),
            headers: Vec::new(),
        });
        history.load(vec![finished]);

        state.poll_capture(&history);
        assert_eq!(state.tokens, ["token=deadbeef"]);
    }

    fn temp_token_path(tag: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "perseid-sequencer-{tag}-{}-{nonce}.txt",
            std::process::id()
        ))
    }

    fn paste(ctx: &Context, state: &mut SequencerState, text: &str) {
        let input = egui::RawInput {
            events: vec![egui::Event::Paste(text.to_string())],
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |_ui| state.handle_input(ctx));
        output.textures_delta.clear();
    }

    #[test]
    fn add_tokens_ignores_empty_oversized_and_enforces_capacity() {
        let mut state = SequencerState::new();
        let oversized = "x".repeat(sequencer::MAX_TOKEN_CHARS + 1);
        assert_eq!(
            state.add_tokens(["".to_string(), "   ".to_string(), oversized]),
            0
        );
        assert!(state.tokens.is_empty());

        state.tokens = vec!["seed".to_string(); MAX_TOKENS];
        state.seen_tokens.insert("seed".to_string());
        assert_eq!(state.add_tokens(["new".to_string()]), 0);
        let feedback = state.feedback.as_ref().unwrap();
        assert!(feedback.error);
        assert!(feedback.text.contains("at most"));
    }

    #[test]
    fn status_summary_and_sample_stats_track_the_sample() {
        let mut state = SequencerState::new();
        assert_eq!(state.status_summary(), None);
        assert_eq!(state.token_stats(), (None, None));

        state.add_tokens(["ab".to_string(), "abcd".to_string()]);
        assert_eq!(state.token_stats(), (Some(2), Some(4)));
        assert_eq!(state.status_summary(), None);

        state.capture.active = true;
        assert_eq!(
            state.status_summary().as_deref(),
            Some("Sequencer capturing (2 tokens)")
        );
    }

    #[test]
    fn clipboard_paste_adds_tokens_and_counts_pending_frames() {
        let ctx = Context::default();
        let mut state = SequencerState::new();
        state.handle_input(&ctx);
        assert!(state.paste.is_none());
        state.request_paste(&ctx);
        assert_eq!(state.paste.as_ref().map(|paste| paste.frames_left), Some(3));

        paste(&ctx, &mut state, "one,two\nthree");
        assert_eq!(state.tokens, ["one", "two", "three"]);
        assert!(state.paste.is_none());
        let feedback = state.feedback.as_ref().unwrap();
        assert!(!feedback.error);
        assert_eq!(feedback.text, "Pasted 3 token(s).");

        state.request_paste(&ctx);
        paste(&ctx, &mut state, "one,two\nthree");
        let feedback = state.feedback.as_ref().unwrap();
        assert!(feedback.error);
        assert!(feedback.text.contains("No new tokens"));

        state.paste = Some(PasteState { frames_left: 2 });
        let mut output = ctx.run_ui(egui::RawInput::default(), |_ui| state.handle_input(&ctx));
        output.textures_delta.clear();
        assert_eq!(state.paste.as_ref().map(|paste| paste.frames_left), Some(1));
        let mut output = ctx.run_ui(egui::RawInput::default(), |_ui| state.handle_input(&ctx));
        output.textures_delta.clear();
        assert!(state.paste.is_none());

        state.request_paste(&ctx);
        let input = egui::RawInput {
            events: vec![egui::Event::Copy],
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |_ui| state.handle_input(&ctx));
        output.textures_delta.clear();
        assert_eq!(state.paste.as_ref().map(|paste| paste.frames_left), Some(2));
    }

    #[test]
    fn start_capture_rejects_bad_regex_and_a_full_sample() {
        let history = History::new(10, 1024);
        let mut state = SequencerState::new();
        state.settings.capture_regex = "   ".to_string();
        state.start_capture(&history);
        assert!(!state.capture_active());
        assert!(
            state
                .feedback
                .as_ref()
                .is_some_and(|feedback| feedback.text.contains("enter a capture regex"))
        );

        state.settings.capture_regex = "(".to_string();
        state.start_capture(&history);
        assert!(!state.capture_active());
        assert!(
            state
                .feedback
                .as_ref()
                .is_some_and(|feedback| feedback.text.contains("invalid capture regex"))
        );

        state.settings = SequencerSettings::default();
        state.tokens = vec!["seed".to_string(); MAX_TOKENS];
        state.start_capture(&history);
        assert!(!state.capture_active());
        assert!(
            state
                .feedback
                .as_ref()
                .is_some_and(|feedback| feedback.text.contains("token limit"))
        );
    }

    #[test]
    fn start_and_stop_capture_report_scan_progress() {
        let mut history = History::new(10, 1024);
        history.load(vec![history_item(1, b"old response")]);
        let mut state = SequencerState::new();

        state.start_capture(&history);
        assert!(state.capture_active());
        assert!(state.capture.seen.contains(&1));
        assert!(
            state
                .feedback
                .as_ref()
                .is_some_and(|feedback| feedback.text.contains("Live capture started"))
        );

        state.capture.responses_scanned = 4;
        state.stop_capture();
        assert!(!state.capture_active());
        let feedback = state.feedback.as_ref().unwrap();
        assert!(!feedback.error);
        assert!(feedback.text.contains("after scanning 4 response(s)"));
    }

    #[test]
    fn poll_capture_stops_when_the_expression_becomes_invalid() {
        let mut history = History::new(10, 1024);
        history.load(vec![history_item(1, b"token=deadbeef")]);
        let mut state = SequencerState::new();
        state.capture.active = true;
        state.capture.last_revision = 0;
        state.settings.capture_regex = "(".to_string();

        state.poll_capture(&history);
        assert!(!state.capture_active());
        assert!(state.tokens.is_empty());
        assert!(
            state
                .feedback
                .as_ref()
                .is_some_and(|feedback| feedback.text.contains("invalid capture regex"))
        );
    }

    #[test]
    fn poll_capture_clears_seen_ids_when_the_history_is_cleared() {
        let mut history = History::new(10, 1024);
        history.load(vec![history_item(1, b"old response")]);
        let mut state = SequencerState::new();
        state.start_capture(&history);
        assert!(!state.capture.seen.is_empty());

        history.clear();
        state.poll_capture(&history);
        assert!(state.capture.seen.is_empty());
        assert!(state.tokens.is_empty());
    }

    #[test]
    fn poll_capture_skips_tunnels_and_unfinished_responses() {
        let mut history = History::new(10, 1024);
        let mut tunnel = history_item(1, b"");
        tunnel.request.method = "CONNECT".to_string();
        tunnel.phase = Phase::Finished {
            duration: Duration::from_millis(1),
            result: ExchangeResult::Tunnel {
                client_to_server: 10,
                server_to_client: 20,
            },
        };
        let mut pending = history_item(2, b"");
        pending.phase = Phase::InFlight;
        pending.response = None;
        let mut dropped = history_item(3, b"");
        dropped.response = None;
        dropped.phase = Phase::Finished {
            duration: Duration::from_millis(1),
            result: ExchangeResult::Dropped,
        };
        history.load(vec![
            tunnel,
            pending,
            dropped,
            history_item(4, b"token=deadbeef"),
        ]);

        let mut state = SequencerState::new();
        state.capture.active = true;
        state.capture.last_revision = 0;
        state.settings.capture_regex = r"token=[0-9a-f]+".to_string();
        state.poll_capture(&history);

        assert_eq!(state.tokens, ["token=deadbeef"]);
        assert_eq!(state.capture.responses_scanned, 1);
        assert!(state.capture.seen.contains(&1));
        assert!(state.capture.seen.contains(&4));
        assert!(!state.capture.seen.contains(&2));
        assert!(!state.capture.seen.contains(&3));
        assert!(
            state
                .feedback
                .as_ref()
                .is_some_and(|feedback| feedback.text.contains("from 1 new response"))
        );
    }

    #[test]
    fn poll_capture_prunes_stale_seen_ids() {
        let mut history = History::new(10, 1024);
        history.load(vec![history_item(1, b"nothing")]);
        let mut state = SequencerState::new();
        state.capture.active = true;
        state.capture.last_revision = 0;
        state.capture.seen = (1_000..1_200).collect();

        state.poll_capture(&history);
        assert_eq!(state.capture.seen.len(), 1);
        assert!(state.capture.seen.contains(&1));
    }

    #[test]
    fn poll_capture_reports_no_new_tokens_and_stops_at_the_limit() {
        let mut history = History::new(10, 1024);
        history.load(vec![history_item(1, b"token=deadbeef")]);
        let mut state = SequencerState::new();
        state.add_tokens(["token=deadbeef".to_string()]);
        state.capture.active = true;
        state.capture.last_revision = 0;
        state.settings.capture_regex = r"token=[0-9a-f]+".to_string();
        state.poll_capture(&history);
        assert!(
            state
                .feedback
                .as_ref()
                .is_some_and(|feedback| feedback.text.contains("no new tokens matched"))
        );

        let mut history = History::new(10, 1024);
        history.load(vec![history_item(1, b"token=deadbeef token=cafebabe")]);
        let mut state = SequencerState::new();
        state.tokens = vec!["seed".to_string(); MAX_TOKENS - 1];
        state.capture.active = true;
        state.capture.last_revision = 0;
        state.settings.capture_regex = r"token=[0-9a-f]+".to_string();
        state.poll_capture(&history);
        assert_eq!(state.tokens.len(), MAX_TOKENS);
        assert!(!state.capture_active());
        assert!(
            state
                .feedback
                .as_ref()
                .is_some_and(|feedback| feedback.text.contains("live capture stopped"))
        );
    }

    #[test]
    fn capture_from_history_reports_unusable_exchanges() {
        let mut item = history_item(1, b"");
        item.response = None;
        item.phase = Phase::InFlight;
        let mut state = SequencerState::new();
        assert!(
            state
                .capture_from_history(&item)
                .unwrap_err()
                .contains("no response")
        );

        state.settings.capture_regex = r"token=[0-9a-f]+".to_string();
        let item = history_item(2, b"nothing to see");
        assert!(
            state
                .capture_from_history(&item)
                .unwrap_err()
                .contains("no tokens matched")
        );

        let item = history_item(3, b"token=deadbeef");
        state.add_tokens(["token=deadbeef".to_string()]);
        assert!(
            state
                .capture_from_history(&item)
                .unwrap_err()
                .contains("already loaded")
        );
    }

    #[test]
    fn analyze_now_reports_short_samples_and_decoding_errors() {
        let mut state = SequencerState::new();
        state.analyze_now("manual load");
        assert!(
            state
                .feedback
                .as_ref()
                .is_some_and(|feedback| feedback.text.contains("at least two tokens"))
        );

        state.add_tokens(["zz".to_string(), "yy".to_string()]);
        state.settings.analysis.format = sequencer::TokenFormat::Hex;
        state.analyze_now("manual load");
        assert!(
            state
                .feedback
                .as_ref()
                .is_some_and(|feedback| feedback.text.contains("not valid hexadecimal"))
        );
        assert!(state.result.is_none());
    }

    #[test]
    fn read_token_file_handles_empty_missing_oversized_and_lossy_inputs() {
        assert!(
            read_token_file("")
                .unwrap_err()
                .contains("enter a file path")
        );

        let missing = temp_token_path("missing");
        assert!(
            read_token_file(missing.to_str().unwrap())
                .unwrap_err()
                .contains("could not read")
        );

        let good = temp_token_path("good");
        std::fs::write(&good, b"one\n\xFFtwo").unwrap();
        let text = read_token_file(good.to_str().unwrap()).unwrap();
        assert!(text.contains("one"));
        assert!(text.contains('\u{FFFD}'));
        std::fs::remove_file(&good).unwrap();

        let large = temp_token_path("large");
        let file = std::fs::File::create(&large).unwrap();
        file.set_len(MAX_TOKEN_FILE_BYTES + 1).unwrap();
        drop(file);
        let error = read_token_file(large.to_str().unwrap()).unwrap_err();
        assert!(error.contains("token files are limited"));
        std::fs::remove_file(&large).unwrap();
    }

    #[test]
    fn capture_text_requires_a_response_and_body() {
        let mut item = history_item(1, b"");
        item.response = None;
        item.phase = Phase::InFlight;
        assert!(capture_text(&item, CapturePart::Headers).is_none());
        assert!(capture_text(&item, CapturePart::Body).is_none());
        assert!(capture_text(&item, CapturePart::Both).is_none());

        let mut dropped = history_item(2, b"body bytes");
        dropped.phase = Phase::Finished {
            duration: Duration::from_millis(1),
            result: ExchangeResult::Dropped,
        };
        assert!(capture_text(&dropped, CapturePart::Body).is_none());
        let headers = capture_text(&dropped, CapturePart::Headers).unwrap();
        assert!(headers.contains("HTTP/1.1 200 OK"));
        let both = capture_text(&dropped, CapturePart::Both).unwrap();
        assert!(both.contains("HTTP/1.1 200 OK"));
        assert!(!both.contains("body bytes"));
    }
}
