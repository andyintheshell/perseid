//! Data helpers shared by the UI modules.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};

use crate::history::{History, HistoryItem};
use crate::proxy::RequestInfo;
use crate::site_map;
use crate::view::{self, websocket_direction_text, websocket_opcode_text};
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
        format!(
            "{}://{}{}",
            request_scheme(request),
            request.host,
            request.path
        )
    }
}

/// Scheme for a captured request, including absolute-form request targets.
fn request_scheme(request: &RequestInfo) -> &str {
    if !request.scheme.is_empty() {
        &request.scheme
    } else if request.uri.starts_with("https://") {
        "https"
    } else {
        "http"
    }
}

/// Burp's Host column: the scheme and authority without the path.
pub(super) fn request_host_label(item: &HistoryItem) -> String {
    if item.is_tunnel() {
        return format!("https://{}", item.request.host);
    }
    format!("{}://{}", request_scheme(&item.request), item.request.host)
}

/// Burp's URL column: the request path rather than the full absolute URL.
pub(super) fn request_path_label(item: &HistoryItem) -> String {
    if item.is_tunnel() {
        return String::new();
    }
    if item.request.path.is_empty() {
        "/".to_string()
    } else {
        item.request.path.clone()
    }
}

/// Lower-case file extension for a URL path, without the leading dot.
pub(super) fn path_extension(path: &str) -> String {
    let path = path.split(['?', '#']).next().unwrap_or(path);
    let name = path.rsplit('/').next().unwrap_or(path);
    let Some((stem, extension)) = name.rsplit_once('.') else {
        return String::new();
    };
    if stem.is_empty() || extension.is_empty() {
        String::new()
    } else {
        extension.to_ascii_lowercase()
    }
}

pub(super) fn request_extension(item: &HistoryItem) -> String {
    path_extension(&request_path_label(item))
}

/// Whether the exchange travelled over TLS. CONNECT tunnels are always TLS.
pub(super) fn request_uses_tls(item: &HistoryItem) -> bool {
    item.is_tunnel() || request_scheme(&item.request).eq_ignore_ascii_case("https")
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

/// Cached HTML title for one response body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct CachedTitle {
    signature: (u16, u64, bool),
    title: String,
}

pub(super) type TitleCache = RefCell<HashMap<u64, CachedTitle>>;

/// HTML title for the history Title column.
///
/// The title is cached by response status, observed body length, and completion
/// so scrolling a large history does not repeatedly decode the same body. A
/// streaming response stays blank until the body is complete or truncated; the
/// title is not a status indicator and should not flicker while bytes arrive.
pub(super) fn history_title(cache: &TitleCache, item: &HistoryItem) -> String {
    if item.is_tunnel() || site_map::mime_label(&mime_type(item)) != "HTML" {
        return String::new();
    }
    let Some(head) = item.response.as_ref() else {
        return String::new();
    };
    let Some(body) = item.response_body() else {
        return String::new();
    };
    if !body.is_complete() && !body.is_truncated() {
        return String::new();
    }
    let signature = (head.status, body.total_len(), body.is_complete());
    if let Some(cached) = cache
        .borrow()
        .get(&item.id)
        .filter(|cached| cached.signature == signature)
    {
        return cached.title.clone();
    }
    let title = site_map::html_title(&view::response_body_for_search(item)).unwrap_or_default();
    cache.borrow_mut().insert(
        item.id,
        CachedTitle {
            signature,
            title: title.clone(),
        },
    );
    title
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
        "{} {} {} {} {} {} {}",
        item.request.method,
        item.request.host,
        item.request.path,
        item.request.uri,
        status,
        mime_type(item),
        request_extension(item)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::Phase;
    use crate::proxy::{
        CapturedBody, ExchangeResult, ResponseHead, WebSocketConnectionInfo, WebSocketDirection,
        WebSocketOpcode,
    };
    use std::time::{Duration, SystemTime};

    fn request(method: &str, uri: &str, scheme: &str, host: &str, path: &str) -> RequestInfo {
        RequestInfo {
            method: method.to_string(),
            uri: uri.to_string(),
            scheme: scheme.to_string(),
            host: host.to_string(),
            path: path.to_string(),
            version: "HTTP/1.1".to_string(),
            headers: Vec::new(),
        }
    }

    fn item(request: RequestInfo) -> HistoryItem {
        HistoryItem {
            id: 1,
            number: 1,
            timestamp: SystemTime::now(),
            request,
            request_body: CapturedBody::default(),
            response: None,
            phase: Phase::InFlight,
        }
    }

    fn get(uri: &str, scheme: &str, host: &str, path: &str) -> HistoryItem {
        item(request("GET", uri, scheme, host, path))
    }

    fn numbered(id: u64, host: &str) -> HistoryItem {
        let path = format!("/{id}");
        let mut item = get(&path, "http", host, &path);
        item.id = id;
        item.number = id;
        item
    }

    fn response(status: u16, headers: Vec<(String, String)>) -> ResponseHead {
        ResponseHead {
            version: "HTTP/1.1".to_string(),
            status,
            reason: String::new(),
            headers,
        }
    }

    fn header(name: &str, value: &str) -> (String, String) {
        (name.to_string(), value.to_string())
    }

    #[test]
    fn request_url_prefers_an_absolute_request_uri() {
        let insecure = get("http://other.example.com/x", "http", "example.com", "/x");
        assert_eq!(request_url(&insecure), "http://other.example.com/x");

        let secure = get("https://secure.example.com/a", "https", "example.com", "/a");
        assert_eq!(request_url(&secure), "https://secure.example.com/a");
    }

    #[test]
    fn request_url_builds_from_scheme_host_and_path() {
        let item = get("/a?b=c", "https", "example.com:8443", "/a?b=c");
        assert_eq!(request_url(&item), "https://example.com:8443/a?b=c");

        let schemeless = get("/index", "", "example.com", "/index");
        assert_eq!(request_url(&schemeless), "http://example.com/index");
    }

    #[test]
    fn request_url_uses_https_for_connect_tunnels() {
        let tunnel = item(request(
            "CONNECT",
            "example.com:443",
            "https",
            "example.com:443",
            "",
        ));
        assert_eq!(request_url(&tunnel), "https://example.com:443");
    }

    #[test]
    fn mime_type_strips_content_type_parameters() {
        let mut item = get("/", "http", "example.com", "/");
        item.response = Some(response(
            200,
            vec![header("Content-Type", " text/html ; charset=utf-8")],
        ));
        assert_eq!(mime_type(&item), "text/html");
    }

    #[test]
    fn mime_type_matches_the_header_name_case_insensitively() {
        let mut item = get("/", "http", "example.com", "/");
        item.response = Some(response(
            200,
            vec![header("CONTENT-TYPE", "application/json")],
        ));
        assert_eq!(mime_type(&item), "application/json");
    }

    #[test]
    fn mime_type_uses_the_first_content_type_header() {
        let mut item = get("/", "http", "example.com", "/");
        item.response = Some(response(
            200,
            vec![
                header("content-type", "application/json"),
                header("content-type", "text/plain"),
            ],
        ));
        assert_eq!(mime_type(&item), "application/json");
    }

    #[test]
    fn mime_type_falls_back_to_a_dash() {
        let mut item = get("/", "http", "example.com", "/");
        assert_eq!(mime_type(&item), "-");

        item.response = Some(response(200, Vec::new()));
        assert_eq!(mime_type(&item), "-");

        item.response = Some(response(
            200,
            vec![header("content-type", " ; charset=utf-8")],
        ));
        assert_eq!(mime_type(&item), "-");
    }

    #[test]
    fn mime_type_labels_connect_tunnels() {
        let mut tunnel = item(request(
            "CONNECT",
            "example.com:443",
            "https",
            "example.com:443",
            "",
        ));
        assert_eq!(mime_type(&tunnel), "TLS tunnel");

        tunnel.phase = Phase::Finished {
            duration: Duration::from_millis(1),
            result: ExchangeResult::InterceptedTls { requests: 3 },
        };
        assert_eq!(mime_type(&tunnel), "TLS intercepted");
    }

    #[test]
    fn captured_hosts_counts_per_host_in_sorted_order() {
        let mut history = History::new(10, 1024);
        history.load(vec![
            numbered(1, "b.example"),
            numbered(2, "a.example"),
            numbered(3, "b.example"),
            numbered(4, ""),
        ]);
        assert_eq!(
            captured_hosts(&history),
            vec![("a.example".to_string(), 1), ("b.example".to_string(), 2)]
        );
    }

    #[test]
    fn captured_hosts_of_an_empty_history_is_empty() {
        let history = History::new(10, 1024);
        assert!(captured_hosts(&history).is_empty());
    }

    #[test]
    fn matches_filter_searches_method_host_path_uri_and_status() {
        let mut item = get(
            "http://example.com/path?q=1",
            "http",
            "example.com",
            "/path?q=1",
        );
        assert!(matches_filter(&item, "get"));
        assert!(matches_filter(&item, "example.com"));
        assert!(matches_filter(&item, "/path?q=1"));
        assert!(!matches_filter(&item, "404"));
        assert!(!matches_filter(&item, "zzz"));

        item.response = Some(response(404, Vec::new()));
        assert!(matches_filter(&item, "404"));
    }

    #[test]
    fn matches_filter_is_case_insensitive() {
        let item = get("/x", "http", "Example.COM", "/x");
        assert!(matches_filter(&item, "example.com"));
    }

    #[test]
    fn matches_filter_searches_mime_and_extension() {
        let mut item = get("/assets/app.css", "https", "example.com", "/assets/app.css");
        item.response = Some(response(
            200,
            vec![header("content-type", "text/css; charset=utf-8")],
        ));
        assert!(matches_filter(&item, "text/css"));
        assert!(matches_filter(&item, "css"));
    }

    #[test]
    fn host_and_path_labels_split_the_url_like_burp() {
        let search = get(
            "/search?q=one",
            "https",
            "example.com:8443",
            "/search?q=one",
        );
        assert_eq!(request_host_label(&search), "https://example.com:8443");
        assert_eq!(request_path_label(&search), "/search?q=one");
        assert_eq!(request_extension(&search), "");

        let schemeless = get("/", "", "example.com", "/");
        assert_eq!(request_host_label(&schemeless), "http://example.com");
        assert_eq!(request_path_label(&schemeless), "/");

        let absolute = get("https://example.com/", "", "example.com", "/");
        assert_eq!(request_host_label(&absolute), "https://example.com");
        assert!(request_uses_tls(&absolute));

        let tunnel = item(request(
            "CONNECT",
            "example.com:443",
            "https",
            "example.com:443",
            "",
        ));
        assert_eq!(request_host_label(&tunnel), "https://example.com:443");
        assert_eq!(request_path_label(&tunnel), "");
        assert!(request_uses_tls(&tunnel));
    }

    #[test]
    fn path_extension_uses_the_last_segment_without_the_query() {
        assert_eq!(path_extension("/app.js?v=2"), "js");
        assert_eq!(path_extension("/assets/logo.PNG#frag"), "png");
        assert_eq!(path_extension("/archive.tar.gz"), "gz");
        assert_eq!(path_extension("/.well-known/security.txt"), "txt");
        assert_eq!(path_extension("/.hidden"), "");
        assert_eq!(path_extension("/folder.d/file"), "");
        assert_eq!(path_extension("/"), "");
    }

    #[test]
    fn request_uses_tls_for_https_requests_and_tunnels() {
        assert!(request_uses_tls(&get("/", "https", "example.com", "/")));
        assert!(request_uses_tls(&get(
            "https://example.com/",
            "",
            "example.com",
            "/"
        )));
        assert!(!request_uses_tls(&get("/", "http", "example.com", "/")));
    }

    #[test]
    fn history_title_caches_and_ignores_non_html_responses() {
        let cache = TitleCache::default();
        let mut html = get("/", "https", "example.com", "/");
        html.response = Some(response(
            200,
            vec![header("content-type", "text/html; charset=utf-8")],
        ));
        html.phase = Phase::Finished {
            duration: Duration::from_millis(1),
            result: ExchangeResult::Response {
                body: CapturedBody::from_bytes(
                    b"<html><head><title>Example Domain</title></head></html>".to_vec(),
                ),
            },
        };
        assert_eq!(history_title(&cache, &html), "Example Domain");
        assert_eq!(history_title(&cache, &html), "Example Domain");
        assert!(cache.borrow().contains_key(&html.id));

        let plain = get("/", "https", "example.com", "/");
        assert_eq!(history_title(&cache, &plain), "");
    }

    fn websocket_item() -> WebSocketMessageItem {
        WebSocketMessageItem {
            id: 7,
            number: 1,
            timestamp: SystemTime::now(),
            connection: WebSocketConnectionInfo {
                id: 7,
                url: "wss://chat.example.com/socket".to_string(),
                host: "chat.example.com:443".to_string(),
                tls: true,
                listener_port: 8080,
                protocol: Some("graphql-ws".to_string()),
            },
            direction: WebSocketDirection::ToServer,
            opcode: WebSocketOpcode::Text,
            payload: CapturedBody::from_bytes(b"ping".to_vec()),
        }
    }

    #[test]
    fn websocket_matches_filter_covers_connection_metadata() {
        let item = websocket_item();
        assert!(websocket_matches_filter(&item, "wss://chat.example.com"));
        assert!(websocket_matches_filter(&item, "chat.example.com:443"));
        assert!(websocket_matches_filter(&item, "8080"));
        assert!(websocket_matches_filter(&item, "graphql-ws"));
        assert!(websocket_matches_filter(&item, "7"));
        assert!(!websocket_matches_filter(&item, "8081"));
    }

    #[test]
    fn websocket_matches_filter_covers_direction_and_opcode_text() {
        let item = websocket_item();
        assert!(websocket_matches_filter(&item, "to server"));
        assert!(websocket_matches_filter(&item, "text"));
        assert!(!websocket_matches_filter(&item, "binary"));

        let mut client = websocket_item();
        client.direction = WebSocketDirection::ToClient;
        client.opcode = WebSocketOpcode::Binary;
        client.connection.protocol = None;
        assert!(websocket_matches_filter(&client, "to client"));
        assert!(websocket_matches_filter(&client, "binary"));
        assert!(!websocket_matches_filter(&client, "graphql-ws"));
    }
}
