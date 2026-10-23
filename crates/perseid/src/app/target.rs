//! Target tab: the captured-site map and the scope editor.

use egui::TextEdit;

use super::util::captured_hosts;
use super::widgets::{card, empty_state, page_pad, section_bar, section_tab};
use super::*;

impl PerseidApp {
    pub(super) fn target_tab(&mut self, ui: &mut egui::Ui) {
        section_bar(ui, "perseid.target.sections", |ui| {
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
            if section_tab(ui, self.target_section == TargetSection::Scope, "Scope").clicked() {
                self.target_section = TargetSection::Scope;
            }
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
                    "Browse an HTTP or HTTPS site through the proxy to populate the site map. HTTPS requests appear after decryption."
                };
                empty_state(ui, "No HTTP endpoints", body);
                return;
            }

            egui::Panel::left(TARGET_TREE_ID)
                .resizable(true)
                .default_size(340.0)
                .min_size(200.0)
                .frame(
                    egui::Frame::NONE
                        .fill(theme::SURFACE)
                        .inner_margin(egui::Margin::same(5)),
                )
                .show(ui, |ui| {
                    ui.label(
                        RichText::new(format!(
                            "{} hosts / {} endpoints",
                            map.hosts.len(),
                            map.endpoint_count()
                        ))
                        .small()
                        .color(theme::TEXT_MUTED),
                    );
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
                                .id_salt(("perseid.site-map", host.host.as_str()))
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
                    .inner_margin(egui::Margin::same(5)),
            )
            .show(ui, |ui| self.detail_panel(ui));
    }

    fn scope_view(&mut self, ui: &mut egui::Ui) {
        let captured_hosts = captured_hosts(&self.history);
        let mut dirty = false;
        let mut settings_dirty = false;
        let mut clear_scope = false;

        page_pad(ui, |ui| {
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.set_max_width(780.0);
                    card(ui, |ui| {
                        ui.label(
                        RichText::new("Scope rules")
                            .size(14.0)
                            .strong()
                            .color(theme::TEXT),
                    );
                    ui.label(
                        "Scope filters the site map and HTTP history. It can also block \
                         out-of-scope traffic before it is forwarded.",
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
                    let mut enforce = self.scope_enforce;
                    if ui
                        .checkbox(&mut enforce, "Block out-of-scope traffic")
                        .on_hover_text(
                            "Return 403 for requests and CONNECT tunnels outside the scope. \
                             An empty scope allows every host.",
                        )
                        .changed()
                    {
                        self.scope_enforce = enforce;
                        settings_dirty = true;
                    }
                    if (self.scope_enabled || self.scope_enforce) && self.scope.is_empty() {
                        ui.label(
                            RichText::new(
                                "No rules yet, so every host is considered in scope and nothing is blocked.",
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
                        if (ui.button("Add").clicked() || enter)
                            && let Some(rule) =
                                site_map::normalize_scope_input(&self.scope_input)
                        {
                            if self.scope.insert(rule) {
                                dirty = true;
                                settings_dirty = true;
                            }
                            self.scope_input.clear();
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
                                    settings_dirty = true;
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
                            .size(13.5)
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
                            if !self.scope.is_empty()
                                && site_map::host_in_scope(host, &self.scope)
                            {
                                ui.label(
                                    RichText::new("in scope")
                                        .small()
                                        .color(theme::GREEN),
                                );
                            } else if ui.small_button("Add").clicked() {
                                self.scope.insert(host.clone());
                                dirty = true;
                                settings_dirty = true;
                            }
                        });
                    }
                });

                ui.add_space(8.0);
                ui.label(
                    RichText::new(
                        "When blocking is enabled, scope is checked before interception and again \
                         after edits, so a rewrite cannot move a request outside the guardrail \
                         unnoticed.",
                    )
                    .small()
                    .color(theme::TEXT_MUTED),
                );
            });
        });

        if clear_scope {
            self.scope.clear();
            dirty = true;
            settings_dirty = true;
        }
        if dirty {
            self.invalidate_views();
        }
        if settings_dirty {
            self.apply_proxy_settings();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::time::{Duration, SystemTime};

    use crate::history::FALLBACK_HISTORY_BYTES;
    use crate::proxy::{ProxyEvent, RequestInfo};

    use super::*;

    /// Build history rows the way the proxy reports captured exchanges.
    fn history(entries: &[(&str, &str, &str)]) -> History {
        let mut history = History::new(100, FALLBACK_HISTORY_BYTES);
        for (index, (method, host, path)) in entries.iter().enumerate() {
            let id = index as u64 + 1;
            history.apply(ProxyEvent::Started {
                id,
                timestamp: SystemTime::UNIX_EPOCH + Duration::from_secs(id),
                request: RequestInfo {
                    method: method.to_string(),
                    uri: format!("http://{host}{path}"),
                    scheme: "http".to_string(),
                    host: host.to_string(),
                    path: path.to_string(),
                    version: "HTTP/1.1".to_string(),
                    headers: Vec::new(),
                },
            });
        }
        history
    }

    #[test]
    fn captured_host_list_counts_each_host_and_sorts_by_name() {
        let history = history(&[
            ("GET", "zeta.test", "/"),
            ("GET", "alpha.test", "/a"),
            ("POST", "alpha.test", "/b"),
        ]);
        assert_eq!(
            captured_hosts(&history),
            vec![("alpha.test".to_string(), 2), ("zeta.test".to_string(), 1)]
        );
    }

    #[test]
    fn captured_host_list_skips_requests_without_a_host() {
        let history = history(&[("GET", "", "/orphan"), ("GET", "alpha.test", "/")]);
        assert_eq!(
            captured_hosts(&history),
            vec![("alpha.test".to_string(), 1)]
        );
    }

    #[test]
    fn site_map_tree_groups_endpoints_under_their_host() {
        let history = history(&[
            ("GET", "example.com", "/a"),
            ("POST", "example.com", "/a"),
            ("GET", "api.example.com", "/v1"),
        ]);
        let map = SiteMap::build(&history, None);
        let hosts: Vec<&str> = map.hosts.iter().map(|host| host.host.as_str()).collect();
        assert_eq!(hosts, vec!["api.example.com", "example.com"]);
        assert_eq!(map.endpoint_count(), 2);
        let example = &map.hosts[1];
        assert_eq!(example.entries.len(), 1);
        assert_eq!(example.entries[0].count, 2);
    }

    #[test]
    fn site_map_tree_hides_hosts_outside_the_scope() {
        let scope = BTreeSet::from(["example.com".to_string()]);
        let history = history(&[
            ("GET", "example.com", "/"),
            ("GET", "api.example.com", "/v1"),
            ("GET", "other.test", "/"),
        ]);
        let map = SiteMap::build(&history, Some(&scope));
        let hosts: Vec<&str> = map.hosts.iter().map(|host| host.host.as_str()).collect();
        assert_eq!(hosts, vec!["api.example.com", "example.com"]);
    }

    #[test]
    fn scope_helper_treats_every_host_as_in_scope_without_rules() {
        assert!(site_map::host_in_scope("anything.test", &BTreeSet::new()));
    }

    #[test]
    fn scope_helper_matches_subdomains_and_ignores_ports() {
        let scope = BTreeSet::from(["example.com".to_string()]);
        assert!(site_map::host_in_scope("example.com:8443", &scope));
        assert!(site_map::host_in_scope("api.example.com", &scope));
        assert!(!site_map::host_in_scope("notexample.com", &scope));
    }

    #[test]
    fn scope_input_normalizes_urls_ports_and_case() {
        assert_eq!(
            site_map::normalize_scope_input(" https://Example.com/path "),
            Some("example.com".to_string())
        );
        assert_eq!(
            site_map::normalize_scope_input("example.com:8443"),
            Some("example.com:8443".to_string())
        );
        assert_eq!(site_map::normalize_scope_input("   "), None);
    }
}
