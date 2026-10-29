//! egui layout for the floating Search window.
//!
//! The controls, status line, results table, and detail pane live here; the
//! matching itself is delegated to [`super::engine`].

use std::collections::BTreeSet;

use egui::{Key, Label, RichText, TextEdit};
use egui_extras::{Column, TableBuilder};

use crate::app::MessagePane;
use crate::app::inspector::{self, InspectorState};
use crate::app::message_view::{message_tabs, message_text_with_matches, message_view_text};
use crate::app::widgets::{empty_state, section_tab, table_frame};
use crate::theme;
use crate::view;

use super::engine::{MATCH_HIGHLIGHT_LIMIT, SEARCH_RESULT_LIMIT, SearchOptions};
use super::{SearchSources, SearchState};

const SEARCH_DETAIL_ID: &str = "perseid.search.detail";
const SEARCH_QUERY_ID: &str = "perseid.search.query";
const SEARCH_RESULTS_ID: &str = "perseid.search.results";

impl SearchState {
    /// Draw the Search window. Called every frame, but a closed window costs
    /// only this early return.
    pub(super) fn show(
        &mut self,
        ctx: &egui::Context,
        sources: &SearchSources<'_>,
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

        self.poll();
        if self.running {
            ctx.request_repaint_after(std::time::Duration::from_millis(75));
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
                let history_changed = self.last_history_revision
                    != Some(sources.history.revision())
                    || self.last_websocket_revision != Some(sources.websocket_history.revision());
                let mut run = self.options.dynamic_update && self.ran && history_changed;

                self.controls(ui, &mut run);
                if run {
                    self.run(sources, scope);
                }
                self.status_line(ui);
                self.ensure_detail(sources);

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
                    .show(ui, |ui| self.detail(ui, inspector));

                egui::CentralPanel::default()
                    .frame(egui::Frame::NONE.fill(theme::SURFACE))
                    .show(ui, |ui| self.results_table(ui));
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
                    .hint_text("Search captured HTTP, WebSockets, Repeater, and Intruder"),
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
            } else if self.running {
                ui.spinner();
                let text = self.progress.as_ref().map_or_else(
                    || "Searching stored traffic…".to_owned(),
                    |progress| {
                        format!(
                            "Searching stored {} · {} / {}",
                            progress.source.label(),
                            progress.scanned,
                            progress.total
                        )
                    },
                );
                ui.label(RichText::new(text).small().color(theme::TEXT_MUTED));
            } else if self.ran {
                let count = self.results.len();
                let noun = if count == 1 { "result" } else { "results" };
                let mut text = format!("Search completed · {count} {noun}");
                if self.limit_reached {
                    text.push_str(&format!(" · capped at {SEARCH_RESULT_LIMIT} results"));
                }
                ui.label(RichText::new(text).small().color(theme::TEXT_MUTED));
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
                        "Searches stored HTTP and WebSocket history plus Repeater tabs and Intruder results. Encoded bodies are decoded before matching.",
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

    fn results_table(&mut self, ui: &mut egui::Ui) {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("Results").strong().color(theme::TEXT));
            if self.ran || !self.results.is_empty() {
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
        if !self.ran && !self.running {
            empty_state(
                ui,
                "Search all stored traffic",
                "Enter a term, choose request or response locations, and press Go or Enter. Stored HTTP and WebSocket history is scanned in the background; Repeater and Intruder results are searched in place.",
            );
            return;
        }
        if self.results.is_empty() {
            if self.running {
                empty_state(
                    ui,
                    "Searching stored traffic",
                    "New results appear as stored HTTP and WebSocket rows are scanned.",
                );
            } else {
                let body = if self.options.negative {
                    "Every selected location contained the term. Try a different term or turn off negative match."
                } else {
                    "Try a different term, enable more locations or tools, or check the scope and negative match options."
                };
                empty_state(ui, "No matches", body);
            }
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
                .column(Column::exact(80.0))
                .column(Column::initial(150.0).at_least(100.0))
                .column(Column::initial(360.0).at_least(200.0))
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
                        let hit = &self.results[row.index()];

                        row.set_selected(selected == Some(hit.key));
                        row.col(|ui| {
                            ui.label(
                                RichText::new(hit.key.source.label())
                                    .small()
                                    .color(theme::TEXT),
                            );
                        });
                        row.col(|ui| {
                            ui.add(
                                Label::new(RichText::new(&hit.host).color(theme::TEXT)).truncate(),
                            )
                            .on_hover_text(&hit.host);
                        });
                        row.col(|ui| {
                            ui.add(
                                Label::new(RichText::new(&hit.url).color(theme::TEXT)).truncate(),
                            )
                            .on_hover_text(&hit.url);
                        });
                        row.col(|ui| {
                            if let Some(status) = hit.status {
                                ui.label(
                                    RichText::new(status.to_string())
                                        .color(view::status_class_color(status)),
                                );
                            } else {
                                ui.label(RichText::new("—").small().color(theme::TEXT_MUTED));
                            }
                        });
                        row.col(|ui| {
                            ui.label(
                                RichText::new(view::human_bytes(hit.length))
                                    .small()
                                    .color(theme::TEXT),
                            );
                        });
                        row.col(|ui| {
                            let time = hit
                                .timestamp
                                .map_or_else(|| "—".to_owned(), view::format_time);
                            ui.label(RichText::new(time).small().color(theme::TEXT_MUTED));
                        });

                        if row.response().clicked() {
                            clicked = Some(hit.key);
                        }
                    });
                });
        });

        if let Some(key) = clicked
            && self.selected != Some(key)
        {
            self.selected = Some(key);
            self.detail = None;
        }
    }

    fn detail(&mut self, ui: &mut egui::Ui, inspector: &mut InspectorState) {
        let Some(cache) = &self.detail else {
            if self.selected.is_some() {
                empty_state(
                    ui,
                    "Result no longer available",
                    "This row is still listed, but its message is no longer in the current project or workspace. Run the search again to refresh.",
                );
            } else {
                empty_state(
                    ui,
                    "No result selected",
                    "Choose a row in the results table to inspect its request and response.",
                );
            }
            return;
        };

        let mut pane = self.pane;
        if cache.single_message {
            pane = MessagePane::Response;
        }
        if pane == MessagePane::Response && !cache.has_response {
            pane = MessagePane::Request;
        }
        if pane == MessagePane::Request && !cache.has_request {
            pane = MessagePane::Response;
        }
        let mode = match pane {
            MessagePane::Request => self.request_view,
            MessagePane::Response => self.response_view,
        };
        let text = if cache.single_message {
            message_view_text(&cache.response, mode)
        } else {
            match pane {
                MessagePane::Request => message_view_text(&cache.request, mode),
                MessagePane::Response => message_view_text(&cache.response, mode),
            }
        };
        let matches = self
            .matcher
            .as_ref()
            .map(|matcher| matcher.ranges(text))
            .unwrap_or_default();
        let capped = matches.len() >= MATCH_HIGHLIGHT_LIMIT;

        ui.add_space(1.0);
        ui.horizontal(|ui| {
            if cache.single_message {
                section_tab(ui, true, "Message");
            } else {
                if cache.has_request
                    && section_tab(ui, pane == MessagePane::Request, "Request").clicked()
                {
                    self.pane = MessagePane::Request;
                }
                if cache.has_response
                    && section_tab(ui, pane == MessagePane::Response, "Response").clicked()
                {
                    self.pane = MessagePane::Response;
                }
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
                    RichText::new(view::human_bytes(cache.length))
                        .small()
                        .color(theme::TEXT_MUTED),
                );
                if let Some(duration) = cache.duration {
                    ui.label(
                        RichText::new(view::format_duration(duration))
                            .small()
                            .color(theme::TEXT_MUTED),
                    );
                }
                if let Some(status) = cache.status {
                    ui.colored_label(view::status_class_color(status), status.to_string());
                }
                if let Some(number) = cache.number {
                    ui.label(
                        RichText::new(format!("#{number}"))
                            .small()
                            .color(theme::TEXT_MUTED),
                    );
                }
                ui.label(
                    RichText::new(cache.key.source.label())
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

        if let Some(model) = &cache.inspector {
            inspector::side_panel(ui, inspector, cache.key.id, model);
        }

        let shown_missing = match pane {
            MessagePane::Request => !cache.has_request,
            MessagePane::Response => !cache.has_response,
        };
        if shown_missing {
            let (title, body) = if pane == MessagePane::Response {
                (
                    "No response retained",
                    "This Repeater tab or Intruder result has no completed response.",
                )
            } else {
                (
                    "Request unavailable",
                    "The request for this result is no longer available in the workspace.",
                )
            };
            empty_state(ui, title, body);
            return;
        }

        message_text_with_matches(ui, cache.key.cache_id(), pane, mode, text, &matches);
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
            "Show messages that do not match in any selected location.",
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
            "Search request lines and headers, plus WebSocket connection metadata.",
        );
        changed |= checkbox(
            &mut columns[1],
            &mut options.request_body,
            "Request body",
            "Search decoded request bodies. WebSocket payloads use either body option.",
        );
        changed |= checkbox(
            &mut columns[0],
            &mut options.response_headers,
            "Response headers",
            "Search response status lines and headers, plus WebSocket connection metadata.",
        );
        changed |= checkbox(
            &mut columns[1],
            &mut options.response_body,
            "Response body",
            "Search decoded response bodies. WebSocket payloads use either body option.",
        );
    });
    changed
}

fn tools_group(ui: &mut egui::Ui, options: &mut SearchOptions) -> bool {
    let mut changed = false;
    ui.columns(2, |columns| {
        changed |= checkbox(
            &mut columns[0],
            &mut options.proxy_history,
            "Proxy history",
            "Search every stored HTTP exchange, including rows outside the loaded history snapshot.",
        );
        changed |= checkbox(
            &mut columns[1],
            &mut options.websockets,
            "WebSockets",
            "Search every stored WebSocket message and its connection URL.",
        );
        changed |= checkbox(
            &mut columns[0],
            &mut options.repeater,
            "Repeater",
            "Search the request editor and retained response of every Repeater tab.",
        );
        changed |= checkbox(
            &mut columns[1],
            &mut options.intruder,
            "Intruder",
            "Search the rendered request and retained response of every Intruder result.",
        );
    });
    changed
}

fn checkbox(ui: &mut egui::Ui, value: &mut bool, label: &str, tooltip: &str) -> bool {
    ui.checkbox(value, label).on_hover_text(tooltip).changed()
}
