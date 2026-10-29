//! Intruder engine control and workspace persistence.

use std::time::Duration;

use crate::project::{
    BodyWorkspace, IntruderAttackRecord, IntruderPlanWorkspace, IntruderResultOutcomeWorkspace,
    IntruderResultWorkspace, IntruderWorkspace, TextSelectionWorkspace,
};
use crate::proxy::{
    AttackType, DEFAULT_REPEATER_TIMEOUT, Intruder, IntruderConfig, IntruderError, IntruderEvent,
    IntruderPlan, IntruderResult, MAX_INTRUDER_CONCURRENCY, RepeaterRequest, RepeaterResult,
    marker_ranges, split_raw_request,
};
use crate::view;

use super::super::{MainTab, PerseidApp};
use super::state::{IntruderState, PayloadSet, authority_from_uri, result_size};

impl PerseidApp {
    /// Send the selected HTTP exchange to the Intruder workspace.
    pub(in crate::app) fn send_selected_to_intruder(&mut self) {
        let Some(item) = self
            .selected
            .and_then(|id| self.history.get(id))
            .filter(|item| !item.is_tunnel())
            .cloned()
        else {
            return;
        };
        self.send_history_to_intruder(&item);
    }

    pub(in crate::app) fn send_history_to_intruder(&mut self, item: &crate::history::HistoryItem) {
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
    pub(in crate::app) fn drain_intruder_results(&mut self) {
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
            cookie_jar: Some(self.cookie_jar.clone()),
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

    pub(in crate::app) fn toggle_intruder_attack(&mut self) {
        if self.intruder.running {
            self.stop_intruder_attack();
        } else {
            self.start_intruder_attack();
        }
    }
}

fn attack_record(attack: AttackType) -> IntruderAttackRecord {
    match attack {
        AttackType::Sniper => IntruderAttackRecord::Sniper,
        AttackType::BatteringRam => IntruderAttackRecord::BatteringRam,
        AttackType::Pitchfork => IntruderAttackRecord::Pitchfork,
        AttackType::ClusterBomb => IntruderAttackRecord::ClusterBomb,
    }
}

fn attack_from_record(attack: IntruderAttackRecord) -> AttackType {
    match attack {
        IntruderAttackRecord::Sniper => AttackType::Sniper,
        IntruderAttackRecord::BatteringRam => AttackType::BatteringRam,
        IntruderAttackRecord::Pitchfork => AttackType::Pitchfork,
        IntruderAttackRecord::ClusterBomb => AttackType::ClusterBomb,
    }
}

fn plan_workspace(plan: &IntruderPlan) -> IntruderPlanWorkspace {
    IntruderPlanWorkspace {
        attack: attack_record(plan.attack),
        scheme: plan.scheme.clone(),
        host: plan.host.clone(),
        template: plan.template.clone(),
        payload_sets: plan.payload_sets.clone(),
        concurrency: plan.concurrency,
        scope: plan.scope.clone(),
    }
}

fn plan_from_workspace(plan: &IntruderPlanWorkspace) -> IntruderPlan {
    IntruderPlan {
        attack: attack_from_record(plan.attack),
        scheme: plan.scheme.clone(),
        host: plan.host.clone(),
        template: plan.template.clone(),
        payload_sets: plan.payload_sets.clone(),
        concurrency: plan.concurrency,
        scope: plan.scope.clone(),
    }
}

fn result_workspace(result: &IntruderResult) -> IntruderResultWorkspace {
    IntruderResultWorkspace {
        attack_id: result.attack_id,
        sequence: result.sequence,
        payloads: result.payloads.clone(),
        duration_ms: u64::try_from(result.duration.as_millis()).unwrap_or(u64::MAX),
        outcome: match &result.result {
            RepeaterResult::Response {
                version,
                status,
                reason,
                headers,
                body,
                body_error,
            } => IntruderResultOutcomeWorkspace::Response {
                version: version.clone(),
                status: *status,
                reason: reason.clone(),
                headers: headers.clone(),
                body: BodyWorkspace::from_body(body),
                body_error: body_error.clone(),
            },
            RepeaterResult::Failed { error } => IntruderResultOutcomeWorkspace::Failed {
                error: error.clone(),
            },
        },
    }
}

fn result_from_workspace(result: &IntruderResultWorkspace) -> IntruderResult {
    IntruderResult {
        attack_id: result.attack_id,
        sequence: result.sequence,
        payloads: result.payloads.clone(),
        duration: Duration::from_millis(result.duration_ms),
        result: match &result.outcome {
            IntruderResultOutcomeWorkspace::Response {
                version,
                status,
                reason,
                headers,
                body,
                body_error,
            } => RepeaterResult::Response {
                version: version.clone(),
                status: *status,
                reason: reason.clone(),
                headers: headers.clone(),
                body: body.to_body(),
                body_error: body_error.clone(),
            },
            IntruderResultOutcomeWorkspace::Failed { error } => RepeaterResult::Failed {
                error: error.clone(),
            },
        },
    }
}

fn clamp_boundary(text: &str, mut offset: usize) -> usize {
    offset = offset.min(text.len());
    while offset > 0 && !text.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

fn restore_selection(
    text: &str,
    selection: Option<&TextSelectionWorkspace>,
) -> Option<std::ops::Range<usize>> {
    let selection = selection?;
    let start = clamp_boundary(text, selection.start);
    let end = clamp_boundary(text, selection.end);
    (start != end).then(|| start.min(end)..start.max(end))
}

impl PerseidApp {
    pub(in crate::app) fn intruder_workspace(&self) -> IntruderWorkspace {
        let state = &self.intruder;
        IntruderWorkspace {
            scheme: state.scheme.clone(),
            host: state.host.clone(),
            request: state.request.clone(),
            attack: attack_record(state.attack),
            payload_sets: state
                .payload_sets
                .iter()
                .map(|set| set.text.clone())
                .collect(),
            active_set: state.active_set,
            concurrency: state.concurrency,
            update_host: state.update_host,
            note: state.note.clone(),
            selection: state
                .selection
                .as_ref()
                .map(|range| TextSelectionWorkspace {
                    start: range.start,
                    end: range.end,
                }),
            cursor: state.cursor,
            results: state.results.iter().map(result_workspace).collect(),
            selected: state.selected,
            detail_pane: state.detail_pane.into(),
            detail_view: state.detail_view.into(),
            attack_id: state.attack_id,
            last_plan: state.last_plan.as_ref().map(plan_workspace),
            total: state.total,
            received: state.received,
            error: state.error.clone(),
            finished_note: state.finished_note.clone(),
        }
    }

    pub(in crate::app) fn apply_intruder_workspace(&mut self, workspace: &IntruderWorkspace) {
        if workspace.scheme.is_empty()
            && workspace.host.is_empty()
            && workspace.request.is_empty()
            && workspace.payload_sets.is_empty()
            && workspace.results.is_empty()
            && workspace.last_plan.is_none()
        {
            self.intruder = IntruderState::new();
            return;
        }
        let state = &mut self.intruder;
        state.payload_file_dialog = None;
        state.scheme = workspace.scheme.clone();
        state.host = workspace.host.clone();
        state.request = workspace.request.clone();
        state.attack = attack_from_record(workspace.attack);
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
        state.error = workspace.error.clone();
        state.notice = None;
        state.selection = restore_selection(&state.request, workspace.selection.as_ref());
        state.cursor = clamp_boundary(&state.request, workspace.cursor);
        state.results = workspace
            .results
            .iter()
            .map(result_from_workspace)
            .collect();
        state.result_bytes = state.results.iter().fold(0usize, |total, result| {
            total.saturating_add(result_size(result))
        });
        state.selected = workspace.selected.filter(|selected| {
            state
                .results
                .iter()
                .any(|result| result.sequence == *selected)
        });
        state.detail = None;
        state.detail_pane = workspace.detail_pane.into();
        state.detail_view = workspace.detail_view.into();
        state.attack_id = workspace.attack_id;
        state.last_plan = workspace.last_plan.as_ref().map(plan_from_workspace);
        state.started_at = None;
        state.total = workspace.total;
        state.received = workspace.received;
        state.running = false;
        state.stopping = false;
        state.finished_note = workspace.finished_note.clone();
        state.ensure_detail();
    }
}
