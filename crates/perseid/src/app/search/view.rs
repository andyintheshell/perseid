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
use crate::history::History;
use crate::theme;
use crate::view;

use super::SearchState;
use super::engine::{MATCH_HIGHLIGHT_LIMIT, SearchOptions, result_host, result_url};

const SEARCH_DETAIL_ID: &str = "perseid.search.detail";
const SEARCH_QUERY_ID: &str = "perseid.search.query";
const SEARCH_RESULTS_ID: &str = "perseid.search.results";

impl SearchState {
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
