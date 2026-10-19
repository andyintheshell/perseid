//! Target tab: the captured-site map and the scope editor.

use egui::TextEdit;

use super::util::captured_hosts;
use super::widgets::{card, empty_state, section_tab};
use super::*;

impl WiremothApp {
    pub(super) fn target_tab(&mut self, ui: &mut egui::Ui) {
        egui::Frame::NONE
            .fill(theme::SURFACE_ALT)
            .inner_margin(egui::Margin::symmetric(10, 5))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    if section_tab(
                        ui,
                        self.target_section == TargetSection::SiteMap,
                        "Site map",
                    )
                    .clicked()
                    {
                        self.target_section = TargetSection::SiteMap;
                        ui.ctx().memory_mut(|memory| memory.stop_text_input());
                    }
                    if section_tab(ui, self.target_section == TargetSection::Scope, "Scope")
                        .clicked()
                    {
                        self.target_section = TargetSection::Scope;
                    }
                });
            });

        match self.target_section {
            TargetSection::SiteMap => self.site_map_view(ui),
            TargetSection::Scope => self.scope_view(ui),
        }
    }

    fn site_map_view(&mut self, ui: &mut egui::Ui) {
        self.ensure_site_map();
        let mut picked = None;
        let selected = self.selected;

        if let Some((_, map)) = &self.site_map {
            if map.is_empty() {
                let body = if self.scope_enabled {
                    "No in-scope HTTP endpoints. Adjust the scope in Target > Scope, or turn off scope filtering."
                } else {
                    "Browse an HTTP site through the proxy to populate the site map. HTTPS traffic is captured as CONNECT tunnels until TLS interception lands."
                };
                empty_state(ui, "No HTTP endpoints", body);
                return;
            }

            egui::Panel::left(TARGET_TREE_ID)
                .resizable(true)
                .default_size(380.0)
                .min_size(220.0)
                .frame(
                    egui::Frame::NONE
                        .fill(theme::SURFACE)
                        .inner_margin(egui::Margin::same(8)),
                )
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Site map").strong().color(theme::TEXT));
                        ui.label(
                            RichText::new(format!(
                                "{} hosts / {} endpoints",
                                map.hosts.len(),
                                map.endpoint_count()
                            ))
                            .small()
                            .color(theme::TEXT_MUTED),
                        );
                    });
                    ui.separator();
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            for host in &map.hosts {
                                let header =
                                    format!("http://{}  ({})", host.host, host.entries.len());
                                egui::CollapsingHeader::new(
                                    RichText::new(header).strong().color(theme::TEXT),
                                )
                                .id_salt(("wiremoth.site-map", host.host.as_str()))
                                .default_open(true)
                                .show(ui, |ui| {
                                    for entry in &host.entries {
                                        let selected_here = selected == Some(entry.id);
                                        let label = if entry.count > 1 {
                                            format!(
                                                "{} {}  x{}",
                                                entry.method, entry.path, entry.count
                                            )
                                        } else {
                                            format!("{} {}", entry.method, entry.path)
                                        };
                                        let text = RichText::new(label).monospace().color(
                                            if selected_here {
                                                theme::ACCENT_DARK
                                            } else {
                                                theme::TEXT
                                            },
                                        );
                                        if ui.selectable_label(selected_here, text).clicked() {
                                            picked = Some(entry.id);
                                        }
                                    }
                                });
                            }
                            ui.add_space(6.0);
                            ui.label(
                                RichText::new(
                                    "HTTP only. CONNECT tunnels are omitted until TLS interception lands.",
                                )
                                .small()
                                .color(theme::TEXT_MUTED),
                            );
                        });
                });
        }

        if let Some(id) = picked {
            self.select_id(id);
        }

        egui::CentralPanel::default()
            .frame(
                egui::Frame::NONE
                    .fill(theme::CANVAS)
                    .inner_margin(egui::Margin::same(8)),
            )
            .show(ui, |ui| self.detail_panel(ui));
    }

    fn scope_view(&mut self, ui: &mut egui::Ui) {
        let captured_hosts = captured_hosts(&self.history);
        let mut dirty = false;
        let mut clear_scope = false;

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_max_width(780.0);
                card(ui, |ui| {
                    ui.label(
                        RichText::new("Target scope")
                            .size(17.0)
                            .strong()
                            .color(theme::TEXT),
                    );
                    ui.label(
                        "Scope currently filters the site map and HTTP history. Capture and \
                         forwarding still apply to every host.",
                    );
                    ui.add_space(8.0);
                    let mut enabled = self.scope_enabled;
                    if ui
                        .checkbox(&mut enabled, "Filter views to the scope")
                        .changed()
                    {
                        self.scope_enabled = enabled;
                        dirty = true;
                    }
                    if self.scope_enabled && self.scope.is_empty() {
                        ui.label(
                            RichText::new(
                                "No rules yet, so every host is considered in scope.",
                            )
                            .small()
                            .color(theme::TEXT_MUTED),
                        );
                    }

                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        ui.label("Include host");
                        let response = ui.add(
                            TextEdit::singleline(&mut self.scope_input)
                                .hint_text("example.com or https://example.com/path")
                                .desired_width(340.0),
                        );
                        let enter =
                            response.lost_focus() && ui.input(|input| input.key_pressed(Key::Enter));
                        if ui.button("Add").clicked() || enter {
                            if let Some(rule) =
                                site_map::normalize_scope_input(&self.scope_input)
                            {
                                if self.scope.insert(rule) {
                                    dirty = true;
                                }
                                self.scope_input.clear();
                            }
                        }
                    });

                    ui.add_space(8.0);
                    if self.scope.is_empty() {
                        ui.label(
                            RichText::new("No explicit scope rules.")
                                .small()
                                .color(theme::TEXT_MUTED),
                        );
                    } else {
                        let rules: Vec<String> = self.scope.iter().cloned().collect();
                        for rule in rules {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(&rule).monospace().color(theme::TEXT));
                                if ui.small_button("Remove").clicked() {
                                    self.scope.remove(&rule);
                                    dirty = true;
                                }
                            });
                        }
                        if ui.button("Clear scope").clicked() {
                            clear_scope = true;
                        }
                    }
                });

                ui.add_space(10.0);
                card(ui, |ui| {
                    ui.label(
                        RichText::new("Captured hosts")
                            .size(15.0)
                            .strong()
                            .color(theme::TEXT),
                    );
                    ui.label(
                        RichText::new(
                            "Add a host from observed traffic. Matching also covers subdomains.",
                        )
                        .small()
                        .color(theme::TEXT_MUTED),
                    );
                    ui.add_space(4.0);
                    if captured_hosts.is_empty() {
                        ui.label(
                            RichText::new("No hosts captured yet.")
                                .small()
                                .color(theme::TEXT_MUTED),
                        );
                    }
                    for (host, count) in &captured_hosts {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(host).monospace().color(theme::TEXT));
                            ui.label(
                                RichText::new(format!("x{count}"))
                                    .small()
                                    .color(theme::TEXT_MUTED),
                            );
                            if self.scope.contains(host) {
                                ui.label(
                                    RichText::new("in scope")
                                        .small()
                                        .color(theme::GREEN),
                                );
                            } else if ui.small_button("Add").clicked() {
                                self.scope.insert(host.clone());
                                dirty = true;
                            }
                        });
                    }
                });

                ui.add_space(8.0);
                ui.label(
                    RichText::new(
                        "Scope is view-only for now; interception and Intruder will honour it in later milestones.",
                    )
                    .small()
                    .color(theme::TEXT_MUTED),
                );
            });

        if clear_scope {
            self.scope.clear();
            dirty = true;
        }
        if dirty {
            self.invalidate_views();
        }
    }
}
