//! Proxy > TLS: certificate authority setup and interception controls.

use egui::TextEdit;

use super::widgets::card;
use super::*;

/// A certificate authority action requested by this frame's UI.
enum CaAction {
    Generate { replace: bool },
    Delete,
}

impl WiremothApp {
    pub(super) fn tls_settings_view(&mut self, ui: &mut egui::Ui) {
        let mut settings_dirty = false;
        let mut action: Option<CaAction> = None;
        let mut copy_text: Option<String> = None;
        let cert_path = ca_store::cert_path(&self.ca_dir);
        let key_path = ca_store::key_path(&self.ca_dir);
        let has_ca = self.tls_ca.is_some();
        let proxy_port = self.listen.port();

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_max_width(780.0);
                card(ui, |ui| {
                    ui.label(
                        RichText::new("TLS interception")
                            .size(17.0)
                            .strong()
                            .color(theme::TEXT),
                    );
                    ui.label(
                        "Wiremoth decrypts HTTPS by issuing per-host certificates from a local \
                         certificate authority (CA). Trust is installed in the browser by hand \
                         and removed the same way; Wiremoth never touches the system trust store.",
                    );
                    ui.add_space(10.0);

                    if has_ca {
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new("Certificate authority")
                                    .strong()
                                    .color(theme::TEXT),
                            );
                            ui.label(RichText::new("ready").strong().color(theme::GREEN));
                        });
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("Certificate").color(theme::TEXT_MUTED));
                            ui.label(
                                RichText::new(cert_path.display().to_string())
                                    .monospace()
                                    .small()
                                    .color(theme::TEXT),
                            );
                        });
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("Private key").color(theme::TEXT_MUTED));
                            ui.label(
                                RichText::new(format!("{} (owner-only)", key_path.display()))
                                    .monospace()
                                    .small()
                                    .color(theme::TEXT_MUTED),
                            );
                        });
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            if ui.button("Copy certificate path").clicked() {
                                copy_text = Some(cert_path.display().to_string());
                            }
                            if ui
                                .button("Regenerate")
                                .on_hover_text(
                                    "Create a new CA in place. Browsers that trusted the old \
                                     certificate must import the new one.",
                                )
                                .clicked()
                            {
                                action = Some(CaAction::Generate { replace: true });
                            }
                            if ui
                                .button("Delete files")
                                .on_hover_text(
                                    "Remove the CA files from disk. Also remove the old entry \
                                     from each browser's certificate store.",
                                )
                                .clicked()
                            {
                                action = Some(CaAction::Delete);
                            }
                        });
                    } else {
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new("Certificate authority")
                                    .strong()
                                    .color(theme::TEXT),
                            );
                            ui.label(RichText::new("not created").strong().color(theme::AMBER));
                        });
                        ui.add_space(4.0);
                        ui.label(
                            "Generate a CA, then import its certificate into the browser you \
                             test with. The files stay in:",
                        );
                        ui.label(
                            RichText::new(self.ca_dir.display().to_string())
                                .monospace()
                                .small()
                                .color(theme::TEXT),
                        );
                        ui.add_space(6.0);
                        if ui.button("Generate certificate authority").clicked() {
                            action = Some(CaAction::Generate { replace: false });
                        }
                    }

                    if let Some(error) = &self.tls_ca_error {
                        ui.add_space(6.0);
                        ui.label(RichText::new(error).color(theme::RED));
                    }

                    ui.add_space(12.0);
                    let mut decrypt = self.tls_intercept;
                    let toggle = ui.add_enabled(
                        has_ca,
                        egui::Checkbox::new(&mut decrypt, "Decrypt HTTPS (TLS interception)"),
                    );
                    if toggle.changed() {
                        self.tls_intercept = decrypt;
                        settings_dirty = true;
                    }
                    if has_ca {
                        ui.label(
                            RichText::new(
                                "Applies to new CONNECT tunnels immediately; existing tunnels \
                                 keep the mode they started with.",
                            )
                            .small()
                            .color(theme::TEXT_MUTED),
                        );
                    } else {
                        ui.label(
                            RichText::new("Generate the certificate authority first.")
                                .small()
                                .color(theme::TEXT_MUTED),
                        );
                    }

                    ui.add_space(12.0);
                    ui.label(
                        RichText::new("Pass-through hosts")
                            .strong()
                            .color(theme::TEXT),
                    );
                    ui.label(
                        "Tunnels to these hosts are never decrypted. Matching covers \
                         subdomains and ignores ports.",
                    );
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.label("Exclude host");
                        let response = ui.add(
                            TextEdit::singleline(&mut self.tls_passthrough_input)
                                .hint_text("banking.example.com")
                                .desired_width(300.0),
                        );
                        let enter = response.lost_focus()
                            && ui.input(|input| input.key_pressed(Key::Enter));
                        if (ui.button("Add").clicked() || enter)
                            && let Some(host) =
                                site_map::normalize_scope_input(&self.tls_passthrough_input)
                        {
                            if !self.tls_passthrough.contains(&host) {
                                self.tls_passthrough.push(host);
                                settings_dirty = true;
                            }
                            self.tls_passthrough_input.clear();
                        }
                    });
                    ui.add_space(6.0);
                    if self.tls_passthrough.is_empty() {
                        ui.label(
                            RichText::new(
                                "Every CONNECT target is intercepted while decryption is on.",
                            )
                            .small()
                            .color(theme::TEXT_MUTED),
                        );
                    } else {
                        let hosts: Vec<String> = self.tls_passthrough.clone();
                        for host in hosts {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(&host).monospace().color(theme::TEXT));
                                if ui.small_button("Remove").clicked() {
                                    self.tls_passthrough.retain(|existing| existing != &host);
                                    settings_dirty = true;
                                }
                            });
                        }
                        if ui.button("Clear list").clicked() {
                            self.tls_passthrough.clear();
                            settings_dirty = true;
                        }
                    }
                });

                ui.add_space(10.0);
                card(ui, |ui| {
                    ui.label(
                        RichText::new("Browser trust setup")
                            .size(15.0)
                            .strong()
                            .color(theme::TEXT),
                    );
                    ui.label("1. Generate the certificate authority above.");
                    ui.label("2. Import its certificate into the browser you test with:");
                    ui.label(
                        "Firefox: Settings -> Privacy & Security -> Certificates -> View \
                         Certificates -> Authorities -> Import, select the certificate file, \
                         and trust it to identify websites.",
                    );
                    ui.label(
                        "Chromium on Linux: certutil -d sql:$HOME/.pki/nssdb -A -t \"C,,\" \
                         -n \"Wiremoth CA\" -i <certificate path>",
                    );
                    ui.label(format!(
                        "3. Point the browser at the Wiremoth proxy (127.0.0.1:{proxy_port})."
                    ));
                    ui.label(
                        "4. Enable Decrypt HTTPS, then browse. Decrypted requests appear in \
                         HTTP history as separate https:// entries.",
                    );
                    ui.add_space(6.0);
                    ui.label(
                        "Removal: remove the Wiremoth authority from the browser's certificate \
                         manager, then use Delete files above. Clearing Decrypt HTTPS alone \
                         stops decryption but leaves the trust in place.",
                    );
                    ui.hyperlink_to(
                        RichText::new("Full setup and removal guide").color(theme::ACCENT_DARK),
                        "https://github.com/andyintheshell/wiremoth/blob/main/docs/tls.md",
                    );
                });

                ui.add_space(10.0);
                card(ui, |ui| {
                    ui.label(
                        RichText::new("Authorized testing only")
                            .size(15.0)
                            .strong()
                            .color(theme::TEXT),
                    );
                    ui.label(
                        "Intercepted HTTPS can include passwords, session tokens, and personal \
                         data. Only point Wiremoth at systems you own or are explicitly \
                         authorized to test.",
                    );
                });
            });

        if let Some(text) = copy_text {
            ui.ctx().copy_text(text);
        }

        let mut authority_changed = false;
        match action {
            Some(CaAction::Generate { replace }) => match ca_store::generate(&self.ca_dir) {
                Ok(ca) => {
                    self.tls_ca = Some(ca);
                    self.tls_ca_error = None;
                    authority_changed = true;
                    if replace {
                        log::info!("regenerated the TLS interception certificate authority");
                    } else {
                        log::info!("generated a TLS interception certificate authority");
                    }
                }
                Err(err) => self.tls_ca_error = Some(err),
            },
            Some(CaAction::Delete) => match ca_store::delete(&self.ca_dir) {
                Ok(()) => {
                    self.tls_ca = None;
                    self.tls_intercept = false;
                    self.tls_ca_error = None;
                    authority_changed = true;
                    settings_dirty = true;
                }
                Err(err) => self.tls_ca_error = Some(err),
            },
            None => {}
        }
        if authority_changed && let Some(proxy) = &self.proxy {
            proxy.set_tls_authority(self.tls_ca.clone());
        }
        if settings_dirty {
            self.apply_proxy_settings();
        }
    }
}
