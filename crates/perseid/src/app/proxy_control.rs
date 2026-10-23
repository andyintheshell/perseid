//! Proxy lifecycle and the settings pushed to the running proxy.

use super::*;
use crate::proxy::{Proxy, ProxyConfig, ProxySettings, TlsConfig, display_ip};

impl PerseidApp {
    /// URL of the built-in certificate page for the running proxy, if any.
    ///
    /// Loopback aliases are used when the proxy binds a wildcard address so
    /// the link is clickable from the same machine.
    pub(super) fn certificate_page_url(&self) -> Option<String> {
        self.proxy.as_ref().map(|proxy| {
            let addr = proxy.local_addr();
            format!("http://{}:{}/", display_ip(addr.ip()), addr.port())
        })
    }

    pub(super) fn start_proxy(&mut self, notify: Arc<dyn Fn() + Send + Sync>) {
        let (settings, rule_error) = self.sanitized_proxy_settings();
        self.rules_error = rule_error;
        let config = ProxyConfig {
            listen: self.listen,
            capture_limit: self.capture_limit,
            intercept_limit: self.intercept_limit,
            settings,
            tls: TlsConfig {
                authority: self.tls_ca.clone(),
                extra_roots_pem: Vec::new(),
            },
            notify: Some(notify),
            ..ProxyConfig::default()
        };
        match Proxy::start(config) {
            Ok(proxy) => {
                log::info!("proxy listening on {}", proxy.local_addr());
                proxy.set_paused(self.paused);
                self.proxy = Some(proxy);
                self.proxy_error = None;
                self.dropped_seen = 0;
                self.clear_intercept_queue();
            }
            Err(err) => {
                log::error!("could not start the proxy: {err}");
                self.proxy = None;
                self.proxy_error = Some(err.to_string());
            }
        }
    }

    pub(super) fn proxy_settings(&self) -> ProxySettings {
        ProxySettings {
            // Burp's master toggle arms interception; requests are held by
            // default, while responses need the separate response rule set.
            intercept_requests: self.intercept_enabled,
            intercept_responses: self.intercept_enabled && self.intercept_responses,
            scope_enforce: self.scope_enforce,
            scope_rules: self.scope.iter().cloned().collect(),
            tls_intercept: self.tls_intercept,
            tls_passthrough: self.tls_passthrough.clone(),
            match_replace: self.match_replace.clone(),
        }
    }

    /// Settings sent to the proxy: enabled rules that compile. A broken regex
    /// stays visible in the editor but never becomes part of the live policy.
    pub(super) fn sanitized_proxy_settings(&self) -> (ProxySettings, Option<String>) {
        sanitized_settings(self.proxy_settings())
    }

    /// Push interception, scope, and rewrite settings to the running proxy.
    /// Invalid regular expressions stay visible in the UI while valid rules
    /// and toggles remain active.
    pub(super) fn apply_proxy_settings(&mut self) {
        let (settings, rule_error) = self.sanitized_proxy_settings();
        let applied = if let Some(proxy) = &self.proxy {
            proxy.set_settings(settings)
        } else {
            Ok(())
        };
        self.rules_error = match (rule_error, applied) {
            (Some(err), _) => Some(err),
            (None, Ok(())) => None,
            (None, Err(err)) => Some(err.to_string()),
        };
    }
    pub(super) fn capture_toggle(&mut self, ui: &mut egui::Ui) {
        let mut capturing = !self.paused;
        if ui
            .checkbox(&mut capturing, "Capture")
            .on_hover_text("Record new exchanges in HTTP history")
            .changed()
        {
            self.paused = !capturing;
            if let Some(proxy) = &self.proxy {
                proxy.set_paused(self.paused);
            }
        }
    }
}

/// Settings sent to the proxy: enabled rules that compile.
///
/// A broken regex stays visible in the editor but never becomes part of the
/// live policy: every rule that fails validation is dropped and the first
/// validation error is reported.
fn sanitized_settings(mut settings: ProxySettings) -> (ProxySettings, Option<String>) {
    let error = settings.validate().err().map(|err| err.to_string());
    if error.is_some() {
        settings
            .match_replace
            .retain(|rule| rule.validate().is_ok());
    }
    (settings, error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proxy::{RuleDirection, RulePart};

    #[test]
    fn sanitized_settings_keep_only_rules_that_compile() {
        let rule = |pattern: &str, regex: bool| MatchReplaceRule {
            enabled: true,
            direction: RuleDirection::Request,
            part: RulePart::All,
            pattern: pattern.to_string(),
            replacement: String::new(),
            regex,
        };

        let settings = ProxySettings {
            match_replace: vec![rule("(", true), rule("ok", false)],
            ..ProxySettings::default()
        };
        let (sanitized, error) = sanitized_settings(settings);
        assert!(error.is_some());
        assert_eq!(sanitized.match_replace.len(), 1);
        assert_eq!(sanitized.match_replace[0].pattern, "ok");

        let (clean, error) = sanitized_settings(ProxySettings {
            match_replace: vec![rule("ok", false)],
            ..ProxySettings::default()
        });
        assert!(error.is_none());
        assert_eq!(clean.match_replace.len(), 1);
    }
}
