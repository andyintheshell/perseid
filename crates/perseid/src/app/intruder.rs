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

use std::ops::Range;
use std::time::Instant;

use egui::text::{CCursor, CCursorRange};
use egui::text_edit::TextEditState;
use egui::{FontId, Label, TextEdit};
use egui_extras::{Column, TableBuilder};

use crate::project::{IntruderAttackRecord, IntruderWorkspace};
use crate::proxy::{
    AttackType, DEFAULT_REPEATER_TIMEOUT, Intruder, IntruderConfig, IntruderError, IntruderEvent,
    IntruderPlan, IntruderResult, MAX_INTRUDER_CONCURRENCY, RepeaterRequest, RepeaterResult,
    auto_mark, clear_markers, marker_ranges, split_raw_request, toggle_marker, update_host_header,
};
use crate::view::{self, MessageViews};

use super::message_view::{message_text, message_view_text};
use super::widgets::{empty_state, message_mode_tab, table_frame};
use super::*;

const INTRUDER_TOOLBAR_ID: &str = "perseid.intruder.toolbar";
const INTRUDER_POSITIONS_ID: &str = "perseid.intruder.positions";
const INTRUDER_PAYLOADS_ID: &str = "perseid.intruder.payloads";
const INTRUDER_RESULTS_ID: &str = "perseid.intruder.results";
const INTRUDER_RESULTS_HEADER_ID: &str = "perseid.intruder.results.header";
const INTRUDER_DETAIL_ID: &str = "perseid.intruder.detail";
const INTRUDER_EDITOR_ID: &str = "perseid.intruder.request.editor";

/// Results retained by the workspace. The engine bounds its own queue; the UI
/// keeps enough history for review without allowing a long attack to consume
/// memory without limit.
const MAX_RESULT_ROWS: usize = 1_000;
const MAX_RESULT_BYTES: usize = 64 * 1024 * 1024;
/// Offset message-view ids so a result sequence cannot collide with a history
/// exchange that happens to have the same number.
const RESULT_MESSAGE_ID_BASE: u64 = 1_000_000_000;

#[derive(Default)]
struct PayloadSet {
    /// One payload per line. Blank lines are ignored so pasted lists can keep
    /// their formatting.
    text: String,
}

struct IntruderDetail {
    sequence: u64,
    request: Option<MessageViews>,
    response: Option<MessageViews>,
    response_error: Option<String>,
    truncated: bool,
    complete: bool,
}

impl IntruderDetail {
    fn notes(&self) -> Vec<String> {
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

pub(super) struct IntruderState {
    scheme: String,
    host: String,
    request: String,
    attack: AttackType,
    payload_sets: Vec<PayloadSet>,
    active_set: usize,
    concurrency: usize,
    update_host: bool,
    /// Warning about a request seeded from history, for example a truncated
    /// capture.
    note: Option<String>,
    error: Option<String>,
    notice: Option<String>,
    /// The last non-empty request-editor selection, used by Add/Clear $$.
    selection: Option<Range<usize>>,
    cursor: usize,
    results: Vec<IntruderResult>,
    result_bytes: usize,
    selected: Option<u64>,
    detail: Option<IntruderDetail>,
    detail_pane: MessagePane,
    detail_view: MessageView,
    attack_id: Option<u64>,
    last_plan: Option<IntruderPlan>,
    started_at: Option<Instant>,
    total: u64,
    received: u64,
    running: bool,
    stopping: bool,
    finished_note: Option<String>,
}

impl IntruderState {
    pub(super) fn new() -> Self {
        Self {
            scheme: "https".to_string(),
            host: String::new(),
            request: "GET / HTTP/1.1\nHost: \n\n".to_string(),
            attack: AttackType::Sniper,
            payload_sets: vec![PayloadSet::default()],
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

    pub(super) fn is_running(&self) -> bool {
        self.running
    }

    pub(super) fn status_summary(&self) -> Option<String> {
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
    fn load_template(
        &mut self,
        scheme: String,
        host: String,
        request: String,
        note: Option<String>,
    ) {
        self.scheme = scheme;
        self.host = host;
        self.request = request;
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

    fn sync_payload_sets(&mut self, positions: usize) {
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

    fn active_set_mut(&mut self) -> &mut PayloadSet {
        let index = if self.attack.uses_one_set() {
            0
        } else {
            self.active_set
                .min(self.payload_sets.len().saturating_sub(1))
        };
        &mut self.payload_sets[index]
    }

    fn set_count(&self) -> usize {
        if self.attack.uses_one_set() {
            1
        } else {
            marker_ranges(&self.request).map_or(1, |ranges| ranges.len().max(1))
        }
    }

    fn build_plan(&self) -> IntruderPlan {
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
    fn request_count(&self) -> u64 {
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

    fn payload_count(&self) -> u64 {
        payload_line_count(&self.active_set().text)
    }

    fn begin_attack(&mut self, attack_id: u64, total: u64, plan: IntruderPlan) {
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

    fn record_result(&mut self, result: IntruderResult) {
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

    fn finish(&mut self, attack_id: u64, total: u64, cancelled: bool) {
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

    fn fail(&mut self, message: impl Into<String>) {
        self.running = false;
        self.stopping = false;
        self.started_at = None;
        self.error = Some(message.into());
    }

    fn clear_results(&mut self) {
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

    fn ensure_detail(&mut self) {
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

    fn target_range(&self) -> Range<usize> {
        self.selection.clone().unwrap_or(self.cursor..self.cursor)
    }

    fn select_request(&mut self, ctx: &egui::Context, range: Range<usize>) {
        let len = self.request.len();
        let start = range.start.min(len);
        let end = range.end.max(start).min(len);
        self.selection = (start != end).then_some(start..end);
        self.cursor = start;

        let start_char = self.request[..start].chars().count();
        let end_char = self.request[..end].chars().count();
        let editor_id = egui::Id::unique(INTRUDER_EDITOR_ID);
        let mut state = TextEditState::load(ctx, editor_id).unwrap_or_default();
        state.cursor.set_char_range(Some(CCursorRange::two(
            CCursor::new(start_char),
            CCursor::new(end_char),
        )));
        state.store(ctx, editor_id);
        ctx.memory_mut(|memory| memory.request_focus(editor_id));
    }

    #[cfg(feature = "screenshots")]
    pub(super) fn seed_screenshot(&mut self) {
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

impl PerseidApp {
    /// Send the selected HTTP exchange to the Intruder workspace.
    pub(super) fn send_selected_to_intruder(&mut self) {
        let Some(item) = self
            .selected
            .and_then(|id| self.history.get(id))
            .filter(|item| !item.is_tunnel())
        else {
            return;
        };

        if self.intruder.running
            && let Some(engine) = &self.intruder_engine
        {
            engine.stop();
        }

        let scheme = if item.request.scheme.is_empty() {
            "http".to_string()
        } else {
            item.request.scheme.clone()
        };
        let note = view::seeded_body_note(&item.request_body, "starting an attack");

        self.intruder.load_template(
            scheme,
            item.request.host.clone(),
            view::format_request(item),
            note,
        );
        self.main_tab = MainTab::Intruder;
    }

    /// Poll the attack engine and apply every queued result.
    pub(super) fn drain_intruder_results(&mut self) {
        let mut events = Vec::new();
        let alive = if let Some(engine) = self.intruder_engine.as_mut() {
            while let Some(event) = engine.try_recv() {
                events.push(event);
            }
            engine.is_running()
        } else {
            true
        };

        for event in events {
            match event {
                IntruderEvent::Result(result) => self.intruder.record_result(result),
                IntruderEvent::Finished {
                    attack_id,
                    total,
                    cancelled,
                    ..
                } => self.intruder.finish(attack_id, total, cancelled),
            }
        }

        if !alive && self.intruder_engine.is_some() {
            self.intruder_engine = None;
            if self.intruder.running {
                self.intruder
                    .fail("the Intruder engine stopped unexpectedly");
            }
        }
    }

    fn ensure_intruder(&mut self) -> Result<(), String> {
        if self
            .intruder_engine
            .as_ref()
            .is_some_and(Intruder::is_running)
        {
            return Ok(());
        }
        self.intruder_engine = None;
        let config = IntruderConfig {
            capture_limit: self.capture_limit,
            timeout: DEFAULT_REPEATER_TIMEOUT,
            extra_roots_pem: Vec::new(),
            notify: Some(self.notify.clone()),
        };
        match Intruder::start(config) {
            Ok(engine) => {
                self.intruder_engine = Some(engine);
                Ok(())
            }
            Err(err) => Err(err.to_string()),
        }
    }

    pub(super) fn start_intruder_attack(&mut self) {
        if self.intruder.running {
            return;
        }
        self.intruder.error = None;
        self.intruder.notice = None;

        let positions = match marker_ranges(&self.intruder.request) {
            Ok(positions) => positions.len(),
            Err(err) => {
                self.intruder.error = Some(err.to_string());
                return;
            }
        };
        self.intruder.sync_payload_sets(positions);
        let mut plan = self.intruder.build_plan();
        if self.scope_enforce {
            plan.scope = self.scope.iter().cloned().collect();
        }

        let prepared = match plan.prepare() {
            Ok(prepared) => prepared,
            Err(err) => {
                self.intruder.error = Some(err.to_string());
                return;
            }
        };
        let total = prepared.total();
        let rendered = match prepared.render(1) {
            Ok(rendered) => rendered,
            Err(err) => {
                self.intruder.error = Some(err.to_string());
                return;
            }
        };
        let (head, body) = split_raw_request(&rendered.raw);
        let request = match RepeaterRequest::parse(
            head,
            body.as_bytes().to_vec(),
            &plan.scheme,
            &plan.host,
        ) {
            Ok(request) => request,
            Err(err) => {
                self.intruder.error = Some(err.to_string());
                return;
            }
        };
        // The parsed URI is the effective target: an absolute-form request
        // line overrides the target bar, so scope must check that authority
        // rather than only the configured host.
        let target_host = authority_from_uri(&request.uri);
        if self.scope_enforce
            && !crate::proxy::host_in_scope(&target_host, self.scope.iter().map(String::as_str))
        {
            self.intruder.error = Some(format!(
                "{target_host} is outside the enforced scope. Add it in Target > Scope or turn off scope enforcement."
            ));
            return;
        }

        if let Err(err) = self.ensure_intruder() {
            self.intruder.error = Some(err);
            return;
        }

        let result = self
            .intruder_engine
            .as_ref()
            .expect("Intruder was just started")
            .start_attack(plan.clone());
        match result {
            Ok(attack_id) => self.intruder.begin_attack(attack_id, total, plan),
            Err(err) => {
                let closed = matches!(err, IntruderError::Closed);
                self.intruder.error = Some(err.to_string());
                if closed {
                    self.intruder_engine = None;
                }
            }
        }
    }

    pub(super) fn stop_intruder_attack(&mut self) {
        if !self.intruder.running || self.intruder.stopping {
            return;
        }
        if let Some(engine) = &self.intruder_engine {
            engine.stop();
        }
        self.intruder.stopping = true;
        self.intruder.notice =
            Some("Stopping after the requests already in flight finish…".to_string());
    }

    pub(super) fn toggle_intruder_attack(&mut self) {
        if self.intruder.running {
            self.stop_intruder_attack();
        } else {
            self.start_intruder_attack();
        }
    }

    pub(super) fn intruder_view(&mut self, ui: &mut egui::Ui) {
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
    }

    fn intruder_toolbar(&mut self, ui: &mut egui::Ui) {
        let mut start = false;
        let mut stop = false;
        let mut host_changed = false;

        egui::Panel::top(INTRUDER_TOOLBAR_ID)
            .frame(
                egui::Frame::NONE
                    .fill(theme::SURFACE_ALT)
                    .inner_margin(egui::Margin::symmetric(10, 4)),
            )
            .show(ui, |ui| {
                let state = &mut self.intruder;
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Attack").small().color(theme::TEXT_MUTED));
                    egui::ComboBox::from_id_salt("perseid.intruder.attack")
                        .width(168.0)
                        .selected_text(state.attack.label())
                        .show_ui(ui, |ui| {
                            for attack in AttackType::ALL {
                                ui.selectable_value(&mut state.attack, attack, attack.label())
                                    .on_hover_text(attack.description());
                            }
                        });

                    ui.add_space(4.0);
                    if state.running {
                        let button = egui::Button::new(
                            RichText::new("Stop attack").strong().color(theme::RED),
                        )
                        .fill(theme::SURFACE)
                        .stroke(Stroke::new(1.0, theme::BORDER))
                        .min_size(egui::vec2(86.0, 24.0));
                        if ui
                            .add_enabled(!state.stopping, button)
                            .on_hover_text(
                                "Stop scheduling new requests. In-flight requests still finish.",
                            )
                            .on_disabled_hover_text("Already stopping")
                            .clicked()
                        {
                            stop = true;
                        }
                        if state.stopping {
                            ui.spinner();
                        }
                    } else {
                        let button = egui::Button::new(
                            RichText::new("Start attack").strong().color(Color32::WHITE),
                        )
                        .fill(theme::ACCENT_DARK)
                        .stroke(Stroke::new(1.0, theme::ACCENT_DARK))
                        .min_size(egui::vec2(94.0, 24.0));
                        if ui
                            .add(button)
                            .on_hover_text(
                                "Validate the template and start the attack (Ctrl+Enter)",
                            )
                            .clicked()
                        {
                            start = true;
                        }
                    }

                    ui.separator();
                    ui.label(RichText::new("Target").small().color(theme::TEXT_MUTED));
                    egui::ComboBox::from_id_salt("perseid.intruder.scheme")
                        .width(70.0)
                        .selected_text(&state.scheme)
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut state.scheme, "https".to_string(), "https");
                            ui.selectable_value(&mut state.scheme, "http".to_string(), "http");
                        });
                    if ui
                        .add(
                            TextEdit::singleline(&mut state.host)
                                .desired_width(230.0)
                                .hint_text("host[:port]"),
                        )
                        .on_hover_text(
                            "Connection target. Absolute-form request targets override this.",
                        )
                        .changed()
                    {
                        host_changed = true;
                    }
                    if ui
                        .checkbox(&mut state.update_host, "Update Host header")
                        .on_hover_text(
                            "Keep the Host header in the request in sync with the target above.",
                        )
                        .changed()
                        && state.update_host
                    {
                        host_changed = true;
                    }

                    ui.separator();
                    ui.label(
                        RichText::new("Concurrency")
                            .small()
                            .color(theme::TEXT_MUTED),
                    );
                    ui.add(
                        egui::DragValue::new(&mut state.concurrency)
                            .range(1..=MAX_INTRUDER_CONCURRENCY)
                            .speed(0.2),
                    )
                    .on_hover_text(format!(
                        "Simultaneous requests, 1–{MAX_INTRUDER_CONCURRENCY}"
                    ));

                    if state.running {
                        ui.separator();
                        ui.spinner();
                        ui.label(
                            RichText::new(format!("{} / {}", state.received, state.total))
                                .small()
                                .color(theme::TEXT_MUTED),
                        );
                    }
                });
            });

        if host_changed && self.intruder.update_host {
            self.intruder.request = update_host_header(&self.intruder.request, &self.intruder.host);
        }
        if start {
            self.start_intruder_attack();
        }
        if stop {
            self.stop_intruder_attack();
        }
    }

    fn intruder_positions(&mut self, ui: &mut egui::Ui) {
        let mut add_marker = false;
        let mut clear_all = false;
        let mut auto = false;

        egui::Panel::top(INTRUDER_POSITIONS_ID)
            .frame(
                egui::Frame::NONE
                    .fill(theme::SURFACE)
                    .inner_margin(egui::Margin::symmetric(10, 4)),
            )
            .show(ui, |ui| {
                let state = &mut self.intruder;
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("Positions")
                            .small()
                            .color(theme::TEXT_MUTED),
                    );
                    if ui
                        .button("Add $$")
                        .on_hover_text(
                            "Wrap the selected text in $$ markers. With no selection, insert an empty position at the cursor. Selecting an existing position removes its markers.",
                        )
                        .clicked()
                    {
                        add_marker = true;
                    }
                    if ui
                        .button("Clear $$")
                        .on_hover_text("Remove every position marker from the request")
                        .clicked()
                    {
                        clear_all = true;
                    }
                    if ui
                        .button("Auto $$")
                        .on_hover_text(
                            "Add positions around query values, form values, and cookie values",
                        )
                        .clicked()
                    {
                        auto = true;
                    }

                    ui.separator();
                    match marker_ranges(&state.request) {
                        Ok(ranges) if ranges.is_empty() => {
                            ui.label(
                                RichText::new("no payload positions")
                                    .small()
                                    .color(theme::TEXT_MUTED),
                            );
                        }
                        Ok(ranges) => {
                            ui.label(
                                RichText::new(format!(
                                    "{} payload position{}",
                                    ranges.len(),
                                    if ranges.len() == 1 { "" } else { "s" }
                                ))
                                .small()
                                .color(theme::ACCENT_DARK),
                            );
                        }
                        Err(_) => {
                            ui.label(
                                RichText::new("unclosed $$ marker")
                                    .small()
                                    .color(theme::RED),
                            );
                        }
                    }
                    if let Some(selection) = &state.selection {
                        ui.label(
                            RichText::new(format!(
                                "selection: {} B",
                                selection.end.saturating_sub(selection.start)
                            ))
                            .small()
                            .color(theme::TEXT_MUTED),
                        );
                    }
                });

                if let Some(error) = &state.error {
                    ui.add(
                        Label::new(RichText::new(error).small().color(theme::RED)).truncate(),
                    )
                    .on_hover_text(error);
                } else if let Some(notice) = &state.notice {
                    ui.add(
                        Label::new(RichText::new(notice).small().color(theme::AMBER)).truncate(),
                    )
                    .on_hover_text(notice);
                }
            });

        if add_marker {
            let target = self.intruder.target_range();
            let (request, range) = toggle_marker(&self.intruder.request, target);
            self.intruder.request = request;
            self.intruder.error = None;
            self.intruder.notice = None;
            self.intruder.select_request(ui.ctx(), range);
        }
        if clear_all {
            self.intruder.request = clear_markers(&self.intruder.request);
            self.intruder.error = None;
            self.intruder.notice = Some("Removed every position marker".to_string());
            self.intruder.selection = None;
            self.intruder.cursor = self.intruder.cursor.min(self.intruder.request.len());
        }
        if auto {
            let (request, count) = auto_mark(&self.intruder.request);
            self.intruder.request = request;
            self.intruder.error = None;
            self.intruder.notice = Some(if count == 0 {
                "No new values to mark".to_string()
            } else {
                format!(
                    "Added {count} payload position{}",
                    if count == 1 { "" } else { "s" }
                )
            });
            self.intruder.selection = None;
            self.intruder.cursor = self.intruder.cursor.min(self.intruder.request.len());
        }
    }

    fn intruder_editor(&mut self, ui: &mut egui::Ui) {
        egui::CentralPanel::default()
            .frame(
                egui::Frame::NONE
                    .fill(theme::SURFACE)
                    .inner_margin(egui::Margin::same(5)),
            )
            .show(ui, |ui| {
                let state = &mut self.intruder;
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Request").strong().color(theme::TEXT));
                    ui.label(
                        RichText::new("editable raw message")
                            .small()
                            .color(theme::TEXT_MUTED),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            RichText::new("Ctrl+Enter starts or stops the attack")
                                .small()
                                .color(theme::TEXT_MUTED),
                        );
                    });
                });
                if let Some(note) = &state.note {
                    ui.add_space(1.0);
                    ui.label(RichText::new(note).small().color(theme::AMBER));
                }
                ui.add_space(2.0);
                ui.separator();
                ui.add_space(2.0);

                egui::Frame::NONE
                    .fill(Color32::WHITE)
                    .stroke(Stroke::new(1.0, theme::BORDER))
                    .corner_radius(4.0)
                    .inner_margin(egui::Margin::same(8))
                    .show(ui, |ui| {
                        let min_height = ui.available_height().max(160.0);
                        let font = FontId::monospace(crate::highlight::MESSAGE_FONT_SIZE);
                        let mut layouter =
                            |ui: &egui::Ui, buffer: &dyn egui::TextBuffer, wrap_width: f32| {
                                let job = request_layout_job(buffer.as_str(), wrap_width, &font);
                                ui.fonts_mut(|fonts| fonts.layout_job(job))
                            };
                        let output = TextEdit::multiline(&mut state.request)
                            .id(egui::Id::unique(INTRUDER_EDITOR_ID))
                            .code_editor()
                            .desired_width(f32::INFINITY)
                            .min_size(egui::vec2(0.0, min_height))
                            .hint_text("Paste or edit a raw HTTP request, then wrap values in $$")
                            .layouter(&mut layouter)
                            .show(ui);
                        if output.response.has_focus()
                            && let Some(range) = output.state.cursor.char_range()
                        {
                            let range = range.as_sorted_char_range();
                            let start = byte_offset_for_char(&state.request, range.start.0);
                            let end = byte_offset_for_char(&state.request, range.end.0);
                            state.cursor = start;
                            state.selection = (start != end).then_some(start..end);
                        }
                        if output.response.changed() {
                            state.error = None;
                        }
                    });
            });
    }

    fn intruder_payloads(&mut self, ui: &mut egui::Ui) {
        egui::Panel::right(INTRUDER_PAYLOADS_ID)
            .resizable(true)
            .default_size(320.0)
            .min_size(230.0)
            .frame(
                egui::Frame::NONE
                    .fill(theme::SURFACE)
                    .inner_margin(egui::Margin::same(6)),
            )
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("perseid.intruder.payloads.scroll")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let state = &mut self.intruder;
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new("Payloads").strong().color(theme::TEXT),
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.label(
                                        RichText::new(format!(
                                            "{} payload{}",
                                            state.payload_count(),
                                            if state.payload_count() == 1 { "" } else { "s" }
                                        ))
                                        .small()
                                        .color(theme::TEXT_MUTED),
                                    );
                                },
                            );
                        });
                        ui.add_space(2.0);
                        ui.separator();

                        if state.attack.uses_one_set() {
                            ui.label(
                                RichText::new("Payload position")
                                    .small()
                                    .color(theme::TEXT_MUTED),
                            );
                            ui.label(
                                RichText::new("All positions share one payload set")
                                    .color(theme::TEXT),
                            );
                        } else {
                            let positions =
                                marker_ranges(&state.request).map_or(0, |ranges| ranges.len());
                            ui.label(
                                RichText::new("Payload position")
                                    .small()
                                    .color(theme::TEXT_MUTED),
                            );
                            if positions == 0 {
                                ui.label(
                                    RichText::new(
                                        "Add at least one $$ position to the request first.",
                                    )
                                    .small()
                                    .color(theme::TEXT_MUTED),
                                );
                            } else {
                                let active = state.active_set.min(positions - 1);
                                egui::ComboBox::from_id_salt("perseid.intruder.payload.position")
                                    .width(180.0)
                                    .selected_text(format!(
                                        "Position {} of {positions}",
                                        active + 1
                                    ))
                                    .show_ui(ui, |ui| {
                                        for index in 0..positions {
                                            ui.selectable_value(
                                                &mut state.active_set,
                                                index,
                                                format!("Position {}", index + 1),
                                            );
                                        }
                                    });
                            }
                        }

                        ui.add_space(3.0);
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new("Request count")
                                    .small()
                                    .color(theme::TEXT_MUTED),
                            );
                            ui.label(
                                RichText::new(state.request_count().to_string())
                                    .strong()
                                    .color(theme::TEXT),
                            );
                            ui.separator();
                            ui.label(
                                RichText::new(state.attack.label())
                                    .small()
                                    .color(theme::TEXT_MUTED),
                            );
                        });
                        ui.add_space(3.0);
                        ui.separator();
                        ui.add_space(3.0);

                        ui.label(
                            RichText::new("Payload list")
                                .small()
                                .color(theme::TEXT_MUTED),
                        );
                        ui.label(
                            RichText::new("One payload per line; blank lines are ignored.")
                                .small()
                                .color(theme::TEXT_MUTED),
                        );
                        ui.add_space(2.0);
                        let set = state.active_set_mut();
                        ui.add(
                            TextEdit::multiline(&mut set.text)
                                .code_editor()
                                .desired_width(f32::INFINITY)
                                .desired_rows(8)
                                .hint_text("admin\nguest\ntest"),
                        );
                        ui.horizontal(|ui| {
                            if ui
                                .button("Clear list")
                                .on_hover_text("Remove every payload from this set")
                                .clicked()
                            {
                                set.text.clear();
                            }
                            if ui
                                .button("Deduplicate")
                                .on_hover_text("Remove duplicate payloads, keeping the first occurrence")
                                .clicked()
                            {
                                let mut seen = std::collections::BTreeSet::new();
                                let unique: Vec<String> = payloads(&set.text)
                                    .into_iter()
                                    .filter(|payload| seen.insert(payload.clone()))
                                    .collect();
                                set.text = unique.join("\n");
                            }
                        });

                        ui.add_space(6.0);
                        ui.separator();
                        ui.add_space(3.0);
                        ui.label(
                            RichText::new(state.attack.description())
                                .small()
                                .color(theme::TEXT_MUTED),
                        );
                        ui.add_space(4.0);
                        ui.label(
                            RichText::new(
                                "Intruder talks directly to the target, like Repeater. Proxy interception, matching rules, and capture do not apply.",
                            )
                            .small()
                            .color(theme::TEXT_MUTED),
                        );
                    });
            });
    }

    fn intruder_results(&mut self, ui: &mut egui::Ui) {
        if self.intruder.attack_id.is_none() && self.intruder.results.is_empty() {
            return;
        }
        self.intruder.ensure_detail();

        egui::Panel::bottom(INTRUDER_RESULTS_ID)
            .resizable(true)
            .default_size(280.0)
            .min_size(170.0)
            .frame(
                egui::Frame::NONE
                    .fill(theme::SURFACE)
                    .inner_margin(egui::Margin::same(4)),
            )
            .show(ui, |ui| {
                egui::Panel::top(INTRUDER_RESULTS_HEADER_ID)
                    .frame(
                        egui::Frame::NONE
                            .fill(theme::SURFACE_ALT)
                            .inner_margin(egui::Margin::symmetric(8, 3)),
                    )
                    .show(ui, |ui| {
                        let state = &mut self.intruder;
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("Results").strong().color(theme::TEXT));
                            if state.running {
                                ui.spinner();
                            }
                            ui.label(
                                RichText::new(format!("{} / {}", state.received, state.total))
                                    .small()
                                    .color(theme::TEXT_MUTED),
                            );
                            if let Some(note) = &state.finished_note {
                                ui.label(RichText::new(note).small().color(theme::TEXT_MUTED));
                            }
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui
                                        .add_enabled(
                                            !state.running && !state.results.is_empty(),
                                            egui::Button::new("Clear results"),
                                        )
                                        .on_hover_text("Discard the result table")
                                        .on_disabled_hover_text(if state.running {
                                            "Wait for the attack to finish"
                                        } else {
                                            "No results to clear"
                                        })
                                        .clicked()
                                    {
                                        state.clear_results();
                                    }
                                    if state.running
                                        && let Some(started) = state.started_at
                                    {
                                        ui.label(
                                            RichText::new(view::format_duration(started.elapsed()))
                                                .small()
                                                .color(theme::TEXT_MUTED),
                                        );
                                    }
                                },
                            );
                        });
                    });

                egui::Panel::right(INTRUDER_DETAIL_ID)
                    .resizable(true)
                    .default_size((ui.available_width() * 0.42).max(280.0))
                    .min_size(220.0)
                    .frame(
                        egui::Frame::NONE
                            .fill(theme::SURFACE)
                            .inner_margin(egui::Margin::same(4)),
                    )
                    .show(ui, |ui| self.intruder_detail(ui));

                egui::CentralPanel::default()
                    .frame(egui::Frame::NONE.fill(theme::SURFACE))
                    .show(ui, |ui| self.intruder_result_table(ui));
            });
    }

    fn intruder_result_table(&mut self, ui: &mut egui::Ui) {
        let state = &mut self.intruder;
        let rows = state.results.len();
        if rows == 0 {
            empty_state(
                ui,
                if state.running {
                    "Waiting for the first result"
                } else {
                    "No results"
                },
                if state.running {
                    "Requests run concurrently; rows appear as soon as each response arrives."
                } else {
                    "Start an attack to fill the result table."
                },
            );
            return;
        }

        ui.style_mut().interaction.selectable_labels = false;
        let mut clicked = None;
        table_frame(ui, |ui| {
            let table = TableBuilder::new(ui)
                .striped(true)
                .resizable(true)
                .sense(egui::Sense::click())
                .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                .column(Column::exact(46.0))
                .column(Column::initial(220.0).at_least(90.0))
                .column(Column::exact(64.0))
                .column(Column::exact(78.0))
                .column(Column::remainder().at_least(64.0));

            table
                .header(20.0, |mut header| {
                    for title in ["#", "Payloads", "Status", "Length", "Time"] {
                        header.col(|ui| {
                            ui.label(RichText::new(title).strong().small().color(theme::TEXT));
                        });
                    }
                })
                .body(|body| {
                    body.rows(20.0, rows, |mut row| {
                        let result = &state.results[row.index()];
                        row.set_selected(state.selected == Some(result.sequence));
                        row.col(|ui| {
                            ui.label(
                                RichText::new(result.sequence.to_string())
                                    .small()
                                    .color(theme::TEXT),
                            );
                        });
                        let payloads = result.payloads.join(" · ");
                        row.col(|ui| {
                            ui.add(
                                Label::new(
                                    RichText::new(if payloads.is_empty() {
                                        "—".to_string()
                                    } else {
                                        payloads.clone()
                                    })
                                    .small()
                                    .color(theme::TEXT),
                                )
                                .truncate(),
                            )
                            .on_hover_text(&payloads);
                        });
                        row.col(|ui| match &result.result {
                            RepeaterResult::Response { status, .. } => {
                                ui.label(
                                    RichText::new(status.to_string())
                                        .small()
                                        .color(view::status_class_color(*status)),
                                );
                            }
                            RepeaterResult::Failed { .. } => {
                                ui.label(RichText::new("error").small().color(theme::RED));
                            }
                        });
                        row.col(|ui| {
                            let length = match &result.result {
                                RepeaterResult::Response { body, .. } => {
                                    view::human_bytes(body.total_len())
                                }
                                RepeaterResult::Failed { .. } => "—".to_string(),
                            };
                            ui.label(RichText::new(length).small().color(theme::TEXT));
                        });
                        row.col(|ui| {
                            ui.label(
                                RichText::new(view::format_duration(result.duration))
                                    .small()
                                    .color(theme::TEXT_MUTED),
                            );
                        });

                        let hover = match &result.result {
                            RepeaterResult::Response {
                                status,
                                reason,
                                body,
                                ..
                            } => format!(
                                "Payloads: {}\nStatus: {status} {reason}\nLength: {}",
                                if payloads.is_empty() {
                                    "—"
                                } else {
                                    payloads.as_str()
                                },
                                view::human_bytes(body.total_len())
                            ),
                            RepeaterResult::Failed { error } => {
                                format!("Payloads: {payloads}\nError: {error}")
                            }
                        };
                        let response = row.response().on_hover_text(hover);
                        if response.clicked() {
                            clicked = Some(result.sequence);
                        }
                    });
                });
        });

        if let Some(sequence) = clicked {
            state.selected = Some(sequence);
            state.detail = None;
            state.detail_pane = MessagePane::Response;
        }
    }

    fn intruder_detail(&mut self, ui: &mut egui::Ui) {
        let state = &mut self.intruder;
        let Some(sequence) = state.selected else {
            empty_state(
                ui,
                "Select a result",
                "Click a row to inspect its request and response.",
            );
            return;
        };
        let Some(result) = state.results.iter().find(|row| row.sequence == sequence) else {
            empty_state(
                ui,
                "Result no longer retained",
                "The selected row was removed by the result-history limit or cleared.",
            );
            return;
        };

        let mut pane = state.detail_pane;
        let mut view_mode = state.detail_view;
        ui.horizontal(|ui| {
            ui.label(RichText::new("Selected result").strong().color(theme::TEXT));
            ui.label(
                RichText::new(format!("#{}", result.sequence))
                    .small()
                    .color(theme::TEXT_MUTED),
            );
            match &result.result {
                RepeaterResult::Response { status, reason, .. } => {
                    ui.colored_label(
                        view::status_class_color(*status),
                        RichText::new(if reason.is_empty() {
                            status.to_string()
                        } else {
                            format!("{status} {reason}")
                        })
                        .strong(),
                    );
                }
                RepeaterResult::Failed { .. } => {
                    ui.colored_label(theme::RED, RichText::new("request failed").strong());
                }
            }
            ui.label(
                RichText::new(view::format_duration(result.duration))
                    .small()
                    .color(theme::TEXT_MUTED),
            );
        });
        if !result.payloads.is_empty() {
            ui.add(
                Label::new(
                    RichText::new(format!("Payloads: {}", result.payloads.join(" · ")))
                        .small()
                        .color(theme::TEXT_MUTED),
                )
                .truncate(),
            )
            .on_hover_text(result.payloads.join("\n"));
        }

        ui.horizontal(|ui| {
            if message_mode_tab(ui, pane == MessagePane::Request, "Request").clicked() {
                pane = MessagePane::Request;
            }
            if message_mode_tab(ui, pane == MessagePane::Response, "Response").clicked() {
                pane = MessagePane::Response;
            }
            ui.separator();
            if message_mode_tab(ui, view_mode == MessageView::Pretty, "Pretty").clicked() {
                view_mode = MessageView::Pretty;
            }
            if message_mode_tab(ui, view_mode == MessageView::Raw, "Raw").clicked() {
                view_mode = MessageView::Raw;
            }
            if message_mode_tab(ui, view_mode == MessageView::Hex, "Hex").clicked() {
                view_mode = MessageView::Hex;
            }
        });
        state.detail_pane = pane;
        state.detail_view = view_mode;

        if let Some(detail) = state.detail.as_ref()
            && pane == MessagePane::Response
        {
            let notes = detail.notes();
            if !notes.is_empty() {
                ui.label(RichText::new(notes.join(" · ")).small().color(theme::AMBER));
            }
        }

        ui.add_space(2.0);
        ui.separator();
        ui.add_space(2.0);

        let Some(detail) = state.detail.as_ref() else {
            empty_state(
                ui,
                "No message details",
                "The selected result has no retained data.",
            );
            return;
        };
        let views = match pane {
            MessagePane::Request => detail.request.as_ref(),
            MessagePane::Response => detail.response.as_ref(),
        };
        match views {
            Some(views) => {
                let text = message_view_text(views, view_mode);
                message_text(
                    ui,
                    RESULT_MESSAGE_ID_BASE + result.sequence,
                    pane,
                    view_mode,
                    text,
                );
            }
            None if pane == MessagePane::Request => {
                empty_state(
                    ui,
                    "Request not available",
                    "The template used for this attack is no longer in memory.",
                );
            }
            None => {
                let error = detail
                    .response_error
                    .as_deref()
                    .unwrap_or("The request failed before a response was received.");
                empty_state(ui, "Request failed", error);
            }
        }
    }
}

/// One payload per line. `str::lines` also accepts CRLF input, which makes
/// pasted Windows payload files work without preprocessing.
fn payloads(text: &str) -> Vec<String> {
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(str::to_string)
        .collect()
}

fn payload_line_count(text: &str) -> u64 {
    text.lines().filter(|line| !line.trim().is_empty()).count() as u64
}

/// Build the request editor layout while keeping `$$` visible and marking the
/// enclosed position. The text is appended verbatim so selection, copying, and
/// editing still operate on the raw request.
fn request_layout_job(text: &str, wrap_width: f32, font: &FontId) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    job.wrap.max_width = wrap_width;
    job.wrap.break_anywhere = true;
    job.keep_trailing_whitespace = true;

    let marker = crate::proxy::MARKER;
    let mut cursor = 0;
    while let Some(relative_open) = text[cursor..].find(marker) {
        let open = cursor + relative_open;
        append_text(
            &mut job,
            &text[cursor..open],
            font,
            theme::TEXT,
            Color32::TRANSPARENT,
        );
        let content_start = open + marker.len();
        let Some(relative_close) = text[content_start..].find(marker) else {
            append_text(
                &mut job,
                &text[open..],
                font,
                theme::RED,
                Color32::TRANSPARENT,
            );
            return job;
        };
        let close = content_start + relative_close;
        append_text(
            &mut job,
            marker,
            font,
            theme::ACCENT_DARK,
            theme::ACCENT_SOFT,
        );
        append_text(
            &mut job,
            &text[content_start..close],
            font,
            theme::TEXT,
            theme::SELECTED,
        );
        append_text(
            &mut job,
            marker,
            font,
            theme::ACCENT_DARK,
            theme::ACCENT_SOFT,
        );
        cursor = close + marker.len();
    }
    append_text(
        &mut job,
        &text[cursor..],
        font,
        theme::TEXT,
        Color32::TRANSPARENT,
    );
    job
}

fn append_text(
    job: &mut egui::text::LayoutJob,
    text: &str,
    font: &FontId,
    color: Color32,
    background: Color32,
) {
    if text.is_empty() {
        return;
    }
    job.append(
        text,
        0.0,
        egui::TextFormat {
            font_id: font.clone(),
            color,
            background,
            ..Default::default()
        },
    );
}

fn byte_offset_for_char(text: &str, char_index: usize) -> usize {
    text.char_indices()
        .nth(char_index)
        .map_or(text.len(), |(offset, _)| offset)
}

fn authority_from_uri(uri: &str) -> String {
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

fn result_size(result: &IntruderResult) -> usize {
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

impl PerseidApp {
    pub(super) fn intruder_workspace(&self) -> IntruderWorkspace {
        let state = &self.intruder;
        IntruderWorkspace {
            scheme: state.scheme.clone(),
            host: state.host.clone(),
            request: state.request.clone(),
            attack: match state.attack {
                AttackType::Sniper => IntruderAttackRecord::Sniper,
                AttackType::BatteringRam => IntruderAttackRecord::BatteringRam,
                AttackType::Pitchfork => IntruderAttackRecord::Pitchfork,
                AttackType::ClusterBomb => IntruderAttackRecord::ClusterBomb,
            },
            payload_sets: state
                .payload_sets
                .iter()
                .map(|set| set.text.clone())
                .collect(),
            active_set: state.active_set,
            concurrency: state.concurrency,
            update_host: state.update_host,
            note: state.note.clone(),
        }
    }

    pub(super) fn apply_intruder_workspace(&mut self, workspace: &IntruderWorkspace) {
        if workspace.scheme.is_empty()
            && workspace.host.is_empty()
            && workspace.request.is_empty()
            && workspace.payload_sets.is_empty()
        {
            self.intruder = IntruderState::new();
            return;
        }
        let state = &mut self.intruder;
        state.scheme = workspace.scheme.clone();
        state.host = workspace.host.clone();
        state.request = workspace.request.clone();
        state.attack = match workspace.attack {
            IntruderAttackRecord::Sniper => AttackType::Sniper,
            IntruderAttackRecord::BatteringRam => AttackType::BatteringRam,
            IntruderAttackRecord::Pitchfork => AttackType::Pitchfork,
            IntruderAttackRecord::ClusterBomb => AttackType::ClusterBomb,
        };
        state.payload_sets = if workspace.payload_sets.is_empty() {
            vec![PayloadSet::default()]
        } else {
            workspace
                .payload_sets
                .iter()
                .cloned()
                .map(|text| PayloadSet { text })
                .collect()
        };
        state.active_set = workspace
            .active_set
            .min(state.payload_sets.len().saturating_sub(1));
        state.concurrency = workspace.concurrency.clamp(1, MAX_INTRUDER_CONCURRENCY);
        state.update_host = workspace.update_host;
        state.note = workspace.note.clone();
        state.error = None;
        state.notice = None;
        state.selection = None;
        state.cursor = 0;
        state.results.clear();
        state.result_bytes = 0;
        state.selected = None;
        state.detail = None;
        state.detail_pane = MessagePane::Response;
        state.detail_view = MessageView::Pretty;
        state.attack_id = None;
        state.last_plan = None;
        state.started_at = None;
        state.total = 0;
        state.received = 0;
        state.running = false;
        state.stopping = false;
        state.finished_note = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn payloads_ignore_blank_lines_and_accept_crlf() {
        assert_eq!(
            payloads("admin\r\n\r\nguest\n  \ntest"),
            vec!["admin", "guest", "test"]
        );
        assert_eq!(payload_line_count("a\nb\n\n"), 2);
    }

    #[test]
    fn request_layout_preserves_every_byte_and_marks_positions() {
        let font = FontId::monospace(crate::highlight::MESSAGE_FONT_SIZE);
        let cases = [
            "GET /?id=$$1$$ HTTP/1.1\nHost: example.test\n\n",
            "POST / HTTP/1.1\r\n\r\nuser=$$admin$$",
            "GET /?q=$$unclosed HTTP/1.1\n\n",
            "GET / HTTP/1.1\n\n",
        ];
        for text in cases {
            let job = request_layout_job(text, 200.0, &font);
            assert_eq!(job.text, text);
        }
        let job = request_layout_job(cases[0], 200.0, &font);
        assert!(
            job.sections
                .iter()
                .any(|section| section.format.background == theme::SELECTED),
            "the payload position should receive a background highlight"
        );
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
    fn byte_offsets_are_utf8_character_boundaries() {
        let text = "Привет $$мир$$";
        let start = byte_offset_for_char(text, 9);
        let end = byte_offset_for_char(text, 12);
        assert_eq!(&text[start..end], "мир");
        assert_eq!(byte_offset_for_char(text, 10_000), text.len());
    }
}
