//! Burp-style search across captured HTTP history.
//!
//! Search is deliberately a floating window rather than another workspace
//! tab. The magnifier in the top toolbar opens it, and the window stays out of
//! the way of the Proxy and Repeater layouts while it searches request and
//! response heads and bodies.
//!
//! Matching uses the same decoded body representation as the message editor:
//! supported `Content-Encoding` values are unpacked before searching, so a
//! compressed response can be found by its readable text. Results keep
//! history IDs rather than copies of the messages, so selecting a row can
//! reuse the existing Pretty/Raw/Hex detail and Inspector models without
//! duplicating captured bytes.
//!
//! The window state lives here; query compilation and matching are in
//! [`engine`], and the egui layout is in [`view`].

mod engine;
mod view;

use crate::app::inspector::InspectorModel;
use crate::app::{MessagePane, MessageView};
use crate::history::History;
use crate::project::{SearchOptionsWorkspace, SearchWorkspace};
use crate::view::MessageViews;

use self::engine::{SearchMatcher, SearchOptions};
use super::PerseidApp;

impl From<SearchOptions> for SearchOptionsWorkspace {
    fn from(options: SearchOptions) -> Self {
        Self {
            case_sensitive: options.case_sensitive,
            regex: options.regex,
            negative: options.negative,
            in_scope_only: options.in_scope_only,
            dynamic_update: options.dynamic_update,
            proxy_history: options.proxy_history,
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
            request_headers: options.request_headers,
            request_body: options.request_body,
            response_headers: options.response_headers,
            response_body: options.response_body,
        }
    }
}

/// Formatted detail for the selected search result. Rebuilt when the
/// selection changes, just like the Proxy detail cache.
pub(super) struct SearchDetailCache {
    pub(super) id: u64,
    pub(super) request: MessageViews,
    pub(super) response: MessageViews,
    pub(super) inspector: InspectorModel,
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
    pub(super) results: Vec<u64>,
    pub(super) selected: Option<u64>,
    pub(super) detail: Option<SearchDetailCache>,
    pub(super) pane: MessagePane,
    pub(super) request_view: MessageView,
    pub(super) response_view: MessageView,
    pub(super) focus_query: bool,
    pub(super) last_history_revision: Option<u64>,
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

    pub(super) fn clear_history(&mut self) {
        self.results.clear();
        self.selected = None;
        self.detail = None;
        self.matcher = None;
        self.error = None;
        self.ran = false;
        self.dirty = false;
        self.last_history_revision = None;
    }

    pub(super) fn workspace(&self) -> SearchWorkspace {
        SearchWorkspace {
            open: self.open,
            query: self.query.clone(),
            options: self.options.into(),
            ran: self.ran,
            dirty: self.dirty,
            results: self.results.clone(),
            selected: self.selected,
            pane: self.pane.into(),
            request_view: self.request_view.into(),
            response_view: self.response_view.into(),
            error: self.error.clone(),
        }
    }

    pub(super) fn apply_workspace(&mut self, workspace: &SearchWorkspace, history: &History) {
        self.open = workspace.open;
        self.query = workspace.query.clone();
        self.options = (&workspace.options).into();
        self.error = workspace.error.clone();
        self.ran = workspace.ran;
        self.dirty = workspace.dirty;
        self.results = workspace
            .results
            .iter()
            .copied()
            .filter(|id| history.get(*id).is_some())
            .collect();
        self.selected = workspace.selected.filter(|id| self.results.contains(id));
        self.pane = workspace.pane.into();
        self.request_view = workspace.request_view.into();
        self.response_view = workspace.response_view.into();
        self.detail = None;
        self.focus_query = false;
        self.table_width = None;
        self.last_history_revision = Some(history.revision());
        self.matcher = if self.ran && !self.dirty && self.error.is_none() {
            SearchMatcher::new(&self.query, self.options.case_sensitive, self.options.regex)
                .ok()
                .map(|matcher| matcher.with_negative(self.options.negative))
        } else {
            None
        };
        self.ensure_detail(history);
    }

    /// Build the deterministic Search scene used by the documentation
    /// screenshots: a decoded JSON response with several visible matches.
    #[cfg(feature = "screenshots")]
    pub(super) fn seed_screenshot(
        &mut self,
        history: &crate::history::History,
        scope: &std::collections::BTreeSet<String>,
    ) {
        self.open = true;
        self.query = "Router".to_owned();
        self.options = SearchOptions::default();
        self.pane = MessagePane::Response;
        self.request_view = MessageView::Pretty;
        self.response_view = MessageView::Pretty;
        self.focus_query = false;
        self.run(history, scope);
        if let Some(id) = history.iter().find(|item| item.id == 2).map(|item| item.id) {
            self.selected = Some(id);
            self.detail = None;
            self.ensure_detail(history);
        }
    }
}

impl PerseidApp {
    pub(super) fn search_window(&mut self, ctx: &egui::Context) {
        let Self {
            search,
            history,
            scope,
            inspector,
            ..
        } = self;
        search.show(ctx, history, scope, inspector);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn views() -> MessageViews {
        MessageViews {
            pretty: "pretty".to_owned(),
            raw: "raw".to_owned(),
            hex: "hex".to_owned(),
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
        let mut state = SearchState {
            open: true,
            query: "needle".to_owned(),
            results: vec![1, 2],
            selected: Some(2),
            detail: Some(SearchDetailCache {
                id: 2,
                request: views(),
                response: views(),
                inspector: InspectorModel {
                    request: Vec::new(),
                    response: Vec::new(),
                },
            }),
            matcher: Some(SearchMatcher::new("needle", false, false).expect("valid matcher")),
            error: Some("boom".to_owned()),
            ran: true,
            dirty: true,
            last_history_revision: Some(7),
            ..Default::default()
        };

        state.clear_history();

        assert!(state.results.is_empty());
        assert_eq!(state.selected, None);
        assert!(state.detail.is_none());
        assert!(state.matcher.is_none());
        assert!(state.error.is_none());
        assert!(!state.ran);
        assert!(!state.dirty);
        assert_eq!(state.last_history_revision, None);
        // Clearing captured history does not dismiss the window or the query.
        assert!(state.open);
        assert_eq!(state.query, "needle");
    }
}
