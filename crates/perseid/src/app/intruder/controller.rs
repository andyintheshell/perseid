//! Intruder engine control and workspace persistence.

use crate::project::{IntruderAttackRecord, IntruderWorkspace};
use crate::proxy::{
    AttackType, DEFAULT_REPEATER_TIMEOUT, Intruder, IntruderConfig, IntruderError, IntruderEvent,
    MAX_INTRUDER_CONCURRENCY, RepeaterRequest, marker_ranges, split_raw_request,
};
use crate::view;

use super::super::{MainTab, MessagePane, MessageView, PerseidApp};
use super::state::{IntruderState, PayloadSet, authority_from_uri};

impl PerseidApp {
    /// Send the selected HTTP exchange to the Intruder workspace.
    pub(in crate::app) fn send_selected_to_intruder(&mut self) {
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

impl PerseidApp {
    pub(in crate::app) fn intruder_workspace(&self) -> IntruderWorkspace {
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

    pub(in crate::app) fn apply_intruder_workspace(&mut self, workspace: &IntruderWorkspace) {
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
