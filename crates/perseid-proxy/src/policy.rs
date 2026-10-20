//! Immutable snapshots of the runtime policy.

use log::debug;

use crate::config::ProxySettings;
use crate::rules::{CompiledRule, RuleDirection, RuleError};

/// A consistent view of policy for one exchange.
///
/// Stored behind an `RwLock`; requests clone a snapshot so policy changes never
/// require holding a lock across an await.
#[derive(Clone, Debug)]
pub(crate) struct PolicySnapshot {
    intercept_requests: bool,
    intercept_responses: bool,
    scope_enforce: bool,
    scope_rules: Vec<String>,
    tls_intercept: bool,
    tls_passthrough: Vec<String>,
    rules: Vec<CompiledRule>,
}

impl PolicySnapshot {
    pub(crate) fn from_settings(settings: &ProxySettings) -> Result<Self, RuleError> {
        let mut rules = Vec::new();
        for rule in &settings.match_replace {
            if let Some(compiled) = CompiledRule::build(rule)? {
                rules.push(compiled);
            }
        }

        Ok(Self {
            intercept_requests: settings.intercept_requests,
            intercept_responses: settings.intercept_responses,
            scope_enforce: settings.scope_enforce,
            scope_rules: settings.scope_rules.clone(),
            tls_intercept: settings.tls_intercept,
            tls_passthrough: settings.tls_passthrough.clone(),
            rules,
        })
    }

    pub(crate) fn intercept_requests(&self) -> bool {
        self.intercept_requests
    }

    pub(crate) fn intercept_responses(&self) -> bool {
        self.intercept_responses
    }

    /// Scope only becomes a traffic guardrail once it has at least one rule.
    /// An empty scope means "everything", matching the view filter.
    pub(crate) fn scope_enforced(&self) -> bool {
        self.scope_enforce && !self.scope_rules.is_empty()
    }

    pub(crate) fn in_scope(&self, host: &str) -> bool {
        host_in_scope(host, self.scope_rules.iter().map(String::as_str))
    }

    /// Whether TLS interception is enabled in principle. The proxy still
    /// requires a certificate authority before it can terminate TLS.
    pub(crate) fn tls_intercept(&self) -> bool {
        self.tls_intercept
    }

    /// Whether `host` is excluded from TLS interception. An empty list excludes
    /// nothing.
    pub(crate) fn tls_passthrough(&self, host: &str) -> bool {
        any_host_matches(host, self.tls_passthrough.iter().map(String::as_str))
    }

    pub(crate) fn has_request_rules(&self) -> bool {
        self.rules
            .iter()
            .any(|rule| rule.direction == RuleDirection::Request)
    }

    pub(crate) fn has_request_head_rules(&self) -> bool {
        self.rules
            .iter()
            .any(|rule| rule.direction == RuleDirection::Request && rule.part.includes_head())
    }

    pub(crate) fn has_request_body_rules(&self) -> bool {
        self.rules
            .iter()
            .any(|rule| rule.direction == RuleDirection::Request && rule.part.includes_body())
    }

    pub(crate) fn has_response_rules(&self) -> bool {
        self.rules
            .iter()
            .any(|rule| rule.direction == RuleDirection::Response)
    }

    pub(crate) fn has_response_head_rules(&self) -> bool {
        self.rules
            .iter()
            .any(|rule| rule.direction == RuleDirection::Response && rule.part.includes_head())
    }

    pub(crate) fn has_response_body_rules(&self) -> bool {
        self.rules
            .iter()
            .any(|rule| rule.direction == RuleDirection::Response && rule.part.includes_body())
    }

    /// Apply request rules to an editable head and, when available, body.
    pub(crate) fn apply_request(&self, head: &mut String, body: Option<&mut Vec<u8>>) {
        self.apply_head(RuleDirection::Request, head);
        if let Some(body) = body {
            self.apply_body(RuleDirection::Request, body);
        }
    }

    /// Apply response rules to an editable head and, when available, body.
    pub(crate) fn apply_response(&self, head: &mut String, body: Option<&mut Vec<u8>>) {
        self.apply_head(RuleDirection::Response, head);
        if let Some(body) = body {
            self.apply_body(RuleDirection::Response, body);
        }
    }

    pub(crate) fn apply_request_head(&self, head: &mut String) {
        self.apply_head(RuleDirection::Request, head);
    }

    pub(crate) fn apply_request_body(&self, body: &mut Vec<u8>) {
        self.apply_body(RuleDirection::Request, body);
    }

    pub(crate) fn apply_response_head(&self, head: &mut String) {
        self.apply_head(RuleDirection::Response, head);
    }

    pub(crate) fn apply_response_body(&self, body: &mut Vec<u8>) {
        self.apply_body(RuleDirection::Response, body);
    }

    fn apply_head(&self, direction: RuleDirection, head: &mut String) {
        for rule in &self.rules {
            if rule.direction == direction && rule.part.includes_head() {
                *head = rule.apply(head);
            }
        }
    }

    fn apply_body(&self, direction: RuleDirection, body: &mut Vec<u8>) {
        for rule in &self.rules {
            if rule.direction == direction && rule.part.includes_body() {
                apply_body_rule(rule, body);
            }
        }
    }
}

fn apply_body_rule(rule: &CompiledRule, body: &mut Vec<u8>) {
    let Ok(text) = std::str::from_utf8(body) else {
        debug!("skipping body match/replace rule: body is not UTF-8");
        return;
    };
    let replaced = rule.apply(text);
    if replaced != text {
        *body = replaced.into_bytes();
    }
}

/// Whether a host matches a scope. Rules are authorities; matching covers
/// subdomains and ignores ports. An empty rule set includes everything.
pub fn host_in_scope<'a>(host: &str, rules: impl IntoIterator<Item = &'a str>) -> bool {
    let mut any_rule = false;
    let mut matched = false;

    for rule in rules {
        let rule = normalize_host(rule);
        if rule.is_empty() {
            continue;
        }
        any_rule = true;
        if host_matches(host, &rule) {
            matched = true;
            break;
        }
    }

    !any_rule || matched
}

/// Whether a host matches any non-empty rule. Used for exclusions such as the
/// TLS pass-through list, where an empty list matches nothing.
pub(crate) fn any_host_matches<'a>(host: &str, rules: impl IntoIterator<Item = &'a str>) -> bool {
    rules
        .into_iter()
        .any(|rule| !rule.trim().is_empty() && host_matches(host, rule))
}

fn host_matches(host: &str, rule: &str) -> bool {
    let host = normalize_host(host);
    let rule = normalize_host(rule);
    let host_without_port = without_port(&host);
    let rule_without_port = without_port(&rule);
    host == rule
        || host_without_port == rule_without_port
        || (!rule_without_port.is_empty()
            && host_without_port.ends_with(&format!(".{rule_without_port}")))
}

fn normalize_host(host: &str) -> String {
    host.trim().trim_end_matches('.').to_ascii_lowercase()
}

fn without_port(host: &str) -> &str {
    if let Some(rest) = host.strip_prefix('[') {
        return rest.split_once(']').map(|(inner, _)| inner).unwrap_or(host);
    }
    if host.matches(':').count() == 1 {
        return host.split_once(':').map(|(name, _)| name).unwrap_or(host);
    }
    host
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::{MatchReplaceRule, RulePart};

    fn settings() -> ProxySettings {
        ProxySettings::default()
    }

    #[test]
    fn scope_matches_hosts_subdomains_and_ports() {
        assert!(host_in_scope("example.com", ["example.com"]));
        assert!(host_in_scope("example.com:8080", ["example.com"]));
        assert!(host_in_scope("api.example.com", ["example.com"]));
        assert!(!host_in_scope("example.net", ["example.com"]));
        assert!(!host_in_scope("notexample.com", ["example.com"]));
        assert!(host_in_scope("anything.test", std::iter::empty()));
        assert!(host_in_scope("anything.test", [""]));
    }

    #[test]
    fn empty_scope_never_enforces() {
        let mut settings = settings();
        settings.scope_enforce = true;
        let policy = PolicySnapshot::from_settings(&settings).unwrap();
        assert!(!policy.scope_enforced());
        assert!(policy.in_scope("anything.test"));
    }

    #[test]
    fn tls_interception_is_off_until_enabled() {
        let policy = PolicySnapshot::from_settings(&settings()).unwrap();
        assert!(!policy.tls_intercept());

        let mut enabled = settings();
        enabled.tls_intercept = true;
        let policy = PolicySnapshot::from_settings(&enabled).unwrap();
        assert!(policy.tls_intercept());
    }

    #[test]
    fn tls_passthrough_matches_subdomains_and_ignores_ports() {
        let mut settings = settings();
        settings.tls_passthrough = vec!["banking.example".to_string(), "10.0.0.5".to_string()];
        let policy = PolicySnapshot::from_settings(&settings).unwrap();

        assert!(policy.tls_passthrough("banking.example"));
        assert!(policy.tls_passthrough("banking.example:443"));
        assert!(policy.tls_passthrough("login.banking.example"));
        assert!(policy.tls_passthrough("10.0.0.5:8443"));
        assert!(!policy.tls_passthrough("example.com"));
        assert!(!policy.tls_passthrough("notbanking.example"));

        // An empty list excludes nothing, so every tunnel is a candidate.
        settings.tls_passthrough.clear();
        let policy = PolicySnapshot::from_settings(&settings).unwrap();
        assert!(!policy.tls_passthrough("example.com"));
    }

    #[test]
    fn rules_apply_in_order_to_heads_and_utf8_bodies() {
        let mut settings = settings();
        settings.match_replace = vec![
            MatchReplaceRule {
                enabled: true,
                direction: RuleDirection::Request,
                part: RulePart::Head,
                pattern: "secret".to_string(),
                replacement: "redacted".to_string(),
                regex: false,
            },
            MatchReplaceRule {
                enabled: true,
                direction: RuleDirection::Request,
                part: RulePart::Body,
                pattern: "old".to_string(),
                replacement: "new".to_string(),
                regex: false,
            },
        ];
        let policy = PolicySnapshot::from_settings(&settings).unwrap();
        let mut head = "POST / HTTP/1.1\r\nX-Test: secret\r\n\r\n".to_string();
        let mut body = b"old value".to_vec();
        policy.apply_request(&mut head, Some(&mut body));
        assert!(head.contains("X-Test: redacted"));
        assert_eq!(body, b"new value");
    }

    #[test]
    fn invalid_regex_is_rejected() {
        let mut settings = settings();
        settings.match_replace = vec![MatchReplaceRule {
            enabled: true,
            direction: RuleDirection::Request,
            part: RulePart::Head,
            pattern: "(".to_string(),
            replacement: "x".to_string(),
            regex: true,
        }];
        assert!(PolicySnapshot::from_settings(&settings).is_err());
    }
}
