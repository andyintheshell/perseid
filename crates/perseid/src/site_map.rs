//! Site map derived from captured HTTP exchanges for the Target tab.
//!
//! The map groups captured exchanges by host, path, and method, keeping the
//! latest exchange for each endpoint. The URL tree then folds query variants
//! of the same path into one node while the Contents table keeps one row per
//! endpoint. CONNECT tunnels are omitted because they carry no HTTP request of
//! their own; decrypted requests appear as their own entries.
//!
//! Everything here is derived from the bounded history snapshot. The site map
//! is not an independent record store and it does not crawl or infer URLs.

use std::collections::{BTreeMap, BTreeSet};
use std::time::SystemTime;

use crate::history::History;
use crate::proxy::RequestInfo;
use crate::view;

/// One endpoint in the site map: a unique method plus request target.
#[derive(Debug, Clone)]
pub struct SiteEntry {
    pub host: String,
    pub scheme: String,
    pub path: String,
    pub method: String,
    /// Latest exchange for this endpoint. Repeated captures do not create
    /// duplicate rows; `count` records how often the endpoint was seen.
    pub id: u64,
    pub count: usize,
    pub status: Option<u16>,
    pub in_flight: bool,
    pub dropped: bool,
    pub length: u64,
    pub mime: String,
    pub title: String,
    pub params: usize,
    pub timestamp: SystemTime,
}

impl SiteEntry {
    /// The URL path without a query string or fragment. The tree uses this so
    /// `?sort=price` and `?sort=name` share a single leaf node.
    pub fn base_path(&self) -> String {
        base_path(&self.path)
    }

    /// Base path normalized for tree matching: no trailing slash except for
    /// the root path.
    pub fn tree_path(&self) -> String {
        normalize_tree_path(&self.base_path())
    }

    pub fn url(&self) -> String {
        format!("{}://{}{}", self.scheme, self.host, self.path)
    }
}

#[derive(Debug)]
pub struct SiteHost {
    pub host: String,
    pub scheme: String,
    pub entries: Vec<SiteEntry>,
}

#[derive(Debug, Default)]
pub struct SiteMap {
    pub hosts: Vec<SiteHost>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SiteNodeId {
    Host {
        host: String,
    },
    Directory {
        host: String,
        /// Normalized absolute path without a trailing slash, except `/`.
        path: String,
    },
    Endpoint {
        host: String,
        path: String,
    },
}

impl SiteNodeId {
    pub fn host(&self) -> &str {
        match self {
            Self::Host { host } | Self::Directory { host, .. } | Self::Endpoint { host, .. } => {
                host
            }
        }
    }

    /// Stable key used for expansion state and egui widget IDs.
    pub fn key(&self) -> String {
        match self {
            Self::Host { host } => format!("host:{host}"),
            Self::Directory { host, path } => format!("dir:{host}:{path}"),
            Self::Endpoint { host, path } => format!("endpoint:{host}:{path}"),
        }
    }

    /// Whether this node contains an endpoint row in the contents table.
    pub fn contains(&self, entry: &SiteEntry) -> bool {
        if self.host() != entry.host {
            return false;
        }
        let path = entry.tree_path();
        match self {
            Self::Host { .. } => true,
            Self::Directory { path: dir, .. } => {
                dir == "/" || path == *dir || path.starts_with(&format!("{dir}/"))
            }
            Self::Endpoint { path: endpoint, .. } => path == *endpoint,
        }
    }
}

/// One rendered node in the URL tree.
#[derive(Debug, Clone)]
pub struct SiteTreeNode {
    pub id: SiteNodeId,
    pub label: String,
    /// Scheme for host nodes; empty for folders and resources.
    pub scheme: String,
    /// Methods captured for this exact path. Used for tooltips.
    pub methods: Vec<String>,
    /// Endpoint rows behind the visible entries at and below this node.
    pub count: usize,
    pub children: Vec<SiteTreeNode>,
}

impl SiteTreeNode {
    pub fn is_directory(&self) -> bool {
        matches!(
            self.id,
            SiteNodeId::Host { .. } | SiteNodeId::Directory { .. }
        )
    }

    pub fn find(&self, id: &SiteNodeId) -> Option<&SiteTreeNode> {
        if &self.id == id {
            return Some(self);
        }
        self.children.iter().find_map(|child| child.find(id))
    }
}

/// Burp's default site-map display filters. Every setting only affects what is
/// shown: the history remains unchanged and hidden items return immediately
/// when the filter is relaxed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SiteMapFilter {
    pub hide_not_found: bool,
    pub hide_css: bool,
    pub hide_images: bool,
    pub hide_binary: bool,
    pub hide_4xx: bool,
    pub hide_empty_folders: bool,
    pub parameterized_only: bool,
}

impl Default for SiteMapFilter {
    fn default() -> Self {
        Self {
            hide_not_found: true,
            hide_css: true,
            hide_images: true,
            hide_binary: true,
            hide_4xx: true,
            hide_empty_folders: true,
            parameterized_only: false,
        }
    }
}

impl SiteMapFilter {
    /// A filter that hides nothing.
    pub const SHOW_ALL: Self = Self {
        hide_not_found: false,
        hide_css: false,
        hide_images: false,
        hide_binary: false,
        hide_4xx: false,
        hide_empty_folders: false,
        parameterized_only: false,
    };

    pub fn allows(self, entry: &SiteEntry) -> bool {
        if self.parameterized_only && entry.params == 0 {
            return false;
        }
        if self.hide_not_found && entry.status == Some(404) {
            return false;
        }
        if self.hide_css && entry.mime.eq_ignore_ascii_case("text/css") {
            return false;
        }
        if self.hide_images && entry.mime.to_ascii_lowercase().starts_with("image/") {
            return false;
        }
        if self.hide_binary && is_binary_mime(&entry.mime) {
            return false;
        }
        if self.hide_4xx
            && entry
                .status
                .is_some_and(|status| (400..500).contains(&status))
        {
            return false;
        }
        true
    }

    /// The text Burp shows in the filter bar.
    pub fn summary(self) -> String {
        let mut parts = Vec::new();
        if self.hide_not_found {
            parts.push("Hiding not found items".to_string());
        }
        let mut mime = Vec::new();
        if self.hide_css {
            mime.push("CSS");
        }
        if self.hide_images {
            mime.push("image");
        }
        if self.hide_binary {
            mime.push("general binary");
        }
        if !mime.is_empty() {
            let joined = match mime.as_slice() {
                [one] => (*one).to_string(),
                [one, two] => format!("{one} and {two}"),
                _ => format!(
                    "{}, {} and {}",
                    mime[..mime.len() - 2].join(", "),
                    mime[mime.len() - 2],
                    mime[mime.len() - 1]
                ),
            };
            parts.push(format!("hiding {joined} content"));
        }
        if self.hide_4xx {
            parts.push("hiding 4xx responses".to_string());
        }
        if self.hide_empty_folders {
            parts.push("hiding empty folders".to_string());
        }
        if self.parameterized_only {
            parts.push("showing only parameterized requests".to_string());
        }
        if parts.is_empty() {
            "Showing all items".to_string()
        } else {
            parts.join("; ")
        }
    }
}

impl SiteMap {
    pub fn build(history: &History, scope: Option<&BTreeSet<String>>) -> Self {
        let mut hosts: BTreeMap<String, HostBuilder> = BTreeMap::new();

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
            let scheme = request_scheme(&item.request);

            let builder = hosts.entry(host.clone()).or_default();
            let entry = builder
                .entries
                .entry((path.clone(), item.request.method.clone()))
                .or_insert_with(|| SiteEntry {
                    host: host.clone(),
                    scheme: scheme.clone(),
                    path: path.clone(),
                    method: item.request.method.clone(),
                    id: item.id,
                    count: 0,
                    status: None,
                    in_flight: false,
                    dropped: false,
                    length: 0,
                    mime: String::new(),
                    title: String::new(),
                    params: 0,
                    timestamp: item.timestamp,
                });
            entry.count += 1;
            if item.id >= entry.id {
                update_entry(entry, item, &scheme);
            }
        }

        let hosts = hosts
            .into_iter()
            .map(|(host, builder)| {
                let mut entries: Vec<SiteEntry> = builder.entries.into_values().collect();
                entries.sort_by(|left, right| {
                    left.path
                        .cmp(&right.path)
                        .then_with(|| left.method.cmp(&right.method))
                });
                let scheme = entries
                    .iter()
                    .max_by_key(|entry| entry.id)
                    .map(|entry| entry.scheme.clone())
                    .unwrap_or_else(|| "http".to_string());
                SiteHost {
                    host,
                    scheme,
                    entries,
                }
            })
            .collect();

        Self { hosts }
    }

    pub fn is_empty(&self) -> bool {
        self.hosts.is_empty()
    }

    #[cfg(test)]
    pub fn endpoint_count(&self) -> usize {
        self.hosts.iter().map(|host| host.entries.len()).sum()
    }

    /// Build the URL tree for the given display filter.
    ///
    /// Directories with no visible descendants disappear when
    /// `hide_empty_folders` is set. When it is cleared, those folders remain
    /// visible (without the hidden leaf items) so the hierarchy still shows
    /// where filtered content lives.
    pub fn tree(&self, filter: SiteMapFilter) -> Vec<SiteTreeNode> {
        let mut nodes = Vec::new();
        for host in &self.hosts {
            let mut root = TreeDirectory::new("/".to_string(), "/".to_string(), true);
            for entry in &host.entries {
                let visible = filter.allows(entry);
                root.insert(entry, visible);
            }

            let Some(root_node) = root.finish(host, filter.hide_empty_folders) else {
                continue;
            };
            nodes.push(SiteTreeNode {
                id: SiteNodeId::Host {
                    host: host.host.clone(),
                },
                label: host.host.clone(),
                scheme: host.scheme.clone(),
                methods: Vec::new(),
                count: root_node.count,
                children: vec![root_node],
            });
        }
        nodes
    }
}

impl SiteEntry {
    pub fn mime_label(&self) -> String {
        mime_label(&self.mime)
    }

    pub fn status_label(&self) -> String {
        if let Some(status) = self.status {
            status.to_string()
        } else if self.in_flight {
            "…".to_string()
        } else if self.dropped {
            String::new()
        } else {
            "failed".to_string()
        }
    }
}

#[derive(Default)]
struct HostBuilder {
    entries: BTreeMap<(String, String), SiteEntry>,
}

struct TreeDirectory {
    label: String,
    path: String,
    is_directory: bool,
    visible_entries: usize,
    visible_methods: BTreeSet<String>,
    children: BTreeMap<String, TreeDirectory>,
}

impl TreeDirectory {
    fn new(label: String, path: String, is_directory: bool) -> Self {
        Self {
            label,
            path,
            is_directory,
            visible_entries: 0,
            visible_methods: BTreeSet::new(),
            children: BTreeMap::new(),
        }
    }

    fn insert(&mut self, entry: &SiteEntry, visible: bool) {
        let base = entry.base_path();
        let segments: Vec<&str> = base.split('/').filter(|part| !part.is_empty()).collect();
        let is_directory_path = base.ends_with('/') || segments.is_empty();
        let directory_segments = if is_directory_path {
            segments.len()
        } else {
            segments.len().saturating_sub(1)
        };

        let mut node = self;
        for segment in &segments[..directory_segments] {
            let path = join_path(&node.path, segment);
            node = node
                .children
                .entry((*segment).to_string())
                .or_insert_with(|| TreeDirectory::new((*segment).to_string(), path, true));
            node.is_directory = true;
        }

        if !is_directory_path {
            let file = segments.last().copied().unwrap_or_default();
            let path = join_path(&node.path, file);
            node = node
                .children
                .entry(file.to_string())
                .or_insert_with(|| TreeDirectory::new(file.to_string(), path, false));
        } else {
            node.is_directory = true;
        }

        if visible {
            node.visible_entries += 1;
            node.visible_methods.insert(entry.method.clone());
        }
    }

    fn finish(self, host: &SiteHost, hide_empty_folders: bool) -> Option<SiteTreeNode> {
        let mut children: Vec<SiteTreeNode> = self
            .children
            .into_values()
            .filter_map(|child| child.finish(host, hide_empty_folders))
            .collect();
        children.sort_by(|left, right| {
            left.label
                .to_ascii_lowercase()
                .cmp(&right.label.to_ascii_lowercase())
                .then_with(|| left.label.cmp(&right.label))
                .then_with(|| left.is_directory().cmp(&right.is_directory()).reverse())
        });

        let is_directory = self.is_directory || self.path == "/" || !children.is_empty();
        let visible =
            self.visible_entries + children.iter().map(|child| child.count).sum::<usize>();
        if visible == 0 && (hide_empty_folders || !is_directory) {
            return None;
        }

        let id = if is_directory {
            SiteNodeId::Directory {
                host: host.host.clone(),
                path: self.path,
            }
        } else {
            SiteNodeId::Endpoint {
                host: host.host.clone(),
                path: self.path,
            }
        };

        let id = match id {
            SiteNodeId::Directory { host, mut path } => {
                path = normalize_tree_path(&path);
                SiteNodeId::Directory { host, path }
            }
            other => other,
        };

        Some(SiteTreeNode {
            id,
            label: self.label,
            scheme: String::new(),
            methods: self.visible_methods.into_iter().collect(),
            count: visible,
            children,
        })
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

/// The tree path for a URL path: query and fragment removed, empty normalized
/// to `/`.
pub fn base_path(path: &str) -> String {
    let end = path.find(['?', '#']).unwrap_or(path.len());
    let base = &path[..end];
    if base.is_empty() {
        "/".to_string()
    } else {
        base.to_string()
    }
}

/// Whether a response is one Burp would classify as general binary content.
pub fn is_binary_mime(mime: &str) -> bool {
    let mime = mime.to_ascii_lowercase();
    if mime.starts_with("audio/")
        || mime.starts_with("video/")
        || mime.starts_with("font/")
        || mime.ends_with("+zip")
    {
        return true;
    }
    matches!(
        mime.as_str(),
        "application/octet-stream"
            | "application/pdf"
            | "application/zip"
            | "application/gzip"
            | "application/x-gzip"
            | "application/x-tar"
            | "application/x-7z-compressed"
            | "application/x-rar-compressed"
            | "application/wasm"
            | "application/protobuf"
            | "application/x-protobuf"
            | "application/vnd.ms-fontobject"
    )
}

/// Burp-style shorthand for the MIME type column.
pub fn mime_label(mime: &str) -> String {
    match mime.to_ascii_lowercase().as_str() {
        "" => String::new(),
        "text/html" | "application/xhtml+xml" => "HTML".to_string(),
        "text/css" => "CSS".to_string(),
        "text/plain" => "text".to_string(),
        "application/json" | "application/problem+json" => "JSON".to_string(),
        "application/javascript" | "text/javascript" | "application/x-javascript" => {
            "script".to_string()
        }
        other if other.starts_with("image/") => "image".to_string(),
        _ if is_binary_mime(mime) => "binary".to_string(),
        _ => mime.to_string(),
    }
}

/// Extract an HTML title for the site-map Title column.
pub fn html_title(bytes: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(bytes);
    let lower = text.to_ascii_lowercase();
    let start = lower.find("<title")?;
    let open_end = start + lower[start..].find('>')? + 1;
    let close = open_end + lower[open_end..].find("</title")?;
    let raw = text.get(open_end..close)?;
    let decoded = decode_html_entities(raw);
    let collapsed = decoded.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() {
        None
    } else {
        Some(collapsed.chars().take(256).collect())
    }
}

fn update_entry(entry: &mut SiteEntry, item: &crate::history::HistoryItem, scheme: &str) {
    entry.id = item.id;
    entry.scheme = scheme.to_string();
    entry.timestamp = item.timestamp;
    entry.status = item.status();
    entry.in_flight = item.is_in_flight();
    entry.dropped = item.is_dropped();
    entry.length = item.transferred();
    entry.mime = response_mime(item.response.as_ref());
    entry.title = if entry.mime.eq_ignore_ascii_case("text/html")
        || entry.mime.eq_ignore_ascii_case("application/xhtml+xml")
    {
        let body = view::response_body_for_search(item);
        html_title(&body).unwrap_or_default()
    } else {
        String::new()
    };
    entry.params = parameter_count(&item.request, item.request_body.data());
}

fn request_scheme(request: &RequestInfo) -> String {
    if !request.scheme.is_empty() {
        return request.scheme.to_ascii_lowercase();
    }
    if request.uri.starts_with("https://") {
        "https".to_string()
    } else {
        "http".to_string()
    }
}

fn response_mime(response: Option<&crate::proxy::ResponseHead>) -> String {
    response
        .and_then(|head| {
            head.headers
                .iter()
                .find(|(name, _)| name.eq_ignore_ascii_case("content-type"))
        })
        .map(|(_, value)| value.split(';').next().unwrap_or(value).trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_default()
}

/// Count query and form parameters the way Burp's Params flag does.
fn parameter_count(request: &RequestInfo, body: &[u8]) -> usize {
    let mut count = parameter_count_in_path(&request.path);
    let content_type = request
        .headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("content-type"))
        .map(|(_, value)| value.to_ascii_lowercase())
        .unwrap_or_default();
    if content_type.starts_with("application/x-www-form-urlencoded") {
        count += form_parameter_count(body);
    } else if content_type.starts_with("multipart/form-data") {
        let text = String::from_utf8_lossy(body);
        count += text.matches("; name=\"").count();
    }
    count
}

fn parameter_count_in_path(path: &str) -> usize {
    let Some((_, query)) = path.split_once('?') else {
        return 0;
    };
    query
        .split('#')
        .next()
        .unwrap_or(query)
        .split('&')
        .filter(|part| {
            let name = part.split('=').next().unwrap_or(part);
            !name.trim().is_empty()
        })
        .count()
}

fn form_parameter_count(body: &[u8]) -> usize {
    let text = String::from_utf8_lossy(body);
    text.split('&')
        .filter(|part| {
            let name = part.split('=').next().unwrap_or(part);
            !name.trim().is_empty()
        })
        .count()
}

fn decode_html_entities(input: &str) -> String {
    input
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&nbsp;", " ")
}

fn join_path(parent: &str, segment: &str) -> String {
    if parent == "/" {
        format!("/{segment}")
    } else {
        format!("{parent}/{segment}")
    }
}

fn normalize_tree_path(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        "/".to_string()
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::time::{Duration, SystemTime};

    use crate::history::{FALLBACK_HISTORY_BYTES, History};
    use crate::proxy::{
        CapturedBody, ExchangeEnd, ExchangeResult, ProxyEvent, RequestInfo, ResponseHead,
    };

    use super::*;

    fn request(method: &str, host: &str, path: &str) -> RequestInfo {
        RequestInfo {
            method: method.to_string(),
            uri: format!("http://{host}{path}"),
            scheme: "http".to_string(),
            host: host.to_string(),
            path: path.to_string(),
            version: "HTTP/1.1".to_string(),
            headers: Vec::new(),
        }
    }

    fn history(entries: &[(&str, &str, &str)]) -> History {
        let mut history = History::new(100, FALLBACK_HISTORY_BYTES);
        for (index, (method, host, path)) in entries.iter().enumerate() {
            history.apply(ProxyEvent::Started {
                id: index as u64 + 1,
                timestamp: SystemTime::UNIX_EPOCH + Duration::from_secs(index as u64 + 1),
                request: request(method, host, path),
            });
        }
        history
    }

    fn finish(history: &mut History, id: u64, status: u16, mime: &str, body: &[u8]) {
        let mut headers = Vec::new();
        if !mime.is_empty() {
            headers.push(("content-type".to_string(), mime.to_string()));
        }
        history.apply(ProxyEvent::ResponseHead {
            id,
            head: ResponseHead {
                version: "HTTP/1.1".to_string(),
                status,
                reason: String::new(),
                headers,
            },
        });
        history.apply(ProxyEvent::Finished {
            id,
            end: ExchangeEnd {
                duration: Duration::from_millis(5),
                request_body: CapturedBody::default(),
                result: ExchangeResult::Response {
                    body: CapturedBody::from_bytes(body.to_vec()),
                },
            },
        });
    }

    #[test]
    fn groups_by_host_path_and_method_with_latest_exchange() {
        let history = history(&[
            ("GET", "example.com", "/a"),
            ("GET", "example.com", "/a"),
            ("POST", "example.com", "/a"),
            ("GET", "example.com", "/b"),
            ("GET", "other.test", "/"),
        ]);
        let map = SiteMap::build(&history, None);
        assert_eq!(map.hosts.len(), 2);
        assert_eq!(map.endpoint_count(), 4);

        let example = &map.hosts[0];
        assert_eq!(example.host, "example.com");
        assert_eq!(example.entries.len(), 3);
        let get_a = example
            .entries
            .iter()
            .find(|entry| entry.path == "/a" && entry.method == "GET")
            .unwrap();
        assert_eq!(get_a.count, 2);
        assert_eq!(get_a.id, 2);
        assert_eq!(
            example
                .entries
                .iter()
                .find(|entry| entry.path == "/a" && entry.method == "POST")
                .unwrap()
                .id,
            3
        );
    }

    #[test]
    fn captures_status_mime_length_title_and_params() {
        let mut history = history(&[("GET", "example.com", "/page?lang=en")]);
        finish(
            &mut history,
            1,
            200,
            "text/html; charset=utf-8",
            b"<html><head><title>  Demo &amp; Shop </title></head></html>",
        );
        let map = SiteMap::build(&history, None);
        let entry = &map.hosts[0].entries[0];
        assert_eq!(entry.status, Some(200));
        assert_eq!(entry.mime, "text/html");
        assert_eq!(entry.title, "Demo & Shop");
        assert_eq!(entry.params, 1);
        assert!(entry.length > 0);
    }

    #[test]
    fn form_body_parameters_are_counted() {
        let mut history = History::new(10, 1024);
        let mut request = request("POST", "example.com", "/login?next=/home");
        request.headers.push((
            "content-type".to_string(),
            "application/x-www-form-urlencoded".to_string(),
        ));
        history.apply(ProxyEvent::Started {
            id: 1,
            timestamp: SystemTime::UNIX_EPOCH,
            request,
        });
        history.apply(ProxyEvent::Finished {
            id: 1,
            end: ExchangeEnd {
                duration: Duration::from_millis(1),
                request_body: CapturedBody::from_bytes(b"user=alice&pass=secret".to_vec()),
                result: ExchangeResult::Failed {
                    error: "offline".to_string(),
                },
            },
        });
        let map = SiteMap::build(&history, None);
        assert_eq!(map.hosts[0].entries[0].params, 3);
    }

    #[test]
    fn tree_keeps_query_variants_under_one_leaf() {
        let history = history(&[
            ("GET", "example.com", "/products?sort=price"),
            ("GET", "example.com", "/products?sort=name"),
            ("POST", "example.com", "/products"),
            ("GET", "example.com", "/assets/site.css"),
        ]);
        let map = SiteMap::build(&history, None);
        let tree = map.tree(SiteMapFilter::SHOW_ALL);
        assert_eq!(tree.len(), 1);
        let root = &tree[0].children[0];
        assert_eq!(root.label, "/");
        let leaf = root.find(&SiteNodeId::Endpoint {
            host: "example.com".to_string(),
            path: "/products".to_string(),
        });
        let leaf = leaf.expect("products leaf");
        assert_eq!(leaf.methods, vec!["GET".to_string(), "POST".to_string()]);
        let assets = root
            .children
            .iter()
            .find(|node| node.label == "assets")
            .unwrap();
        assert!(assets.is_directory());
        assert_eq!(assets.children[0].label, "site.css");
        assert_eq!(
            root.children
                .iter()
                .map(|node| node.label.as_str())
                .collect::<Vec<_>>(),
            vec!["assets", "products"]
        );
    }

    #[test]
    fn tree_hides_filtered_mime_and_not_found_items() {
        let mut history = history(&[
            ("GET", "example.com", "/page"),
            ("GET", "example.com", "/site.css"),
            ("GET", "example.com", "/missing"),
        ]);
        finish(&mut history, 1, 200, "text/html", b"<title>Page</title>");
        finish(&mut history, 2, 200, "text/css", b"body{}");
        finish(&mut history, 3, 404, "text/html", b"not found");
        let map = SiteMap::build(&history, None);

        let filtered = map.tree(SiteMapFilter::default());
        let labels: Vec<&str> = filtered[0].children[0]
            .children
            .iter()
            .map(|node| node.label.as_str())
            .collect();
        assert_eq!(labels, vec!["page"]);

        let all = map.tree(SiteMapFilter::SHOW_ALL);
        assert_eq!(all[0].children[0].children.len(), 3);
    }

    #[test]
    fn hide_empty_folders_controls_filtered_directories() {
        let mut history = history(&[("GET", "example.com", "/assets/site.css")]);
        finish(&mut history, 1, 200, "text/css", b"body{}");
        let map = SiteMap::build(&history, None);

        let hidden = map.tree(SiteMapFilter::default());
        assert!(hidden.is_empty());

        let keep_empty = SiteMapFilter {
            hide_empty_folders: false,
            ..SiteMapFilter::default()
        };
        let kept = map.tree(keep_empty);
        assert_eq!(kept[0].children[0].label, "/");
        let assets = &kept[0].children[0].children[0];
        assert_eq!(assets.label, "assets");
        assert!(assets.children.is_empty());
    }

    #[test]
    fn node_matching_handles_host_directories_and_endpoints() {
        let history = history(&[
            ("GET", "example.com", "/a"),
            ("GET", "example.com", "/assets/app.js"),
            ("GET", "example.com", "/assets/logo.png"),
            ("GET", "other.test", "/a"),
        ]);
        let map = SiteMap::build(&history, None);
        let host = SiteNodeId::Host {
            host: "example.com".to_string(),
        };
        let assets = SiteNodeId::Directory {
            host: "example.com".to_string(),
            path: "/assets".to_string(),
        };
        let endpoint = SiteNodeId::Endpoint {
            host: "example.com".to_string(),
            path: "/a".to_string(),
        };
        let matches = |node: &SiteNodeId| {
            map.hosts
                .iter()
                .flat_map(|host| &host.entries)
                .filter(|entry| node.contains(entry))
                .map(|entry| entry.path.as_str())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            matches(&host),
            vec!["/a", "/assets/app.js", "/assets/logo.png"]
        );
        assert_eq!(matches(&assets), vec!["/assets/app.js", "/assets/logo.png"]);
        assert_eq!(matches(&endpoint), vec!["/a"]);
    }

    #[test]
    fn filter_classification_and_summary_match_burp_defaults() {
        let filter = SiteMapFilter::default();
        assert_eq!(
            filter.summary(),
            "Hiding not found items; hiding CSS, image and general binary content; hiding 4xx responses; hiding empty folders"
        );
        assert_eq!(SiteMapFilter::SHOW_ALL.summary(), "Showing all items");
        assert!(is_binary_mime("application/octet-stream"));
        assert!(!is_binary_mime("image/png"));
        assert!(!is_binary_mime("application/json"));
        assert_eq!(mime_label("text/plain"), "text");
        assert_eq!(mime_label("text/html"), "HTML");
        assert_eq!(mime_label("application/json"), "JSON");
    }

    #[test]
    fn html_title_handles_case_entities_and_whitespace() {
        assert_eq!(
            html_title(b"<HTML><HEAD><TITLE> A &amp; B\n C </TITLE>"),
            Some("A & B C".to_string())
        );
        assert_eq!(html_title(b"<p>no title</p>"), None);
        assert_eq!(html_title(b"<title> </title>"), None);
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
