//! Query compilation, history matching, and result formatting for Search.
//!
//! Everything here is independent of egui so the matching rules can be tested
//! without a UI context. The window layout lives in [`super::view`].

use std::collections::BTreeSet;

use regex::bytes::{Regex, RegexBuilder};

use crate::history::{History, HistoryItem};
use crate::site_map;
use crate::view;

use super::{SearchDetailCache, SearchState};
use crate::app::inspector::InspectorModel;

/// Upper bound on highlight ranges produced for one message. A one-character
/// search in a large body would otherwise build an unusable layout job; the UI
/// reports the count as a lower bound when the cap is reached.
pub(super) const MATCH_HIGHLIGHT_LIMIT: usize = 5_000;

/// Which parts of an exchange a search examines.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) struct SearchOptions {
    pub(super) case_sensitive: bool,
    pub(super) regex: bool,
    pub(super) negative: bool,
    pub(super) in_scope_only: bool,
    pub(super) dynamic_update: bool,
    pub(super) proxy_history: bool,
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
            request_headers: true,
            request_body: true,
            response_headers: true,
            response_body: true,
        }
    }
}

impl SearchOptions {
    fn any_location(self) -> bool {
        self.request_headers || self.request_body || self.response_headers || self.response_body
    }

    fn any_tool(self) -> bool {
        self.proxy_history
    }
}

/// A compiled query. Negative matching is applied per exchange in
/// [`SearchState::run`]; the matcher itself only answers "does this text
/// contain a match?".
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

    fn with_negative(mut self, negative: bool) -> Self {
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

impl SearchState {
    pub(super) fn run(&mut self, history: &History, scope: &BTreeSet<String>) {
        self.error = None;
        self.dirty = false;
        self.ran = false;
        self.results.clear();
        self.selected = None;
        self.detail = None;
        self.matcher = None;
        self.last_history_revision = Some(history.revision());

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

        for item in history.iter() {
            if self.options.in_scope_only && !site_map::host_in_scope(&item.request.host, scope) {
                continue;
            }

            let mut matched = false;
            if self.options.request_headers && matcher.is_match(view::request_head(item).as_bytes())
            {
                matched = true;
            }
            if !matched
                && self.options.request_body
                && matcher.is_match(&view::request_body_for_search(item))
            {
                matched = true;
            }
            if !matched
                && self.options.response_headers
                && matcher.is_match(view::response_head(item).as_bytes())
            {
                matched = true;
            }
            if !matched
                && self.options.response_body
                && matcher.is_match(&view::response_body_for_search(item))
            {
                matched = true;
            }

            if self.options.negative {
                matched = !matched;
            }
            if matched {
                self.results.push(item.id);
            }
        }

        self.matcher = Some(matcher);
        self.ran = true;
        self.selected = self.results.first().copied();
    }

    pub(super) fn ensure_detail(&mut self, history: &History) {
        let selected = self.selected.filter(|id| history.get(*id).is_some());
        if selected != self.selected {
            self.selected = selected;
        }
        let selected = self.selected;
        if self.detail.as_ref().map(|detail| detail.id) != selected {
            self.detail = selected
                .and_then(|id| history.get(id))
                .map(|item| SearchDetailCache {
                    id: item.id,
                    request: view::request_views(item),
                    response: view::response_views(item),
                    inspector: InspectorModel::from_history(item),
                });
        }
    }
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

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::time::{Duration, SystemTime};

    use crate::history::Phase;
    use crate::proxy::{
        CapturedBody, ExchangeEnd, ExchangeResult, ProxyEvent, RequestInfo, ResponseHead,
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

    fn search(query: &str) -> SearchState {
        SearchState {
            query: query.to_owned(),
            ..SearchState::default()
        }
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

        let scope = BTreeSet::new();
        let mut state = search("needle");
        state.options.request_headers = false;
        state.options.response_headers = false;
        state.run(&history, &scope);

        assert_eq!(state.results, vec![1, 2]);
        assert!(state.ran);
        assert_eq!(state.selected, Some(1));
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
        let scope = BTreeSet::new();

        let mut case_sensitive = search("permission");
        case_sensitive.options.case_sensitive = true;
        case_sensitive.run(&history, &scope);
        assert!(case_sensitive.results.is_empty());

        let mut regex = search(r"den.*d");
        regex.options.regex = true;
        regex.run(&history, &scope);
        assert_eq!(regex.results, vec![1]);

        let mut invalid = search("(");
        invalid.options.regex = true;
        invalid.run(&history, &scope);
        assert!(invalid.results.is_empty());
        assert!(invalid.error.is_some());
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

        let scope = BTreeSet::new();
        let mut state = search("permission");
        state.options.negative = true;
        state.run(&history, &scope);

        assert_eq!(state.results, vec![2]);
        assert_eq!(
            state
                .matcher
                .as_ref()
                .map(|matcher| matcher.ranges("permission")),
            Some(Vec::new())
        );
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
        let mut state = search("needle");
        state.options.in_scope_only = true;
        state.run(&history, &scope);
        assert_eq!(state.results, vec![1]);

        state.options.response_body = false;
        state.options.request_body = false;
        state.options.response_headers = false;
        state.options.request_headers = false;
        state.options.in_scope_only = false;
        state.run(&history, &scope);
        assert!(state.results.is_empty());
        assert!(state.error.is_some());
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

        let scope = BTreeSet::new();
        let mut state = search("permission");
        state.run(&history, &scope);
        assert_eq!(state.results, vec![1]);
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
    fn ensure_detail_follows_the_selection_and_reuses_the_cache() {
        let mut history = History::new(10, 1024 * 1024);
        seed_exchange(&mut history, 1, "example.com", b"", b"{}", None);

        let mut state = SearchState {
            selected: Some(99),
            ..Default::default()
        };
        state.ensure_detail(&history);
        assert_eq!(state.selected, None, "evicted ids are dropped");
        assert!(state.detail.is_none());

        state.selected = Some(1);
        state.ensure_detail(&history);
        assert_eq!(state.selected, Some(1));
        assert_eq!(state.detail.as_ref().map(|detail| detail.id), Some(1));

        let cached = state.detail.as_ref().map(|detail| detail as *const _);
        state.ensure_detail(&history);
        assert_eq!(
            state.detail.as_ref().map(|detail| detail as *const _),
            cached,
            "an already-built detail cache is kept"
        );
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
        assert!(!options.any_tool());
    }
}
