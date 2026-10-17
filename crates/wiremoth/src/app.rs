//! The egui application: proxy history table plus request/response panes.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use egui::{Align, Color32, Key, Label, RichText, TextEdit};
use egui_extras::{Column, TableBuilder};
use wiremoth_proxy::{Proxy, ProxyConfig};

use crate::Args;
use crate::history::{History, HistoryItem};
use crate::view;

/// Bound on events applied per frame so a flood cannot stall rendering.
const MAX_EVENTS_PER_FRAME: usize = 1024;
const FILTER_ID: &str = "wiremoth.filter";
const RESPONSE_PANE_ID: &str = "wiremoth.detail.response";
const ORANGE: Color32 = Color32::from_rgb(255, 176, 80);

pub struct WiremothApp {
    proxy: Option<Proxy>,
    proxy_error: Option<String>,
    notify: Arc<dyn Fn() + Send + Sync>,
    listen: SocketAddr,
    capture_limit: usize,
    history: History,
    paused: bool,
    filter: String,
    visible: Vec<u64>,
    visible_key: Option<(String, u64)>,
    selected: Option<u64>,
    scroll_to_selected: bool,
    detail: Option<DetailCache>,
    filter_id: Option<egui::Id>,
}

/// Formatted request/response text for the selected exchange, rebuilt only
/// when the selection or the history changes.
struct DetailCache {
    id: u64,
    revision: u64,
    request: String,
    response: String,
}

impl WiremothApp {
    pub fn new(cc: &eframe::CreationContext<'_>, args: Args) -> Self {
        let notify: Arc<dyn Fn() + Send + Sync> = {
            let ctx = cc.egui_ctx.clone();
            Arc::new(move || ctx.request_repaint())
        };

        let mut app = Self {
            proxy: None,
            proxy_error: None,
            notify,
            listen: args.listen,
            capture_limit: args.capture_limit,
            history: History::new(args.history_limit),
            paused: false,
            filter: String::new(),
            visible: Vec::new(),
            visible_key: None,
            selected: None,
            scroll_to_selected: false,
            detail: None,
            filter_id: None,
        };
        if !args.no_proxy {
            let notify = app.notify.clone();
            app.start_proxy(notify);
        }
        app
    }

    fn start_proxy(&mut self, notify: Arc<dyn Fn() + Send + Sync>) {
        let config = ProxyConfig {
            listen: self.listen,
            capture_limit: self.capture_limit,
            notify: Some(notify),
            ..ProxyConfig::default()
        };
        match Proxy::start(config) {
            Ok(proxy) => {
                log::info!("proxy listening on {}", proxy.local_addr());
                self.proxy = Some(proxy);
                self.proxy_error = None;
            }
            Err(err) => {
                log::error!("could not start the proxy: {err}");
                self.proxy = None;
                self.proxy_error = Some(err.to_string());
            }
        }
    }

    fn drain_events(&mut self, ctx: &egui::Context) {
        let Some(proxy) = &self.proxy else {
            return;
        };
        for _ in 0..MAX_EVENTS_PER_FRAME {
            let Some(event) = proxy.try_recv() else {
                break;
            };
            let id = self.history.apply(event);
            if self.selected == Some(id) {
                self.detail = None;
            }
        }
        // Keep durations and in-flight rows ticking while anything is open.
        if self.history.in_flight() > 0 {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }

    fn rebuild_visible(&mut self) {
        let key = (self.filter.clone(), self.history.revision());
        if self.visible_key.as_ref() == Some(&key) {
            return;
        }
        self.visible_key = Some(key);
        self.visible.clear();

        let needle = self.filter.trim().to_lowercase();
        if needle.is_empty() {
            self.visible.extend(self.history.iter().map(|item| item.id));
        } else {
            for item in self.history.iter() {
                if matches_filter(item, &needle) {
                    self.visible.push(item.id);
                }
            }
        }
    }

    fn clear_history(&mut self) {
        self.history.clear();
        self.selected = None;
        self.detail = None;
        self.visible_key = None;
    }

    fn selected_row_index(&self) -> Option<usize> {
        self.selected
            .and_then(|id| self.visible.iter().position(|&visible| visible == id))
    }

    fn move_selection(&mut self, delta: isize) {
        if self.visible.is_empty() {
            self.selected = None;
            return;
        }
        let next = match self.selected_row_index() {
            Some(position) => (position as isize + delta)
                .clamp(0, self.visible.len() as isize - 1) as usize,
            None => 0,
        };
        self.select_id(self.visible[next]);
    }

    fn select_id(&mut self, id: u64) {
        if self.selected != Some(id) {
            self.selected = Some(id);
            self.detail = None;
        }
        self.scroll_to_selected = true;
    }

    fn keyboard(&mut self, ctx: &egui::Context) {
        let Some(filter_id) = self.filter_id else {
            return;
        };

        if ctx.input(|i| i.modifiers.command && i.key_pressed(Key::F)) {
            ctx.memory_mut(|memory| memory.request_focus(filter_id));
            return;
        }

        if ctx.memory(|memory| memory.focused()) == Some(filter_id) {
            if ctx.input(|i| i.key_pressed(Key::Escape)) {
                ctx.memory_mut(|memory| memory.surrender_focus(filter_id));
            }
            return;
        }
        if ctx.egui_wants_keyboard_input() {
            return;
        }

        if ctx.input(|i| i.key_pressed(Key::ArrowDown) || i.key_pressed(Key::J)) {
            self.move_selection(1);
        }
        if ctx.input(|i| i.key_pressed(Key::ArrowUp) || i.key_pressed(Key::K)) {
            self.move_selection(-1);
        }
        if ctx.input(|i| i.key_pressed(Key::Home)) && !self.visible.is_empty() {
            self.select_id(self.visible[0]);
        }
        if ctx.input(|i| i.key_pressed(Key::End)) && !self.visible.is_empty() {
            let last = self.visible[self.visible.len() - 1];
            self.select_id(last);
        }
    }

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        ui.add_space(3.0);
        ui.horizontal(|ui| {
            ui.heading("Wiremoth");
            ui.separator();

            if let Some(proxy) = &self.proxy {
                ui.colored_label(Color32::LIGHT_GREEN, format!("proxy on {}", proxy.local_addr()));
            } else {
                ui.colored_label(Color32::LIGHT_RED, "proxy not running");
                if ui.button("Retry").clicked() {
                    let notify = self.notify.clone();
                    self.start_proxy(notify);
                }
            }
            if let Some(error) = &self.proxy_error {
                ui.label(RichText::new(error).color(ORANGE));
            }

            ui.separator();
            let mut capturing = !self.paused;
            if ui
                .checkbox(&mut capturing, "Capture")
                .on_hover_text("Toggle whether new exchanges are recorded")
                .changed()
            {
                self.paused = !capturing;
                if let Some(proxy) = &self.proxy {
                    proxy.set_paused(self.paused);
                }
            }
            if ui
                .button("Clear")
                .on_hover_text("Discard all captured exchanges")
                .clicked()
            {
                self.clear_history();
            }

            ui.separator();
            ui.label("Filter");
            let filter_id = ui.make_persistent_id(FILTER_ID);
            self.filter_id = Some(filter_id);
            ui.add(
                TextEdit::singleline(&mut self.filter)
                    .id(filter_id)
                    .hint_text("method, host, path, status")
                    .desired_width(220.0),
            );
            if ui
                .button("x")
                .on_hover_text("Clear the filter (Ctrl+F to focus)")
                .clicked()
            {
                self.filter.clear();
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(format!(
                    "{} shown / {} captured",
                    self.visible.len(),
                    self.history.len()
                ));
            });
        });
        ui.add_space(3.0);
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            ui.label(format!("{} exchanges", self.history.len()));

            let in_flight = self.history.in_flight();
            if in_flight > 0 {
                ui.separator();
                ui.colored_label(Color32::LIGHT_BLUE, format!("{in_flight} in flight"));
            }

            if let Some(proxy) = &self.proxy {
                let dropped = proxy.dropped_events();
                if dropped > 0 {
                    ui.separator();
                    ui.colored_label(ORANGE, format!("{dropped} events dropped (consumer too slow)"));
                }
            }
            let evicted = self.history.evicted();
            if evicted > 0 {
                ui.separator();
                ui.label(format!("{evicted} evicted by history limit"));
            }

            ui.separator();
            ui.label(format!("capture limit {}", view::human_bytes(self.capture_limit as u64)));

            if self.paused {
                ui.separator();
                ui.colored_label(ORANGE, "capture paused");
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.weak("HTTPS: CONNECT passthrough — TLS interception not implemented yet");
            });
        });
        ui.add_space(2.0);
    }

    fn history_table(&mut self, ui: &mut egui::Ui) {
        let mut clicked = None;
        let rows = self.visible.len();

        let mut table = TableBuilder::new(ui)
            .striped(true)
            .resizable(true)
            .sense(egui::Sense::click())
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::exact(48.0))
            .column(Column::exact(62.0))
            .column(Column::exact(66.0))
            .column(Column::exact(52.0))
            .column(Column::initial(170.0).at_least(80.0))
            .column(Column::remainder().at_least(120.0))
            .column(Column::exact(76.0))
            .column(Column::exact(64.0));

        if self.scroll_to_selected {
            if let Some(row) = self.selected_row_index() {
                table = table.scroll_to_row(row, Some(Align::Center));
            }
            self.scroll_to_selected = false;
        }

        table
            .header(20.0, |mut header| {
                for title in ["#", "At", "Method", "Status", "Host", "Path", "Size", "Took"] {
                    header.col(|ui| {
                        ui.strong(title);
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
                        ui.label(item.id.to_string());
                    });
                    row.col(|ui| {
                        ui.label(view::format_time(item.timestamp));
                    });
                    row.col(|ui| {
                        ui.label(item.request.method.as_str());
                    });
                    row.col(|ui| {
                        ui.label(
                            RichText::new(view::status_text(item)).color(view::status_color(item)),
                        );
                    });
                    row.col(|ui| {
                        ui.add(Label::new(item.request.host.as_str()).truncate());
                    });
                    row.col(|ui| {
                        let path = if item.is_tunnel() {
                            "—"
                        } else {
                            item.request.path.as_str()
                        };
                        ui.add(Label::new(path).truncate());
                    });
                    row.col(|ui| {
                        ui.label(view::human_bytes(item.transferred()));
                    });
                    row.col(|ui| {
                        ui.label(
                            item.duration()
                                .map(view::format_duration)
                                .unwrap_or_else(|| "…".to_string()),
                        );
                    });

                    if row.response().clicked() {
                        clicked = Some(id);
                    }
                });
            });

        if let Some(id) = clicked {
            self.select_id(id);
        }
    }

    fn ensure_detail(&mut self, id: u64) {
        let revision = self.history.revision();
        if let Some(cache) = &self.detail
            && cache.id == id
            && cache.revision == revision
        {
            return;
        }
        self.detail = self.history.get(id).map(|item| DetailCache {
            id,
            revision,
            request: view::format_request(item),
            response: view::format_response(item),
        });
    }

    fn detail_panel(&mut self, ui: &mut egui::Ui) {
        let Some(id) = self.selected else {
            ui.centered_and_justified(|ui| {
                ui.weak("Select an exchange to inspect it (click a row, j/k, or the arrow keys)");
            });
            return;
        };
        self.ensure_detail(id);

        let Some(cache) = &self.detail else {
            ui.centered_and_justified(|ui| {
                ui.weak("This exchange is no longer in history");
            });
            return;
        };

        egui::Panel::right(RESPONSE_PANE_ID)
            .resizable(true)
            .default_size((ui.available_width() / 2.0).max(200.0))
            .show(ui, |ui| {
                text_pane(ui, "Response", &cache.response);
            });
        text_pane(ui, "Request", &cache.request);
    }
}

impl eframe::App for WiremothApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.drain_events(&ctx);
        self.keyboard(&ctx);
        self.rebuild_visible();

        egui::Panel::top("wiremoth.toolbar").show(ui, |ui| self.toolbar(ui));
        egui::Panel::bottom("wiremoth.statusbar").show(ui, |ui| self.status_bar(ui));
        egui::Panel::bottom("wiremoth.detail")
            .resizable(true)
            .default_size(320.0)
            .min_size(120.0)
            .show(ui, |ui| self.detail_panel(ui));
        egui::CentralPanel::default().show(ui, |ui| self.history_table(ui));

        // Keep the fetch loop honest about dropped events even while idle.
        if self
            .proxy
            .as_ref()
            .is_some_and(|proxy| proxy.dropped_events() > 0)
        {
            ctx.request_repaint_after(Duration::from_millis(500));
        }
    }
}

fn text_pane(ui: &mut egui::Ui, title: &str, text: &str) {
    ui.add_space(3.0);
    ui.horizontal(|ui| {
        ui.strong(title);
    });
    ui.separator();
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.add(Label::new(RichText::new(text).monospace()));
        });
}

fn matches_filter(item: &HistoryItem, needle: &str) -> bool {
    let status = item.status().map(|status| status.to_string()).unwrap_or_default();
    let haystack = format!(
        "{} {} {} {} {}",
        item.request.method, item.request.host, item.request.path, item.request.uri, status
    );
    haystack.to_lowercase().contains(needle)
}
