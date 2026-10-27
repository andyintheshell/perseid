//! Burp-style search across captured and retained workbench traffic.
//!
//! Search is deliberately a floating window rather than another workspace
//! tab. The magnifier in the top toolbar opens it, and the window stays out of
//! the way of the Proxy and Repeater layouts while it searches request and
//! response heads and bodies.
//!
//! HTTP and WebSocket rows come from the project store, so results include
//! traffic that is older than the bounded UI snapshot. Repeater tabs and
//! retained Intruder results are searched from their in-memory workspaces.
//! Matching uses the same decoded body representation as the message editor:
//! supported `Content-Encoding` values are unpacked before searching, so a
//! compressed response can be found by its readable text.
//!
//! The window state lives here; query compilation, matching, and the stored
//! scan worker are in [`engine`], and the egui layout is in [`view`].

mod engine;
mod view;

use std::collections::{BTreeSet, HashSet};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::app::inspector::InspectorModel;
use crate::app::intruder::IntruderState;
use crate::app::repeater::RepeaterTab;
use crate::app::{MessagePane, MessageView};
use crate::history::History;
use crate::project::{
    ProjectSnapshot, SearchOptionsWorkspace, SearchResultWorkspace, SearchSourceRecord,
    SearchWorkspace,
};
use crate::view::MessageViews;
use crate::websocket_history::WebSocketHistory;

use self::engine::{
    SEARCH_RESULT_LIMIT, SearchHit, SearchJob, SearchJobEvent, SearchKey, SearchMatcher,
    SearchOptions, SearchProgress, SearchSource,
};
use super::PerseidApp;

impl From<SearchSource> for SearchSourceRecord {
    fn from(source: SearchSource) -> Self {
        match source {
            SearchSource::ProxyHttp => Self::ProxyHttp,
            SearchSource::ProxyWebSocket => Self::ProxyWebSocket,
            SearchSource::Repeater => Self::Repeater,
            SearchSource::Intruder => Self::Intruder,
        }
    }
}

impl From<SearchSourceRecord> for SearchSource {
    fn from(source: SearchSourceRecord) -> Self {
        match source {
            SearchSourceRecord::ProxyHttp => Self::ProxyHttp,
            SearchSourceRecord::ProxyWebSocket => Self::ProxyWebSocket,
            SearchSourceRecord::Repeater => Self::Repeater,
            SearchSourceRecord::Intruder => Self::Intruder,
        }
    }
}

impl From<SearchOptions> for SearchOptionsWorkspace {
    fn from(options: SearchOptions) -> Self {
        Self {
            case_sensitive: options.case_sensitive,
            regex: options.regex,
            negative: options.negative,
            in_scope_only: options.in_scope_only,
            dynamic_update: options.dynamic_update,
            proxy_history: options.proxy_history,
            websockets: options.websockets,
            repeater: options.repeater,
            intruder: options.intruder,
            request_headers: options.request_headers,
            request_body: options.request_body,
            response_headers: options.response_headers,
            response_body: options.response_body,
        }
    }
}

impl From<&SearchOptionsWorkspace> for SearchOptions {
    fn from(options: &SearchOptionsWorkspace) -> Self {
        Self {
            case_sensitive: options.case_sensitive,
            regex: options.regex,
            negative: options.negative,
            in_scope_only: options.in_scope_only,
            dynamic_update: options.dynamic_update,
            proxy_history: options.proxy_history,
            websockets: options.websockets,
            repeater: options.repeater,
            intruder: options.intruder,
            request_headers: options.request_headers,
            request_body: options.request_body,
            response_headers: options.response_headers,
            response_body: options.response_body,
        }
    }
}

fn timestamp_ms(timestamp: Option<SystemTime>) -> Option<u64> {
    timestamp
        .and_then(|timestamp| timestamp.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis() as u64)
}

fn timestamp_from_ms(milliseconds: Option<u64>) -> Option<SystemTime> {
    milliseconds.map(|milliseconds| UNIX_EPOCH + Duration::from_millis(milliseconds))
}

impl From<&SearchHit> for SearchResultWorkspace {
    fn from(hit: &SearchHit) -> Self {
        Self {
            source: hit.key.source.into(),
            id: hit.key.id,
            number: hit.number,
            timestamp_ms: timestamp_ms(hit.timestamp),
            host: hit.host.clone(),
            url: hit.url.clone(),
            status: hit.status,
            length: hit.length,
        }
    }
}

impl From<&SearchResultWorkspace> for SearchHit {
    fn from(item: &SearchResultWorkspace) -> Self {
        Self {
            key: SearchKey {
                source: item.source.into(),
                id: item.id,
            },
            number: item.number,
            timestamp: timestamp_from_ms(item.timestamp_ms),
            host: item.host.clone(),
            url: item.url.clone(),
            status: item.status,
            length: item.length,
        }
    }
}

/// Read-only inputs for one search run.
pub(super) struct SearchSources<'a> {
    pub(super) history: &'a History,
    pub(super) websocket_history: &'a WebSocketHistory,
    pub(super) repeater_tabs: &'a [RepeaterTab],
    pub(super) intruder: &'a IntruderState,
    pub(super) snapshot: Option<ProjectSnapshot>,
}

/// Formatted detail for the selected search result. Rebuilt when the
/// selection changes, just like the Proxy detail cache.
pub(super) struct SearchDetailCache {
    pub(super) key: SearchKey,
    pub(super) request: MessageViews,
    pub(super) response: MessageViews,
    pub(super) has_request: bool,
    pub(super) has_response: bool,
    pub(super) single_message: bool,
    pub(super) inspector: Option<InspectorModel>,
    pub(super) status: Option<u16>,
    pub(super) length: u64,
    pub(super) duration: Option<Duration>,
    pub(super) number: Option<u64>,
}

/// Persistent Search window state.
pub(super) struct SearchState {
    /// Whether the floating window is visible. The toolbar icon uses this for
    /// its selected state.
    pub(super) open: bool,
    pub(super) query: String,
    pub(super) options: SearchOptions,
    pub(super) matcher: Option<SearchMatcher>,
    pub(super) error: Option<String>,
    pub(super) ran: bool,
    pub(super) dirty: bool,
    pub(super) results: Vec<SearchHit>,
    pub(super) selected: Option<SearchKey>,
    pub(super) detail: Option<SearchDetailCache>,
    pub(super) pane: MessagePane,
    pub(super) request_view: MessageView,
    pub(super) response_view: MessageView,
    pub(super) focus_query: bool,
    pub(super) last_history_revision: Option<u64>,
    pub(super) last_websocket_revision: Option<u64>,
    pub(super) running: bool,
    pub(super) progress: Option<SearchProgress>,
    pub(super) limit_reached: bool,
    /// Results currently in the table. This is rebuilt from the list when a
    /// workspace is applied and makes streaming scans cheap to deduplicate.
    keys: HashSet<SearchKey>,
    job: Option<SearchJob>,
    /// Last width the results table was laid out at. The table remembers
    /// column widths across frames, so a change here resets them to sensible
    /// defaults instead of preserving widths from the window's first, often
    /// narrower, layout pass.
    pub(super) table_width: Option<f32>,
}

impl Default for SearchState {
    fn default() -> Self {
        Self {
            open: false,
            query: String::new(),
            options: SearchOptions::default(),
            matcher: None,
            error: None,
            ran: false,
            dirty: false,
            results: Vec::new(),
            selected: None,
            detail: None,
            pane: MessagePane::Response,
            request_view: MessageView::Pretty,
            response_view: MessageView::Pretty,
            focus_query: false,
            last_history_revision: None,
            last_websocket_revision: None,
            running: false,
            progress: None,
            limit_reached: false,
            keys: HashSet::new(),
            job: None,
            table_width: None,
        }
    }
}

impl SearchState {
    /// Open the window and focus the query field, or close it when the toolbar
    /// icon is clicked while the window is already visible.
    pub(super) fn toggle(&mut self) {
        self.open = !self.open;
        if self.open {
            self.focus_query = true;
        }
    }

    /// Open the window and focus the query field without closing it. Used by
    /// the keyboard shortcut when the window is already visible.
    pub(super) fn focus_or_open(&mut self) {
        self.open = true;
        self.focus_query = true;
    }

    fn cancel_job(&mut self) {
        if let Some(job) = self.job.take() {
            job.cancel();
        }
        self.running = false;
        self.progress = None;
    }

    fn reset_results(&mut self) {
        self.cancel_job();
        self.results.clear();
        self.keys.clear();
        self.selected = None;
        self.detail = None;
        self.limit_reached = false;
    }

    /// Drop every retained result, for example when the project changes.
    #[cfg(test)]
    pub(super) fn clear_history(&mut self) {
        self.reset_results();
        self.matcher = None;
        self.error = None;
        self.ran = false;
        self.dirty = false;
        self.last_history_revision = None;
        self.last_websocket_revision = None;
    }

    /// Drop results from one tool. Clearing captured HTTP or WebSocket
    /// traffic also stops an in-flight stored scan so it cannot re-add records
    /// from the old database snapshot.
    pub(super) fn clear_source(&mut self, source: SearchSource) {
        if matches!(
            source,
            SearchSource::ProxyHttp | SearchSource::ProxyWebSocket
        ) {
            self.cancel_job();
        }
        self.results.retain(|hit| hit.key.source != source);
        self.keys.retain(|key| key.source != source);
        if self.selected.is_some_and(|key| key.source == source) {
            self.selected = self.results.first().map(|hit| hit.key);
            self.detail = None;
        }
    }

    pub(super) fn clear_http_source(&mut self) {
        self.clear_source(SearchSource::ProxyHttp);
    }

    pub(super) fn clear_websocket_source(&mut self) {
        self.clear_source(SearchSource::ProxyWebSocket);
    }

    pub(super) fn workspace(&self) -> SearchWorkspace {
        let selected = self
            .selected
            .and_then(|key| self.results.iter().find(|hit| hit.key == key));
        SearchWorkspace {
            open: self.open,
            query: self.query.clone(),
            options: self.options.into(),
            ran: self.ran,
            dirty: self.dirty,
            // Legacy fields only describe HTTP rows. New readers use the
            // structured item list below, which preserves every source.
            results: self
                .results
                .iter()
                .filter(|hit| hit.key.source == SearchSource::ProxyHttp)
                .map(|hit| hit.key.id)
                .collect(),
            selected: selected
                .filter(|hit| hit.key.source == SearchSource::ProxyHttp)
                .map(|hit| hit.key.id),
            result_items: self
                .results
                .iter()
                .map(SearchResultWorkspace::from)
                .collect(),
            selected_item: selected.map(SearchResultWorkspace::from),
            limit_reached: self.limit_reached,
            pane: self.pane.into(),
            request_view: self.request_view.into(),
            response_view: self.response_view.into(),
            error: self.error.clone(),
        }
    }

    pub(super) fn apply_workspace(
        &mut self,
        workspace: &SearchWorkspace,
        history: &History,
        websocket_history: &WebSocketHistory,
    ) {
        self.reset_results();
        self.open = workspace.open;
        self.query = workspace.query.clone();
        self.options = (&workspace.options).into();
        self.error = workspace.error.clone();
        self.ran = workspace.ran;
        self.dirty = workspace.dirty;
        self.limit_reached = workspace.limit_reached;
        if workspace.result_items.is_empty() {
            // Workspaces written before structured results stored only HTTP
            // IDs. Keep their old behavior: restore rows still in the loaded
            // snapshot.
            self.results = workspace
                .results
                .iter()
                .filter_map(|id| history.get(*id))
                .take(SEARCH_RESULT_LIMIT)
                .map(engine::http_hit)
                .collect();
        } else {
            self.results = workspace
                .result_items
                .iter()
                .take(SEARCH_RESULT_LIMIT)
                .map(SearchHit::from)
                .collect();
        }
        if workspace.result_items.len() > SEARCH_RESULT_LIMIT
            || workspace.results.len() > SEARCH_RESULT_LIMIT
        {
            self.limit_reached = true;
        }
        self.keys = self.results.iter().map(|hit| hit.key).collect();
        let selected = workspace
            .selected_item
            .as_ref()
            .map(SearchHit::from)
            .map(|hit| hit.key)
            .or_else(|| {
                workspace.selected.map(|id| SearchKey {
                    source: SearchSource::ProxyHttp,
                    id,
                })
            });
        self.selected = selected.filter(|key| self.keys.contains(key));
        self.pane = workspace.pane.into();
        self.request_view = workspace.request_view.into();
        self.response_view = workspace.response_view.into();
        self.detail = None;
        self.focus_query = false;
        self.table_width = None;
        self.last_history_revision = Some(history.revision());
        self.last_websocket_revision = Some(websocket_history.revision());
        self.matcher = if self.ran && !self.dirty && self.error.is_none() {
            SearchMatcher::new(&self.query, self.options.case_sensitive, self.options.regex)
                .ok()
                .map(|matcher| matcher.with_negative(self.options.negative))
        } else {
            None
        };
    }

    fn push_hit(&mut self, hit: SearchHit) {
        if self.keys.contains(&hit.key) {
            return;
        }
        if self.results.len() >= SEARCH_RESULT_LIMIT {
            self.limit_reached = true;
            return;
        }
        self.keys.insert(hit.key);
        if self.selected.is_none() {
            self.selected = Some(hit.key);
        }
        self.results.push(hit);
    }

    /// Keep the table grouped by tool with captured traffic in capture order.
    /// Stored rows stream in after the loaded snapshot and are merged here so
    /// older records appear before newer ones, not after them.
    fn sort_results(&mut self) {
        self.results.sort_by(|left, right| {
            left.key
                .source
                .rank()
                .cmp(&right.key.source.rank())
                .then_with(|| match (left.timestamp, right.timestamp) {
                    (Some(left), Some(right)) => left.cmp(&right),
                    (Some(_), None) => std::cmp::Ordering::Less,
                    (None, Some(_)) => std::cmp::Ordering::Greater,
                    (None, None) => std::cmp::Ordering::Equal,
                })
                .then_with(|| left.key.id.cmp(&right.key.id))
        });
    }

    /// Compile the query and run every selected source.
    pub(super) fn run(&mut self, sources: &SearchSources<'_>, scope: &BTreeSet<String>) {
        self.reset_results();
        self.error = None;
        self.dirty = false;
        self.ran = false;
        self.matcher = None;
        self.last_history_revision = Some(sources.history.revision());
        self.last_websocket_revision = Some(sources.websocket_history.revision());

        if !self.options.any_tool() {
            self.error = Some("Select at least one tool.".to_owned());
            return;
        }
        if !self.options.any_location() {
            self.error = Some("Select at least one location.".to_owned());
            return;
        }
        if self.query.is_empty() {
            return;
        }

        let matcher = match SearchMatcher::new(
            &self.query,
            self.options.case_sensitive,
            self.options.regex,
        ) {
            Ok(matcher) => matcher.with_negative(self.options.negative),
            Err(error) => {
                self.error = Some(error);
                return;
            }
        };

        if self.options.repeater {
            for tab in sources.repeater_tabs {
                if !engine::host_in_scope(self.options, scope, tab.host())
                    || !engine::matches_document(
                        &matcher,
                        self.options,
                        tab.request(),
                        tab.response_raw(),
                    )
                {
                    continue;
                }
                let scheme = if tab.scheme().is_empty() {
                    "http"
                } else {
                    tab.scheme()
                };
                let length = tab
                    .response_size()
                    .max(u64::try_from(tab.request().len()).unwrap_or(u64::MAX));
                self.push_hit(SearchHit::text(
                    SearchKey {
                        source: SearchSource::Repeater,
                        id: tab.id(),
                    },
                    None,
                    None,
                    format!("{scheme}://{}", tab.host()),
                    tab.title(),
                    tab.response_status(),
                    length,
                ));
            }
        }

        if self.options.intruder
            && engine::host_in_scope(self.options, scope, sources.intruder.search_host())
        {
            let plan = sources.intruder.search_plan();
            for result in sources.intruder.search_results() {
                let request = plan
                    .and_then(|plan| plan.render_sequence(result.sequence).ok())
                    .map(|rendered| rendered.raw)
                    .unwrap_or_default();
                let response = engine::repeater_response_raw(&result.result);
                if !engine::matches_document(&matcher, self.options, &request, response.as_deref())
                {
                    continue;
                }
                let scheme = if sources.intruder.search_scheme().is_empty() {
                    "http"
                } else {
                    sources.intruder.search_scheme()
                };
                let mut url = engine::request_target(&request);
                if !result.payloads.is_empty() {
                    url.push_str(" · ");
                    url.push_str(&result.payloads.join(", "));
                }
                self.push_hit(SearchHit::text(
                    SearchKey {
                        source: SearchSource::Intruder,
                        id: result.sequence,
                    },
                    Some(result.sequence),
                    None,
                    format!("{scheme}://{}", sources.intruder.search_host()),
                    url,
                    engine::repeater_response_status(&result.result),
                    engine::repeater_response_length(&result.result),
                ));
            }
        }

        let mut background = false;
        // Search the loaded caches first so a dynamic update sees the row that
        // just arrived, even if the project writer has not committed it yet.
        // The stored scan below fills in older rows and deduplicates by key.
        if self.options.proxy_history {
            for hit in engine::scan_loaded_http(sources.history, &matcher, self.options, scope) {
                self.push_hit(hit);
            }
        }
        if self.options.websockets {
            for hit in
                engine::scan_loaded_ws(sources.websocket_history, &matcher, self.options, scope)
            {
                self.push_hit(hit);
            }
        }

        if let Some(snapshot) = &sources.snapshot
            && (self.options.proxy_history || self.options.websockets)
            && self.results.len() < SEARCH_RESULT_LIMIT
        {
            match engine::start_scan(
                snapshot.clone(),
                matcher.clone(),
                self.options,
                scope.clone(),
                SEARCH_RESULT_LIMIT.saturating_sub(self.results.len()),
            ) {
                Ok(job) => {
                    self.job = Some(job);
                    self.running = true;
                    background = true;
                }
                Err(error) => self.error = Some(error),
            }
        }

        self.matcher = Some(matcher);
        self.ran = true;
        self.sort_results();
        self.selected = self.results.first().map(|hit| hit.key);
        if !background && self.results.len() >= SEARCH_RESULT_LIMIT {
            self.limit_reached = true;
        }
    }

    /// Drain background scan events. Called once per frame while the window is
    /// open; a scan that finishes while the window is closed is picked up when
    /// it is reopened.
    pub(super) fn poll(&mut self) {
        let (events, disconnected) = match &self.job {
            Some(job) => {
                let mut events = Vec::new();
                let disconnected = loop {
                    match job.events.try_recv() {
                        Ok(event) => events.push(event),
                        Err(std::sync::mpsc::TryRecvError::Empty) => break false,
                        Err(std::sync::mpsc::TryRecvError::Disconnected) => break true,
                    }
                };
                (events, disconnected)
            }
            None => return,
        };

        let mut done = disconnected;
        for event in events {
            match event {
                SearchJobEvent::Progress(progress) => self.progress = Some(progress),
                SearchJobEvent::Hit(hit) => self.push_hit(hit),
                SearchJobEvent::Finished(source) => {
                    if self.progress.as_ref().is_some_and(|p| p.source == source) {
                        self.progress = None;
                    }
                }
                SearchJobEvent::LimitReached => self.limit_reached = true,
                SearchJobEvent::Error(error) => self.error = Some(error),
                SearchJobEvent::Done => done = true,
            }
        }
        if done {
            self.job = None;
            self.running = false;
            self.progress = None;
            self.sort_results();
            if self.selected.is_none() {
                self.selected = self.results.first().map(|hit| hit.key);
            }
        }
    }

    pub(super) fn ensure_detail(&mut self, sources: &SearchSources<'_>) {
        let selected = self.selected.filter(|key| self.keys.contains(key));
        if selected != self.selected {
            self.selected = selected;
            self.detail = None;
        }
        let Some(key) = self.selected else {
            self.detail = None;
            return;
        };
        if self.detail.as_ref().is_some_and(|detail| detail.key == key) {
            return;
        }
        self.detail = build_detail(sources, key);
    }

    /// Build the deterministic Search scene used by the documentation
    /// screenshots: a decoded JSON response with several visible matches.
    #[cfg(feature = "screenshots")]
    pub(super) fn seed_screenshot(
        &mut self,
        history: &crate::history::History,
        scope: &std::collections::BTreeSet<String>,
    ) {
        let websocket_history = WebSocketHistory::new(1, 1);
        let intruder = IntruderState::new();
        let sources = SearchSources {
            history,
            websocket_history: &websocket_history,
            repeater_tabs: &[],
            intruder: &intruder,
            snapshot: None,
        };
        self.open = true;
        self.query = "Router".to_owned();
        self.options = SearchOptions {
            websockets: false,
            repeater: false,
            intruder: false,
            ..SearchOptions::default()
        };
        self.pane = MessagePane::Response;
        self.request_view = MessageView::Pretty;
        self.response_view = MessageView::Pretty;
        self.focus_query = false;
        self.run(&sources, scope);
        if let Some(key) = self
            .results
            .iter()
            .find(|hit| hit.key.source == SearchSource::ProxyHttp && hit.key.id == 2)
            .map(|hit| hit.key)
        {
            self.selected = Some(key);
            self.detail = None;
            self.ensure_detail(&sources);
        }
    }
}

fn build_detail(sources: &SearchSources<'_>, key: SearchKey) -> Option<SearchDetailCache> {
    match key.source {
        SearchSource::ProxyHttp => {
            let item = sources.history.get(key.id).cloned().or_else(|| {
                sources
                    .snapshot
                    .as_ref()
                    .and_then(|snapshot| snapshot.get_http(key.id).ok().flatten())
            })?;
            Some(SearchDetailCache {
                key,
                request: crate::view::request_views(&item),
                response: crate::view::response_views(&item),
                has_request: true,
                has_response: item.response.is_some(),
                single_message: false,
                inspector: Some(InspectorModel::from_history(&item)),
                status: item.status(),
                length: item.transferred(),
                duration: item.duration(),
                number: Some(item.number),
            })
        }
        SearchSource::ProxyWebSocket => {
            let item = sources.websocket_history.get(key.id).cloned().or_else(|| {
                sources
                    .snapshot
                    .as_ref()
                    .and_then(|snapshot| snapshot.get_ws(key.id).ok().flatten())
            })?;
            let views = crate::view::websocket_message_views(&item);
            Some(SearchDetailCache {
                key,
                request: MessageViews::default(),
                response: views,
                has_request: false,
                has_response: true,
                single_message: true,
                inspector: None,
                status: None,
                length: item.payload.total_len(),
                duration: None,
                number: Some(item.number),
            })
        }
        SearchSource::Repeater => {
            let tab = sources
                .repeater_tabs
                .iter()
                .find(|tab| tab.id() == key.id)?;
            let response = tab.response_views().cloned();
            Some(SearchDetailCache {
                key,
                request: engine::request_views_from_raw(tab.request()),
                has_request: true,
                has_response: response.is_some(),
                response: response.unwrap_or_default(),
                single_message: false,
                inspector: None,
                status: tab.response_status(),
                length: tab
                    .response_size()
                    .max(u64::try_from(tab.request().len()).unwrap_or(u64::MAX)),
                duration: tab.response_duration(),
                number: None,
            })
        }
        SearchSource::Intruder => {
            let detail = sources.intruder.search_detail(key.id)?;
            Some(SearchDetailCache {
                key,
                has_request: detail.request.is_some(),
                request: detail.request.unwrap_or_default(),
                has_response: detail.response.is_some(),
                response: detail.response.unwrap_or_default(),
                single_message: false,
                inspector: None,
                status: detail.status,
                length: detail.length,
                duration: detail.duration,
                number: Some(detail.sequence),
            })
        }
    }
}

impl PerseidApp {
    pub(super) fn search_window(&mut self, ctx: &egui::Context) {
        let Self {
            search,
            history,
            websocket_history,
            scope,
            repeater_tabs,
            intruder,
            project,
            inspector,
            ..
        } = self;
        let snapshot = project
            .handle()
            .and_then(|handle| handle.database_snapshot());
        let sources = SearchSources {
            history,
            websocket_history,
            repeater_tabs,
            intruder,
            snapshot,
        };
        search.show(ctx, &sources, scope, inspector);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::time::{Duration, SystemTime};

    use crate::history::{History, Phase};
    use crate::project::{ProjectHandle, SearchResultWorkspace, SearchSourceRecord};
    use crate::proxy::{
        CapturedBody, ExchangeEnd, ExchangeResult, ProxyEvent, RequestInfo, ResponseHead,
        WebSocketConnectionInfo, WebSocketDirection, WebSocketOpcode,
    };

    use super::*;

    fn views() -> MessageViews {
        MessageViews {
            pretty: "pretty".to_owned(),
            raw: "raw".to_owned(),
            hex: "hex".to_owned(),
        }
    }

    fn history_item(id: u64, host: &str) -> crate::history::HistoryItem {
        crate::history::HistoryItem {
            id,
            number: id,
            timestamp: SystemTime::UNIX_EPOCH,
            request: RequestInfo {
                method: "GET".to_owned(),
                uri: format!("https://{host}/search"),
                scheme: "https".to_owned(),
                host: host.to_owned(),
                path: "/search".to_owned(),
                version: "HTTP/1.1".to_owned(),
                headers: Vec::new(),
            },
            request_body: CapturedBody::default(),
            response: None,
            phase: Phase::InFlight,
        }
    }

    fn empty_history() -> History {
        History::new(10, 1024 * 1024)
    }

    fn sources<'a>(
        history: &'a History,
        websocket_history: &'a WebSocketHistory,
        repeater_tabs: &'a [RepeaterTab],
        intruder: &'a IntruderState,
    ) -> SearchSources<'a> {
        SearchSources {
            history,
            websocket_history,
            repeater_tabs,
            intruder,
            snapshot: None,
        }
    }

    fn search_state(query: &str) -> SearchState {
        SearchState {
            query: query.to_owned(),
            ..SearchState::default()
        }
    }

    #[test]
    fn toggle_manages_open_state_and_query_focus() {
        let mut state = SearchState::default();
        assert!(!state.open);
        assert!(!state.focus_query);

        state.toggle();
        assert!(state.open);
        assert!(state.focus_query, "opening focuses the query");

        state.focus_query = false;
        state.toggle();
        assert!(!state.open, "a second toggle closes the window");
        assert!(!state.focus_query);

        state.focus_or_open();
        assert!(state.open);
        assert!(state.focus_query);

        state.focus_query = false;
        state.focus_or_open();
        assert!(state.open, "focus_or_open keeps an open window open");
        assert!(state.focus_query, "the shortcut refocuses the query");
    }

    #[test]
    fn clear_history_resets_results_and_detail() {
        let key = SearchKey {
            source: SearchSource::ProxyHttp,
            id: 2,
        };
        let mut state = SearchState {
            open: true,
            query: "needle".to_owned(),
            results: vec![SearchHit::text(
                key,
                Some(2),
                None,
                "https://example.com",
                "/",
                Some(200),
                4,
            )],
            keys: HashSet::from([key]),
            selected: Some(key),
            detail: Some(SearchDetailCache {
                key,
                request: views(),
                response: views(),
                has_request: true,
                has_response: true,
                single_message: false,
                inspector: Some(InspectorModel {
                    request: Vec::new(),
                    response: Vec::new(),
                }),
                status: Some(200),
                length: 4,
                duration: None,
                number: Some(2),
            }),
            matcher: Some(SearchMatcher::new("needle", false, false).expect("valid matcher")),
            error: Some("boom".to_owned()),
            ran: true,
            dirty: true,
            last_history_revision: Some(7),
            last_websocket_revision: Some(4),
            ..Default::default()
        };

        state.clear_history();

        assert!(state.results.is_empty());
        assert!(state.keys.is_empty());
        assert_eq!(state.selected, None);
        assert!(state.detail.is_none());
        assert!(state.matcher.is_none());
        assert!(state.error.is_none());
        assert!(!state.ran);
        assert!(!state.dirty);
        assert_eq!(state.last_history_revision, None);
        assert_eq!(state.last_websocket_revision, None);
        // Clearing captured history does not dismiss the window or the query.
        assert!(state.open);
        assert_eq!(state.query, "needle");
    }

    #[test]
    fn clear_source_drops_only_that_tool_and_picks_a_new_selection() {
        let http = SearchKey {
            source: SearchSource::ProxyHttp,
            id: 1,
        };
        let repeater = SearchKey {
            source: SearchSource::Repeater,
            id: 1,
        };
        let mut state = SearchState {
            results: vec![
                SearchHit::text(http, None, None, "a", "/", None, 0),
                SearchHit::text(repeater, None, None, "b", "/", None, 0),
            ],
            keys: HashSet::from([http, repeater]),
            selected: Some(http),
            ..Default::default()
        };

        state.clear_source(SearchSource::ProxyHttp);
        assert_eq!(state.results.len(), 1);
        assert_eq!(state.selected, Some(repeater));
        assert!(state.keys.contains(&repeater));
    }

    #[test]
    fn run_searches_loaded_http_websocket_and_repeater_sources() {
        let mut history = empty_history();
        history.load(vec![history_item(1, "example.com")]);

        let mut websocket_history = WebSocketHistory::new(10, 1024);
        websocket_history.apply(ProxyEvent::WebSocketMessage {
            id: 8,
            timestamp: SystemTime::UNIX_EPOCH,
            connection: WebSocketConnectionInfo {
                id: 1,
                url: "wss://socket.example/search".to_owned(),
                host: "socket.example".to_owned(),
                tls: true,
                listener_port: 8080,
                protocol: None,
            },
            direction: WebSocketDirection::ToClient,
            opcode: WebSocketOpcode::Text,
            payload: CapturedBody::from_bytes(b"needle in a frame".to_vec()),
        });

        let tab = RepeaterTab::from_history(3, &history_item(3, "repeater.example"));
        let tabs = [tab];
        let intruder = IntruderState::new();
        let sources = sources(&history, &websocket_history, &tabs, &intruder);

        // The loaded HTTP item has no matching body; the Repeater request and
        // WebSocket URL/payload are matches. Disable HTTP so the assertion
        // focuses on the non-HTTP sources.
        let mut state = search_state("search");
        state.options.proxy_history = false;
        state.run(&sources, &BTreeSet::new());

        let found: Vec<_> = state.results.iter().map(|hit| hit.key.source).collect();
        assert_eq!(
            found,
            vec![SearchSource::ProxyWebSocket, SearchSource::Repeater]
        );
        assert!(
            !state.running,
            "no stored scan is started without a snapshot"
        );
        assert!(state.ran);
        assert_eq!(state.selected, state.results.first().map(|hit| hit.key));
        assert!(state.detail.is_none());
        state.ensure_detail(&sources);
        assert_eq!(
            state.detail.as_ref().map(|detail| detail.key),
            state.selected
        );
    }

    #[test]
    fn run_requires_a_tool_and_a_location() {
        let history = empty_history();
        let websocket_history = WebSocketHistory::new(10, 1024);
        let intruder = IntruderState::new();
        let sources = sources(&history, &websocket_history, &[], &intruder);

        let mut state = search_state("needle");
        state.options.proxy_history = false;
        state.options.websockets = false;
        state.options.repeater = false;
        state.options.intruder = false;
        state.run(&sources, &BTreeSet::new());
        assert_eq!(state.error.as_deref(), Some("Select at least one tool."));

        let mut state = search_state("needle");
        state.options.request_headers = false;
        state.options.request_body = false;
        state.options.response_headers = false;
        state.options.response_body = false;
        state.run(&sources, &BTreeSet::new());
        assert_eq!(
            state.error.as_deref(),
            Some("Select at least one location.")
        );
    }

    #[test]
    fn result_cap_dedupes_and_marks_the_limited_scan() {
        let key = |id| SearchKey {
            source: SearchSource::ProxyHttp,
            id,
        };
        let hit = |id| SearchHit::text(key(id), None, None, "example.com", "/", None, 0);
        let mut state = SearchState::default();
        for id in 0..SEARCH_RESULT_LIMIT as u64 {
            state.push_hit(hit(id));
        }
        assert_eq!(state.results.len(), SEARCH_RESULT_LIMIT);
        assert_eq!(state.keys.len(), SEARCH_RESULT_LIMIT);
        assert!(!state.limit_reached);

        state.push_hit(hit(0));
        assert_eq!(state.results.len(), SEARCH_RESULT_LIMIT);
        assert!(
            !state.limit_reached,
            "a duplicate at the cap is not a new result"
        );

        state.push_hit(hit(SEARCH_RESULT_LIMIT as u64));
        assert_eq!(state.results.len(), SEARCH_RESULT_LIMIT);
        assert!(
            state.limit_reached,
            "a new result beyond the cap sets the notice"
        );
    }

    #[test]
    fn source_records_round_trip_every_tool() {
        for source in [
            SearchSource::ProxyHttp,
            SearchSource::ProxyWebSocket,
            SearchSource::Repeater,
            SearchSource::Intruder,
        ] {
            let record: SearchSourceRecord = source.into();
            assert_eq!(SearchSource::from(record), source);
        }
    }

    #[test]
    fn sorting_groups_sources_and_orders_timed_and_untimed_rows() {
        let hit = |source, id, timestamp| {
            SearchHit::text(SearchKey { source, id }, None, timestamp, "h", "u", None, 0)
        };
        let at = |seconds| Some(SystemTime::UNIX_EPOCH + Duration::from_secs(seconds));
        let mut state = SearchState {
            results: vec![
                hit(SearchSource::Intruder, 1, None),
                hit(SearchSource::Repeater, 2, None),
                hit(SearchSource::Repeater, 1, None),
                hit(SearchSource::ProxyWebSocket, 1, at(5)),
                hit(SearchSource::ProxyHttp, 2, at(20)),
                hit(SearchSource::ProxyHttp, 1, at(10)),
            ],
            ..Default::default()
        };

        state.sort_results();

        assert_eq!(
            state
                .results
                .iter()
                .map(|hit| (hit.key.source, hit.key.id))
                .collect::<Vec<_>>(),
            vec![
                (SearchSource::ProxyHttp, 1),
                (SearchSource::ProxyHttp, 2),
                (SearchSource::ProxyWebSocket, 1),
                (SearchSource::Repeater, 1),
                (SearchSource::Repeater, 2),
                (SearchSource::Intruder, 1),
            ]
        );
    }

    #[test]
    fn clear_source_wrappers_only_drop_their_own_tool() {
        let key = |source, id| SearchKey { source, id };
        let hit = |key| SearchHit::text(key, None, None, "h", "u", None, 0);
        let http = key(SearchSource::ProxyHttp, 1);
        let ws = key(SearchSource::ProxyWebSocket, 1);
        let repeater = key(SearchSource::Repeater, 1);
        let mut state = SearchState {
            results: vec![hit(http), hit(ws), hit(repeater)],
            keys: HashSet::from([http, ws, repeater]),
            selected: Some(http),
            ..Default::default()
        };

        state.clear_http_source();
        assert_eq!(state.selected, Some(ws));
        state.clear_websocket_source();
        assert_eq!(state.selected, Some(repeater));
        assert_eq!(state.results.len(), 1);
        assert!(state.keys.contains(&repeater));
    }

    #[test]
    fn run_rejects_invalid_regex_and_empty_queries_without_results() {
        let history = empty_history();
        let websocket_history = WebSocketHistory::new(10, 1024);
        let intruder = IntruderState::new();
        let sources = sources(&history, &websocket_history, &[], &intruder);

        let mut state = search_state("(");
        state.options.regex = true;
        state.run(&sources, &BTreeSet::new());
        assert!(
            state
                .error
                .as_deref()
                .is_some_and(|error| error.starts_with("Invalid regex:"))
        );
        assert!(!state.ran);

        let mut state = search_state("");
        state.run(&sources, &BTreeSet::new());
        assert!(state.error.is_none());
        assert!(!state.ran);
        assert!(state.results.is_empty());
    }

    #[test]
    fn workspace_application_caps_oversized_result_lists() {
        let history = empty_history();
        let websocket_history = WebSocketHistory::new(10, 1024);
        let workspace = SearchWorkspace {
            ran: true,
            result_items: (0..=SEARCH_RESULT_LIMIT as u64)
                .map(|id| SearchResultWorkspace {
                    id,
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        };

        let mut state = SearchState::default();
        state.apply_workspace(&workspace, &history, &websocket_history);

        assert_eq!(state.results.len(), SEARCH_RESULT_LIMIT);
        assert_eq!(state.keys.len(), SEARCH_RESULT_LIMIT);
        assert!(state.limit_reached);
    }

    #[test]
    fn repeater_and_missing_details_are_reported_from_the_workspace() {
        let history = empty_history();
        let websocket_history = WebSocketHistory::new(10, 1024);
        let tab = RepeaterTab::from_history(3, &history_item(3, "repeater.example"));
        let tabs = [tab];
        let intruder = IntruderState::new();
        let sources = sources(&history, &websocket_history, &tabs, &intruder);

        let mut state = SearchState::default();
        let key = SearchKey {
            source: SearchSource::Repeater,
            id: 3,
        };
        state.selected = Some(key);
        state.keys.insert(key);
        state.ensure_detail(&sources);
        let detail = state.detail.as_ref().expect("Repeater detail");
        assert!(detail.has_request);
        assert!(!detail.has_response);
        assert!(!detail.single_message);

        let missing = SearchKey {
            source: SearchSource::Repeater,
            id: 99,
        };
        state.selected = Some(missing);
        state.keys.insert(missing);
        state.detail = None;
        state.ensure_detail(&sources);
        assert!(state.detail.is_none());
    }

    #[test]
    fn workspace_round_trips_structured_results_and_legacy_ids() {
        let http_key = SearchKey {
            source: SearchSource::ProxyHttp,
            id: 11,
        };
        let ws_key = SearchKey {
            source: SearchSource::ProxyWebSocket,
            id: 12,
        };
        let state = SearchState {
            open: true,
            query: "needle".to_owned(),
            ran: true,
            results: vec![
                SearchHit::text(
                    http_key,
                    Some(4),
                    Some(SystemTime::UNIX_EPOCH + Duration::from_millis(5)),
                    "https://example.com",
                    "/a",
                    Some(200),
                    10,
                ),
                SearchHit::text(
                    ws_key,
                    Some(5),
                    Some(SystemTime::UNIX_EPOCH),
                    "wss://socket.example",
                    "/socket",
                    None,
                    7,
                ),
            ],
            keys: HashSet::from([http_key, ws_key]),
            selected: Some(ws_key),
            limit_reached: true,
            ..Default::default()
        };

        let workspace = state.workspace();
        assert_eq!(workspace.results, vec![11]);
        assert_eq!(workspace.selected, None);
        assert_eq!(workspace.result_items.len(), 2);
        assert_eq!(
            workspace.selected_item.as_ref().map(|item| item.source),
            Some(SearchSourceRecord::ProxyWebSocket)
        );
        assert!(workspace.limit_reached);

        let history = empty_history();
        let websocket_history = WebSocketHistory::new(10, 1024);
        let mut restored = SearchState::default();
        restored.apply_workspace(&workspace, &history, &websocket_history);
        assert_eq!(restored.results, state.results);
        assert_eq!(restored.selected, Some(ws_key));
        assert_eq!(restored.workspace(), workspace);
    }

    #[test]
    fn legacy_workspace_results_are_restored_from_loaded_history() {
        let mut history = empty_history();
        history.load(vec![history_item(7, "legacy.example")]);
        let websocket_history = WebSocketHistory::new(10, 1024);
        let workspace = SearchWorkspace {
            ran: true,
            results: vec![7, 99],
            selected: Some(7),
            ..Default::default()
        };

        let mut state = SearchState::default();
        state.apply_workspace(&workspace, &history, &websocket_history);
        assert_eq!(state.results.len(), 1);
        assert_eq!(state.results[0].key.id, 7);
        assert_eq!(state.results[0].key.source, SearchSource::ProxyHttp);
        assert_eq!(state.selected, Some(state.results[0].key));
    }

    #[test]
    fn stored_scan_streams_hits_and_completes() {
        let project = ProjectHandle::temporary().expect("temporary project");
        seed_project_exchange(&project, 1, "stored.example", b"needle");
        let history = empty_history();
        let websocket_history = WebSocketHistory::new(10, 1024);
        let intruder = IntruderState::new();
        let snapshot = project.database_snapshot();
        let sources = SearchSources {
            history: &history,
            websocket_history: &websocket_history,
            repeater_tabs: &[],
            intruder: &intruder,
            snapshot,
        };

        let mut state = search_state("needle");
        state.options.websockets = false;
        state.options.repeater = false;
        state.options.intruder = false;
        state.run(&sources, &BTreeSet::new());
        assert!(state.running);

        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while state.running && std::time::Instant::now() < deadline {
            state.poll();
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(!state.running, "stored scan should finish");
        assert_eq!(state.results.len(), 1);
        assert_eq!(state.results[0].key.source, SearchSource::ProxyHttp);
        assert_eq!(state.results[0].key.id, 1);
        state.ensure_detail(&sources);
        assert!(
            state.detail.is_some(),
            "detail is fetched from the project store"
        );
    }

    #[test]
    fn stored_websocket_scan_loads_message_detail() {
        let project = ProjectHandle::temporary().expect("temporary project");
        let id = seed_project_ws(
            &project,
            "wss://socket.example/chat",
            b"hello from the socket",
        );
        let history = empty_history();
        let websocket_history = WebSocketHistory::new(10, 1024);
        let intruder = IntruderState::new();
        let snapshot = project.database_snapshot();
        let sources = SearchSources {
            history: &history,
            websocket_history: &websocket_history,
            repeater_tabs: &[],
            intruder: &intruder,
            snapshot,
        };

        let mut state = search_state("socket");
        state.options.proxy_history = false;
        state.options.repeater = false;
        state.options.intruder = false;
        state.run(&sources, &BTreeSet::new());
        assert!(state.running);

        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while state.running && std::time::Instant::now() < deadline {
            state.poll();
            std::thread::sleep(Duration::from_millis(5));
        }

        assert!(!state.running, "stored WebSocket scan should finish");
        assert_eq!(state.results.len(), 1);
        assert_eq!(state.results[0].key.source, SearchSource::ProxyWebSocket);
        assert_eq!(state.results[0].key.id, id);
        state.ensure_detail(&sources);
        let detail = state.detail.as_ref().expect("WebSocket detail");
        assert!(detail.single_message);
        assert!(detail.has_response);
        assert!(!detail.has_request);
        assert!(detail.response.raw.contains("hello from the socket"));
    }

    fn seed_project_exchange(project: &ProjectHandle, id: u64, host: &str, response_body: &[u8]) {
        let timestamp = SystemTime::UNIX_EPOCH + Duration::from_secs(id);
        project.capture(&ProxyEvent::Started {
            id,
            timestamp,
            request: RequestInfo {
                method: "GET".to_owned(),
                uri: format!("https://{host}/search"),
                scheme: "https".to_owned(),
                host: host.to_owned(),
                path: "/search".to_owned(),
                version: "HTTP/1.1".to_owned(),
                headers: vec![("host".to_owned(), host.to_owned())],
            },
        });
        project.capture(&ProxyEvent::ResponseHead {
            id,
            head: ResponseHead {
                version: "HTTP/1.1".to_owned(),
                status: 200,
                reason: "OK".to_owned(),
                headers: vec![("content-type".to_owned(), "text/plain".to_owned())],
            },
        });
        project.capture(&ProxyEvent::Finished {
            id,
            end: ExchangeEnd {
                duration: Duration::from_millis(2),
                request_body: CapturedBody::default(),
                result: ExchangeResult::Response {
                    body: CapturedBody::from_bytes(response_body.to_vec()),
                },
            },
        });
        let receipt = project.flush();
        receipt
            .recv_timeout(Duration::from_secs(5))
            .expect("flush reply")
            .expect("flush project");
    }

    fn seed_project_ws(project: &ProjectHandle, url: &str, payload: &[u8]) -> u64 {
        let host = url
            .split_once("://")
            .map_or(url, |(_, rest)| rest)
            .split('/')
            .next()
            .unwrap_or_default()
            .to_owned();
        let event = ProxyEvent::WebSocketMessage {
            id: 1,
            timestamp: SystemTime::UNIX_EPOCH + Duration::from_secs(1),
            connection: WebSocketConnectionInfo {
                id: 1,
                url: url.to_owned(),
                host,
                tls: true,
                listener_port: 8080,
                protocol: None,
            },
            direction: WebSocketDirection::ToClient,
            opcode: WebSocketOpcode::Text,
            payload: CapturedBody::from_bytes(payload.to_vec()),
        };
        let id = project
            .capture(&event)
            .event
            .expect("captured WebSocket message")
            .id();
        let receipt = project.flush();
        receipt
            .recv_timeout(Duration::from_secs(5))
            .expect("flush reply")
            .expect("flush project");
        id
    }

    #[test]
    fn structured_result_conversions_keep_every_field() {
        let hit = SearchHit::text(
            SearchKey {
                source: SearchSource::Intruder,
                id: 9,
            },
            Some(2),
            Some(SystemTime::UNIX_EPOCH + Duration::from_millis(44)),
            "https://example.com",
            "/login · admin",
            Some(302),
            12,
        );
        let workspace = SearchResultWorkspace::from(&hit);
        assert_eq!(workspace.source, SearchSourceRecord::Intruder);
        assert_eq!(workspace.timestamp_ms, Some(44));
        assert_eq!(SearchHit::from(&workspace), hit);
    }
}
