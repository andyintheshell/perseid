//! Data helpers shared by the UI modules.

use std::collections::BTreeMap;

use crate::history::{History, HistoryItem};

pub(super) fn request_url(item: &HistoryItem) -> String {
    if item.is_tunnel() {
        return format!("https://{}", item.request.host);
    }
    if item.request.uri.starts_with("http://") || item.request.uri.starts_with("https://") {
        item.request.uri.clone()
    } else {
        let scheme = if item.request.scheme.is_empty() {
            "http"
        } else {
            item.request.scheme.as_str()
        };
        format!("{scheme}://{}{}", item.request.host, item.request.path)
    }
}

pub(super) fn mime_type(item: &HistoryItem) -> String {
    if item.is_tunnel() {
        return if item.tls_intercepted_requests().is_some() {
            "TLS intercepted".to_string()
        } else {
            "TLS tunnel".to_string()
        };
    }
    item.response
        .as_ref()
        .and_then(|head| {
            head.headers
                .iter()
                .find(|(name, _)| name.eq_ignore_ascii_case("content-type"))
        })
        .map(|(_, value)| value.split(';').next().unwrap_or(value).trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "-".to_string())
}

pub(super) fn captured_hosts(history: &History) -> Vec<(String, usize)> {
    let mut hosts: BTreeMap<String, usize> = BTreeMap::new();
    for item in history.iter() {
        if !item.request.host.is_empty() {
            *hosts.entry(item.request.host.clone()).or_insert(0) += 1;
        }
    }
    hosts.into_iter().collect()
}

pub(super) fn matches_filter(item: &HistoryItem, needle: &str) -> bool {
    let status = item
        .status()
        .map(|status| status.to_string())
        .unwrap_or_default();
    let haystack = format!(
        "{} {} {} {} {}",
        item.request.method, item.request.host, item.request.path, item.request.uri, status
    );
    haystack.to_lowercase().contains(needle)
}
