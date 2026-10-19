//! Proxy tab: the HTTP history table, its filter toolbar, and the message
//! detail panes.

use egui::{Align, Label, TextEdit};
use egui_extras::{Column, TableBuilder};

use super::util::{mime_type, request_url};
use super::widgets::{empty_state, message_tab_bar, message_text, section_tab};
use super::*;

impl WiremothApp {
    pub(super) fn proxy_tab(&mut self, ui: &mut egui::Ui) {
        egui::Frame::NONE
            .fill(theme::SURFACE_ALT)
            .inner_margin(egui::Margin::symmetric(10, 5))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    if section_tab(
                        ui,
                        self.proxy_section == ProxySection::Intercept,
                        "Intercept",
                    )
                    .clicked()
                    {
                        self.proxy_section = ProxySection::Intercept;
                        ui.ctx().memory_mut(|memory| memory.stop_text_input());
                    }
                    if section_tab(
                        ui,
                        self.proxy_section == ProxySection::HttpHistory,
                        "HTTP history",
                    )
                    .clicked()
                    {
                        self.proxy_section = ProxySection::HttpHistory;
                    }
                    let rules_label = if self.rules_error.is_some() {
                        "Match/replace !"
                    } else {
                        "Match/replace"
                    };
                    let rules_hint = self
                        .rules_error
                        .as_deref()
                        .unwrap_or("Rule changes apply to traffic immediately");
                    if section_tab(
                        ui,
                        self.proxy_section == ProxySection::MatchReplace,
                        rules_label,
                    )
                    .on_hover_text(rules_hint)
                    .clicked()
                    {
                        self.proxy_section = ProxySection::MatchReplace;
                        ui.ctx().memory_mut(|memory| memory.stop_text_input());
                    }
                });
            });

        match self.proxy_section {
            ProxySection::Intercept => self.intercept_view(ui),
            ProxySection::HttpHistory => self.http_history_view(ui),
            ProxySection::MatchReplace => self.match_replace_view(ui),
        }
    }

    fn http_history_view(&mut self, ui: &mut egui::Ui) {
        egui::Frame::NONE
            .fill(theme::SURFACE)
            .inner_margin(egui::Margin::symmetric(10, 6))
            .show(ui, |ui| self.history_toolbar(ui));
        self.rebuild_visible();
        // The filter or scope can hide the selected exchange. Do not leave its
        // detail visible next to a table where the row no longer exists.
        if self.selected.is_some_and(|id| !self.visible.contains(&id)) {
            self.selected = None;
            self.detail = None;
            self.scroll_to_selected = false;
        }

        egui::Panel::bottom(PROXY_DETAIL_ID)
            .resizable(true)
            .default_size(340.0)
            .min_size(280.0)
            .frame(
                egui::Frame::NONE
                    .fill(theme::SURFACE)
                    .inner_margin(egui::Margin::same(8)),
            )
            .show(ui, |ui| self.detail_panel(ui));

        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(theme::SURFACE))
            .show(ui, |ui| self.history_table(ui));
    }

    fn history_toolbar(&mut self, ui: &mut egui::Ui) {
        let mut clear = false;
        ui.horizontal(|ui| {
            ui.label(RichText::new("HTTP history").strong().color(theme::TEXT));
            ui.separator();
            ui.label(RichText::new("Filter").color(theme::TEXT_MUTED));
            let filter_id = egui::Id::unique(FILTER_ID);
            let response = ui.add(
                TextEdit::singleline(&mut self.filter)
                    .id(filter_id)
                    .hint_text("method, host, path, status")
                    .desired_width(260.0),
            );
            if response.changed() {
                self.visible_key = None;
            }
            let clear_filter = ui
                .add_enabled(!self.filter.is_empty(), egui::Button::new("x"))
                .on_hover_text("Clear the filter (Ctrl+F to focus)")
                .on_disabled_hover_text("No filter is active");
            if clear_filter.clicked() {
                self.filter.clear();
                self.visible_key = None;
            }

            if self.scope_enabled {
                ui.separator();
                if ui
                    .link("scope filter")
                    .on_hover_text(
                        "Only in-scope hosts are shown. Click to edit the scope in Target.",
                    )
                    .clicked()
                {
                    self.main_tab = MainTab::Target;
                    self.target_section = TargetSection::Scope;
                }
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .button("Clear history")
                    .on_hover_text("Discard all captured exchanges")
                    .clicked()
                {
                    clear = true;
                }
                ui.label(
                    RichText::new(format!(
                        "{} shown / {} captured",
                        self.visible.len(),
                        self.history.len()
                    ))
                    .small()
                    .color(theme::TEXT_MUTED),
                );
            });
        });
        if clear {
            self.clear_history();
        }
    }

    fn history_table(&mut self, ui: &mut egui::Ui) {
        let rows = self.visible.len();
        if rows == 0 {
            empty_state(
                ui,
                "No exchanges to show",
                "Start the proxy, point a browser at it, and browse an HTTP site. The filter and scope can also hide the history.",
            );
            return;
        }

        let mut clicked = None;
        let mut table = TableBuilder::new(ui)
            .striped(true)
            .resizable(true)
            .sense(egui::Sense::click())
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::exact(44.0))
            .column(Column::exact(64.0))
            .column(Column::initial(420.0).at_least(180.0))
            .column(Column::exact(56.0))
            .column(Column::exact(76.0))
            .column(Column::initial(130.0).at_least(80.0))
            .column(Column::remainder().at_least(64.0));

        if self.scroll_to_selected {
            if let Some(row) = self.selected_row_index() {
                table = table.scroll_to_row(row, Some(Align::Center));
            }
            self.scroll_to_selected = false;
        }

        table
            .header(22.0, |mut header| {
                for title in ["#", "Method", "URL", "Status", "Length", "MIME type", "Time"] {
                    header.col(|ui| {
                        ui.label(
                            RichText::new(title)
                                .strong()
                                .small()
                                .color(theme::TEXT_MUTED),
                        );
                    });
                }
            })
            .body(|body| {
                body.rows(20.0, rows, |mut row| {
                    let index = row.index();
                    let id = self.visible[index];
                    let Some(item) = self.history.get(id) else {
                        return;
                    };

                    row.set_selected(self.selected == Some(id));
                    row.col(|ui| {
                        ui.label(
                            RichText::new(item.id.to_string())
                                .small()
                                .color(theme::TEXT_MUTED),
                        );
                    });
                    row.col(|ui| {
                        ui.label(RichText::new(&item.request.method).color(theme::TEXT));
                    });
                    let url = request_url(item);
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
                                .color(theme::TEXT_MUTED),
                        );
                    });
                    row.col(|ui| {
                        let mime = mime_type(item);
                        let color = if item.is_tunnel() {
                            theme::AMBER
                        } else {
                            theme::TEXT_MUTED
                        };
                        ui.add(Label::new(RichText::new(mime).small().color(color)).truncate());
                    });
                    row.col(|ui| {
                        ui.label(
                            RichText::new(view::format_time(item.timestamp))
                                .small()
                                .color(theme::TEXT_MUTED),
                        );
                    });

                    let hover = if item.is_tunnel() {
                        format!(
                            "{url}\nCONNECT tunnel: payload not decrypted until TLS interception lands."
                        )
                    } else {
                        url
                    };
                    if row.response().on_hover_text(hover).clicked() {
                        clicked = Some(id);
                    }
                });
            });

        if let Some(id) = clicked {
            self.select_id(id);
        }
    }

    pub(super) fn ensure_detail(&mut self, id: u64) {
        if self.detail.as_ref().is_some_and(|cache| cache.id == id)
            && self.history.get(id).is_some()
        {
            return;
        }
        self.detail = self.history.get(id).map(|item| DetailCache {
            id,
            request: view::request_views(item),
            response: view::response_views(item),
        });
    }

    pub(super) fn detail_panel(&mut self, ui: &mut egui::Ui) {
        let Some(id) = self.selected else {
            empty_state(
                ui,
                "Select an exchange",
                "Click a row, or use j/k and the arrow keys, to inspect the request and response.",
            );
            return;
        };
        self.ensure_detail(id);

        let mut pane = self.message_pane;
        let mut mode = self.message_view;
        let item = self.history.get(id);
        message_tab_bar(ui, &mut pane, &mut mode, item);
        self.message_pane = pane;
        self.message_view = mode;

        let Some(cache) = &self.detail else {
            empty_state(
                ui,
                "Exchange no longer in history",
                "The selected exchange was evicted or cleared.",
            );
            return;
        };
        let text = match (pane, mode) {
            (MessagePane::Request, MessageView::Raw) => &cache.request.raw,
            (MessagePane::Request, MessageView::Headers) => &cache.request.head,
            (MessagePane::Request, MessageView::Hex) => &cache.request.hex,
            (MessagePane::Response, MessageView::Raw) => &cache.response.raw,
            (MessagePane::Response, MessageView::Headers) => &cache.response.head,
            (MessagePane::Response, MessageView::Hex) => &cache.response.hex,
        };
        message_text(ui, id, pane, mode, text);
    }
}
