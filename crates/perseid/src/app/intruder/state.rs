//! Intruder workspace state: template, payload sets, attack lifecycle, and
//! retained results.
//!
//! Only the result detail pane builds message views here; everything else is
//! kept independent of egui so the attack bookkeeping can be tested on its own.

use std::io::Read;
use std::ops::Range;
use std::path::Path;
use std::time::Instant;

use crate::proxy::{
    AttackType, IntruderPlan, IntruderResult, RepeaterResult, marker_ranges, split_raw_request,
};
use crate::view::{self, MessageViews};

use super::super::file_dialog::FileDialog;
use super::super::{MessagePane, MessageView};

/// Results retained by the workspace. The engine bounds its own queue; the UI
/// keeps enough history for review without allowing a long attack to consume
/// memory without limit.
const MAX_PAYLOAD_FILE_BYTES: u64 = 16 * 1024 * 1024;
const MAX_RESULT_ROWS: usize = 1_000;
const MAX_RESULT_BYTES: usize = 64 * 1024 * 1024;

#[derive(Default)]
pub(super) struct PayloadSet {
    /// One payload per line. Blank lines are ignored so pasted lists can keep
    /// their formatting.
    pub(super) text: String,
}

pub(super) struct IntruderDetail {
    pub(super) sequence: u64,
    pub(super) request: Option<MessageViews>,
    pub(super) response: Option<MessageViews>,
    pub(super) response_error: Option<String>,
    pub(super) truncated: bool,
    pub(super) complete: bool,
}

impl IntruderDetail {
    pub(super) fn notes(&self) -> Vec<String> {
        let mut notes = Vec::new();
        if self.truncated {
            notes.push("response body truncated at the capture limit".to_string());
        }
        if !self.complete {
            notes.push("response body did not finish".to_string());
        }
        if let Some(error) = &self.response_error {
            notes.push(error.clone());
        }
        notes
    }
}

pub(super) struct PayloadFileDialog {
    pub(super) browser: FileDialog,
    pub(super) error: Option<String>,
}

pub(in crate::app) struct IntruderState {
    pub(super) scheme: String,
    pub(super) host: String,
    pub(super) request: String,
    pub(super) attack: AttackType,
    pub(super) payload_sets: Vec<PayloadSet>,
    pub(super) payload_file_dialog: Option<PayloadFileDialog>,
    pub(super) active_set: usize,
    pub(super) concurrency: usize,
    pub(super) update_host: bool,
    /// Warning about a request seeded from history, for example a truncated
    /// capture.
    pub(super) note: Option<String>,
    pub(super) error: Option<String>,
    pub(super) notice: Option<String>,
    /// The last non-empty request-editor selection, used by Add/Clear $$.
    pub(super) selection: Option<Range<usize>>,
    pub(super) cursor: usize,
    pub(super) results: Vec<IntruderResult>,
    pub(super) result_bytes: usize,
    pub(super) selected: Option<u64>,
    pub(super) detail: Option<IntruderDetail>,
    pub(super) detail_pane: MessagePane,
    pub(super) detail_view: MessageView,
    pub(super) attack_id: Option<u64>,
    pub(super) last_plan: Option<IntruderPlan>,
    pub(super) started_at: Option<Instant>,
    pub(super) total: u64,
    pub(super) received: u64,
    pub(super) running: bool,
    pub(super) stopping: bool,
    pub(super) finished_note: Option<String>,
}

impl IntruderState {
    pub(in crate::app) fn new() -> Self {
        Self {
            scheme: "https".to_string(),
            host: String::new(),
            request: "GET / HTTP/1.1\nHost: \n\n".to_string(),
            attack: AttackType::Sniper,
            payload_sets: vec![PayloadSet::default()],
            payload_file_dialog: None,
            active_set: 0,
            concurrency: 5,
            update_host: true,
            note: None,
            error: None,
            notice: None,
            selection: None,
            cursor: 0,
            results: Vec::new(),
            result_bytes: 0,
            selected: None,
            detail: None,
            detail_pane: MessagePane::Response,
            detail_view: MessageView::Pretty,
            attack_id: None,
            last_plan: None,
            started_at: None,
            total: 0,
            received: 0,
            running: false,
            stopping: false,
            finished_note: None,
        }
    }

    pub(in crate::app) fn is_running(&self) -> bool {
        self.running
    }

    pub(in crate::app) fn status_summary(&self) -> Option<String> {
        if !self.running {
            return None;
        }
        let state = if self.stopping { "stopping" } else { "running" };
        Some(format!("Intruder {}/{} {state}", self.received, self.total))
    }

    /// Replace the template from a captured exchange. The payloads are reset
    /// because they belong to the previous request. Any running attack has
    /// already been asked to stop; clear its UI state because its completion
    /// event belongs to the old template and its results are ignored.
    pub(super) fn load_template(
        &mut self,
        scheme: String,
        host: String,
        request: String,
        note: Option<String>,
    ) {
        self.scheme = scheme;
        self.host = host;
        self.request = request;
        self.payload_file_dialog = None;
        self.payload_sets.clear();
        self.payload_sets.push(PayloadSet::default());
        self.active_set = 0;
        self.note = note;
        self.error = None;
        self.notice = None;
        self.selection = None;
        self.cursor = 0;
        self.results.clear();
        self.result_bytes = 0;
        self.selected = None;
        self.detail = None;
        self.attack_id = None;
        self.last_plan = None;
        self.started_at = None;
        self.total = 0;
        self.received = 0;
        self.running = false;
        self.stopping = false;
        self.finished_note = None;
        self.detail_pane = MessagePane::Response;
        self.detail_view = MessageView::Pretty;
    }

    pub(super) fn sync_payload_sets(&mut self, positions: usize) {
        let needed = if self.attack.uses_one_set() {
            1
        } else {
            positions.max(1)
        };
        while self.payload_sets.len() < needed {
            self.payload_sets.push(PayloadSet::default());
        }
        self.active_set = if self.attack.uses_one_set() {
            0
        } else {
            self.active_set.min(needed.saturating_sub(1))
        };
    }

    fn active_set(&self) -> &PayloadSet {
        let index = if self.attack.uses_one_set() {
            0
        } else {
            self.active_set
                .min(self.payload_sets.len().saturating_sub(1))
        };
        &self.payload_sets[index]
    }

    pub(super) fn active_set_mut(&mut self) -> &mut PayloadSet {
        let index = if self.attack.uses_one_set() {
            0
        } else {
            self.active_set
                .min(self.payload_sets.len().saturating_sub(1))
        };
        &mut self.payload_sets[index]
    }

    /// Append the file only after it has been read and validated completely.
    pub(super) fn append_payload_file(&mut self, path: &Path) -> Result<usize, String> {
        let text = read_payload_file(path)?;
        let count = payload_line_count(&text) as usize;
        if count == 0 {
            return Err("The file contains no non-blank payloads.".to_string());
        }
        let set = self.active_set_mut();
        if !set.text.is_empty() && !set.text.ends_with('\n') {
            set.text.push('\n');
        }
        set.text.push_str(&text);
        Ok(count)
    }

    fn set_count(&self) -> usize {
        if self.attack.uses_one_set() {
            1
        } else {
            marker_ranges(&self.request).map_or(1, |ranges| ranges.len().max(1))
        }
    }

    pub(super) fn build_plan(&self) -> IntruderPlan {
        let set_count = self.set_count();
        let mut payload_sets = Vec::with_capacity(set_count);
        for index in 0..set_count {
            let values = self
                .payload_sets
                .get(index)
                .map_or_else(Vec::new, |set| payloads(&set.text));
            payload_sets.push(values);
        }
        IntruderPlan {
            attack: self.attack,
            scheme: self.scheme.clone(),
            host: self.host.trim().to_string(),
            template: self.request.clone(),
            payload_sets,
            concurrency: self.concurrency,
            scope: Vec::new(),
        }
    }

    /// Number of requests the current configuration will generate.
    ///
    /// This deliberately counts lines instead of building a plan so the value
    /// can be shown on every frame without cloning the payload lists.
    pub(super) fn request_count(&self) -> u64 {
        let Ok(positions) = marker_ranges(&self.request) else {
            return 0;
        };
        if positions.is_empty() {
            return 0;
        }
        let set_count = if self.attack.uses_one_set() {
            1
        } else {
            positions.len()
        };
        let mut counts = Vec::with_capacity(set_count);
        for index in 0..set_count {
            let Some(set) = self.payload_sets.get(index) else {
                return 0;
            };
            let count = payload_line_count(&set.text);
            if count == 0 {
                return 0;
            }
            counts.push(count);
        }
        match self.attack {
            AttackType::Sniper => (positions.len() as u64).saturating_mul(counts[0]),
            AttackType::BatteringRam => counts[0],
            AttackType::Pitchfork => counts.iter().copied().min().unwrap_or(0),
            AttackType::ClusterBomb => counts
                .iter()
                .try_fold(1u64, |total, count| total.checked_mul(*count))
                .unwrap_or(0),
        }
    }

    pub(super) fn payload_count(&self) -> u64 {
        payload_line_count(&self.active_set().text)
    }

    pub(super) fn begin_attack(&mut self, attack_id: u64, total: u64, plan: IntruderPlan) {
        self.attack_id = Some(attack_id);
        self.last_plan = Some(plan);
        self.results.clear();
        self.result_bytes = 0;
        self.selected = None;
        self.detail = None;
        self.total = total;
        self.received = 0;
        self.running = true;
        self.stopping = false;
        self.started_at = Some(Instant::now());
        self.error = None;
        self.notice = None;
        self.finished_note = None;
        self.detail_pane = MessagePane::Response;
        self.detail_view = MessageView::Pretty;
    }

    pub(super) fn record_result(&mut self, result: IntruderResult) {
        if self.attack_id != Some(result.attack_id) {
            return;
        }
        self.received = self.received.saturating_add(1);
        let size = result_size(&result);
        let index = self
            .results
            .binary_search_by_key(&result.sequence, |row| row.sequence)
            .unwrap_or_else(|index| index);
        self.results.insert(index, result);
        self.result_bytes = self.result_bytes.saturating_add(size);

        while self.results.len() > MAX_RESULT_ROWS
            || (self.result_bytes > MAX_RESULT_BYTES && self.results.len() > 1)
        {
            let removed = self.results.remove(0);
            self.result_bytes = self.result_bytes.saturating_sub(result_size(&removed));
            if self.selected == Some(removed.sequence) {
                self.selected = None;
                self.detail = None;
            }
        }

        if self.selected.is_none()
            && let Some(first) = self.results.first()
        {
            self.selected = Some(first.sequence);
            self.detail = None;
        }
    }

    pub(super) fn finish(&mut self, attack_id: u64, total: u64, cancelled: bool) {
        // A finished event from a stopped or replaced attack must not clear
        // the state of the attack currently running.
        if self.attack_id != Some(attack_id) {
            return;
        }
        if total > 0 {
            self.total = total;
        }
        self.finished_note = Some(if cancelled {
            format!("Attack stopped after {} request(s)", self.received)
        } else {
            format!(
                "Attack finished: {} request(s) in {}",
                self.received,
                self.started_at.map_or_else(
                    || "—".to_string(),
                    |start| view::format_duration(start.elapsed())
                )
            )
        });
        self.running = false;
        self.stopping = false;
        self.started_at = None;
    }

    pub(super) fn fail(&mut self, message: impl Into<String>) {
        self.running = false;
        self.stopping = false;
        self.started_at = None;
        self.error = Some(message.into());
    }

    pub(super) fn clear_results(&mut self) {
        if self.running {
            return;
        }
        self.results.clear();
        self.result_bytes = 0;
        self.selected = None;
        self.detail = None;
        self.attack_id = None;
        self.last_plan = None;
        self.started_at = None;
        self.total = 0;
        self.received = 0;
        self.finished_note = None;
    }

    pub(super) fn ensure_detail(&mut self) {
        let Some(sequence) = self.selected else {
            self.detail = None;
            return;
        };
        if self
            .detail
            .as_ref()
            .is_some_and(|detail| detail.sequence == sequence)
        {
            return;
        }
        let Some(result) = self
            .results
            .iter()
            .find(|result| result.sequence == sequence)
        else {
            self.detail = None;
            return;
        };

        let request = self.last_plan.as_ref().and_then(|plan| {
            let rendered = plan.render_sequence(sequence).ok()?;
            let (head, body) = split_raw_request(&rendered.raw);
            Some(view::editor_views(head, body.as_bytes()))
        });
        let (response, response_error, truncated, complete) = match &result.result {
            RepeaterResult::Response {
                version,
                status,
                reason,
                headers,
                body,
                body_error,
            } => (
                Some(view::response_views_from_parts(
                    version, *status, reason, headers, body,
                )),
                body_error.clone(),
                body.is_truncated(),
                body.is_complete(),
            ),
            RepeaterResult::Failed { error } => (None, Some(error.clone()), false, true),
        };
        self.detail = Some(IntruderDetail {
            sequence,
            request,
            response,
            response_error,
            truncated,
            complete,
        });
    }

    pub(super) fn target_range(&self) -> Range<usize> {
        self.selection.clone().unwrap_or(self.cursor..self.cursor)
    }

    #[cfg(feature = "screenshots")]
    pub(in crate::app) fn seed_screenshot(&mut self) {
        self.scheme = "https".to_string();
        self.host = "shop.example.com".to_string();
        self.request = "GET /product?id=$$1$$ HTTP/1.1\r\n\
Host: shop.example.com\r\n\
User-Agent: Mozilla/5.0 (X11; Linux x86_64; rv:153.0) Firefox/153.0\r\n\
Accept: text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8\r\n\
Accept-Language: en-US,en;q=0.5\r\n\
Accept-Encoding: gzip, deflate, br\r\n\
Connection: keep-alive\r\n\
\r\n"
            .to_string();
        self.attack = AttackType::Sniper;
        self.concurrency = 5;
        self.update_host = true;
        self.payload_sets = vec![PayloadSet {
            text: "1\n2\n3\n4\n5".to_string(),
        }];
        self.active_set = 0;
        self.note = None;
        self.error = None;
        self.notice = None;
        self.selection = None;
        self.cursor = 0;
        self.detail_pane = MessagePane::Response;
        self.detail_view = MessageView::Pretty;

        let plan = self.build_plan();
        self.last_plan = Some(plan.clone());
        self.attack_id = Some(1);
        self.total = 5;
        self.received = 5;
        self.running = false;
        self.stopping = false;
        self.started_at = None;
        self.finished_note = Some("Attack finished: 5 request(s) in 1.2 s".to_string());

        self.results.clear();
        self.result_bytes = 0;
        let samples = [
            (
                200,
                "OK",
                Some("application/json"),
                br#"{"id":1,"name":"Router","in_stock":true}"#.as_slice(),
                "19 ms",
            ),
            (
                200,
                "OK",
                Some("application/json"),
                br#"{"id":2,"name":"Cable","in_stock":true}"#.as_slice(),
                "22 ms",
            ),
            (302, "Found", None, b"".as_slice(), "11 ms"),
            (
                404,
                "Not Found",
                Some("text/html"),
                b"<!DOCTYPE html><html><body>Not found</body></html>".as_slice(),
                "9 ms",
            ),
            (
                500,
                "Internal Server Error",
                Some("text/plain"),
                b"upstream rejected the request".as_slice(),
                "16 ms",
            ),
        ];
        for (index, (status, reason, content_type, body, duration)) in
            samples.into_iter().enumerate()
        {
            let sequence = index as u64 + 1;
            let body = crate::proxy::CapturedBody::from_bytes(body.to_vec());
            let mut headers = vec![
                ("Server".to_string(), "DemoServer/1.0".to_string()),
                ("Content-Length".to_string(), body.total_len().to_string()),
            ];
            if let Some(content_type) = content_type {
                headers.push(("Content-Type".to_string(), content_type.to_string()));
            }
            if status == 302 {
                headers.push(("Location".to_string(), "/product?id=1".to_string()));
            }
            let millis = duration.trim_end_matches(" ms").parse::<u64>().unwrap_or(1);
            self.results.push(IntruderResult {
                attack_id: 1,
                sequence,
                payloads: vec![sequence.to_string()],
                duration: std::time::Duration::from_millis(millis),
                result: RepeaterResult::Response {
                    version: "HTTP/1.1".to_string(),
                    status,
                    reason: reason.to_string(),
                    headers,
                    body,
                    body_error: None,
                },
            });
        }
        self.selected = Some(2);
        self.detail = None;
        self.ensure_detail();
    }
}

fn read_payload_file(path: &Path) -> Result<String, String> {
    let metadata = std::fs::metadata(path)
        .map_err(|error| format!("Could not inspect {}: {error}", path.display()))?;
    if !metadata.is_file() {
        return Err("Choose a regular file.".to_string());
    }
    if metadata.len() > MAX_PAYLOAD_FILE_BYTES {
        return Err("Payload files are limited to 16 MiB.".to_string());
    }
    let file = std::fs::File::open(path)
        .map_err(|error| format!("Could not open {}: {error}", path.display()))?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("Could not inspect {}: {error}", path.display()))?;
    if !metadata.is_file() {
        return Err("Choose a regular file.".to_string());
    }
    if metadata.len() > MAX_PAYLOAD_FILE_BYTES {
        return Err("Payload files are limited to 16 MiB.".to_string());
    }
    let mut bytes = Vec::new();
    file.take(MAX_PAYLOAD_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("Could not read {}: {error}", path.display()))?;
    if bytes.len() as u64 > MAX_PAYLOAD_FILE_BYTES {
        return Err("Payload files are limited to 16 MiB.".to_string());
    }
    let text =
        String::from_utf8(bytes).map_err(|_| "Payload files must be UTF-8 text.".to_string())?;
    Ok(text.strip_prefix('\u{feff}').unwrap_or(&text).to_owned())
}

/// One payload per line. `str::lines` also accepts CRLF input, which makes
/// pasted Windows payload files work without preprocessing.
pub(super) fn payloads(text: &str) -> Vec<String> {
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(str::to_string)
        .collect()
}

fn payload_line_count(text: &str) -> u64 {
    text.lines().filter(|line| !line.trim().is_empty()).count() as u64
}

pub(super) fn authority_from_uri(uri: &str) -> String {
    let Some((_, rest)) = uri.split_once("://") else {
        return String::new();
    };
    rest.split(['/', '?', '#'])
        .next()
        .unwrap_or_default()
        .rsplit('@')
        .next()
        .unwrap_or_default()
        .to_string()
}

pub(super) fn result_size(result: &IntruderResult) -> usize {
    let payloads: usize = result
        .payloads
        .iter()
        .map(|payload| payload.len() + std::mem::size_of::<String>())
        .sum();
    let result = match &result.result {
        RepeaterResult::Response { headers, body, .. } => {
            body.data().len()
                + headers
                    .iter()
                    .map(|(name, value)| name.len() + value.len())
                    .sum::<usize>()
                + 512
        }
        RepeaterResult::Failed { error } => error.len() + 256,
    };
    payloads.saturating_add(result).saturating_add(128)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payload_file_appends_to_active_set_and_preserves_line_content() {
        let path = std::env::temp_dir().join(format!("perseid-payload-{}", uuid::Uuid::new_v4()));
        std::fs::write(&path, b"\xef\xbb\xbfalpha\r\n\r\n spaced \r\nomega").unwrap();
        let mut state = IntruderState::new();
        state.attack = AttackType::ClusterBomb;
        state.request = "GET /$$a$$/$$b$$ HTTP/1.1\n\n".into();
        state.sync_payload_sets(2);
        state.payload_sets[0].text = "untouched".into();
        state.payload_sets[1].text = "existing".into();
        state.active_set = 1;
        assert_eq!(state.append_payload_file(&path).unwrap(), 3);
        assert_eq!(state.payload_sets[0].text, "untouched");
        assert_eq!(
            payloads(&state.payload_sets[1].text),
            ["existing", "alpha", " spaced ", "omega"]
        );
        assert_eq!(state.payload_count(), 4);
        assert_eq!(
            state.build_plan().payload_sets[1],
            ["existing", "alpha", " spaced ", "omega"]
        );
        state.payload_sets[1].text = "existing\n".into();
        assert_eq!(state.append_payload_file(&path).unwrap(), 3);
        assert!(state.payload_sets[1].text.starts_with("existing\nalpha"));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn payload_file_errors_leave_existing_list_unchanged() {
        let root =
            std::env::temp_dir().join(format!("perseid-payload-errors-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let path = root.join("words");
        let mut state = IntruderState::new();
        state.payload_sets[0].text = "keep".into();
        assert!(
            state
                .append_payload_file(&path)
                .unwrap_err()
                .contains("Could not inspect")
        );
        assert!(
            state
                .append_payload_file(&root)
                .unwrap_err()
                .contains("regular file")
        );
        std::fs::write(&path, [0xff]).unwrap();
        assert!(
            state
                .append_payload_file(&path)
                .unwrap_err()
                .contains("UTF-8")
        );
        std::fs::write(&path, b"\n  \r\n").unwrap();
        assert!(
            state
                .append_payload_file(&path)
                .unwrap_err()
                .contains("no non-blank")
        );
        std::fs::File::create(&path)
            .unwrap()
            .set_len(MAX_PAYLOAD_FILE_BYTES + 1)
            .unwrap();
        assert!(
            state
                .append_payload_file(&path)
                .unwrap_err()
                .contains("16 MiB")
        );
        assert_eq!(state.payload_sets[0].text, "keep");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn payloads_ignore_blank_lines_and_accept_crlf() {
        assert_eq!(
            payloads("admin\r\n\r\nguest\n  \ntest"),
            vec!["admin", "guest", "test"]
        );
        assert_eq!(payload_line_count("a\nb\n\n"), 2);
    }

    #[test]
    fn result_count_matches_the_engine_for_each_attack() {
        let mut state = IntruderState::new();
        state.request = "GET /?a=$$1$$&b=$$2$$ HTTP/1.1\n\n".to_string();
        state.payload_sets = vec![
            PayloadSet {
                text: "a\nb".to_string(),
            },
            PayloadSet {
                text: "1\n2\n3".to_string(),
            },
        ];
        state.attack = AttackType::Sniper;
        assert_eq!(state.request_count(), 4);
        state.attack = AttackType::BatteringRam;
        assert_eq!(state.request_count(), 2);
        state.attack = AttackType::Pitchfork;
        assert_eq!(state.request_count(), 2);
        state.attack = AttackType::ClusterBomb;
        assert_eq!(state.request_count(), 6);
    }

    #[test]
    fn loading_a_template_clears_the_previous_attack_state() {
        let mut state = IntruderState::new();
        state.running = true;
        state.stopping = true;
        state.attack_id = Some(7);
        state.started_at = Some(Instant::now());
        state.total = 10;
        state.received = 4;

        state.load_template(
            "http".to_string(),
            "127.0.0.1:8090".to_string(),
            "GET /new HTTP/1.1\nHost: 127.0.0.1:8090\n\n".to_string(),
            None,
        );

        assert!(!state.running);
        assert!(!state.stopping);
        assert!(state.attack_id.is_none());
        assert!(state.started_at.is_none());
        assert_eq!(state.total, 0);
        assert_eq!(state.received, 0);
        assert!(state.results.is_empty());
    }

    #[test]
    fn sync_payload_sets_preserves_text_when_positions_change() {
        let mut state = IntruderState::new();
        state.attack = AttackType::Pitchfork;
        state.payload_sets[0].text = "a\nb".to_string();

        state.sync_payload_sets(2);
        assert_eq!(state.payload_sets.len(), 2);
        assert_eq!(state.payload_sets[0].text, "a\nb");
        assert_eq!(state.payload_sets[1].text, "");

        state.payload_sets[1].text = "x\ny\nz".to_string();
        state.active_set = 1;
        state.sync_payload_sets(1);
        assert_eq!(
            state.active_set, 0,
            "the active set is clamped to the new range"
        );
        assert_eq!(state.payload_sets.len(), 2, "sets are never discarded");
        assert_eq!(state.payload_sets[1].text, "x\ny\nz");

        state.attack = AttackType::Sniper;
        state.sync_payload_sets(3);
        assert_eq!(
            state.payload_sets.len(),
            2,
            "one-set attacks need no extra sets"
        );
        assert_eq!(state.active_set, 0);
        assert_eq!(state.payload_sets[0].text, "a\nb");
    }

    #[test]
    fn build_plan_uses_only_the_sets_the_attack_needs() {
        let mut state = IntruderState::new();
        state.scheme = "http".to_string();
        state.host = "  example.test:8080  ".to_string();
        state.request = "GET /?a=$$1$$&b=$$2$$ HTTP/1.1\n\n".to_string();
        state.payload_sets = vec![
            PayloadSet {
                text: "a\n\nb".to_string(),
            },
            PayloadSet {
                text: "1\n2\n3".to_string(),
            },
        ];
        state.concurrency = 7;

        state.attack = AttackType::Pitchfork;
        let plan = state.build_plan();
        assert_eq!(plan.attack, AttackType::Pitchfork);
        assert_eq!(plan.scheme, "http");
        assert_eq!(plan.host, "example.test:8080");
        assert_eq!(plan.template, state.request);
        assert_eq!(plan.payload_sets, vec![vec!["a", "b"], vec!["1", "2", "3"]]);
        assert_eq!(plan.concurrency, 7);
        assert!(plan.scope.is_empty());

        state.attack = AttackType::Sniper;
        let plan = state.build_plan();
        assert_eq!(
            plan.payload_sets,
            vec![vec!["a", "b"]],
            "one-set attacks include only the first set"
        );
    }

    #[test]
    fn target_range_prefers_the_selection() {
        let mut state = IntruderState::new();
        state.cursor = 4;
        assert_eq!(state.target_range(), 4..4);

        state.selection = Some(2..6);
        assert_eq!(state.target_range(), 2..6);
    }

    #[test]
    fn stale_attack_events_are_ignored() {
        let mut state = IntruderState::new();
        let plan = state.build_plan();
        state.begin_attack(7, 10, plan);
        assert!(state.running);
        assert_eq!(state.total, 10);

        state.record_result(IntruderResult {
            attack_id: 6,
            sequence: 1,
            payloads: Vec::new(),
            duration: std::time::Duration::ZERO,
            result: RepeaterResult::Failed {
                error: "stale".to_string(),
            },
        });
        assert_eq!(state.received, 0, "results from another attack are dropped");
        assert!(state.results.is_empty());

        state.finish(6, 99, true);
        assert!(
            state.running,
            "a stale finish must not stop the current attack"
        );
        assert_eq!(state.total, 10);

        state.finish(7, 12, false);
        assert!(!state.running);
        assert_eq!(state.total, 12);
        assert!(
            state
                .finished_note
                .as_deref()
                .is_some_and(|note| { note.starts_with("Attack finished: 0 request(s) in ") })
        );

        state.begin_attack(8, 4, state.build_plan());
        state.finish(7, 1, true);
        assert!(
            state.running,
            "a replaced attack keeps running after an old finish"
        );
    }

    #[test]
    fn record_result_evicts_the_oldest_rows_at_the_limit() {
        let mut state = IntruderState::new();
        let overflow = 5u64;
        state.begin_attack(1, MAX_RESULT_ROWS as u64 + overflow, state.build_plan());

        for sequence in 0..MAX_RESULT_ROWS as u64 + overflow {
            state.record_result(IntruderResult {
                attack_id: 1,
                sequence,
                payloads: vec![sequence.to_string()],
                duration: std::time::Duration::ZERO,
                result: RepeaterResult::Failed {
                    error: "boom".to_string(),
                },
            });
        }

        assert_eq!(state.results.len(), MAX_RESULT_ROWS);
        assert_eq!(state.results.first().unwrap().sequence, overflow);
        assert_eq!(
            state.results.last().unwrap().sequence,
            MAX_RESULT_ROWS as u64 + overflow - 1
        );
        let retained_bytes: usize = state.results.iter().map(result_size).sum();
        assert_eq!(state.result_bytes, retained_bytes);
        assert_eq!(
            state.selected,
            Some(overflow),
            "the selection follows the oldest retained row"
        );
    }

    #[test]
    fn clear_results_waits_until_the_attack_stops() {
        let mut state = IntruderState::new();
        state.begin_attack(1, 2, state.build_plan());
        state.record_result(IntruderResult {
            attack_id: 1,
            sequence: 1,
            payloads: vec!["a".to_string()],
            duration: std::time::Duration::ZERO,
            result: RepeaterResult::Failed {
                error: "boom".to_string(),
            },
        });

        state.clear_results();
        assert_eq!(state.results.len(), 1, "a running attack owns its results");
        assert_eq!(state.received, 1);

        state.finish(1, 2, false);
        state.clear_results();
        assert!(state.results.is_empty());
        assert_eq!(state.result_bytes, 0);
        assert!(state.selected.is_none());
        assert!(state.detail.is_none());
        assert!(state.attack_id.is_none());
        assert!(state.last_plan.is_none());
        assert_eq!(state.total, 0);
        assert_eq!(state.received, 0);
        assert!(state.finished_note.is_none());
    }

    #[test]
    fn detail_notes_report_capture_problems() {
        let mut detail = IntruderDetail {
            sequence: 1,
            request: None,
            response: None,
            response_error: None,
            truncated: false,
            complete: true,
        };
        assert!(detail.notes().is_empty());

        detail.truncated = true;
        assert_eq!(
            detail.notes(),
            vec!["response body truncated at the capture limit".to_string()]
        );

        detail.complete = false;
        detail.response_error = Some("connection reset".to_string());
        assert_eq!(
            detail.notes(),
            vec![
                "response body truncated at the capture limit".to_string(),
                "response body did not finish".to_string(),
                "connection reset".to_string(),
            ]
        );
    }

    #[test]
    fn authority_from_uri_strips_scheme_userinfo_and_target() {
        assert_eq!(
            authority_from_uri("https://user:pass@example.test:8443/path?q=1"),
            "example.test:8443"
        );
        assert_eq!(authority_from_uri("http://[::1]:8080"), "[::1]:8080");
        assert_eq!(
            authority_from_uri("https://example.test?q=1#frag"),
            "example.test"
        );
        assert_eq!(authority_from_uri("/relative?q=1"), "");
    }
}
