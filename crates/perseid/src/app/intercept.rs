//! Editable interception queue and raw message editor.

use egui::{Label, TextEdit};
use perseid_proxy::{CapturedBody, InterceptError, InterceptItem, InterceptKind};

use super::widgets::{card, empty_state};
use super::*;

/// UI-side copy of the message currently waiting for a decision.
pub(super) struct InterceptDraft {
    pub head: String,
    pub body: String,
    pub body_editable: bool,
    pub binary_preview: Option<String>,
    pub error: Option<String>,
}

impl InterceptDraft {
    fn from_item(item: &InterceptItem) -> Self {
        let body = item.body();
        let body_editable = body.is_some() && item.body_is_text();
        let (body_text, binary_preview) = match body {
            Some(bytes) if body_editable => (String::from_utf8_lossy(bytes).into_owned(), None),
            Some(bytes) => (
                String::new(),
                Some(view::format_binary_preview(&CapturedBody::from_bytes(
                    bytes.to_vec(),
                ))),
            ),
            None => (String::new(), None),
        };

        Self {
            head: item.head().to_string(),
            body: body_text,
            body_editable,
            binary_preview,
            error: None,
        }
    }
}

impl PerseidApp {
    pub(super) fn sync_intercept_draft(&mut self) {
        if self.intercept_draft.is_some() {
            return;
        }
        self.intercept_draft = self.intercept_queue.front().map(InterceptDraft::from_item);
    }

    pub(super) fn intercept_view(&mut self, ui: &mut egui::Ui) {
        let mut settings_changed = false;
        let mut forward = false;
        let mut drop_message = false;
        let listener = self
            .proxy
            .as_ref()
            .map(Proxy::local_addr)
            .unwrap_or(self.listen);
        let pending = self.intercept_queue.len();

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_max_width(920.0);
                card(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new("Intercept")
                                .size(17.0)
                                .strong()
                                .color(theme::TEXT),
                        );
                        if pending > 0 {
                            ui.colored_label(theme::AMBER, format!("{pending} held"));
                        }
                        ui.with_layout(
                            egui::Layout::right_to_left(egui::Align::Center),
                            |ui| {
                                ui.label(
                                    RichText::new(listener.to_string())
                                        .monospace()
                                        .small()
                                        .color(theme::TEXT_MUTED),
                                );
                                if self.proxy.is_some() {
                                    ui.colored_label(theme::GREEN, "listening");
                                } else {
                                    ui.colored_label(theme::RED, "not running");
                                    if ui.button("Retry").clicked() {
                                        let notify = self.notify.clone();
                                        self.start_proxy(notify);
                                    }
                                }
                            },
                        );
                    });
                    ui.add_space(6.0);
                    // Burp's Intercept tab has one Intercept on/off control,
                    // with Forward and Drop beside it, rather than separate
                    // request and response switches.
                    ui.horizontal(|ui| {
                        let on = self.intercept_enabled;
                        let toggle = egui::Button::new(
                            RichText::new(if on {
                                "Intercept is on"
                            } else {
                                "Intercept is off"
                            })
                            .size(13.5)
                            .strong()
                            .color(if on { Color32::WHITE } else { theme::TEXT }),
                        )
                        .fill(if on {
                            theme::ACCENT_DARK
                        } else {
                            theme::SURFACE_ALT
                        })
                        .stroke(Stroke::new(
                            1.0,
                            if on {
                                theme::ACCENT_DARK
                            } else {
                                theme::BORDER
                            },
                        ))
                        .corner_radius(5.0)
                        .min_size(egui::vec2(136.0, 30.0));
                        if ui
                            .add(toggle)
                            .on_hover_text(if on {
                                "Turn interception off. Already held messages still need Forward or Drop."
                            } else {
                                "Hold HTTP and decrypted HTTPS messages before they are forwarded."
                            })
                            .clicked()
                        {
                            self.intercept_enabled = !self.intercept_enabled;
                            settings_changed = true;
                        }

                        ui.add_space(4.0);
                        let can_decide = pending > 0;
                        if ui
                            .add_enabled(can_decide, egui::Button::new("Forward"))
                            .on_hover_text("Send the edited message on the wire")
                            .on_disabled_hover_text("No intercepted message is held")
                            .clicked()
                        {
                            forward = true;
                        }
                        if ui
                            .add_enabled(can_decide, egui::Button::new("Drop"))
                            .on_hover_text("Drop the message and close this exchange")
                            .on_disabled_hover_text("No intercepted message is held")
                            .clicked()
                        {
                            drop_message = true;
                        }
                        if pending > 1 {
                            ui.label(
                                RichText::new(format!("{} more queued", pending - 1))
                                    .small()
                                    .color(theme::TEXT_MUTED),
                            );
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            self.capture_toggle(ui);
                        });
                    });
                    ui.add_space(4.0);
                    let decrypting = self.tls_intercept && self.tls_ca.is_some();
                    let holding_message = if self.intercept_enabled {
                        if decrypting {
                            "New HTTP and decrypted HTTPS messages will wait here."
                        } else {
                            "New HTTP messages will wait here. HTTPS stays an opaque CONNECT tunnel."
                        }
                    } else {
                        "New HTTP traffic is forwarded automatically."
                    };
                    ui.label(
                        RichText::new(holding_message)
                            .small()
                            .color(if self.intercept_enabled {
                                theme::ACCENT_DARK
                            } else {
                                theme::TEXT_MUTED
                            }),
                    );
                    let https_hint = if decrypting {
                        "HTTPS is decrypted into ordinary HTTP messages."
                    } else {
                        "HTTPS is CONNECT passthrough; enable decryption in Proxy > TLS."
                    };
                    ui.label(
                        RichText::new(format!(
                            "{https_hint} Bodies over {} are held for header edits only.",
                            view::human_bytes(self.intercept_limit as u64)
                        ))
                        .small()
                        .color(theme::TEXT_MUTED),
                    );
                    if let Some(error) = &self.proxy_error {
                        ui.label(RichText::new(error).small().color(theme::RED));
                    }
                });

                ui.add_space(8.0);
                let Some(item) = self.intercept_queue.front() else {
                    if self.intercept_enabled {
                        empty_state(
                            ui,
                            "Waiting for traffic",
                            "The next HTTP request or response will appear here before it is forwarded.",
                        );
                    } else {
                        empty_state(
                            ui,
                            "Interception is off",
                            "Click Intercept is off above to start holding HTTP requests and responses.",
                        );
                    }
                    return;
                };

                let kind = item.kind();
                let method = item.request().method.clone();
                let path = item.request().path.clone();
                let body_buffered = item.body().is_some();
                let body_len = item.body().map(<[u8]>::len).unwrap_or(0);
                let body_is_text = item.body_is_text();
                let id = item.id();
                let title = match kind {
                    InterceptKind::Request => "Intercepted request",
                    InterceptKind::Response => "Intercepted response",
                };
                let detail = match kind {
                    InterceptKind::Request => format!("{method} {path}"),
                    InterceptKind::Response => format!("response to {method} {path}"),
                };

                let Some(draft) = self.intercept_draft.as_mut() else {
                    return;
                };

                card(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(title)
                                .size(15.0)
                                .strong()
                                .color(theme::TEXT),
                        );
                        ui.label(
                            RichText::new(detail)
                                .monospace()
                                .small()
                                .color(theme::TEXT_MUTED),
                        );
                    });
                    ui.add_space(4.0);

                    let body_status = if body_buffered {
                        format!(
                            "Body: {} ({})",
                            view::human_bytes(body_len as u64),
                            if body_is_text {
                                "editable text"
                            } else {
                                "binary, read-only"
                            }
                        )
                    } else {
                        "Body: not buffered because it is large or streaming; header edits only"
                            .to_string()
                    };
                    ui.label(
                        RichText::new(body_status)
                            .small()
                            .color(if body_buffered {
                                theme::TEXT_MUTED
                            } else {
                                theme::AMBER
                            }),
                    );
                    ui.add_space(3.0);

                    ui.label(RichText::new("Head").strong().color(theme::TEXT));
                    egui::Frame::NONE
                        .fill(Color32::WHITE)
                        .stroke(Stroke::new(1.0, theme::BORDER))
                        .corner_radius(4.0)
                        .inner_margin(egui::Margin::same(6))
                        .show(ui, |ui| {
                            if ui
                                .add(
                                    TextEdit::multiline(&mut draft.head)
                                        .code_editor()
                                        .desired_rows(8)
                                        .desired_width(f32::INFINITY),
                                )
                                .changed()
                            {
                                draft.error = None;
                            }
                        });
                    ui.add_space(6.0);

                    ui.label(RichText::new("Body").strong().color(theme::TEXT));
                    if draft.body_editable {
                        egui::Frame::NONE
                            .fill(Color32::WHITE)
                            .stroke(Stroke::new(1.0, theme::BORDER))
                            .corner_radius(4.0)
                            .inner_margin(egui::Margin::same(6))
                            .show(ui, |ui| {
                                if ui
                                    .add(
                                        TextEdit::multiline(&mut draft.body)
                                            .code_editor()
                                            .desired_rows(10)
                                            .desired_width(f32::INFINITY),
                                    )
                                    .changed()
                                {
                                    draft.error = None;
                                }
                            });
                    } else if let Some(preview) = &draft.binary_preview {
                        egui::Frame::NONE
                            .fill(theme::SURFACE_TINT)
                            .stroke(Stroke::new(1.0, theme::BORDER))
                            .corner_radius(4.0)
                            .inner_margin(egui::Margin::same(6))
                            .show(ui, |ui| {
                                egui::ScrollArea::both()
                                    .id_salt(("perseid.intercept.binary", id))
                                    .max_height(240.0)
                                    .auto_shrink([false, false])
                                    .show(ui, |ui| {
                                        ui.add(
                                            Label::new(
                                                RichText::new(preview)
                                                    .monospace()
                                                    .color(theme::TEXT),
                                            )
                                            .selectable(true)
                                            .wrap_mode(egui::TextWrapMode::Extend),
                                        );
                                    });
                            });
                    } else {
                        ui.label(
                            RichText::new(
                                "This body is not buffered. Forward the message with the original body, or edit the head.",
                            )
                            .small()
                            .color(theme::TEXT_MUTED),
                        );
                    }

                    if let Some(error) = &draft.error {
                        ui.add_space(4.0);
                        ui.label(RichText::new(error).small().color(theme::RED));
                    }
                });

                if forward || drop_message {
                    let head = draft.head.clone();
                    let body = if forward && draft.body_editable {
                        Some(draft.body.as_bytes().to_vec())
                    } else {
                        None
                    };
                    let result = {
                        let Some(item) = self.intercept_queue.front_mut() else {
                            return;
                        };
                        if forward {
                            item.forward(&head, body)
                        } else {
                            item.drop_message()
                        }
                    };
                    match result {
                        Ok(()) => {
                            self.intercept_queue.pop_front();
                            self.intercept_draft = None;
                            self.sync_intercept_draft();
                            ui.ctx().request_repaint();
                        }
                        // The exchange ended while the message was held; the
                        // client closed the connection before a decision could
                        // be delivered. Discard it so newer messages are not
                        // stuck behind a dead one.
                        Err(InterceptError::Closed | InterceptError::AlreadyResolved) => {
                            self.intercept_queue.pop_front();
                            self.intercept_draft = None;
                            self.sync_intercept_draft();
                            ui.ctx().request_repaint();
                        }
                        Err(err) => {
                            if let Some(draft) = self.intercept_draft.as_mut() {
                                draft.error = Some(err.to_string());
                            }
                        }
                    }
                }
            });

        if settings_changed {
            self.apply_proxy_settings();
        }
    }
}
