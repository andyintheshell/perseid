//! Site map derived from captured HTTP exchanges for the Target tab.
//!
//! The map groups captured exchanges by host and path, keeping the latest
//! exchange for each path. CONNECT tunnels are omitted because they carry no
//! HTTP request of their own; decrypted requests appear as their own entries.

use std::collections::{BTreeMap, BTreeSet};

use crate::history::History;

#[derive(Debug, Default)]
pub struct SiteMap {
    pub hosts: Vec<SiteHost>,
}

#[derive(Debug)]
pub struct SiteHost {
    pub host: String,
    pub entries: Vec<SiteEntry>,
}

#[derive(Debug)]
pub struct SiteEntry {
    pub path: String,
    pub method: String,
    pub id: u64,
    pub count: usize,
}

impl SiteMap {
    pub fn build(history: &History, scope: Option<&BTreeSet<String>>) -> Self {
        let mut hosts: BTreeMap<String, BTreeMap<String, SiteEntry>> = BTreeMap::new();

        for item in history.iter() {
            if item.is_tunnel() {
                continue;
            }
            if let Some(scope) = scope
                && !host_in_scope(&item.request.host, scope)
            {
                continue;
            }

            let host = if item.request.host.is_empty() {
                "(unknown host)".to_string()
            } else {
                item.request.host.clone()
            };
            let path = if item.request.path.is_empty() {
                "/".to_string()
            } else {
                item.request.path.clone()
            };

            let entry = hosts
                .entry(host)
                .or_default()
                .entry(path.clone())
                .or_insert_with(|| SiteEntry {
                    path,
                    method: item.request.method.clone(),
                    id: item.id,
                    count: 0,
                });
            entry.count += 1;
            if item.id >= entry.id {
                entry.id = item.id;
                entry.method = item.request.method.clone();
            }
        }

        let hosts = hosts
            .into_iter()
            .map(|(host, entries)| SiteHost {
                host,
                entries: entries.into_values().collect(),
            })
            .collect();

        Self { hosts }
    }

    pub fn is_empty(&self) -> bool {
        self.hosts.is_empty()
    }

    pub fn endpoint_count(&self) -> usize {
        self.hosts.iter().map(|host| host.entries.len()).sum()
    }
}

/// Whether a captured host matches the scope. An empty scope includes
/// everything so an unconfigured scope does not hide the whole workspace.
///
/// This delegates to the proxy so the view filter and the forwarding guardrail
/// cannot drift apart.
pub fn host_in_scope(host: &str, scope: &BTreeSet<String>) -> bool {
    crate::proxy::host_in_scope(host, scope.iter().map(String::as_str))
}

/// Normalize user input for the scope list: accept a bare host, a host with a
/// port, or a full URL, and keep only the authority.
pub fn normalize_scope_input(input: &str) -> Option<String> {
    let input = input.trim();
    if input.is_empty() {
        return None;
    }
    let input = input
        .split_once("://")
        .map_or(input, |(_, authority)| authority);
    let authority = input.split(['/', '?', '#']).next().unwrap_or(input).trim();
    let authority = authority.trim_end_matches('.');
    if authority.is_empty() {
        None
    } else {
        Some(authority.to_ascii_lowercase())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::time::SystemTime;

    use crate::proxy::{ProxyEvent, RequestInfo};

    use super::*;
    use crate::history::DEFAULT_HISTORY_BYTES;

    fn history(entries: &[(&str, &str, &str)]) -> History {
        let mut history = History::new(100, DEFAULT_HISTORY_BYTES);
        for (index, (method, host, path)) in entries.iter().enumerate() {
            history.apply(ProxyEvent::Started {
                id: index as u64 + 1,
                timestamp: SystemTime::now(),
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
    fn groups_by_host_and_path_with_latest_exchange() {
        let history = history(&[
            ("GET", "example.com", "/a"),
            ("POST", "example.com", "/a"),
            ("GET", "example.com", "/b"),
            ("GET", "other.test", "/"),
        ]);
        let map = SiteMap::build(&history, None);
        assert_eq!(map.hosts.len(), 2);
        assert_eq!(map.endpoint_count(), 3);

        let example = &map.hosts[0];
        assert_eq!(example.host, "example.com");
        assert_eq!(example.entries.len(), 2);
        let a = example
            .entries
            .iter()
            .find(|entry| entry.path == "/a")
            .unwrap();
        assert_eq!(a.count, 2);
        assert_eq!(a.method, "POST");
        assert_eq!(a.id, 2);
    }

    #[test]
    fn omits_connect_tunnels() {
        let history = history(&[
            ("CONNECT", "example.com:443", ""),
            ("GET", "example.com", "/"),
        ]);
        let map = SiteMap::build(&history, None);
        assert_eq!(map.endpoint_count(), 1);
        assert_eq!(map.hosts[0].entries[0].path, "/");
    }

    #[test]
    fn scope_matches_bare_hosts_and_subdomains() {
        let scope = BTreeSet::from(["example.com".to_string()]);
        assert!(host_in_scope("example.com", &scope));
        assert!(host_in_scope("example.com:8080", &scope));
        assert!(host_in_scope("api.example.com", &scope));
        assert!(!host_in_scope("example.net", &scope));
        assert!(!host_in_scope("notexample.com", &scope));
    }

    #[test]
    fn empty_scope_includes_everything() {
        assert!(host_in_scope("anything.test", &BTreeSet::new()));
    }

    #[test]
    fn normalizes_scope_input() {
        assert_eq!(
            normalize_scope_input(" https://Example.COM/path?q=1 "),
            Some("example.com".to_string())
        );
        assert_eq!(
            normalize_scope_input("example.com:8443"),
            Some("example.com:8443".to_string())
        );
        assert_eq!(
            normalize_scope_input("HTTPS://EXAMPLE.com?q=1"),
            Some("example.com".to_string())
        );
        assert_eq!(normalize_scope_input("   "), None);
    }
}
