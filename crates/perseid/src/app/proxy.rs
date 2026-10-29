//! Proxy tab: the HTTP history table, its filter toolbar, and the message
//! detail panes.

use egui::{Align, Label, TextEdit};
use egui_extras::{Column, TableBuilder};

use super::message_view::{
    message_tabs, message_text, message_text_labeled, message_toolbar, message_view_text,
};
use super::util::{
    history_title, mime_type, request_extension, request_host_label, request_path_label,
    request_url, request_uses_tls,
};
use super::widgets::{
    check_mark, empty_state, section_bar, section_tab, section_tab_gear, table_frame,
};
use super::*;
use crate::site_map;

#[derive(Clone, Copy)]
enum HistoryRowAction {
    Repeater,
    Intruder,
    Sequencer,
    ComparerRequest,
    ComparerResponse,
    ClearHistory,
}

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
            if section_tab(
                ui,
                self.proxy_section == ProxySection::WebSocketsHistory,
                "WebSockets history",
            )
            .clicked()
            {
                self.proxy_section = ProxySection::WebSocketsHistory;
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
            ProxySection::WebSocketsHistory => self.websocket_history_view(ui),
            ProxySection::MatchReplace => self.match_replace_view(ui),
            ProxySection::Tls => self.tls_settings_view(ui),
        }
    }

    fn http_history_view(&mut self, ui: &mut egui::Ui) {
        self.rebuild_visible();
        egui::Frame::NONE
            .fill(theme::SURFACE)
            .inner_margin(egui::Margin::symmetric(10, 4))
            .show(ui, |ui| self.history_toolbar(ui));
        // The filter or scope can hide the selected exchange. Do not leave its
        // detail visible next to a table where the row no longer exists.
        if self.selected.is_some_and(|id| !self.visible.contains(&id)) {
            self.selected = None;
            self.detail = None;
            self.scroll_to_selected = false;
        }

        #[cfg(feature = "screenshots")]
        if std::mem::take(&mut self.reset_http_detail_size) {
            let id = ui.scope_id().with(PROXY_DETAIL_ID);
            ui.ctx()
                .data_mut(|data| data.remove::<egui::PanelState>(id));
        }

        egui::Panel::bottom(PROXY_DETAIL_ID)
            .resizable(true)
            // Burp's history detail takes close to half of the workspace.
            // Computing it from the available height keeps that proportion on
            // both compact and large windows.
            .default_size((ui.available_height() * 0.52).max(260.0))
            .min_size(220.0)
            .frame(
                egui::Frame::NONE
                    .fill(theme::SURFACE)
                    .inner_margin(egui::Margin::same(6)),
            )
            .show(ui, |ui| self.detail_panel(ui));

        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(theme::SURFACE))
            .show(ui, |ui| self.history_table(ui));
    }

    fn websocket_history_view(&mut self, ui: &mut egui::Ui) {
        egui::Frame::NONE
            .fill(theme::SURFACE)
            .inner_margin(egui::Margin::symmetric(10, 4))
            .show(ui, |ui| self.websocket_history_toolbar(ui));
        self.rebuild_websocket_visible();
        if self
            .websocket_selected
            .is_some_and(|id| !self.websocket_visible.contains(&id))
        {
            self.websocket_selected = None;
            self.websocket_detail = None;
            self.websocket_scroll_to_selected = false;
        }

        egui::Panel::bottom(PROXY_WEBSOCKET_DETAIL_ID)
            .resizable(true)
            .default_size(300.0)
            .min_size(220.0)
            .frame(
                egui::Frame::NONE
                    .fill(theme::SURFACE)
                    .inner_margin(egui::Margin::same(6)),
            )
            .show(ui, |ui| self.websocket_detail_panel(ui));

        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(theme::SURFACE))
            .show(ui, |ui| self.websocket_history_table(ui));
    }

    fn websocket_history_toolbar(&mut self, ui: &mut egui::Ui) {
        let mut clear = false;
        ui.horizontal(|ui| {
            ui.label(RichText::new("Filter").color(theme::TEXT));
            let filter_id = egui::Id::unique(WEBSOCKET_FILTER_ID);
            let response = ui.add(
                TextEdit::singleline(&mut self.websocket_filter)
                    .id(filter_id)
                    .hint_text("URL, direction, WebSocket ID, protocol")
                    .desired_width(300.0),
            );
            if response.changed() {
                self.websocket_visible_key = None;
            }
            let clear_filter = ui
                .add_enabled(!self.websocket_filter.is_empty(), egui::Button::new("x"))
                .on_hover_text("Clear the filter (Ctrl+F to focus)")
                .on_disabled_hover_text("No filter is active");
            if clear_filter.clicked() {
                self.websocket_filter.clear();
                self.websocket_visible_key = None;
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
                    .on_hover_text("Discard all captured WebSocket messages")
                    .clicked()
                {
                    clear = true;
                }
                ui.label(
                    RichText::new(format!(
                        "{} shown / {} captured",
                        self.websocket_visible.len(),
                        self.websocket_history.len()
                    ))
                    .small()
                    .color(theme::TEXT_MUTED),
                )
                .on_hover_text(format!(
                    "{} retained of {} budget; {} evicted",
                    view::human_bytes(self.websocket_history.retained_bytes() as u64),
                    view::human_bytes(self.websocket_history.byte_limit() as u64),
                    self.websocket_history.evicted()
                ));
                ui.separator();
                ui.label(
                    RichText::new("Captured even when Intercept is off")
                        .small()
                        .color(theme::TEXT_MUTED),
                );
            });
        });
        if clear {
            self.clear_websocket_history();
        }
    }

    fn websocket_history_table(&mut self, ui: &mut egui::Ui) {
        ui.style_mut().interaction.selectable_labels = false;
        let rows = self.websocket_visible.len();
        if rows == 0 {
            empty_state(
                ui,
                "No WebSocket messages to show",
                "WebSocket text and binary messages appear here as they are captured, including decrypted wss:// traffic. Control frames and incomplete fragments are not listed.",
            );
            return;
        }

        let mut clicked = None;
        let mut send_to_repeater = None;
        table_frame(ui, |ui| {
            let mut table = TableBuilder::new(ui)
                .striped(true)
                .resizable(true)
                .sense(egui::Sense::click())
                .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                .column(Column::exact(40.0))
                .column(Column::initial(430.0).at_least(180.0))
                .column(Column::exact(78.0))
                .column(Column::exact(76.0))
                .column(Column::exact(42.0))
                .column(Column::initial(88.0).at_least(70.0))
                .column(Column::exact(80.0))
                .column(Column::remainder().at_least(84.0));

            if self.websocket_scroll_to_selected {
                if let Some(row) = self.selected_websocket_row_index() {
                    table = table.scroll_to_row(row, Some(Align::Center));
                }
                self.websocket_scroll_to_selected = false;
            }

            table
                .header(20.0, |mut header| {
                    for title in [
                        "#",
                        "URL",
                        "Direction",
                        "Length",
                        "TLS",
                        "Time",
                        "Listener port",
                        "WebSocket ID",
                    ] {
                        header.col(|ui| {
                            ui.label(RichText::new(title).strong().small().color(theme::TEXT));
                        });
                    }
                })
                .body(|body| {
                    body.rows(18.0, rows, |mut row| {
                        let index = row.index();
                        let id = self.websocket_visible[index];
                        let Some(item) = self.websocket_history.get(id) else {
                            return;
                        };

                        row.set_selected(self.websocket_selected == Some(id));
                        row.col(|ui| {
                            ui.label(
                                RichText::new(item.number.to_string())
                                    .small()
                                    .color(theme::TEXT),
                            );
                        });
                        row.col(|ui| {
                            ui.add(
                                Label::new(RichText::new(&item.connection.url).color(theme::TEXT))
                                    .truncate(),
                            )
                            .on_hover_text(format!(
                                "{}\n{} message{}",
                                item.connection.url,
                                view::websocket_opcode_text(item.opcode),
                                item.connection
                                    .protocol
                                    .as_deref()
                                    .map(|protocol| format!("\nsubprotocol: {protocol}"))
                                    .unwrap_or_default()
                            ));
                        });
                        row.col(|ui| {
                            let color = match item.direction {
                                crate::proxy::WebSocketDirection::ToServer => theme::AMBER,
                                crate::proxy::WebSocketDirection::ToClient => theme::BLUE,
                            };
                            ui.label(
                                RichText::new(view::websocket_direction_text(item.direction))
                                    .small()
                                    .color(color),
                            );
                        });
                        row.col(|ui| {
                            ui.label(
                                RichText::new(view::human_bytes(item.payload.total_len()))
                                    .small()
                                    .color(theme::TEXT),
                            );
                        });
                        row.col(|ui| {
                            ui.label(
                                RichText::new(if item.connection.tls { "TLS" } else { "-" })
                                    .small()
                                    .color(if item.connection.tls {
                                        theme::GREEN
                                    } else {
                                        theme::TEXT_MUTED
                                    }),
                            );
                        });
                        row.col(|ui| {
                            ui.label(
                                RichText::new(view::format_time(item.timestamp))
                                    .small()
                                    .color(theme::TEXT_MUTED),
                            );
                        });
                        row.col(|ui| {
                            ui.label(
                                RichText::new(item.connection.listener_port.to_string())
                                    .small()
                                    .color(theme::TEXT_MUTED),
                            );
                        });
                        row.col(|ui| {
                            ui.label(
                                RichText::new(item.connection.id.to_string())
                                    .small()
                                    .color(theme::TEXT_MUTED),
                            );
                        });

                        let response = row.response();
                        if response.clicked() {
                            clicked = Some(id);
                        }
                        response.context_menu(|ui| {
                            if ui.button("Send to Repeater").clicked() {
                                send_to_repeater = Some(id);
                                ui.close();
                            }
                        });
                    });
                });
        });

        if let Some(id) = clicked {
            self.select_websocket_id(id);
        }
        if let Some(item) = send_to_repeater
            .and_then(|id| self.websocket_history.get(id))
            .cloned()
        {
            self.send_websocket_to_repeater(&item);
        }
    }

    fn ensure_websocket_detail(&mut self, id: u64) {
        if self
            .websocket_detail
            .as_ref()
            .is_some_and(|cache| cache.id == id)
            && self.websocket_history.get(id).is_some()
        {
            return;
        }
        self.websocket_detail = self
            .websocket_history
            .get(id)
            .map(|item| WebSocketDetailCache {
                id,
                views: view::websocket_message_views(item),
            });
    }

    fn websocket_detail_panel(&mut self, ui: &mut egui::Ui) {
        let Some(id) = self.websocket_selected else {
            empty_state(
                ui,
                "Select a WebSocket message",
                "Click a row, or use j/k and the arrow keys, to inspect a complete text or binary message.",
            );
            return;
        };
        self.ensure_websocket_detail(id);

        let Some(item) = self.websocket_history.get(id) else {
            empty_state(
                ui,
                "Message no longer in history",
                "The selected message was evicted or cleared.",
            );
            return;
        };
        let Some(cache) = &self.websocket_detail else {
            empty_state(
                ui,
                "Message no longer in history",
                "The selected message was evicted or cleared.",
            );
            return;
        };

        ui.horizontal(|ui| {
            ui.label(
                RichText::new("WebSocket message")
                    .strong()
                    .color(theme::TEXT),
            );
            ui.label(
                RichText::new(view::websocket_direction_text(item.direction))
                    .small()
                    .color(match item.direction {
                        crate::proxy::WebSocketDirection::ToServer => theme::AMBER,
                        crate::proxy::WebSocketDirection::ToClient => theme::BLUE,
                    }),
            );
            ui.label(
                RichText::new(view::websocket_opcode_text(item.opcode))
                    .small()
                    .color(theme::TEXT_MUTED),
            );
            if let Some(protocol) = &item.connection.protocol {
                ui.label(
                    RichText::new(protocol.as_str())
                        .small()
                        .color(theme::TEXT_MUTED),
                );
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    RichText::new(format!("WebSocket #{}", item.connection.id))
                        .small()
                        .color(theme::TEXT_MUTED),
                );
                ui.label(
                    RichText::new(format!("row {}", item.number))
                        .small()
                        .color(theme::TEXT_MUTED),
                );
                ui.label(
                    RichText::new(format!(
                        "{} / {}",
                        view::format_time(item.timestamp),
                        view::human_bytes(item.payload.total_len())
                    ))
                    .small()
                    .color(theme::TEXT_MUTED),
                );
                if item.payload.is_truncated() {
                    ui.label(
                        RichText::new("truncated to capture limit")
                            .small()
                            .color(theme::AMBER),
                    );
                }
            });
        });
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(&item.connection.url)
                    .small()
                    .color(theme::TEXT_MUTED),
            );
            ui.label(
                RichText::new(format!(
                    "listener {} · {}",
                    item.connection.listener_port,
                    if item.connection.tls { "TLS" } else { "plain" }
                ))
                .small()
                .color(theme::TEXT_MUTED),
            );
        });
        ui.add_space(2.0);

        let mut message_view = self.websocket_view;
        message_tabs(ui, &mut message_view);
        message_text_labeled(
            ui,
            id,
            MessagePane::Request,
            message_view,
            message_view_text(&cache.views, message_view),
            "Copy WebSocket message",
        );
        self.websocket_view = message_view;
    }

    fn history_table(&mut self, ui: &mut egui::Ui) {
        // The row itself is the click target. Selectable labels inside the
        // cells would capture the click before the row response sees it.
        ui.style_mut().interaction.selectable_labels = false;
        let rows = self.visible.len();
        if rows == 0 {
            if self.history.len() == 0 {
                empty_state(
                    ui,
                    "No exchanges to show",
                    "Start the proxy, point a browser at it, and browse an HTTP or HTTPS site. The filter and scope can also hide the history.",
                );
            } else {
                empty_state(
                    ui,
                    "No exchanges match the filter",
                    "Relax the display filter, clear the search, or turn off scope filtering. Filters never delete captured exchanges.",
                );
                ui.add_space(10.0);
                ui.vertical_centered(|ui| {
                    if ui.button("Show all exchanges").clicked() {
                        self.history_filter = HistoryFilter::SHOW_ALL;
                        self.filter.clear();
                        self.visible_key = None;
                    }
                    if self.scope_enabled && ui.button("Turn off scope filtering").clicked() {
                        self.scope_enabled = false;
                        self.invalidate_views();
                    }
                });
            }
            return;
        }

        let mut clicked = None;
        let mut action = None;
        table_frame(ui, |ui| {
            let mut table = TableBuilder::new(ui)
                .striped(true)
                .resizable(true)
                .sense(egui::Sense::click())
                .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                .column(Column::exact(40.0))
                .column(Column::initial(175.0).at_least(110.0))
                .column(Column::exact(58.0))
                .column(Column::initial(280.0).at_least(150.0))
                .column(Column::exact(48.0))
                .column(Column::exact(62.0))
                .column(Column::exact(70.0))
                .column(Column::initial(72.0).at_least(54.0))
                .column(Column::exact(54.0))
                .column(Column::initial(130.0).at_least(70.0))
                .column(Column::exact(38.0))
                .column(Column::remainder().at_least(74.0));

            if self.scroll_to_selected {
                if let Some(row) = self.selected_row_index() {
                    table = table.scroll_to_row(row, Some(Align::Center));
                }
                self.scroll_to_selected = false;
            }

            table
                .header(20.0, |mut header| {
                    for title in [
                        "#",
                        "Host",
                        "Method",
                        "URL",
                        "Params",
                        "Status code",
                        "Length",
                        "MIME type",
                        "Extension",
                        "Title",
                        "TLS",
                        "Time",
                    ] {
                        header.col(|ui| {
                            ui.label(RichText::new(title).strong().small().color(theme::TEXT));
                        });
                    }
                })
                .body(|body| {
                    body.rows(18.0, rows, |mut row| {
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
                                    .color(theme::TEXT),
                            );
                        });
                        let url = request_url(item);
                        row.col(|ui| {
                            ui.add(
                                Label::new(
                                    RichText::new(request_host_label(item))
                                        .small()
                                        .color(theme::TEXT),
                                )
                                .truncate(),
                            )
                            .on_hover_text(&url);
                        });
                        row.col(|ui| {
                            ui.label(
                                RichText::new(&item.request.method)
                                    .small()
                                    .color(theme::TEXT),
                            );
                        });
                        let path = request_path_label(item);
                        let path = if path.is_empty() {
                            "-".to_string()
                        } else {
                            path
                        };
                        row.col(|ui| {
                            ui.add(Label::new(RichText::new(&path).color(theme::TEXT)).truncate())
                                .on_hover_text(&url);
                        });
                        let parameters =
                            site_map::request_parameter_count(&item.request, &item.request_body);
                        row.col(|ui| {
                            if parameters.count > 0 {
                                check_mark(ui, theme::GREEN).on_hover_text(format!(
                                    "{} parameter{}",
                                    parameters.count,
                                    if parameters.count == 1 { "" } else { "s" }
                                ));
                            } else {
                                ui.label(RichText::new("").small());
                            }
                        });
                        row.col(|ui| {
                            ui.label(
                                RichText::new(view::status_text(item))
                                    .small()
                                    .color(view::status_color(item)),
                            );
                        });
                        row.col(|ui| {
                            ui.label(
                                RichText::new(item.transferred().to_string())
                                    .small()
                                    .color(theme::TEXT),
                            )
                            .on_hover_text(format!(
                                "{} transferred",
                                view::human_bytes(item.transferred())
                            ));
                        });
                        row.col(|ui| {
                            let mime = site_map::mime_label(&mime_type(item));
                            let mime = if mime.is_empty() { "-".to_string() } else { mime };
                            let color = if item.is_tunnel() {
                                theme::AMBER
                            } else {
                                theme::TEXT
                            };
                            ui.add(
                                Label::new(RichText::new(&mime).small().color(color)).truncate(),
                            )
                            .on_hover_text(mime_type(item));
                        });
                        row.col(|ui| {
                            let extension = request_extension(item);
                            ui.label(
                                RichText::new(extension)
                                    .small()
                                    .color(theme::TEXT_MUTED),
                            );
                        });
                        row.col(|ui| {
                            let title = history_title(&self.title_cache, item);
                            if title.is_empty() {
                                ui.label(RichText::new("").small());
                            } else {
                                ui.add(
                                    Label::new(RichText::new(&title).small().color(theme::TEXT))
                                        .truncate(),
                                )
                                .on_hover_text(title);
                            }
                        });
                        row.col(|ui| {
                            if request_uses_tls(item) {
                                check_mark(ui, theme::GREEN).on_hover_text("TLS");
                            } else {
                                ui.label(
                                    RichText::new("-").small().color(theme::TEXT_MUTED),
                                );
                            }
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
                        let can_send = !item.is_tunnel();
                        let has_response = item.response.is_some();
                        let can_capture = has_response && !item.is_in_flight();
                        let response = row.response().on_hover_text(hover);
                        if response.clicked() || response.secondary_clicked() {
                            clicked = Some(id);
                        }
                        response.context_menu(|ui| {
                            if ui
                                .add_enabled(can_send, egui::Button::new("Send to Repeater"))
                                .on_hover_text(
                                    "Replay the highlighted request in an editable Repeater tab",
                                )
                                .on_disabled_hover_text(
                                    "CONNECT tunnels do not contain an HTTP request",
                                )
                                .clicked()
                            {
                                action = Some(HistoryRowAction::Repeater);
                                ui.close();
                            }
                            if ui
                                .add_enabled(can_send, egui::Button::new("Send to Intruder"))
                                .on_hover_text(
                                    "Open the highlighted request in the Intruder workspace",
                                )
                                .on_disabled_hover_text(
                                    "CONNECT tunnels do not contain an HTTP request",
                                )
                                .clicked()
                            {
                                action = Some(HistoryRowAction::Intruder);
                                ui.close();
                            }
                            if can_send {
                                ui.menu_button("Send request to Decoder", |ui| {
                                    if let Some(bytes) = view::captured_message_bytes(item, false) {
                                        super::message_view::decoder_send_menu(ui, &bytes, "Captured request (reconstructed head)");
                                    }
                                });
                            }
                            if has_response {
                                ui.menu_button("Send response to Decoder", |ui| {
                                    if let Some(bytes) = view::captured_message_bytes(item, true) {
                                        super::message_view::decoder_send_menu(ui, &bytes, "Captured response (reconstructed head)");
                                    }
                                });
                            }
                            ui.separator();
                            if ui
                                .add_enabled(
                                    can_send,
                                    egui::Button::new("Send request to Comparer"),
                                )
                                .on_hover_text("Add the highlighted request to Comparer item 1")
                                .on_disabled_hover_text(
                                    "CONNECT tunnels do not contain an HTTP request",
                                )
                                .clicked()
                            {
                                action = Some(HistoryRowAction::ComparerRequest);
                                ui.close();
                            }
                            if ui
                                .add_enabled(
                                    has_response,
                                    egui::Button::new("Send response to Comparer"),
                                )
                                .on_hover_text("Add the highlighted response to Comparer item 2")
                                .on_disabled_hover_text("The highlighted exchange has no response")
                                .clicked()
                            {
                                action = Some(HistoryRowAction::ComparerResponse);
                                ui.close();
                            }
                            ui.separator();
                            if ui
                                .add_enabled(
                                    can_capture,
                                    egui::Button::new("Send response to Sequencer"),
                                )
                                .on_hover_text(
                                    "Extract tokens from the highlighted response with the Sequencer capture regex",
                                )
                                .on_disabled_hover_text(
                                    "Wait for a response before extracting tokens",
                                )
                                .clicked()
                            {
                                action = Some(HistoryRowAction::Sequencer);
                                ui.close();
                            }
                            super::curl::context_menu(ui, item);
                            ui.separator();
                            if ui
                                .button("Clear history")
                                .on_hover_text("Discard all captured exchanges")
                                .clicked()
                            {
                                action = Some(HistoryRowAction::ClearHistory);
                                ui.close();
                            }
                        });
                    });
                });
        });

        if let Some(id) = clicked {
            self.select_id(id);
        }
        match action {
            Some(HistoryRowAction::Repeater) => self.send_selected_to_repeater(),
            Some(HistoryRowAction::Intruder) => self.send_selected_to_intruder(),
            Some(HistoryRowAction::Sequencer) => self.send_selected_to_sequencer(),
            Some(HistoryRowAction::ComparerRequest) => {
                self.send_selected_to_comparer(MessagePane::Request);
            }
            Some(HistoryRowAction::ComparerResponse) => {
                self.send_selected_to_comparer(MessagePane::Response);
            }
            Some(HistoryRowAction::ClearHistory) => self.clear_history(),
            None => {}
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
            inspector: super::inspector::InspectorModel::from_history(item),
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
        // The Inspector occupies a side panel on the right, with its own
        // vertical tab rail. Keep the splitter resizable so wide messages
        // still have room.
        let mut request_view = self.request_view;
        let mut response_view = self.response_view;
        let default_width = (ui.available_width() * 0.5).max(220.0);

        super::inspector::side_panel(ui, &mut self.inspector, id, &cache.inspector);

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
                super::message_view::request_message_text_with_curl(
                    ui,
                    id,
                    request_view,
                    item,
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

#[cfg(test)]
mod decoder_transfer_tests {
    use super::*;
    use crate::proxy::{
        CapturedBody, ExchangeEnd, ExchangeResult, ProxyEvent, RequestInfo, ResponseHead,
    };
    use std::time::{Duration, SystemTime};

    fn frame(
        ctx: &egui::Context,
        app: &mut PerseidApp,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1400.0, 900.0),
            )),
            events,
            ..Default::default()
        };
        ctx.run_ui(input, |ui| app.main_content(ui))
    }

    fn label_pos(output: &egui::FullOutput, label: &str) -> Option<egui::Pos2> {
        fn find(shape: &egui::Shape, label: &str) -> Option<egui::Pos2> {
            match shape {
                egui::Shape::Text(text) if text.galley.text() == label => {
                    Some(text.pos + text.galley.size() * 0.5)
                }
                egui::Shape::Vec(shapes) => shapes.iter().find_map(|shape| find(shape, label)),
                _ => None,
            }
        }
        output
            .shapes
            .iter()
            .find_map(|shape| find(&shape.shape, label))
    }

    fn click(ctx: &egui::Context, app: &mut PerseidApp, label: &str, button: egui::PointerButton) {
        let output = frame(ctx, app, vec![]);
        let pos = label_pos(&output, label).unwrap_or_else(|| panic!("missing label {label}"));
        output.drop_without_applying_deltas();
        for event in [
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::PointerButton {
                pos,
                button,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            },
        ] {
            frame(ctx, app, vec![event]).drop_without_applying_deltas();
        }
    }

    #[test]
    fn history_menus_send_captured_request_and_response_to_decoder() {
        for (menu, expected) in [
            ("Send request to Decoder", &[0, 255][..]),
            ("Send response to Decoder", &[254, 0][..]),
        ] {
            let mut app = PerseidApp::test_app();
            app.main_tab = MainTab::Proxy;
            app.proxy_section = ProxySection::HttpHistory;
            app.history.apply(ProxyEvent::Started {
                id: 1,
                timestamp: SystemTime::UNIX_EPOCH,
                request: RequestInfo {
                    method: "POST".into(),
                    uri: "http://example.test/path".into(),
                    scheme: "http".into(),
                    host: "example.test".into(),
                    path: "/path".into(),
                    version: "HTTP/1.1".into(),
                    headers: vec![],
                },
            });
            app.history.apply(ProxyEvent::ResponseHead {
                id: 1,
                head: ResponseHead {
                    version: "HTTP/1.1".into(),
                    status: 200,
                    reason: "OK".into(),
                    headers: vec![],
                },
            });
            app.history.apply(ProxyEvent::Finished {
                id: 1,
                end: ExchangeEnd {
                    duration: Duration::ZERO,
                    request_body: CapturedBody::from_bytes(vec![0, 255]),
                    result: ExchangeResult::Response {
                        body: CapturedBody::from_bytes(vec![254, 0]),
                    },
                },
            });
            let ctx = egui::Context::default();
            ctx.all_styles_mut(|style| style.animation_time = 0.0);
            frame(&ctx, &mut app, vec![]).drop_without_applying_deltas();
            click(&ctx, &mut app, "/path", egui::PointerButton::Secondary);
            click(&ctx, &mut app, menu, egui::PointerButton::Primary);
            click(&ctx, &mut app, "Append panel", egui::PointerButton::Primary);
            assert_eq!(app.main_tab, MainTab::Decoder);
            let panels = app.decoder_workspace();
            assert_eq!(panels.len(), 1);
            assert!(panels[0].data.ends_with(expected));
            assert!(panels[0].data.windows(4).any(|bytes| bytes == b"\r\n\r\n"));
        }
    }
}
