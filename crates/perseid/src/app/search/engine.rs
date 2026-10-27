//! Query compilation, message matching, and background scans for Search.
//!
//! Everything here is independent of egui so the matching rules can be tested
//! without a UI context. The window layout lives in [`super::view`].

use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::SystemTime;

use regex::bytes::{Regex, RegexBuilder};

use crate::history::{History, HistoryItem};
use crate::project::ProjectSnapshot;
use crate::proxy::RepeaterResult;
use crate::site_map;
use crate::view::{self, MessageViews};
use crate::websocket_history::{WebSocketHistory, WebSocketMessageItem};

/// Upper bound on highlight ranges produced for one message. A one-character
/// search in a large body would otherwise build an unusable layout job; the UI
/// reports the count as a lower bound when the cap is reached.
pub(super) const MATCH_HIGHLIGHT_LIMIT: usize = 5_000;

/// Maximum rows retained for one search. Stored scans stop at this cap so a
/// broad query cannot monopolize memory or the saved workspace.
pub(super) const SEARCH_RESULT_LIMIT: usize = 10_000;

/// How often a stored scan reports progress to the UI.
const PROGRESS_STEP: usize = 128;

/// Which parts of an exchange a search examines.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) struct SearchOptions {
    pub(super) case_sensitive: bool,
    pub(super) regex: bool,
    pub(super) negative: bool,
    pub(super) in_scope_only: bool,
    pub(super) dynamic_update: bool,
    pub(super) proxy_history: bool,
    pub(super) websockets: bool,
    pub(super) repeater: bool,
    pub(super) intruder: bool,
    pub(super) request_headers: bool,
    pub(super) request_body: bool,
    pub(super) response_headers: bool,
    pub(super) response_body: bool,
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            case_sensitive: false,
            regex: false,
            negative: false,
            in_scope_only: false,
            dynamic_update: false,
            proxy_history: true,
            websockets: true,
            repeater: true,
            intruder: true,
            request_headers: true,
            request_body: true,
            response_headers: true,
            response_body: true,
        }
    }
}

impl SearchOptions {
    pub(super) fn any_location(self) -> bool {
        self.request_headers || self.request_body || self.response_headers || self.response_body
    }

    pub(super) fn any_tool(self) -> bool {
        self.proxy_history || self.websockets || self.repeater || self.intruder
    }
}

/// Which workbench surface produced a result.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(in crate::app) enum SearchSource {
    ProxyHttp,
    ProxyWebSocket,
    Repeater,
    Intruder,
}

impl SearchSource {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::ProxyHttp => "Proxy",
            Self::ProxyWebSocket => "WebSockets",
            Self::Repeater => "Repeater",
            Self::Intruder => "Intruder",
        }
    }

    pub(super) fn rank(self) -> u8 {
        match self {
            Self::ProxyHttp => 0,
            Self::ProxyWebSocket => 1,
            Self::Repeater => 2,
            Self::Intruder => 3,
        }
    }
}

/// A stable result reference. IDs are only unique within their source, so the
/// source is part of every selection and detail-cache key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(in crate::app) struct SearchKey {
    pub(super) source: SearchSource,
    pub(super) id: u64,
}

impl SearchKey {
    /// Stable egui state ID that cannot collide when a history ID and a
    /// Repeater or Intruder ID are both present.
    pub(super) fn cache_id(self) -> u64 {
        let source = match self.source {
            SearchSource::ProxyHttp => 1,
            SearchSource::ProxyWebSocket => 2,
            SearchSource::Repeater => 3,
            SearchSource::Intruder => 4,
        };
        (self.id & 0x00FF_FFFF_FFFF_FFFF) | (source << 56)
    }
}

/// One row in the results table. The display fields are a snapshot so stored
/// results remain usable when the full message is not in the UI cache.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::app) struct SearchHit {
    pub(super) key: SearchKey,
    pub(super) number: Option<u64>,
    pub(super) timestamp: Option<SystemTime>,
    pub(super) host: String,
    pub(super) url: String,
    pub(super) status: Option<u16>,
    pub(super) length: u64,
}

impl SearchHit {
    pub(super) fn text(
        key: SearchKey,
        number: Option<u64>,
        timestamp: Option<SystemTime>,
        host: impl Into<String>,
        url: impl Into<String>,
        status: Option<u16>,
        length: u64,
    ) -> Self {
        Self {
            key,
            number,
            timestamp,
            host: host.into(),
            url: url.into(),
            status,
            length,
        }
    }
}

/// A compiled query. Negative matching is applied per message; the matcher
/// itself only answers "does this text contain a match?".
#[derive(Clone)]
pub(in crate::app) struct SearchMatcher {
    regex: Regex,
    negative: bool,
}

impl SearchMatcher {
    pub(super) fn new(query: &str, case_sensitive: bool, regex: bool) -> Result<Self, String> {
        let pattern = if regex {
            query.to_owned()
        } else {
            regex::escape(query)
        };
        let regex = RegexBuilder::new(&pattern)
            .case_insensitive(!case_sensitive)
            .build()
            .map_err(|err| format!("Invalid regex: {err}"))?;
        Ok(Self {
            regex,
            negative: false,
        })
    }

    pub(super) fn with_negative(mut self, negative: bool) -> Self {
        self.negative = negative;
        self
    }

    fn is_match(&self, haystack: &[u8]) -> bool {
        self.regex.is_match(haystack)
    }

    /// Match ranges for highlighting the text of the selected message.
    ///
    /// Regex byte matches are filtered to UTF-8 boundaries because the message
    /// editor hands the ranges back to egui as `str` offsets. The cap prevents
    /// a one-character search in a large body from producing an unusable
    /// layout job; the UI reports that the count is a lower bound.
    pub(super) fn ranges(&self, text: &str) -> Vec<std::ops::Range<usize>> {
        if self.negative {
            return Vec::new();
        }
        self.regex
            .find_iter(text.as_bytes())
            .take(MATCH_HIGHLIGHT_LIMIT + 1)
            .map(|found| found.start()..found.end())
            .filter(|range| text.is_char_boundary(range.start) && text.is_char_boundary(range.end))
            .take(MATCH_HIGHLIGHT_LIMIT)
            .collect()
    }
}

/// Whether a host passes the Target scope filter.
pub(super) fn host_in_scope(options: SearchOptions, scope: &BTreeSet<String>, host: &str) -> bool {
    !options.in_scope_only || site_map::host_in_scope(host, scope)
}

/// Match a captured HTTP exchange against the selected locations.
pub(super) fn matches_http(
    matcher: &SearchMatcher,
    options: SearchOptions,
    item: &HistoryItem,
) -> bool {
    let mut matched = false;
    if options.request_headers && matcher.is_match(view::request_head(item).as_bytes()) {
        matched = true;
    }
    if !matched && options.request_body && matcher.is_match(&view::request_body_for_search(item)) {
        matched = true;
    }
    if !matched
        && options.response_headers
        && matcher.is_match(view::response_head(item).as_bytes())
    {
        matched = true;
    }
    if !matched && options.response_body && matcher.is_match(&view::response_body_for_search(item))
    {
        matched = true;
    }
    matched != options.negative
}

/// Match a captured WebSocket message.
///
/// Connection metadata (the URL, host, and protocol) is treated as a header
/// location. The payload is a body location, selected by either body checkbox
/// because a WebSocket message has no separate request and response sides.
pub(super) fn matches_ws(
    matcher: &SearchMatcher,
    options: SearchOptions,
    item: &WebSocketMessageItem,
) -> bool {
    let mut matched = false;
    if (options.request_headers || options.response_headers)
        && matcher.is_match(ws_connection_text(item).as_bytes())
    {
        matched = true;
    }
    if !matched
        && (options.request_body || options.response_body)
        && matcher.is_match(item.payload.data())
    {
        matched = true;
    }
    matched != options.negative
}

fn ws_connection_text(item: &WebSocketMessageItem) -> String {
    let mut text = item.connection.url.clone();
    text.push('\n');
    text.push_str(&item.connection.host);
    text.push('\n');
    if let Some(protocol) = &item.connection.protocol {
        text.push_str(protocol);
    }
    text
}

/// Match a raw request and optional raw response from Repeater or Intruder.
///
/// The raw strings use the same representation as the message editor, so a
/// decoded response body is searched just like a captured HTTP response.
pub(super) fn matches_document(
    matcher: &SearchMatcher,
    options: SearchOptions,
    request: &str,
    response: Option<&str>,
) -> bool {
    let (request_head, request_body) = split_message(request);
    let mut matched = false;
    if options.request_headers && matcher.is_match(request_head.as_bytes()) {
        matched = true;
    }
    if !matched && options.request_body && matcher.is_match(request_body) {
        matched = true;
    }
    if !matched {
        if let Some(response) = response {
            let (response_head, response_body) = split_message(response);
            if options.response_headers && matcher.is_match(response_head.as_bytes()) {
                matched = true;
            }
            if !matched && options.response_body && matcher.is_match(response_body) {
                matched = true;
            }
        } else if options.negative {
            // A missing side contributes no match. This matters for negative
            // searches: a failed Repeater send should still be listed when
            // the request matches none of the selected locations.
        }
    }
    matched != options.negative
}

/// Split an editor-style raw message into its head and body.
pub(super) fn split_message(raw: &str) -> (&str, &[u8]) {
    for delimiter in ["\r\n\r\n", "\n\n"] {
        if let Some(index) = raw.find(delimiter) {
            return (&raw[..index], &raw.as_bytes()[index + delimiter.len()..]);
        }
    }
    (raw, &[])
}

/// Build editor views for a raw request held by Repeater or Intruder.
pub(super) fn request_views_from_raw(raw: &str) -> MessageViews {
    let (head, body) = split_message(raw);
    view::editor_views(head, body)
}

/// The response as the message editor renders it, or `None` when the send
/// failed before a response head arrived.
pub(super) fn repeater_response_raw(result: &RepeaterResult) -> Option<String> {
    match result {
        RepeaterResult::Response {
            version,
            status,
            reason,
            headers,
            body,
            ..
        } => Some(view::response_views_from_parts(version, *status, reason, headers, body).raw),
        RepeaterResult::Failed { .. } => None,
    }
}

pub(super) fn repeater_response_status(result: &RepeaterResult) -> Option<u16> {
    match result {
        RepeaterResult::Response { status, .. } => Some(*status),
        RepeaterResult::Failed { .. } => None,
    }
}

pub(super) fn repeater_response_length(result: &RepeaterResult) -> u64 {
    match result {
        RepeaterResult::Response { body, .. } => body.total_len(),
        RepeaterResult::Failed { .. } => 0,
    }
}

/// Extract the origin-form target from a raw request line.
pub(super) fn request_target(raw: &str) -> String {
    raw.lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .filter(|target| !target.is_empty())
        .unwrap_or("/")
        .to_owned()
}

pub(super) fn http_hit(item: &HistoryItem) -> SearchHit {
    SearchHit::text(
        SearchKey {
            source: SearchSource::ProxyHttp,
            id: item.id,
        },
        Some(item.number),
        Some(item.timestamp),
        result_host(item),
        result_url(item),
        item.status(),
        item.transferred(),
    )
}

pub(super) fn ws_hit(item: &WebSocketMessageItem) -> SearchHit {
    let scheme = if item.connection.tls { "wss" } else { "ws" };
    let host = if item.connection.host.is_empty() {
        item.connection.url.clone()
    } else {
        format!("{scheme}://{}", item.connection.host)
    };
    let url = if item.connection.url.is_empty() {
        format!(
            "{} {}",
            view::websocket_direction_text(item.direction),
            view::websocket_opcode_text(item.opcode)
        )
    } else {
        item.connection.url.clone()
    };
    SearchHit::text(
        SearchKey {
            source: SearchSource::ProxyWebSocket,
            id: item.id,
        },
        Some(item.number),
        Some(item.timestamp),
        host,
        url,
        None,
        item.payload.total_len(),
    )
}

pub(super) fn result_host(item: &HistoryItem) -> String {
    let scheme = if item.request.scheme.is_empty() {
        "http"
    } else {
        &item.request.scheme
    };
    format!("{scheme}://{}", item.request.host)
}

pub(super) fn result_url(item: &HistoryItem) -> String {
    if item.request.path.is_empty() {
        "/".to_owned()
    } else {
        item.request.path.clone()
    }
}

/// Progress for a stored-history scan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::app) struct SearchProgress {
    pub(super) source: SearchSource,
    pub(super) scanned: usize,
    pub(super) total: usize,
}

/// Messages produced by the background scan.
pub(super) enum SearchJobEvent {
    Progress(SearchProgress),
    Hit(SearchHit),
    Finished(SearchSource),
    LimitReached,
    Error(String),
    Done,
}

/// A running stored-history scan. Dropping it cancels the worker.
pub(super) struct SearchJob {
    pub(super) events: Receiver<SearchJobEvent>,
    cancel: Arc<AtomicBool>,
}

impl SearchJob {
    pub(super) fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

impl Drop for SearchJob {
    fn drop(&mut self) {
        self.cancel();
    }
}

/// Start scanning the persisted HTTP and WebSocket rows.
///
/// Repeater and Intruder are searched synchronously by the caller because they
/// are already bounded in-memory collections.
pub(super) fn start_scan(
    snapshot: ProjectSnapshot,
    matcher: SearchMatcher,
    options: SearchOptions,
    scope: BTreeSet<String>,
    remaining: usize,
) -> Result<SearchJob, String> {
    let (tx, events) = mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let worker_cancel = cancel.clone();
    thread::Builder::new()
        .name("perseid-search".to_owned())
        .spawn(move || {
            run_scan(
                &snapshot,
                &matcher,
                options,
                &scope,
                remaining,
                &tx,
                &worker_cancel,
            );
        })
        .map_err(|err| format!("Could not start the search worker: {err}"))?;
    Ok(SearchJob { events, cancel })
}

fn run_scan(
    snapshot: &ProjectSnapshot,
    matcher: &SearchMatcher,
    options: SearchOptions,
    scope: &BTreeSet<String>,
    limit: usize,
    tx: &Sender<SearchJobEvent>,
    cancel: &AtomicBool,
) {
    let mut hits = 0usize;
    let mut truncated = false;

    if options.proxy_history {
        let result = snapshot.scan_http(|item, scanned, total| {
            if cancel.load(Ordering::Relaxed) {
                return false;
            }
            progress(tx, SearchSource::ProxyHttp, scanned, total);
            if host_in_scope(options, scope, &item.request.host)
                && matches_http(matcher, options, &item)
            {
                if hits >= limit {
                    truncated = true;
                    return false;
                }
                if tx.send(SearchJobEvent::Hit(http_hit(&item))).is_err() {
                    return false;
                }
                hits += 1;
            }
            true
        });
        report_scan_result(tx, SearchSource::ProxyHttp, result);
        let _ = tx.send(SearchJobEvent::Finished(SearchSource::ProxyHttp));
    }

    if options.websockets && !truncated && !cancel.load(Ordering::Relaxed) {
        let result = snapshot.scan_ws(|item, scanned, total| {
            if cancel.load(Ordering::Relaxed) {
                return false;
            }
            progress(tx, SearchSource::ProxyWebSocket, scanned, total);
            if host_in_scope(options, scope, &item.connection.host)
                && matches_ws(matcher, options, &item)
            {
                if hits >= limit {
                    truncated = true;
                    return false;
                }
                if tx.send(SearchJobEvent::Hit(ws_hit(&item))).is_err() {
                    return false;
                }
                hits += 1;
            }
            true
        });
        report_scan_result(tx, SearchSource::ProxyWebSocket, result);
        let _ = tx.send(SearchJobEvent::Finished(SearchSource::ProxyWebSocket));
    }

    if truncated {
        let _ = tx.send(SearchJobEvent::LimitReached);
    }
    let _ = tx.send(SearchJobEvent::Done);
}

fn progress(tx: &Sender<SearchJobEvent>, source: SearchSource, scanned: usize, total: usize) {
    if scanned == 1 || scanned == total || scanned.is_multiple_of(PROGRESS_STEP) {
        let _ = tx.send(SearchJobEvent::Progress(SearchProgress {
            source,
            scanned,
            total,
        }));
    }
}

fn report_scan_result(
    tx: &Sender<SearchJobEvent>,
    source: SearchSource,
    result: Result<(), crate::project::ProjectError>,
) {
    if let Err(error) = result {
        let _ = tx.send(SearchJobEvent::Error(format!(
            "Could not search stored {}: {error}",
            source.label()
        )));
    }
}

/// Search the loaded HTTP snapshot synchronously.
///
/// This is the fallback when no project database is available, and it keeps
/// the engine testable without opening a project.
pub(super) fn scan_loaded_http(
    history: &History,
    matcher: &SearchMatcher,
    options: SearchOptions,
    scope: &BTreeSet<String>,
) -> Vec<SearchHit> {
    let mut hits = Vec::new();
    for item in history.iter() {
        if host_in_scope(options, scope, &item.request.host) && matches_http(matcher, options, item)
        {
            hits.push(http_hit(item));
            if hits.len() >= SEARCH_RESULT_LIMIT {
                break;
            }
        }
    }
    hits
}

pub(super) fn scan_loaded_ws(
    history: &WebSocketHistory,
    matcher: &SearchMatcher,
    options: SearchOptions,
    scope: &BTreeSet<String>,
) -> Vec<SearchHit> {
    let mut hits = Vec::new();
    for item in history.iter() {
        if host_in_scope(options, scope, &item.connection.host)
            && matches_ws(matcher, options, item)
        {
            hits.push(ws_hit(item));
            if hits.len() >= SEARCH_RESULT_LIMIT {
                break;
            }
        }
    }
    hits
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::time::{Duration, SystemTime};

    use crate::history::Phase;
    use crate::proxy::{
        CapturedBody, ExchangeEnd, ExchangeResult, ProxyEvent, RepeaterResult, RequestInfo,
        ResponseHead, WebSocketConnectionInfo, WebSocketDirection, WebSocketOpcode,
    };

    use super::*;

    fn seed_exchange(
        history: &mut History,
        id: u64,
        host: &str,
        request_body: &[u8],
        response_body: &[u8],
        content_encoding: Option<&str>,
    ) {
        let timestamp = SystemTime::UNIX_EPOCH + Duration::from_secs(id);
        let path = "/search";
        history.apply(ProxyEvent::Started {
            id,
            timestamp,
            request: RequestInfo {
                method: "POST".to_owned(),
                uri: format!("https://{host}{path}"),
                scheme: "https".to_owned(),
                host: host.to_owned(),
                path: path.to_owned(),
                version: "HTTP/1.1".to_owned(),
                headers: vec![
                    ("host".to_owned(), host.to_owned()),
                    ("content-type".to_owned(), "text/plain".to_owned()),
                ],
            },
        });

        let mut response_headers = vec![
            ("content-type".to_owned(), "application/json".to_owned()),
            ("content-length".to_owned(), response_body.len().to_string()),
        ];
        if let Some(encoding) = content_encoding {
            response_headers.push(("content-encoding".to_owned(), encoding.to_owned()));
        }
        history.apply(ProxyEvent::ResponseHead {
            id,
            head: ResponseHead {
                version: "HTTP/1.1".to_owned(),
                status: 200,
                reason: "OK".to_owned(),
                headers: response_headers,
            },
        });
        history.apply(ProxyEvent::Finished {
            id,
            end: ExchangeEnd {
                duration: Duration::from_millis(4),
                request_body: CapturedBody::from_bytes(request_body.to_vec()),
                result: ExchangeResult::Response {
                    body: CapturedBody::from_bytes(response_body.to_vec()),
                },
            },
        });
    }

    fn history_item(host: &str, scheme: &str, path: &str) -> HistoryItem {
        HistoryItem {
            id: 1,
            number: 1,
            timestamp: SystemTime::UNIX_EPOCH,
            request: RequestInfo {
                method: "GET".to_owned(),
                uri: String::new(),
                scheme: scheme.to_owned(),
                host: host.to_owned(),
                path: path.to_owned(),
                version: "HTTP/1.1".to_owned(),
                headers: Vec::new(),
            },
            request_body: CapturedBody::default(),
            response: None,
            phase: Phase::InFlight,
        }
    }

    fn ws_item(host: &str, payload: &[u8]) -> WebSocketMessageItem {
        WebSocketMessageItem {
            id: 4,
            number: 7,
            timestamp: SystemTime::UNIX_EPOCH,
            connection: WebSocketConnectionInfo {
                id: 2,
                url: format!("wss://{host}/socket"),
                host: host.to_owned(),
                tls: true,
                listener_port: 8080,
                protocol: Some("chat".to_owned()),
            },
            direction: WebSocketDirection::ToClient,
            opcode: WebSocketOpcode::Text,
            payload: CapturedBody::from_bytes(payload.to_vec()),
        }
    }

    fn no_scope() -> BTreeSet<String> {
        BTreeSet::new()
    }

    fn matcher(query: &str) -> SearchMatcher {
        SearchMatcher::new(query, false, false).expect("valid matcher")
    }

    #[test]
    fn source_labels_ranks_and_cache_ids_are_distinct() {
        assert_eq!(SearchSource::ProxyHttp.label(), "Proxy");
        assert_eq!(SearchSource::ProxyWebSocket.label(), "WebSockets");
        assert_eq!(SearchSource::Repeater.label(), "Repeater");
        assert_eq!(SearchSource::Intruder.label(), "Intruder");
        assert_eq!(
            [
                SearchSource::ProxyHttp,
                SearchSource::ProxyWebSocket,
                SearchSource::Repeater,
                SearchSource::Intruder,
            ]
            .map(SearchSource::rank),
            [0, 1, 2, 3]
        );

        let ids = [
            SearchSource::ProxyHttp,
            SearchSource::ProxyWebSocket,
            SearchSource::Repeater,
            SearchSource::Intruder,
        ]
        .map(|source| {
            SearchKey {
                source,
                id: u64::MAX,
            }
            .cache_id()
        });
        assert_eq!(ids.map(|id| id >> 56), [1, 2, 3, 4]);
        assert_eq!(
            ids.map(|id| id & 0x00FF_FFFF_FFFF_FFFF),
            [0x00FF_FFFF_FFFF_FFFF; 4]
        );
    }

    #[test]
    fn document_helpers_handle_bare_messages_and_request_targets() {
        let request = "POST /login?q=1 HTTP/1.1\r\nHost: example.com\r\n\r\nuser=admin";
        assert_eq!(request_target(request), "/login?q=1");
        assert_eq!(request_target("GET"), "/");
        assert_eq!(request_target(""), "/");

        let views = request_views_from_raw(request);
        assert!(views.raw.contains("user=admin"));
        let (head, body) = split_message("GET / HTTP/1.1");
        assert_eq!(head, "GET / HTTP/1.1");
        assert!(body.is_empty());
        let (head, body) = split_message("POST / HTTP/1.1\n\nbody");
        assert_eq!(head, "POST / HTTP/1.1");
        assert_eq!(body, b"body");
    }

    #[test]
    fn repeater_result_helpers_cover_success_and_failure() {
        let response = RepeaterResult::Response {
            version: "HTTP/1.1".to_owned(),
            status: 201,
            reason: "Created".to_owned(),
            headers: vec![("Content-Type".to_owned(), "text/plain".to_owned())],
            body: CapturedBody::from_bytes(b"done".to_vec()),
            body_error: None,
        };
        let raw = repeater_response_raw(&response).expect("response raw");
        assert!(raw.contains("201 Created"));
        assert!(raw.contains("done"));
        assert_eq!(repeater_response_status(&response), Some(201));
        assert_eq!(repeater_response_length(&response), 4);

        let failed = RepeaterResult::Failed {
            error: "connection refused".to_owned(),
        };
        assert!(repeater_response_raw(&failed).is_none());
        assert_eq!(repeater_response_status(&failed), None);
        assert_eq!(repeater_response_length(&failed), 0);
    }

    #[test]
    fn http_header_locations_match_request_and_response_heads() {
        let mut history = History::new(10, 1024 * 1024);
        seed_exchange(&mut history, 1, "example.com", b"", b"", None);
        let item = history.get(1).expect("seeded item");
        let headers_only = SearchOptions {
            request_body: false,
            response_body: false,
            ..Default::default()
        };

        assert!(matches_http(&matcher("POST"), headers_only, item));
        assert!(matches_http(&matcher("text/plain"), headers_only, item));
        assert!(!matches_http(&matcher("missing"), headers_only, item));
    }

    #[test]
    fn finds_request_and_response_body_matches() {
        let mut history = History::new(10, 1024 * 1024);
        seed_exchange(
            &mut history,
            1,
            "example.com",
            br#"{"token":"needle"}"#,
            br#"{"error":"nothing"}"#,
            None,
        );
        seed_exchange(
            &mut history,
            2,
            "example.com",
            br#"{"token":"other"}"#,
            br#"{"error":"needle"}"#,
            None,
        );

        let options = SearchOptions {
            request_headers: false,
            response_headers: false,
            ..Default::default()
        };
        let hits = scan_loaded_http(&history, &matcher("needle"), options, &no_scope());
        assert_eq!(
            hits.iter().map(|hit| hit.key.id).collect::<Vec<_>>(),
            vec![1, 2]
        );
        assert_eq!(hits[0].key.source, SearchSource::ProxyHttp);
    }

    #[test]
    fn case_sensitivity_and_regex_are_honoured() {
        let mut history = History::new(10, 1024 * 1024);
        seed_exchange(
            &mut history,
            1,
            "example.com",
            b"",
            br#"{"error":"Permission denied"}"#,
            None,
        );
        let options = SearchOptions::default();

        let case_sensitive =
            SearchMatcher::new("permission", true, false).expect("valid literal matcher");
        assert!(scan_loaded_http(&history, &case_sensitive, options, &no_scope()).is_empty());

        let regex = SearchMatcher::new(r"den.*d", false, true).expect("valid regex matcher");
        assert_eq!(
            scan_loaded_http(&history, &regex, options, &no_scope())
                .iter()
                .map(|hit| hit.key.id)
                .collect::<Vec<_>>(),
            vec![1]
        );
        assert!(SearchMatcher::new("(", false, true).is_err());
    }

    #[test]
    fn negative_match_inverts_the_location_result() {
        let mut history = History::new(10, 1024 * 1024);
        seed_exchange(
            &mut history,
            1,
            "example.com",
            b"",
            br#"{"error":"permission denied"}"#,
            None,
        );
        seed_exchange(&mut history, 2, "example.com", b"", br#"{"ok":true}"#, None);

        let options = SearchOptions {
            negative: true,
            ..Default::default()
        };
        let negative = matcher("permission").with_negative(true);
        let hits = scan_loaded_http(&history, &negative, options, &no_scope());
        assert_eq!(
            hits.iter().map(|hit| hit.key.id).collect::<Vec<_>>(),
            vec![2]
        );
        assert!(negative.ranges("permission").is_empty());
    }

    #[test]
    fn location_filters_and_scope_filter_results() {
        let mut history = History::new(10, 1024 * 1024);
        seed_exchange(
            &mut history,
            1,
            "example.com",
            b"",
            br#"{"needle":true}"#,
            None,
        );
        seed_exchange(
            &mut history,
            2,
            "other.test",
            b"",
            br#"{"needle":true}"#,
            None,
        );

        let mut scope = BTreeSet::new();
        scope.insert("example.com".to_owned());
        let mut options = SearchOptions {
            in_scope_only: true,
            ..Default::default()
        };
        let hits = scan_loaded_http(&history, &matcher("needle"), options, &scope);
        assert_eq!(
            hits.iter().map(|hit| hit.key.id).collect::<Vec<_>>(),
            vec![1]
        );

        options.response_body = false;
        options.request_body = false;
        options.response_headers = false;
        options.request_headers = false;
        options.in_scope_only = false;
        assert!(scan_loaded_http(&history, &matcher("needle"), options, &scope).is_empty());
        assert!(!options.any_location());
    }

    #[test]
    fn searches_decoded_compressed_bodies() {
        let mut history = History::new(10, 1024 * 1024);
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        encoder
            .write_all(br#"{"error":"permission denied"}"#)
            .expect("compress test body");
        let compressed = encoder.finish().expect("finish gzip body");
        seed_exchange(
            &mut history,
            1,
            "example.com",
            b"",
            &compressed,
            Some("gzip"),
        );

        let hits = scan_loaded_http(
            &history,
            &matcher("permission"),
            SearchOptions::default(),
            &no_scope(),
        );
        assert_eq!(hits.len(), 1);
    }

    #[test]
    fn websocket_search_matches_connection_metadata_and_payload() {
        let item = ws_item("socket.example.com", b"hello needle");
        let options = SearchOptions::default();

        assert!(matches_ws(&matcher("socket.example"), options, &item));
        assert!(matches_ws(&matcher("needle"), options, &item));
        assert!(!matches_ws(&matcher("missing"), options, &item));

        let mut headers_only = options;
        headers_only.request_body = false;
        headers_only.response_body = false;
        assert!(matches_ws(&matcher("socket.example"), headers_only, &item));
        assert!(!matches_ws(&matcher("needle"), headers_only, &item));

        let hit = ws_hit(&item);
        assert_eq!(hit.key.source, SearchSource::ProxyWebSocket);
        assert_eq!(hit.number, Some(7));
        assert_eq!(hit.host, "wss://socket.example.com");
        assert_eq!(hit.url, "wss://socket.example.com/socket");
        assert_eq!(hit.length, 12);
    }

    #[test]
    fn websocket_search_honours_negative_and_scope_filters() {
        let mut history = WebSocketHistory::new(10, 1024);
        history.apply(ProxyEvent::WebSocketMessage {
            id: 4,
            timestamp: SystemTime::UNIX_EPOCH,
            connection: ws_item("socket.example.com", b"a").connection,
            direction: WebSocketDirection::ToClient,
            opcode: WebSocketOpcode::Text,
            payload: CapturedBody::from_bytes(b"hello".to_vec()),
        });
        history.apply(ProxyEvent::WebSocketMessage {
            id: 5,
            timestamp: SystemTime::UNIX_EPOCH,
            connection: ws_item("other.test", b"a").connection,
            direction: WebSocketDirection::ToClient,
            opcode: WebSocketOpcode::Binary,
            payload: CapturedBody::from_bytes(b"needle".to_vec()),
        });

        let mut scope = BTreeSet::new();
        scope.insert("socket.example.com".to_owned());
        let mut options = SearchOptions {
            in_scope_only: true,
            ..Default::default()
        };
        assert_eq!(
            scan_loaded_ws(&history, &matcher("hello"), options, &scope)
                .iter()
                .map(|hit| hit.key.id)
                .collect::<Vec<_>>(),
            vec![4]
        );

        options.in_scope_only = false;
        options.negative = true;
        let negative = matcher("needle").with_negative(true);
        assert_eq!(
            scan_loaded_ws(&history, &negative, options, &no_scope())
                .iter()
                .map(|hit| hit.key.id)
                .collect::<Vec<_>>(),
            vec![4]
        );
    }

    #[test]
    fn document_search_splits_heads_and_bodies_and_handles_failures() {
        let request = "POST /login HTTP/1.1\nHost: example.com\n\nuser=admin";
        let response = "HTTP/1.1 200 OK\nContent-Type: text/plain\n\nwelcome admin";

        assert!(matches_document(
            &matcher("example.com"),
            SearchOptions::default(),
            request,
            Some(response)
        ));
        assert!(matches_document(
            &matcher("welcome"),
            SearchOptions::default(),
            request,
            Some(response)
        ));
        let body_only = SearchOptions {
            request_headers: false,
            response_headers: false,
            ..Default::default()
        };
        assert!(matches_document(
            &matcher("user=admin"),
            body_only,
            request,
            Some(response)
        ));
        assert!(!matches_document(
            &matcher("example.com"),
            body_only,
            request,
            Some(response)
        ));

        let options = SearchOptions {
            negative: true,
            ..Default::default()
        };
        let negative = matcher("missing").with_negative(true);
        assert!(matches_document(
            &negative,
            options,
            "GET / HTTP/1.1\r\nHost: example.com\r\n\r\n",
            None
        ));
    }

    #[test]
    fn matcher_ranges_reports_matches_and_skips_non_utf8_boundaries() {
        let matcher = SearchMatcher::new("a", false, false).expect("valid literal matcher");
        assert_eq!(
            matcher.ranges("banana"),
            vec![1..2, 3..4, 5..6],
            "literal matching should report every occurrence"
        );

        // The byte class also matches the continuation byte inside `é`; that
        // range is not a valid `str` offset and must be filtered out.
        let matcher = SearchMatcher::new("a|(?-u:[\\x80-\\xBF])", false, true)
            .expect("valid boundary matcher");
        assert_eq!(matcher.ranges("aéa"), vec![0..1, 3..4]);
    }

    #[test]
    fn matcher_ranges_are_capped_and_negative_matchers_highlight_nothing() {
        let matcher = SearchMatcher::new("a", true, false).expect("valid literal matcher");
        let text = "a".repeat(MATCH_HIGHLIGHT_LIMIT + 1);
        let ranges = matcher.ranges(&text);
        assert_eq!(ranges.len(), MATCH_HIGHLIGHT_LIMIT);
        assert_eq!(ranges.first(), Some(&(0..1)));
        assert_eq!(
            ranges.last(),
            Some(&(MATCH_HIGHLIGHT_LIMIT - 1..MATCH_HIGHLIGHT_LIMIT))
        );

        let negative = SearchMatcher::new("a", false, false)
            .expect("valid literal matcher")
            .with_negative(true);
        assert!(negative.ranges("banana").is_empty());
    }

    #[test]
    fn result_host_and_url_use_fallbacks() {
        let item = history_item("example.com", "https", "/a/b");
        assert_eq!(result_host(&item), "https://example.com");
        assert_eq!(result_url(&item), "/a/b");

        let fallback = history_item("example.com", "", "");
        assert_eq!(result_host(&fallback), "http://example.com");
        assert_eq!(result_url(&fallback), "/");
    }

    #[test]
    fn search_options_require_a_location_and_a_tool() {
        let mut options = SearchOptions::default();
        assert!(options.any_location());
        assert!(options.any_tool());

        options.request_headers = false;
        options.request_body = false;
        options.response_headers = false;
        assert!(options.any_location());
        options.response_body = false;
        assert!(!options.any_location());

        options.proxy_history = false;
        options.websockets = false;
        options.repeater = false;
        assert!(options.any_tool());
        options.intruder = false;
        assert!(!options.any_tool());
    }

    #[test]
    fn hit_builders_keep_source_specific_metadata() {
        let item = history_item("example.com", "https", "/search");
        let hit = http_hit(&item);
        assert_eq!(hit.number, Some(1));
        assert_eq!(hit.host, "https://example.com");
        assert_eq!(hit.url, "/search");
        assert_eq!(hit.status, None);

        let ws = ws_hit(&WebSocketMessageItem {
            id: 9,
            number: 3,
            timestamp: SystemTime::UNIX_EPOCH,
            connection: WebSocketConnectionInfo {
                id: 1,
                url: String::new(),
                host: String::new(),
                tls: false,
                listener_port: 1,
                protocol: None,
            },
            direction: WebSocketDirection::ToClient,
            opcode: WebSocketOpcode::Binary,
            payload: CapturedBody::default(),
        });
        assert_eq!(ws.host, "");
        assert_eq!(ws.url, "To client Binary");
    }
}
