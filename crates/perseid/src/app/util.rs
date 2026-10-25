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
