//! Match/replace rules applied to editable request and response messages.
//!
//! Rules run in list order after interception and before a message is sent on
//! the wire. Head rules operate on the request/status line and headers; body
//! rules operate on UTF-8 bodies. Binary bodies are left untouched.

use regex::Regex;

/// Whether a rule applies to requests or responses.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RuleDirection {
    #[default]
    Request,
    Response,
}

/// Which part of a message a rule is allowed to change.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RulePart {
    #[default]
    Head,
    Body,
    All,
}

impl RulePart {
    pub(crate) fn includes_head(self) -> bool {
        matches!(self, Self::Head | Self::All)
    }

    pub(crate) fn includes_body(self) -> bool {
        matches!(self, Self::Body | Self::All)
    }
}

/// A user-configured match/replace rule.
///
/// Patterns are literals by default. With `regex`, the pattern is a Rust
/// [`regex`] expression and `$1`-style capture references work in the
/// replacement. Empty patterns and disabled rules are ignored.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MatchReplaceRule {
    pub enabled: bool,
    pub direction: RuleDirection,
    pub part: RulePart,
    pub pattern: String,
    pub replacement: String,
    pub regex: bool,
}

impl MatchReplaceRule {
    /// Validate this rule if it is enabled and has a pattern.
    pub fn validate(&self) -> Result<(), RuleError> {
        CompiledRule::build(self).map(|_| ())
    }
}

/// A rule could not be compiled.
#[derive(Debug, thiserror::Error)]
pub enum RuleError {
    #[error("invalid regular expression `{pattern}`: {source}")]
    InvalidRegex {
        pattern: String,
        #[source]
        source: regex::Error,
    },
}

#[derive(Clone, Debug)]
enum Matcher {
    Literal(String),
    Regex(Regex),
}

#[derive(Clone, Debug)]
pub(crate) struct CompiledRule {
    matcher: Matcher,
    replacement: String,
    pub(crate) direction: RuleDirection,
    pub(crate) part: RulePart,
}

impl CompiledRule {
    /// Compile one rule. Disabled and empty-pattern rules compile to `None`.
    pub(crate) fn build(rule: &MatchReplaceRule) -> Result<Option<Self>, RuleError> {
        if !rule.enabled || rule.pattern.is_empty() {
            return Ok(None);
        }

        let matcher = if rule.regex {
            Matcher::Regex(
                Regex::new(&rule.pattern).map_err(|source| RuleError::InvalidRegex {
                    pattern: rule.pattern.clone(),
                    source,
                })?,
            )
        } else {
            Matcher::Literal(rule.pattern.clone())
        };

        Ok(Some(Self {
            matcher,
            replacement: rule.replacement.clone(),
            direction: rule.direction,
            part: rule.part,
        }))
    }

    pub(crate) fn apply(&self, text: &str) -> String {
        match &self.matcher {
            Matcher::Literal(pattern) => text.replace(pattern, &self.replacement),
            Matcher::Regex(pattern) => pattern
                .replace_all(text, self.replacement.as_str())
                .into_owned(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(pattern: &str, replacement: &str, regex: bool) -> MatchReplaceRule {
        MatchReplaceRule {
            enabled: true,
            direction: RuleDirection::Request,
            part: RulePart::Head,
            pattern: pattern.to_string(),
            replacement: replacement.to_string(),
            regex,
        }
    }

    #[test]
    fn literal_and_regex_replacement() {
        let literal = CompiledRule::build(&rule("secret", "redacted", false))
            .unwrap()
            .unwrap();
        assert_eq!(literal.apply("a secret value"), "a redacted value");

        let regex = CompiledRule::build(&rule(r"token=(\w+)", "token=[$1]", true))
            .unwrap()
            .unwrap();
        assert_eq!(regex.apply("token=abc123"), "token=[abc123]");
    }

    #[test]
    fn disabled_and_empty_rules_are_ignored() {
        let mut disabled = rule("secret", "redacted", false);
        disabled.enabled = false;
        assert!(CompiledRule::build(&disabled).unwrap().is_none());
        assert!(
            CompiledRule::build(&rule("", "x", false))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn invalid_regex_is_reported() {
        let error = CompiledRule::build(&rule("(", "x", true)).unwrap_err();
        assert!(matches!(error, RuleError::InvalidRegex { .. }));
    }
}
