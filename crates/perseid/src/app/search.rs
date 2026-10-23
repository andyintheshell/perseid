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

use std::collections::BTreeSet;
use std::ops::Range;

use egui::{Label, RichText, TextEdit};
use egui_extras::{Column, TableBuilder};
use regex::bytes::{Regex, RegexBuilder};

use crate::history::{History, HistoryItem};
use crate::site_map;
use crate::theme;
use crate::view::{self, MessageViews};

use super::inspector::{self, InspectorModel, InspectorState};
use super::message_view::{message_tabs, message_text_with_matches, message_view_text};
use super::widgets::{empty_state, section_tab, table_frame};
use super::*;

const SEARCH_DETAIL_ID: &str = "perseid.search.detail";
const SEARCH_QUERY_ID: &str = "perseid.search.query";
const SEARCH_RESULTS_ID: &str = "perseid.search.results";
const MATCH_HIGHLIGHT_LIMIT: usize = 5_000;

/// Which parts of an exchange a search examines.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SearchOptions {
    case_sensitive: bool,
    regex: bool,
    negative: bool,
    in_scope_only: bool,
    dynamic_update: bool,
    proxy_history: bool,
    request_headers: bool,
    request_body: bool,
    response_headers: bool,
    response_body: bool,
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
struct SearchMatcher {
    regex: Regex,
    negative: bool,
}

impl SearchMatcher {
    fn new(query: &str, case_sensitive: bool, regex: bool) -> Result<Self, String> {
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
    fn ranges(&self, text: &str) -> Vec<Range<usize>> {
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

/// Formatted detail for the selected search result. Rebuilt when the
/// selection changes, just like the Proxy detail cache.
struct SearchDetailCache {
    id: u64,
    request: MessageViews,
    response: MessageViews,
    inspector: InspectorModel,
}

/// Persistent Search window state.
pub(super) struct SearchState {
    /// Whether the floating window is visible. The toolbar icon uses this for
    /// its selected state.
    pub(super) open: bool,
    query: String,
    options: SearchOptions,
    matcher: Option<SearchMatcher>,
    error: Option<String>,
    ran: bool,
    dirty: bool,
    results: Vec<u64>,
    selected: Option<u64>,
    detail: Option<SearchDetailCache>,
    pane: MessagePane,
    request_view: MessageView,
    response_view: MessageView,
    focus_query: bool,
    last_history_revision: Option<u64>,
    /// Last width the results table was laid out at. The table remembers
    /// column widths across frames, so a change here resets them to sensible
    /// defaults instead of preserving widths from the window's first, often
    /// narrower, layout pass.
    table_width: Option<f32>,
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

    /// Draw the Search window. Called every frame, but a closed window costs
    /// only this early return.
    pub(super) fn show(
        &mut self,
        ctx: &egui::Context,
        history: &History,
        scope: &BTreeSet<String>,
        inspector: &mut InspectorState,
    ) {
        if !self.open {
            return;
        }
        if ctx.input(|input| input.key_pressed(Key::Escape)) {
            self.open = false;
            return;
        }

        let mut open = self.open;
        let screen = ctx.content_rect();
        let default_pos = egui::pos2(
            (screen.center().x - 500.0).max(screen.left() + 16.0),
            (screen.center().y - 330.0).max(screen.top() + 42.0),
        );

        egui::Window::new("Search")
            .id(egui::Id::unique("perseid.search.window"))
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_size(egui::vec2(1000.0, 660.0))
            .min_size(egui::vec2(680.0, 430.0))
            .default_pos(default_pos)
            .show(ctx, |ui| {
                let history_changed = self.last_history_revision != Some(history.revision());
                let mut run = self.options.dynamic_update && self.ran && history_changed;

                self.controls(ui, &mut run);
                if run {
                    self.run(history, scope);
                }
                self.status_line(ui);
                self.ensure_detail(history);

                egui::Panel::bottom(SEARCH_DETAIL_ID)
                    .resizable(true)
                    .default_size(310.0)
                    .min_size(150.0)
                    .frame(
                        egui::Frame::NONE
                            .fill(theme::SURFACE)
                            .stroke(egui::Stroke::new(1.0, theme::BORDER))
                            .inner_margin(egui::Margin::same(5)),
                    )
                    .show(ui, |ui| self.detail(ui, history, inspector));

                egui::CentralPanel::default()
                    .frame(egui::Frame::NONE.fill(theme::SURFACE))
                    .show(ui, |ui| self.results_table(ui, history));
            });

        self.open = open;
    }

    fn controls(&mut self, ui: &mut egui::Ui, run: &mut bool) {
        let mut changed = false;

        ui.add_space(2.0);
        ui.horizontal(|ui| {
            let width = (ui.available_width() - 54.0).max(120.0);
            let response = ui.add_sized(
                [width, 24.0],
                TextEdit::singleline(&mut self.query)
                    .id(egui::Id::unique(SEARCH_QUERY_ID))
                    .hint_text("Search request and response bodies"),
            );
            if self.focus_query {
                response.request_focus();
                self.focus_query = false;
            }
            if response.changed() {
                changed = true;
            }
            if response.lost_focus() && ui.input(|input| input.key_pressed(Key::Enter)) {
                *run = true;
            }
            if ui
                .add_sized(
                    [45.0, 24.0],
                    egui::Button::new(RichText::new("Go").strong()),
                )
                .on_hover_text("Run the search")
                .clicked()
            {
                *run = true;
            }
        });

        ui.add_space(4.0);
        ui.columns(3, |columns| {
            changed |= group_card(&mut columns[0], "Options", |ui| {
                options_group(ui, &mut self.options)
            });
            changed |= group_card(&mut columns[1], "Locations", |ui| {
                locations_group(ui, &mut self.options)
            });
            changed |= group_card(&mut columns[2], "Tools", |ui| {
                tools_group(ui, &mut self.options)
            });
        });

        if changed {
            self.dirty = true;
            if self.options.dynamic_update {
                *run = true;
            }
        }
    }

    fn status_line(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if let Some(error) = &self.error {
                ui.colored_label(theme::RED, error);
            } else if self.ran {
                let count = self.results.len();
                let noun = if count == 1 { "result" } else { "results" };
                ui.label(
                    RichText::new(format!("Search completed · {count} {noun}"))
                        .small()
                        .color(theme::TEXT_MUTED),
                );
                if self.dirty && !self.options.dynamic_update {
                    ui.label(
                        RichText::new("settings changed — press Go to rerun")
                            .small()
                            .color(theme::AMBER),
                    );
                }
            } else if self.query.is_empty() {
                ui.label(
                    RichText::new(
                        "Search decodes gzip, deflate, and Brotli bodies before matching.",
                    )
                    .small()
                    .color(theme::TEXT_MUTED),
                );
            } else {
                ui.label(
                    RichText::new("Press Go or Enter to search.")
                        .small()
                        .color(theme::TEXT_MUTED),
                );
            }
        });
        ui.add_space(3.0);
    }

    fn run(&mut self, history: &History, scope: &BTreeSet<String>) {
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

    fn results_table(&mut self, ui: &mut egui::Ui, history: &History) {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("Results").strong().color(theme::TEXT));
            if self.ran {
                let count = self.results.len();
                ui.label(
                    RichText::new(format!("({count})"))
                        .small()
                        .color(theme::TEXT_MUTED),
                );
            }
        });
        ui.add_space(3.0);

        if self.error.is_some() {
            empty_state(
                ui,
                "Search could not run",
                "Fix the search options above and try again.",
            );
            return;
        }
        if !self.ran {
            empty_state(
                ui,
                "Search captured traffic",
                "Enter a term, choose request or response locations, and press Go or Enter. Content-encoded bodies are decoded before matching.",
            );
            return;
        }
        if self.results.is_empty() {
            let body = if self.options.negative {
                "Every selected location contained the term. Try a different term or turn off negative match."
            } else {
                "Try a different term, enable more locations, or check the scope and negative match options."
            };
            empty_state(ui, "No matches", body);
            return;
        }

        let mut clicked = None;
        let selected = self.selected;
        table_frame(ui, |ui| {
            let width = ui.available_width();
            let sizing = ui.is_sizing_pass();
            let reset = sizing
                || self
                    .table_width
                    .is_none_or(|last| (last - width).abs() > 1.0);
            if !sizing {
                self.table_width = Some(width);
            }

            let table = TableBuilder::new(ui)
                .id_salt(SEARCH_RESULTS_ID)
                .striped(true)
                .resizable(true)
                .auto_shrink([false, false])
                .sense(egui::Sense::click())
                .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                .column(Column::exact(62.0))
                .column(Column::initial(155.0).at_least(100.0))
                .column(Column::initial(380.0).at_least(220.0))
                .column(Column::exact(76.0))
                .column(Column::exact(76.0))
                .column(Column::remainder().at_least(126.0));

            if reset {
                table.reset();
            }

            table
                .header(20.0, |mut header| {
                    for title in [
                        "Source",
                        "Host",
                        "URL",
                        "Status code",
                        "Length",
                        "Time requested",
                    ] {
                        header.col(|ui| {
                            ui.label(RichText::new(title).strong().small().color(theme::TEXT));
                        });
                    }
                })
                .body(|body| {
                    body.rows(18.0, self.results.len(), |mut row| {
                        let id = self.results[row.index()];
                        let Some(item) = history.get(id) else {
                            return;
                        };

                        row.set_selected(selected == Some(id));
                        row.col(|ui| {
                            ui.label(RichText::new("Proxy").small().color(theme::TEXT));
                        });
                        let host = result_host(item);
                        row.col(|ui| {
                            ui.add(Label::new(RichText::new(&host).color(theme::TEXT)).truncate())
                                .on_hover_text(&host);
                        });
                        let url = result_url(item);
                        row.col(|ui| {
                            ui.add(Label::new(RichText::new(&url).color(theme::TEXT)).truncate())
                                .on_hover_text(&url);
                        });
                        row.col(|ui| {
                            ui.label(
                                RichText::new(view::status_text(item))
                                    .color(view::status_color(item)),
                            );
                        });
                        row.col(|ui| {
                            ui.label(
                                RichText::new(view::human_bytes(item.transferred()))
                                    .small()
                                    .color(theme::TEXT),
                            );
                        });
                        row.col(|ui| {
                            ui.label(
                                RichText::new(view::format_time(item.timestamp))
                                    .small()
                                    .color(theme::TEXT_MUTED),
                            );
                        });

                        if row.response().clicked() {
                            clicked = Some(id);
                        }
                    });
                });
        });

        if let Some(id) = clicked
            && self.selected != Some(id)
        {
            self.selected = Some(id);
            self.detail = None;
        }
    }

    fn ensure_detail(&mut self, history: &History) {
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

    fn detail(&mut self, ui: &mut egui::Ui, history: &History, inspector: &mut InspectorState) {
        let Some(cache) = &self.detail else {
            empty_state(
                ui,
                "No result selected",
                "Choose a row in the results table to inspect its request and response.",
            );
            return;
        };
        let Some(item) = history.get(cache.id) else {
            empty_state(
                ui,
                "Exchange no longer in history",
                "The selected result was evicted or cleared. Run the search again.",
            );
            return;
        };

        let mut pane = self.pane;
        if pane == MessagePane::Response && item.response.is_none() {
            pane = MessagePane::Request;
        }
        let mode = match pane {
            MessagePane::Request => self.request_view,
            MessagePane::Response => self.response_view,
        };
        let text = match pane {
            MessagePane::Request => message_view_text(&cache.request, mode),
            MessagePane::Response => message_view_text(&cache.response, mode),
        };
        let matches = self
            .matcher
            .as_ref()
            .map(|matcher| matcher.ranges(text))
            .unwrap_or_default();
        let capped = matches.len() >= MATCH_HIGHLIGHT_LIMIT;

        ui.add_space(1.0);
        ui.horizontal(|ui| {
            if section_tab(ui, pane == MessagePane::Request, "Request").clicked() {
                self.pane = MessagePane::Request;
            }
            if section_tab(ui, pane == MessagePane::Response, "Response").clicked() {
                self.pane = MessagePane::Response;
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if !matches.is_empty() {
                    let noun = if matches.len() == 1 {
                        "match"
                    } else {
                        "matches"
                    };
                    let count = if capped {
                        format!("{MATCH_HIGHLIGHT_LIMIT}+ {noun}")
                    } else {
                        format!("{} {noun}", matches.len())
                    };
                    ui.label(
                        RichText::new(count)
                            .small()
                            .strong()
                            .color(theme::ACCENT_DARK),
                    );
                }
                ui.label(
                    RichText::new(view::human_bytes(item.transferred()))
                        .small()
                        .color(theme::TEXT_MUTED),
                );
                if let Some(duration) = item.duration() {
                    ui.label(
                        RichText::new(view::format_duration(duration))
                            .small()
                            .color(theme::TEXT_MUTED),
                    );
                }
                ui.colored_label(view::status_color(item), view::status_text(item));
                ui.label(
                    RichText::new(format!("#{}", item.number))
                        .small()
                        .color(theme::TEXT_MUTED),
                );
            });
        });

        ui.add_space(1.0);
        let mut request_view = self.request_view;
        let mut response_view = self.response_view;
        match pane {
            MessagePane::Request => message_tabs(ui, &mut request_view),
            MessagePane::Response => message_tabs(ui, &mut response_view),
        }
        self.request_view = request_view;
        self.response_view = response_view;
        ui.add_space(1.0);

        inspector::side_panel(ui, inspector, cache.id, &cache.inspector);

        if pane == MessagePane::Response && item.response.is_none() {
            empty_state(
                ui,
                "No response captured",
                "The exchange is still in flight or ended before a response was received.",
            );
            return;
        }

        message_text_with_matches(ui, cache.id, pane, mode, text, &matches);
    }

    /// Build the deterministic Search scene used by the documentation
    /// screenshots: a decoded JSON response with several visible matches.
    #[cfg(feature = "screenshots")]
    pub(super) fn seed_screenshot(&mut self, history: &History, scope: &BTreeSet<String>) {
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

fn group_card<R>(
    ui: &mut egui::Ui,
    title: &str,
    add_contents: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    egui::Frame::NONE
        .fill(theme::SURFACE_ALT)
        .stroke(egui::Stroke::new(1.0, theme::BORDER))
        .corner_radius(4.0)
        .inner_margin(egui::Margin::same(8))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.set_min_height(64.0);
            ui.label(
                RichText::new(title)
                    .small()
                    .strong()
                    .color(theme::TEXT_MUTED),
            );
            ui.add_space(2.0);
            add_contents(ui)
        })
        .inner
}

fn options_group(ui: &mut egui::Ui, options: &mut SearchOptions) -> bool {
    let mut changed = false;
    ui.columns(2, |columns| {
        changed |= checkbox(
            &mut columns[0],
            &mut options.case_sensitive,
            "Case sensitive",
            "Match letter case exactly.",
        );
        changed |= checkbox(
            &mut columns[1],
            &mut options.regex,
            "Regex",
            "Treat the query as a regular expression.",
        );
        changed |= checkbox(
            &mut columns[0],
            &mut options.negative,
            "Negative match",
            "Show exchanges that do not match in any selected location.",
        );
        changed |= checkbox(
            &mut columns[1],
            &mut options.in_scope_only,
            "In-scope only",
            "Restrict results to hosts in Target > Scope. An empty scope includes everything.",
        );
        changed |= checkbox(
            &mut columns[0],
            &mut options.dynamic_update,
            "Dynamic update",
            "Rerun automatically when the query, options, or captured history changes.",
        );
    });
    changed
}

fn locations_group(ui: &mut egui::Ui, options: &mut SearchOptions) -> bool {
    let mut changed = false;
    ui.columns(2, |columns| {
        changed |= checkbox(
            &mut columns[0],
            &mut options.request_headers,
            "Request headers",
            "Search the request line and request headers.",
        );
        changed |= checkbox(
            &mut columns[1],
            &mut options.request_body,
            "Request body",
            "Search the decoded request body.",
        );
        changed |= checkbox(
            &mut columns[0],
            &mut options.response_headers,
            "Response headers",
            "Search the response status line and headers.",
        );
        changed |= checkbox(
            &mut columns[1],
            &mut options.response_body,
            "Response body",
            "Search the decoded response body.",
        );
    });
    changed
}

fn tools_group(ui: &mut egui::Ui, options: &mut SearchOptions) -> bool {
    let changed = checkbox(
        ui,
        &mut options.proxy_history,
        "Proxy history",
        "Search captured HTTP and decrypted HTTPS exchanges.",
    );
    ui.label(
        RichText::new("HTTP history only")
            .small()
            .color(theme::TEXT_MUTED),
    );
    changed
}

fn checkbox(ui: &mut egui::Ui, value: &mut bool, label: &str, tooltip: &str) -> bool {
    ui.checkbox(value, label).on_hover_text(tooltip).changed()
}

fn result_host(item: &HistoryItem) -> String {
    let scheme = if item.request.scheme.is_empty() {
        "http"
    } else {
        &item.request.scheme
    };
    format!("{scheme}://{}", item.request.host)
}

fn result_url(item: &HistoryItem) -> String {
    if item.request.path.is_empty() {
        "/".to_owned()
    } else {
        item.request.path.clone()
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
    use std::io::Write;
    use std::time::{Duration, SystemTime};

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
}
