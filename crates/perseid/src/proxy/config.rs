use std::fmt;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use crate::proxy::rules::{MatchReplaceRule, RuleError};
use crate::proxy::tls::CertificateAuthority;

/// Loopback-only by default: captured traffic may contain credentials.
pub const DEFAULT_LISTEN: &str = "127.0.0.1:8080";
/// Bytes retained per body. Bodies larger than this are forwarded in full but
/// only the leading bytes are captured.
pub const DEFAULT_CAPTURE_LIMIT: usize = 1024 * 1024;
/// Maximum body bytes buffered for interception. Larger or streaming bodies
/// are still held for header editing, but their bodies cannot be edited.
pub const DEFAULT_INTERCEPT_LIMIT: usize = 1024 * 1024;
/// Bound on queued [`crate::proxy::ProxyEvent`]s; when full, events are
/// dropped and counted rather than growing memory without limit.
pub const DEFAULT_EVENT_QUEUE: usize = 256;
pub const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Bound on concurrent client connections.
pub const DEFAULT_MAX_CONNECTIONS: usize = 128;
/// Bound on concurrent CONNECT tunnels. A tunnel also holds a connection slot
/// until its HTTP upgrade completes.
pub const DEFAULT_MAX_TUNNELS: usize = 64;

/// Runtime policy shared by interception, scope enforcement, and rewriting.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProxySettings {
    /// Hold requests for inspection and forwarding: plain HTTP, plus the
    /// decrypted requests of intercepted CONNECT tunnels.
    pub intercept_requests: bool,
    /// Hold responses for inspection and forwarding: plain HTTP, plus the
    /// decrypted responses of intercepted CONNECT tunnels.
    pub intercept_responses: bool,
    /// Block HTTP requests and CONNECT tunnels whose host is outside
    /// [`ProxySettings::scope_rules`].
    pub scope_enforce: bool,
    /// Host authorities that make up the target scope. Matching covers
    /// subdomains and ignores ports. Empty means everything.
    pub scope_rules: Vec<String>,
    /// Terminate TLS for CONNECT targets and inspect the requests inside.
    /// Requires a certificate authority in [`ProxyConfig::tls`]; without one
    /// the proxy keeps tunneling without decryption.
    pub tls_intercept: bool,
    /// Host authorities that are never TLS-intercepted. Matching covers
    /// subdomains and ignores ports, like the scope. Empty means no exclusions.
    pub tls_passthrough: Vec<String>,
    /// Rules applied after interception, in list order.
    pub match_replace: Vec<MatchReplaceRule>,
}

impl ProxySettings {
    /// Validate all enabled rules so a bad regular expression is reported
    /// before it reaches the proxy runtime.
    pub fn validate(&self) -> Result<(), RuleError> {
        for rule in &self.match_replace {
            rule.validate()?;
        }
        Ok(())
    }
}

/// TLS interception configuration for [`crate::proxy::Proxy::start`].
#[derive(Clone, Default)]
pub struct TlsConfig {
    /// Certificate authority used to mint per-host certificates. Without one,
    /// TLS interception stays off even when
    /// [`ProxySettings::tls_intercept`] is enabled.
    pub authority: Option<Arc<CertificateAuthority>>,
    /// Extra PEM-encoded trust anchors for upstream (origin server)
    /// connections, on top of the platform trust store. Tests and internal
    /// deployments with their own CA use this; leave it empty for browsing.
    pub extra_roots_pem: Vec<String>,
}

impl fmt::Debug for TlsConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TlsConfig")
            .field("authority", &self.authority.is_some())
            .field("extra_roots", &self.extra_roots_pem.len())
            .finish()
    }
}

/// Configuration for [`crate::proxy::Proxy::start`].
#[derive(Clone)]
pub struct ProxyConfig {
    /// Address to bind. Keep this on loopback unless you know what you're doing.
    pub listen: SocketAddr,
    /// Per-body capture limit in bytes.
    pub capture_limit: usize,
    /// Maximum body bytes buffered for interception or body rewriting.
    pub intercept_limit: usize,
    /// Interception, scope, and match/replace policy.
    pub settings: ProxySettings,
    /// TLS interception setup: the local CA and upstream trust anchors.
    pub tls: TlsConfig,
    /// Maximum number of events queued towards the consumer.
    pub event_queue: usize,
    /// Timeout for establishing upstream TCP connections.
    pub connect_timeout: Duration,
    /// Maximum number of client connections handled at once. Excess
    /// connections are closed and counted; values below one are treated as one.
    pub max_connections: usize,
    /// Maximum number of CONNECT tunnels active at once. Excess CONNECT
    /// requests receive a 503 response; values below one are treated as one.
    pub max_tunnels: usize,
    /// Called whenever an event is enqueued, so a GUI can wake up its event
    /// loop instead of polling. Must not block.
    pub notify: Option<Arc<dyn Fn() + Send + Sync>>,
}

impl ProxyConfig {
    pub fn new(listen: SocketAddr) -> Self {
        Self {
            listen,
            ..Self::default()
        }
    }
}

impl Default for ProxyConfig {
    fn default() -> Self {
        Self {
            listen: SocketAddr::from(([127, 0, 0, 1], 8080)),
            capture_limit: DEFAULT_CAPTURE_LIMIT,
            intercept_limit: DEFAULT_INTERCEPT_LIMIT,
            settings: ProxySettings::default(),
            tls: TlsConfig::default(),
            event_queue: DEFAULT_EVENT_QUEUE,
            connect_timeout: DEFAULT_CONNECT_TIMEOUT,
            max_connections: DEFAULT_MAX_CONNECTIONS,
            max_tunnels: DEFAULT_MAX_TUNNELS,
            notify: None,
        }
    }
}

impl fmt::Debug for ProxyConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProxyConfig")
            .field("listen", &self.listen)
            .field("capture_limit", &self.capture_limit)
            .field("intercept_limit", &self.intercept_limit)
            .field("settings", &self.settings)
            .field("tls", &self.tls)
            .field("event_queue", &self.event_queue)
            .field("connect_timeout", &self.connect_timeout)
            .field("max_connections", &self.max_connections)
            .field("max_tunnels", &self.max_tunnels)
            .field("notify", &self.notify.is_some())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proxy::rules::{RuleDirection, RulePart};

    fn rule(pattern: &str) -> MatchReplaceRule {
        MatchReplaceRule {
            enabled: true,
            direction: RuleDirection::Request,
            part: RulePart::Head,
            pattern: pattern.to_string(),
            replacement: "replacement".to_string(),
            regex: true,
        }
    }

    fn settings_with(rules: Vec<MatchReplaceRule>) -> ProxySettings {
        ProxySettings {
            match_replace: rules,
            ..ProxySettings::default()
        }
    }

    #[test]
    fn defaults_are_loopback_only_and_match_the_documented_limits() {
        let config = ProxyConfig::default();
        assert_eq!(config.listen, SocketAddr::from(([127, 0, 0, 1], 8080)));
        assert_eq!(config.capture_limit, DEFAULT_CAPTURE_LIMIT);
        assert_eq!(config.intercept_limit, DEFAULT_INTERCEPT_LIMIT);
        assert_eq!(config.event_queue, DEFAULT_EVENT_QUEUE);
        assert_eq!(config.connect_timeout, DEFAULT_CONNECT_TIMEOUT);
        assert_eq!(config.max_connections, DEFAULT_MAX_CONNECTIONS);
        assert_eq!(config.max_tunnels, DEFAULT_MAX_TUNNELS);
        assert!(config.notify.is_none());
    }

    #[test]
    fn default_settings_are_permissive() {
        let settings = ProxySettings::default();
        assert!(!settings.intercept_requests);
        assert!(!settings.intercept_responses);
        assert!(!settings.scope_enforce);
        assert!(settings.scope_rules.is_empty());
        assert!(!settings.tls_intercept);
        assert!(settings.tls_passthrough.is_empty());
        assert!(settings.match_replace.is_empty());

        let tls = TlsConfig::default();
        assert!(tls.authority.is_none());
        assert!(tls.extra_roots_pem.is_empty());
    }

    #[test]
    fn new_keeps_every_default_but_the_listen_address() {
        let listen = SocketAddr::from(([0, 0, 0, 0], 9090));
        let config = ProxyConfig::new(listen);
        let defaults = ProxyConfig::default();

        assert_eq!(config.listen, listen);
        assert_eq!(config.capture_limit, defaults.capture_limit);
        assert_eq!(config.intercept_limit, defaults.intercept_limit);
        assert_eq!(config.event_queue, defaults.event_queue);
        assert_eq!(config.connect_timeout, defaults.connect_timeout);
        assert_eq!(config.max_connections, defaults.max_connections);
        assert_eq!(config.max_tunnels, defaults.max_tunnels);
        assert_eq!(config.settings, defaults.settings);
    }

    #[test]
    fn validate_accepts_no_rules() {
        assert!(ProxySettings::default().validate().is_ok());
    }

    #[test]
    fn validate_ignores_disabled_and_empty_rules() {
        let mut disabled = rule("(");
        disabled.enabled = false;
        let empty_regex = rule("");
        let mut empty_literal = rule("");
        empty_literal.regex = false;

        assert!(
            settings_with(vec![disabled, empty_regex, empty_literal])
                .validate()
                .is_ok()
        );
    }

    #[test]
    fn validate_accepts_a_valid_regex() {
        assert!(
            settings_with(vec![rule(r"secret=(\w+)")])
                .validate()
                .is_ok()
        );
    }

    #[test]
    fn validate_rejects_an_invalid_regex_and_names_the_pattern() {
        let error = settings_with(vec![rule("[")])
            .validate()
            .expect_err("an invalid regex must fail validation");

        match &error {
            RuleError::InvalidRegex { pattern, .. } => assert_eq!(pattern, "["),
        }
        let message = error.to_string();
        assert!(message.contains("invalid regular expression"));
        assert!(message.contains('['));
    }

    #[test]
    fn validate_reports_the_first_invalid_rule() {
        let error = settings_with(vec![rule("[a"), rule("(b")])
            .validate()
            .expect_err("an invalid regex must fail validation");

        match error {
            RuleError::InvalidRegex { pattern, .. } => assert_eq!(pattern, "[a"),
        }
    }

    #[test]
    fn tls_debug_shows_authority_presence_and_root_count_without_pem_bodies() {
        let tls = TlsConfig {
            authority: None,
            extra_roots_pem: vec!["-----BEGIN CERTIFICATE-----\nsecret".to_string()],
        };

        let debug = format!("{tls:?}");
        assert!(debug.contains("authority: false"));
        assert!(debug.contains("extra_roots: 1"));
        assert!(!debug.contains("BEGIN CERTIFICATE"));
        assert!(!debug.contains("secret"));
    }

    #[test]
    fn proxy_config_debug_reports_whether_notify_is_set() {
        assert!(format!("{:?}", ProxyConfig::default()).contains("notify: false"));

        let config = ProxyConfig {
            notify: Some(Arc::new(|| {})),
            ..ProxyConfig::default()
        };
        let debug = format!("{config:?}");
        assert!(debug.contains("notify: true"));
        assert!(debug.contains("127.0.0.1:8080"));
    }
}
