//! Window chrome: title bar, tool tabs, status bar, and resize handles.

use super::*;

/// Draw one custom title-bar window control.
fn window_control(ui: &mut egui::Ui, control: WindowControl, maximized: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(26.0, 22.0), egui::Sense::click());
    let hovered = response.hovered();
    let (fill, color) = match control {
        WindowControl::Close if hovered => (theme::RED, Color32::WHITE),
        _ if hovered => (theme::ACCENT_WASH, theme::TEXT),
        _ => (Color32::TRANSPARENT, theme::TEXT),
    };
    if fill != Color32::TRANSPARENT {
        ui.painter().rect_filled(rect, 3.0, fill);
    }

    let center = rect.center();
    let stroke = Stroke::new(1.0, color);
    match control {
        WindowControl::Minimize => {
            ui.painter().line_segment(
                [
                    egui::pos2(center.x - 4.0, center.y + 3.0),
                    egui::pos2(center.x + 4.0, center.y + 3.0),
                ],
                stroke,
            );
        }
        WindowControl::Maximize if maximized => {
            let back = egui::Rect::from_min_size(
                egui::pos2(center.x - 2.0, center.y - 4.0),
                egui::vec2(6.0, 6.0),
            );
            let front = egui::Rect::from_min_size(
                egui::pos2(center.x - 4.0, center.y - 2.0),
                egui::vec2(6.0, 6.0),
            );
            ui.painter()
                .rect_stroke(back, 0.0, stroke, egui::StrokeKind::Inside);
            ui.painter()
                .rect_stroke(front, 0.0, stroke, egui::StrokeKind::Inside);
        }
        WindowControl::Maximize => {
            let square = egui::Rect::from_center_size(center, egui::vec2(7.0, 7.0));
            ui.painter()
                .rect_stroke(square, 0.0, stroke, egui::StrokeKind::Inside);
        }
        WindowControl::Close => {
            ui.painter().line_segment(
                [
                    egui::pos2(center.x - 3.5, center.y - 3.5),
                    egui::pos2(center.x + 3.5, center.y + 3.5),
                ],
                stroke,
            );
            ui.painter().line_segment(
                [
                    egui::pos2(center.x + 3.5, center.y - 3.5),
                    egui::pos2(center.x - 3.5, center.y + 3.5),
                ],
                stroke,
            );
        }
    }

    response
        .on_hover_text(match control {
            WindowControl::Minimize => "Minimize",
            WindowControl::Maximize if maximized => "Restore",
            WindowControl::Maximize => "Maximize",
            WindowControl::Close => "Close",
        })
        .on_hover_cursor(egui::CursorIcon::PointingHand)
}

impl PerseidApp {
    pub(super) fn title_bar(&mut self, ui: &mut egui::Ui) {
        let maximized = ui
            .ctx()
            .input(|input| input.viewport().maximized.unwrap_or(false));

        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(3.0, 2.0);
            ui.spacing_mut().button_padding = egui::vec2(7.0, 2.0);

            if let Some(mark) = &self.mark {
                let response = ui
                    .add(
                        egui::Image::from_texture(mark)
                            .fit_to_exact_size(egui::vec2(17.0, 17.0))
                            .corner_radius(4.0)
                            .sense(egui::Sense::click()),
                    )
                    .on_hover_text("About Perseid")
                    .on_hover_cursor(egui::CursorIcon::PointingHand);
                if response.clicked() {
                    self.about_open = true;
                }
            }
            ui.add_space(3.0);

            ui.scope(|ui| {
                let widgets = &mut ui.style_mut().visuals.widgets;
                widgets.inactive.weak_bg_fill = Color32::TRANSPARENT;
                widgets.inactive.bg_stroke = Stroke::NONE;
                widgets.inactive.fg_stroke = Stroke::new(1.0, theme::TEXT);
                widgets.hovered.weak_bg_fill = theme::ACCENT_WASH;
                widgets.hovered.bg_stroke = Stroke::NONE;
                widgets.hovered.fg_stroke = Stroke::new(1.0, theme::TEXT);
                widgets.active.weak_bg_fill = theme::ACCENT_SOFT;
                widgets.active.bg_stroke = Stroke::NONE;
                widgets.active.fg_stroke = Stroke::new(1.0, theme::TEXT);

                self.project_menu(ui);
                ui.menu_button("Perseid", |ui| {
                    if ui.button("About Perseid").clicked() {
                        self.about_open = true;
                        ui.close();
                    }
                    ui.separator();
                    if ui.button("Quit").clicked() {
                        ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });
                ui.menu_button("View", |ui| {
                    let ctx = ui.ctx().clone();
                    let current = ctx.pixels_per_point();

                    let zoom_in = egui::Button::new("Zoom In")
                        .shortcut_text(ctx.format_shortcut(&ZOOM_IN_SHORTCUT));
                    if ui.add(zoom_in).clicked() {
                        self.set_ui_scale(&ctx, stepped_scale(current, 0.1));
                    }
                    let zoom_out = egui::Button::new("Zoom Out")
                        .shortcut_text(ctx.format_shortcut(&ZOOM_OUT_SHORTCUT));
                    if ui.add(zoom_out).clicked() {
                        self.set_ui_scale(&ctx, stepped_scale(current, -0.1));
                    }
                    let reset = egui::Button::new("Reset Zoom")
                        .shortcut_text(ctx.format_shortcut(&ZOOM_RESET_SHORTCUT));
                    if ui
                        .add_enabled((current - self.base_ui_scale).abs() > 1e-3, reset)
                        .clicked()
                    {
                        self.set_ui_scale(&ctx, self.base_ui_scale);
                    }

                    ui.separator();
                    ui.weak(format!(
                        "UI scale: {:.0}% (system {:.0}%)",
                        current * 100.0,
                        self.native_ui_scale * 100.0,
                    ));
                });
                ui.menu_button("Help", |ui| {
                    if ui.button("About Perseid").clicked() {
                        self.about_open = true;
                        ui.close();
                    }
                    ui.hyperlink_to("Project on GitHub", branding::REPOSITORY_URL);
                });
            });

            // The empty part of the title bar drags the window, like Burp's
            // menu bar. Reserve room on the right for the window controls.
            let drag_height = ui.spacing().interact_size.y.max(20.0);
            let drag_width = (ui.available_width() - 3.0 * 26.0 - 10.0).max(0.0);
            let (drag_rect, drag_response) = ui.allocate_exact_size(
                egui::vec2(drag_width, drag_height),
                egui::Sense::click_and_drag(),
            );
            if drag_response.drag_started() {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
            }
            if drag_response.double_clicked() {
                ui.ctx()
                    .send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
            }
            if drag_rect.width() > 0.0 {
                ui.painter().text(
                    drag_rect.center(),
                    egui::Align2::CENTER_CENTER,
                    "Perseid",
                    egui::FontId::proportional(10.5),
                    theme::TEXT_MUTED,
                );
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if window_control(ui, WindowControl::Close, maximized).clicked() {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
                if window_control(ui, WindowControl::Maximize, maximized).clicked() {
                    ui.ctx()
                        .send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
                }
                if window_control(ui, WindowControl::Minimize, maximized).clicked() {
                    ui.ctx()
                        .send_viewport_cmd(egui::ViewportCommand::Minimized(true));
                }
            });
        });
    }

    pub(super) fn main_tabs(&mut self, ui: &mut egui::Ui) {
        let shortcuts = [
            ui.ctx().format_shortcut(&TAB_TARGET_SHORTCUT),
            ui.ctx().format_shortcut(&TAB_PROXY_SHORTCUT),
            ui.ctx().format_shortcut(&TAB_INTRUDER_SHORTCUT),
            ui.ctx().format_shortcut(&TAB_REPEATER_SHORTCUT),
            ui.ctx().format_shortcut(&TAB_DECODER_SHORTCUT),
            ui.ctx().format_shortcut(&TAB_COMPARER_SHORTCUT),
        ];
        let search_shortcut = ui.ctx().format_shortcut(&SEARCH_SHORTCUT);
        ui.horizontal(|ui| {
            for (index, (tab, label)) in MainTab::ALL.iter().enumerate() {
                let response = main_tab_button(ui, self.main_tab == *tab, label)
                    .on_hover_text(format!("{} ({})", label, shortcuts[index]));
                if response.clicked() && self.main_tab != *tab {
                    self.main_tab = *tab;
                    ui.ctx().memory_mut(|memory| memory.stop_text_input());
                }
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if search_button(ui, self.search.open)
                    .on_hover_text(format!("Search ({search_shortcut})"))
                    .clicked()
                {
                    self.search.toggle();
                }
                ui.separator();
                self.capture_toggle(ui);
                ui.separator();
                if let Some(proxy) = &self.proxy {
                    ui.colored_label(theme::GREEN, format!("proxy on {}", proxy.local_addr()));
                } else {
                    let response = ui.colored_label(theme::RED, "proxy not running");
                    if let Some(error) = &self.proxy_error {
                        response.on_hover_text(error);
                    }
                    if ui.button("Retry").clicked() {
                        let notify = self.notify.clone();
                        self.start_proxy(notify);
                    }
                }
            });
        });
    }

    pub(super) fn status_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!("{} exchanges", self.history.len()))
                    .small()
                    .color(theme::TEXT_MUTED),
            )
            .on_hover_text(format!(
                "{} of {} captured body bytes retained",
                view::human_bytes(self.history.retained_bytes() as u64),
                view::human_bytes(self.history.byte_limit() as u64)
            ));

            self.project_status_widget(ui);

            let in_flight = self.history.in_flight();
            if in_flight > 0 {
                ui.separator();
                ui.colored_label(theme::BLUE, format!("{in_flight} in flight"));
            }

            let pending = self.intercept_queue.len();
            if pending > 0 {
                ui.separator();
                ui.colored_label(theme::AMBER, format!("{pending} awaiting intercept"));
            } else if self.proxy.is_some() && self.intercept_enabled {
                ui.separator();
                ui.label(
                    RichText::new("intercept armed")
                        .small()
                        .color(theme::ACCENT_DARK),
                );
            }

            let repeater_pending = self
                .repeater_tabs
                .iter()
                .filter(|tab| tab.is_pending())
                .count();
            if repeater_pending > 0 {
                ui.separator();
                ui.colored_label(theme::BLUE, format!("{repeater_pending} Repeater pending"));
            }

            if let Some(summary) = self.intruder.status_summary() {
                ui.separator();
                ui.colored_label(theme::BLUE, summary);
            }

            if let Some(proxy) = &self.proxy {
                let dropped = proxy.dropped_events();
                if dropped > 0 {
                    ui.separator();
                    ui.colored_label(
                        theme::AMBER,
                        format!("{dropped} events dropped (consumer too slow)"),
                    );
                }
                let rejected = proxy.rejected_connections() + proxy.rejected_tunnels();
                if rejected > 0 {
                    ui.separator();
                    ui.colored_label(
                        theme::AMBER,
                        format!("{rejected} rejected by connection limits"),
                    );
                }
                let blocked = proxy.blocked_by_scope();
                if blocked > 0 {
                    ui.separator();
                    ui.colored_label(theme::AMBER, format!("{blocked} blocked by scope"));
                }
            }
            let evicted = self.history.evicted();
            if evicted > 0 {
                ui.separator();
                ui.label(
                    RichText::new(format!("{evicted} evicted by history limit"))
                        .small()
                        .color(theme::TEXT_MUTED),
                );
            }

            if self.paused {
                ui.separator();
                ui.colored_label(theme::AMBER, "capture paused");
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                self.memory_indicator(ui);
            });
        });
    }

    /// The window is undecorated so the menu can share the title-bar row with
    /// the window controls. Draw its resize border ourselves.
    pub(super) fn resize_handles(&self, ui: &mut egui::Ui) {
        let rect = ui.ctx().content_rect();
        let edge = 4.0;
        let corner = 10.0;
        let handles = [
            (
                egui::Rect::from_min_max(
                    egui::pos2(rect.left() + corner, rect.top()),
                    egui::pos2(rect.right() - corner, rect.top() + edge),
                ),
                egui::ResizeDirection::North,
                egui::CursorIcon::ResizeVertical,
            ),
            (
                egui::Rect::from_min_max(
                    egui::pos2(rect.left() + corner, rect.bottom() - edge),
                    egui::pos2(rect.right() - corner, rect.bottom()),
                ),
                egui::ResizeDirection::South,
                egui::CursorIcon::ResizeVertical,
            ),
            (
                egui::Rect::from_min_max(
                    egui::pos2(rect.left(), rect.top() + corner),
                    egui::pos2(rect.left() + edge, rect.bottom() - corner),
                ),
                egui::ResizeDirection::West,
                egui::CursorIcon::ResizeHorizontal,
            ),
            (
                egui::Rect::from_min_max(
                    egui::pos2(rect.right() - edge, rect.top() + corner),
                    egui::pos2(rect.right(), rect.bottom() - corner),
                ),
                egui::ResizeDirection::East,
                egui::CursorIcon::ResizeHorizontal,
            ),
            (
                egui::Rect::from_min_max(
                    egui::pos2(rect.left(), rect.top()),
                    egui::pos2(rect.left() + corner, rect.top() + corner),
                ),
                egui::ResizeDirection::NorthWest,
                egui::CursorIcon::ResizeNwSe,
            ),
            (
                egui::Rect::from_min_max(
                    egui::pos2(rect.right() - corner, rect.top()),
                    egui::pos2(rect.right(), rect.top() + corner),
                ),
                egui::ResizeDirection::NorthEast,
                egui::CursorIcon::ResizeNeSw,
            ),
            (
                egui::Rect::from_min_max(
                    egui::pos2(rect.left(), rect.bottom() - corner),
                    egui::pos2(rect.left() + corner, rect.bottom()),
                ),
                egui::ResizeDirection::SouthWest,
                egui::CursorIcon::ResizeNeSw,
            ),
            (
                egui::Rect::from_min_max(
                    egui::pos2(rect.right() - corner, rect.bottom() - corner),
                    egui::pos2(rect.right(), rect.bottom()),
                ),
                egui::ResizeDirection::SouthEast,
                egui::CursorIcon::ResizeNwSe,
            ),
        ];

        for (index, (rect, direction, cursor)) in handles.into_iter().enumerate() {
            let response = ui
                .interact(
                    rect,
                    egui::Id::unique(("perseid.window.resize", index)),
                    egui::Sense::drag(),
                )
                .on_hover_cursor(cursor);
            if response.drag_started() {
                ui.ctx()
                    .send_viewport_cmd(egui::ViewportCommand::BeginResize(direction));
            }
        }
    }
}
