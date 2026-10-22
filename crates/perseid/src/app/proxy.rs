//! Proxy tab: the HTTP history table, its filter toolbar, and the message
//! detail panes.

use egui::{Align, Label, TextEdit};
use egui_extras::{Column, TableBuilder};

use super::message_view::{message_text, message_toolbar, message_view_text};
use super::util::{mime_type, request_url};
use super::widgets::{empty_state, section_bar, section_tab, section_tab_gear, table_frame};
use super::*;

impl PerseidApp {
    pub(super) fn proxy_tab(&mut self, ui: &mut egui::Ui) {
        section_bar(ui, "perseid.proxy.sections", |ui| {
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
            let settings_label = if self.tls_ca_error.is_some() {
                "Proxy settings !"
            } else {
                "Proxy settings"
            };
            let settings_hint = self
                .tls_ca_error
                .as_deref()
                .unwrap_or("Certificate authority and HTTPS decryption controls");
            ui.separator();
            if section_tab_gear(ui, self.proxy_section == ProxySection::Tls, settings_label)
                .on_hover_text(settings_hint)
                .clicked()
            {
                self.proxy_section = ProxySection::Tls;
                ui.ctx().memory_mut(|memory| memory.stop_text_input());
            }
        });

        match self.proxy_section {
            ProxySection::Intercept => self.intercept_view(ui),
            ProxySection::HttpHistory => self.http_history_view(ui),
            ProxySection::MatchReplace => self.match_replace_view(ui),
            ProxySection::Tls => self.tls_settings_view(ui),
        }
    }

    fn http_history_view(&mut self, ui: &mut egui::Ui) {
        egui::Frame::NONE
            .fill(theme::SURFACE)
            .inner_margin(egui::Margin::symmetric(8, 3))
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
            .default_size(300.0)
            .min_size(220.0)
            .frame(
                egui::Frame::NONE
                    .fill(theme::SURFACE)
                    .inner_margin(egui::Margin::same(4)),
            )
            .show(ui, |ui| self.detail_panel(ui));

        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(theme::SURFACE))
            .show(ui, |ui| self.history_table(ui));
    }

    fn history_toolbar(&mut self, ui: &mut egui::Ui) {
        let mut clear = false;
        let mut send_to_repeater = false;
        ui.horizontal(|ui| {
            ui.label(RichText::new("Filter").color(theme::TEXT_MUTED));
            let filter_id = egui::Id::unique(FILTER_ID);
            let response = ui.add(
                TextEdit::singleline(&mut self.filter)
                    .id(filter_id)
                    .hint_text("method, host, path, status")
                    .desired_width(240.0),
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
                let can_send = self
                    .selected
                    .and_then(|id| self.history.get(id))
                    .is_some_and(|item| !item.is_tunnel());
                if ui
                    .add_enabled(can_send, egui::Button::new("Send to Repeater"))
                    .on_hover_text("Replay the selected request in an editable Repeater tab")
                    .on_disabled_hover_text("Select an HTTP exchange first")
                    .clicked()
                {
                    send_to_repeater = true;
                }
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
        if send_to_repeater {
            self.send_selected_to_repeater();
        }
        if clear {
            self.clear_history();
        }
    }

    fn history_table(&mut self, ui: &mut egui::Ui) {
        // The row itself is the click target. Selectable labels inside the
        // cells would capture the click before the row response sees it.
        ui.style_mut().interaction.selectable_labels = false;
        let rows = self.visible.len();
        if rows == 0 {
            empty_state(
                ui,
                "No exchanges to show",
                "Start the proxy, point a browser at it, and browse an HTTP or HTTPS site. The filter and scope can also hide the history.",
            );
            return;
        }

        let mut clicked = None;
        table_frame(ui, |ui| {
            let mut table = TableBuilder::new(ui)
                .striped(true)
                .resizable(true)
                .sense(egui::Sense::click())
                .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                .column(Column::exact(40.0))
                .column(Column::exact(56.0))
                .column(Column::initial(420.0).at_least(180.0))
                .column(Column::exact(50.0))
                .column(Column::exact(68.0))
                .column(Column::initial(115.0).at_least(80.0))
                .column(Column::remainder().at_least(64.0));

            if self.scroll_to_selected {
                if let Some(row) = self.selected_row_index() {
                    table = table.scroll_to_row(row, Some(Align::Center));
                }
                self.scroll_to_selected = false;
            }

            table
                .header(18.0, |mut header| {
                    for title in [
                        "#",
                        "Method",
                        "URL",
                        "Status",
                        "Length",
                        "MIME type",
                        "Time",
                    ] {
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
                    body.rows(16.0, rows, |mut row| {
                        let index = row.index();
                        let id = self.visible[index];
                        let Some(item) = self.history.get(id) else {
                            return;
                        };

                        row.set_selected(self.selected == Some(id));
                        row.col(|ui| {
                            ui.label(
                                RichText::new(item.number.to_string())
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
                            format!("{url}\nCONNECT tunnel: payload was not decrypted.")
                        } else {
                            url
                        };
                        if row.response().on_hover_text(hover).clicked() {
                            clicked = Some(id);
                        }
                    });
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

        let Some(item) = self.history.get(id) else {
            empty_state(
                ui,
                "Exchange no longer in history",
                "The selected exchange was evicted or cleared.",
            );
            return;
        };
        let Some(cache) = &self.detail else {
            empty_state(
                ui,
                "Exchange no longer in history",
                "The selected exchange was evicted or cleared.",
            );
            return;
        };

        // Burp's default message-editor layout is horizontal: request and
        // response are both visible, each with its own Pretty/Raw/Hex tabs.
        // Keep the splitter resizable so wide messages still have room.
        let mut request_view = self.request_view;
        let mut response_view = self.response_view;
        let default_width = (ui.available_width() * 0.5).max(220.0);

        egui::Panel::left(PROXY_REQUEST_ID)
            .resizable(true)
            .default_size(default_width)
            .min_size(150.0)
            .frame(
                egui::Frame::NONE
                    .fill(theme::SURFACE)
                    .inner_margin(egui::Margin::same(3)),
            )
            .show(ui, |ui| {
                message_toolbar(ui, MessagePane::Request, &mut request_view, Some(item));
                message_text(
                    ui,
                    id,
                    MessagePane::Request,
                    request_view,
                    message_view_text(&cache.request, request_view),
                );
            });

        egui::CentralPanel::default()
            .frame(
                egui::Frame::NONE
                    .fill(theme::SURFACE)
                    .inner_margin(egui::Margin::same(3)),
            )
            .show(ui, |ui| {
                message_toolbar(ui, MessagePane::Response, &mut response_view, Some(item));
                message_text(
                    ui,
                    id,
                    MessagePane::Response,
                    response_view,
                    message_view_text(&cache.response, response_view),
                );
            });

        self.request_view = request_view;
        self.response_view = response_view;
    }
}
