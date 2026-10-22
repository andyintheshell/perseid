//! Data helpers shared by the UI modules.

use std::collections::BTreeMap;

use crate::history::{History, HistoryItem};
use crate::proxy::RequestInfo;
use crate::view::{websocket_direction_text, websocket_opcode_text};
use crate::websocket_history::WebSocketMessageItem;

pub(super) fn request_url(item: &HistoryItem) -> String {
    if item.is_tunnel() {
        return format!("https://{}", item.request.host);
    }
    request_info_url(&item.request)
}

pub(super) fn request_info_url(request: &RequestInfo) -> String {
    if request.uri.starts_with("http://") || request.uri.starts_with("https://") {
        request.uri.clone()
    } else {
        let scheme = if request.scheme.is_empty() {
            "http"
        } else {
            request.scheme.as_str()
        };
        format!("{scheme}://{}{}", request.host, request.path)
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

pub(super) fn websocket_matches_filter(item: &WebSocketMessageItem, needle: &str) -> bool {
    let haystack = format!(
        "{} {} {} {} {} {} {}",
        item.connection.url,
        item.connection.host,
        item.connection.id,
        item.connection.listener_port,
        websocket_direction_text(item.direction),
        websocket_opcode_text(item.opcode),
        item.connection.protocol.as_deref().unwrap_or_default(),
    );
    haystack.to_lowercase().contains(needle)
}
