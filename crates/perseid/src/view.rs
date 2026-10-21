//! Formatting and rendering helpers for the history and detail views.

use std::time::{Duration, SystemTime};

use egui::Color32;

use crate::decode::{self, DecodeError};
use crate::history::HistoryItem;
use crate::proxy::CapturedBody;
use crate::{pretty, theme};

/// Maximum body bytes rendered as text.
pub const TEXT_PREVIEW_LIMIT: usize = 256 * 1024;
/// Maximum body bytes rendered as a hex dump (hex is far bulkier than text).
pub const HEX_PREVIEW_LIMIT: usize = 64 * 1024;
/// Binary bodies get a shorter inline hex preview in the interception editor;
/// the Hex tab has the larger virtualized dump.
const INTERCEPT_BINARY_PREVIEW_LIMIT: usize = 8 * 1024;

pub fn human_bytes(n: u64) -> String {
    const KIB: f64 = 1024.0;
    let n = n as f64;
    if n < KIB {
        format!("{n:.0} B")
    } else if n < KIB * KIB {
        format!("{:.1} KiB", n / KIB)
    } else if n < KIB * KIB * KIB {
        format!("{:.1} MiB", n / (KIB * KIB))
    } else {
        format!("{:.2} GiB", n / (KIB * KIB * KIB))
    }
}

pub fn format_time(time: SystemTime) -> String {
    let local: chrono::DateTime<chrono::Local> = time.into();
    local.format("%H:%M:%S").to_string()
}

pub fn format_duration(duration: Duration) -> String {
    let millis = duration.as_secs_f64() * 1000.0;
    if millis < 1.0 {
        "<1 ms".to_string()
    } else if millis < 1000.0 {
        format!("{millis:.0} ms")
    } else {
        format!("{:.2} s", duration.as_secs_f64())
    }
}

pub fn status_text(item: &HistoryItem) -> String {
    if let Some(status) = item.status() {
        status.to_string()
    } else if item.is_in_flight() {
        "…".to_string()
    } else {
        "err".to_string()
    }
}

pub fn status_color(item: &HistoryItem) -> Color32 {
    let Some(status) = item.status() else {
        return if item.is_in_flight() {
            theme::GRAY
        } else {
            theme::RED
        };
    };
    match status / 100 {
        2 => theme::GREEN,
        3 => theme::BLUE,
        4 => theme::AMBER,
        5 => theme::RED,
        _ => theme::GRAY,
    }
}

/// Formatted representations of one side of an exchange, for the
/// Request/Response tabs and their Pretty/Raw/Hex views.
pub struct MessageViews {
    pub pretty: String,
    pub raw: String,
    pub hex: String,
}

type DecodeResult = Result<Option<decode::DecodedBody>, DecodeError>;

/// The request as it was (or will be) sent to the origin server: origin-form
/// target, like the raw view in other interception tools.
pub fn format_request(item: &HistoryItem) -> String {
    let decoded = decode_request(item);
    format_request_with(item, &decoded)
}

fn format_request_with(item: &HistoryItem, decoded: &DecodeResult) -> String {
    let mut out = format_request_head_with(item, decoded);
    out.push('\n');
    out.push_str(&format_raw_body_with_decoded(&item.request_body, decoded));
    out
}

fn format_request_pretty_with(item: &HistoryItem, decoded: &DecodeResult) -> String {
    let mut out = format_request_head_with(item, decoded);
    let bytes = display_body_bytes(&item.request_body, decoded);
    if !bytes.is_empty() {
        out.push('\n');
        out.push_str(&format_pretty_body(
            header_value(&item.request.headers, "content-type"),
            bytes,
        ));
    }
    out
}

fn decode_request(item: &HistoryItem) -> DecodeResult {
    decode_for(
        &item.request_body,
        header_value(&item.request.headers, "content-encoding"),
    )
}

fn format_request_head_with(item: &HistoryItem, decoded: &DecodeResult) -> String {
    let mut out = String::new();
    let target = if item.is_tunnel() {
        &item.request.host
    } else {
        &item.request.path
    };
    out.push_str(&format!(
        "{} {} {}\n",
        item.request.method, target, item.request.version
    ));
    let mut headers = display_headers(&item.request.headers, decoded, Some(&item.request_body));
    // HTTP/2 carries the authority as a pseudo-header, not as an ordinary
    // header. Show it as Host so the displayed message matches the synthetic
    // representation used by the interception editor.
    if item.request.version.starts_with("HTTP/2")
        && header_value(&headers, "host").is_none()
        && !item.request.host.is_empty()
    {
        headers.insert(0, ("host".to_string(), item.request.host.clone()));
    }
    push_headers(&mut out, &headers);
    out
}

fn decode_response(item: &HistoryItem) -> DecodeResult {
    let content_encoding = item
        .response
        .as_ref()
        .and_then(|head| header_value(&head.headers, "content-encoding"));
    match item.response_body() {
        Some(body) => decode_for(body, content_encoding),
        None => Ok(None),
    }
}

fn format_response_with(item: &HistoryItem, decoded: &DecodeResult) -> String {
    let mut out = format_response_head_with(item, decoded);
    if item.response.is_some() {
        out.push('\n');
        if let Some(body) = item.response_body() {
            out.push_str(&format_raw_body_with_decoded(body, decoded));
        }
    }
    out
}

fn format_response_pretty_with(item: &HistoryItem, decoded: &DecodeResult) -> String {
    let mut out = format_response_head_with(item, decoded);
    if item.response.is_some() {
        out.push('\n');
        if let Some(body) = item.response_body() {
            let content_type = item
                .response
                .as_ref()
                .and_then(|head| header_value(&head.headers, "content-type"));
            out.push_str(&format_pretty_body(
                content_type,
                display_body_bytes(body, decoded),
            ));
        }
    }
    out
}

fn format_response_head_with(item: &HistoryItem, decoded: &DecodeResult) -> String {
    let mut out = String::new();
    if let Some(head) = &item.response {
        if head.reason.is_empty() {
            out.push_str(&format!("{} {}\n", head.version, head.status));
        } else {
            out.push_str(&format!(
                "{} {} {}\n",
                head.version, head.status, head.reason
            ));
        }
        let headers = display_headers(&head.headers, decoded, item.response_body());
        push_headers(&mut out, &headers);
    }
    out
}

pub fn request_views(item: &HistoryItem) -> MessageViews {
    let decoded = decode_request(item);
    let pretty = format_request_pretty_with(item, &decoded);
    let raw = format_request_with(item, &decoded);
    let mut bytes = format_request_head_with(item, &decoded).into_bytes();
    bytes.push(b'\n');
    bytes.extend_from_slice(display_body_bytes(&item.request_body, &decoded));
    MessageViews {
        pretty,
        raw,
        hex: hex_preview(&bytes),
    }
}

pub fn response_views(item: &HistoryItem) -> MessageViews {
    let decoded = decode_response(item);
    let pretty = format_response_pretty_with(item, &decoded);
    let raw = format_response_with(item, &decoded);
    let mut bytes = format_response_head_with(item, &decoded).into_bytes();
    if item.response.is_some() {
        bytes.push(b'\n');
        if let Some(body) = item.response_body() {
            bytes.extend_from_slice(display_body_bytes(body, &decoded));
        }
    }
    MessageViews {
        pretty,
        raw,
        hex: hex_preview(&bytes),
    }
}

/// Build the standard Pretty/Raw/Hex views for a Repeater response.
///
/// Repeater responses do not become history rows, so they cannot use
/// [`HistoryItem`]. The formatting rules are otherwise identical: supported
/// content encodings are unpacked, unsupported or malformed encodings fall
/// back to the wire bytes, and Hex stays bounded.
pub fn response_views_from_parts(
    version: &str,
    status: u16,
    reason: &str,
    headers: &[(String, String)],
    body: &CapturedBody,
) -> MessageViews {
    let decoded = decode_for(body, header_value(headers, "content-encoding"));
    let content_type = header_value(headers, "content-type");

    let mut head = String::new();
    if reason.is_empty() || version.starts_with("HTTP/2") {
        head.push_str(&format!("{version} {status}\n"));
    } else {
        head.push_str(&format!("{version} {status} {reason}\n"));
    }
    let display = display_headers(headers, &decoded, Some(body));
    push_headers(&mut head, &display);

    let mut raw = head.clone();
    raw.push('\n');
    raw.push_str(&format_raw_body_with_decoded(body, &decoded));

    let mut pretty = head.clone();
    pretty.push('\n');
    pretty.push_str(&format_pretty_body(
        content_type,
        display_body_bytes(body, &decoded),
    ));

    let mut bytes = head.into_bytes();
    bytes.push(b'\n');
    bytes.extend_from_slice(display_body_bytes(body, &decoded));

    MessageViews {
        pretty,
        raw,
        hex: hex_preview(&bytes),
    }
}

fn decode_for(body: &CapturedBody, content_encoding: Option<&str>) -> DecodeResult {
    if body.total_len() == 0 || body.data().is_empty() {
        return Ok(None);
    }
    let Some(encoding) = content_encoding else {
        return Ok(None);
    };
    decode::decode_body(body.data(), encoding, TEXT_PREVIEW_LIMIT)
}

/// Return the bytes represented by the inspection views. Supported content
/// encodings are unpacked for every view, including Hex, matching Burp's
/// message-editor model. Forwarding still uses the original wire body.
fn display_body_bytes<'a>(body: &'a CapturedBody, decoded: &'a DecodeResult) -> &'a [u8] {
    match decoded {
        Ok(Some(decoded)) => &decoded.bytes,
        _ => body.data(),
    }
}

/// Build the displayed headers for a message.
///
/// Burp unpacks compressed bodies before presenting them, rather than keeping a
/// compressed body underneath a `Content-Encoding` header. Perseid follows the
/// same model: a successfully decoded body removes the encoding header. A
/// `Content-Length` is rewritten only when we know the complete decoded length;
/// truncated or streaming captures drop it instead of advertising a length that
/// does not match the preview.
fn display_headers(
    headers: &[(String, String)],
    decoded: &DecodeResult,
    body: Option<&CapturedBody>,
) -> Vec<(String, String)> {
    let Ok(Some(decoded)) = decoded else {
        return headers.to_vec();
    };

    let complete = body.is_some_and(|body| body.is_complete() && !body.is_truncated());
    let exact_len =
        (complete && !decoded.truncated && !decoded.incomplete).then_some(decoded.bytes.len());
    let content_length_name = headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        .map(|(name, _)| name.clone());

    let mut out = Vec::with_capacity(headers.len());
    for (name, value) in headers {
        if name.eq_ignore_ascii_case("content-encoding")
            || name.eq_ignore_ascii_case("content-length")
        {
            continue;
        }
        out.push((name.clone(), value.clone()));
    }
    if let (Some(name), Some(len)) = (content_length_name, exact_len) {
        out.push((name, len.to_string()));
    }
    out
}

fn hex_preview(bytes: &[u8]) -> String {
    hexdump(&bytes[..bytes.len().min(HEX_PREVIEW_LIMIT)])
}

fn push_headers(out: &mut String, headers: &[(String, String)]) {
    for (name, value) in headers {
        out.push_str(&display_header_name(name));
        out.push_str(": ");
        out.push_str(value);
        out.push('\n');
    }
}

/// Header names arrive lowercased from the HTTP stack. Present them in the
/// title-case form used by browsers and Burp's message editor. This only
/// affects the displayed message; forwarding always uses the captured bytes.
fn display_header_name(name: &str) -> String {
    if let Some(canonical) = canonical_header_name(name) {
        return canonical.to_owned();
    }

    let mut out = String::with_capacity(name.len());
    for (index, part) in name.split('-').enumerate() {
        if index > 0 {
            out.push('-');
        }
        let mut chars = part.chars();
        if let Some(first) = chars.next() {
            out.extend(first.to_uppercase());
            out.push_str(&chars.as_str().to_ascii_lowercase());
        }
    }
    out
}

/// A few header names contain acronyms that plain title-casing gets wrong.
fn canonical_header_name(name: &str) -> Option<&'static str> {
    match name.to_ascii_lowercase().as_str() {
        "etag" => Some("ETag"),
        "www-authenticate" => Some("WWW-Authenticate"),
        "content-md5" => Some("Content-MD5"),
        "content-dpr" => Some("Content-DPR"),
        "dnt" => Some("DNT"),
        "dpr" => Some("DPR"),
        "ect" => Some("ECT"),
        "rtt" => Some("RTT"),
        "te" => Some("TE"),
        "accept-ch" => Some("Accept-CH"),
        "critical-ch" => Some("Critical-CH"),
        "cdn-loop" => Some("CDN-Loop"),
        "cf-connecting-ip" => Some("CF-Connecting-IP"),
        "cf-ray" => Some("CF-Ray"),
        "x-client-ip" => Some("X-Client-IP"),
        "x-http-method-override" => Some("X-HTTP-Method-Override"),
        "x-real-ip" => Some("X-Real-IP"),
        "x-xss-protection" => Some("X-XSS-Protection"),
        "x-ua-compatible" => Some("X-UA-Compatible"),
        "x-dns-prefetch-control" => Some("X-DNS-Prefetch-Control"),
        "sec-ch-ua" => Some("Sec-CH-UA"),
        "sec-ch-ua-mobile" => Some("Sec-CH-UA-Mobile"),
        "sec-ch-ua-platform" => Some("Sec-CH-UA-Platform"),
        "sec-websocket-key" => Some("Sec-WebSocket-Key"),
        "sec-websocket-version" => Some("Sec-WebSocket-Version"),
        "sec-websocket-extensions" => Some("Sec-WebSocket-Extensions"),
        "sec-websocket-protocol" => Some("Sec-WebSocket-Protocol"),
        "sec-websocket-accept" => Some("Sec-WebSocket-Accept"),
        _ => None,
    }
}

/// Render a raw body using the same decoded representation as the other views.
///
/// Raw never switches to a hex dump, even for binary bodies: bytes are decoded
/// lossily and control bytes are shown as visible markers.
fn format_raw_body_with_decoded(body: &CapturedBody, decoded: &DecodeResult) -> String {
    let bytes = display_body_bytes(body, decoded);
    let shown = &bytes[..bytes.len().min(TEXT_PREVIEW_LIMIT)];
    normalize_body_text(visible_raw_text(shown))
}

/// Format a body for the Pretty view. Structured text is laid out by
/// [`pretty`]; everything else keeps the plain raw representation.
fn format_pretty_body(content_type: Option<&str>, bytes: &[u8]) -> String {
    let shown = &bytes[..bytes.len().min(TEXT_PREVIEW_LIMIT)];
    if shown.is_empty() {
        return String::new();
    }
    let text = if looks_binary(shown) {
        visible_raw_text(shown)
    } else {
        pretty::format_body(content_type, shown).unwrap_or_else(|| visible_raw_text(shown))
    };
    normalize_body_text(text)
}

fn normalize_body_text(mut text: String) -> String {
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text
}

fn header_value<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(header, _)| header.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}

/// Render a bounded, read-only hex preview for the interception editor.
pub fn format_binary_preview(body: &CapturedBody) -> String {
    if body.total_len() == 0 {
        return if body.is_complete() {
            "(empty body)\n".to_string()
        } else {
            "(body not read)\n".to_string()
        };
    }

    let data = body.data();
    let shown = &data[..data.len().min(INTERCEPT_BINARY_PREVIEW_LIMIT)];
    let mut out = hexdump(shown);
    push_body_preview_notes(&mut out, body, shown.len(), data.len());
    out
}

/// Decode bytes for the Raw view without letting control bytes erase the
/// visible body. Egui has no glyphs for characters such as NUL, so binary
/// responses could otherwise render as a blank panel even though the captured
/// body was complete.
fn visible_raw_text(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        if matches!(ch, '\n' | '\r' | '\t') {
            out.push(ch);
        } else if ch.is_control() {
            out.push('·');
        } else {
            out.push(ch);
        }
    }
    out
}

fn push_body_preview_notes(out: &mut String, body: &CapturedBody, shown: usize, captured: usize) {
    if shown < captured {
        out.push_str(&format!(
            "\n[Perseid: preview truncated to {} of {} captured]\n",
            human_bytes(shown as u64),
            human_bytes(captured as u64)
        ));
    }
    if body.is_truncated() {
        out.push_str(&format!(
            "\n[Perseid: capture limit reached; {} on the wire]\n",
            human_bytes(body.total_len())
        ));
    }
    if !body.is_complete() {
        out.push_str("\n[Perseid: body did not finish]\n");
    }
}

/// Bodies with NUL bytes or invalid UTF-8 (that is not just a truncated final
/// character) are treated as binary by the Raw view and interception editor.
fn looks_binary(data: &[u8]) -> bool {
    let sample = &data[..data.len().min(4096)];
    if sample.contains(&0) {
        return true;
    }
    match std::str::from_utf8(sample) {
        Ok(_) => false,
        Err(err) => err.error_len().is_some(),
    }
}

fn hexdump(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len() * 4);
    for (row, chunk) in data.chunks(16).enumerate() {
        out.push_str(&format!("{:08x}  ", row * 16));
        for i in 0..16 {
            match chunk.get(i) {
                Some(byte) => out.push_str(&format!("{byte:02x} ")),
                None => out.push_str("   "),
            }
            if i == 7 {
                out.push(' ');
            }
        }
        out.push_str(" |");
        for byte in chunk {
            out.push(if byte.is_ascii() && !byte.is_ascii_control() {
                *byte as char
            } else {
                '.'
            });
        }
        out.push_str("|\n");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::Phase;

    const GZIP_HTML: &[u8] = &[
        31, 139, 8, 0, 0, 0, 0, 0, 2, 255, 179, 81, 116, 241, 119, 14, 137, 12, 112, 85, 200, 40,
        201, 205, 177, 179, 129, 144, 73, 249, 41, 149, 64, 182, 161, 157, 71, 106, 78, 78, 190,
        142, 66, 64, 106, 81, 113, 106, 102, 138, 141, 62, 80, 200, 70, 31, 34, 171, 15, 86, 10, 0,
        188, 47, 138, 112, 64, 0, 0, 0,
    ];
    const BROTLI_HTML: &[u8] = &[
        27, 63, 0, 224, 29, 7, 110, 76, 205, 102, 86, 83, 39, 94, 251, 64, 149, 205, 233, 24, 30,
        52, 57, 69, 150, 6, 27, 112, 2, 146, 88, 128, 129, 125, 151, 82, 45, 157, 203, 66, 60, 26,
        60, 174, 214, 190, 10, 27, 230,
    ];
    const HTML: &[u8] = b"<!DOCTYPE html><html><body><h1>Hello, Perseid</h1></body></html>";

    /// Pull the byte column back out of a rendered hex dump, ignoring offsets,
    /// ASCII columns, and trailing Perseid notes.
    fn hexdump_bytes(dump: &str) -> Vec<u8> {
        let mut bytes = Vec::new();
        for line in dump.lines() {
            let Some((offset, rest)) = line.split_once("  ") else {
                continue;
            };
            if offset.len() != 8 || !offset.bytes().all(|b| b.is_ascii_hexdigit()) {
                continue;
            }
            let Some((hex, _)) = rest.split_once(" |") else {
                continue;
            };
            for byte in hex.split_whitespace() {
                if byte.len() == 2 && byte.bytes().all(|b| b.is_ascii_hexdigit()) {
                    bytes.push(u8::from_str_radix(byte, 16).expect("valid hex pair"));
                }
            }
        }
        bytes
    }

    fn contains(haystack: &[u8], needle: &[u8]) -> bool {
        haystack
            .windows(needle.len())
            .any(|window| window == needle)
    }

    fn item_with_bodies(request: Vec<u8>, response: Vec<u8>) -> HistoryItem {
        use crate::proxy::{ExchangeResult, RequestInfo, ResponseHead};

        HistoryItem {
            id: 1,
            number: 1,
            timestamp: SystemTime::UNIX_EPOCH,
            request: RequestInfo {
                method: "POST".to_string(),
                uri: "http://example.com/upload".to_string(),
                scheme: "http".to_string(),
                host: "example.com".to_string(),
                path: "/upload".to_string(),
                version: "HTTP/1.1".to_string(),
                headers: vec![
                    ("host".to_string(), "example.com".to_string()),
                    (
                        "content-type".to_string(),
                        "application/octet-stream".to_string(),
                    ),
                    ("x-custom-header".to_string(), "demo".to_string()),
                ],
            },
            request_body: CapturedBody::from_bytes(request),
            response: Some(ResponseHead {
                version: "HTTP/1.1".to_string(),
                status: 200,
                reason: "OK".to_string(),
                headers: vec![("content-type".to_string(), "image/png".to_string())],
            }),
            phase: Phase::Finished {
                duration: Duration::from_millis(1),
                result: ExchangeResult::Response {
                    body: CapturedBody::from_bytes(response),
                },
            },
        }
    }

    #[test]
    fn raw_views_never_render_binary_bodies_as_hex() {
        let item = item_with_bodies(
            b"\x89PNG\r\n\x1a\n\x00\xffrequest".to_vec(),
            b"\x00\x01\x02\xff\xfePNG".to_vec(),
        );

        let request = request_views(&item);
        let response = response_views(&item);

        for raw in [&request.raw, &response.raw] {
            assert!(
                !raw.contains("00000000  "),
                "Raw view contains a hex dump:\n{raw}"
            );
            assert!(
                !raw.contains('\0'),
                "Raw view contains a NUL that egui cannot render:\n{raw:?}"
            );
            assert!(
                raw.contains('·'),
                "Raw view masks no control bytes:\n{raw:?}"
            );
            assert!(
                !raw.contains("[Perseid"),
                "Raw view has an annotation:\n{raw}"
            );
        }
        assert!(request.raw.starts_with("POST /upload HTTP/1.1\n"));
        assert!(response.raw.starts_with("HTTP/1.1 200 OK\n"));
        assert!(request.hex.contains("00000000  "));
        assert!(response.hex.contains("00000000  "));
    }

    #[test]
    fn http2_views_show_authority_and_omit_the_reason_phrase() {
        let mut item = item_with_bodies(Vec::new(), b"ok".to_vec());
        item.request.version = "HTTP/2".to_string();
        item.request.headers.retain(|(name, _)| name != "host");
        let response = item.response.as_mut().unwrap();
        response.version = "HTTP/2".to_string();
        response.reason.clear();

        let request = request_views(&item);
        let response = response_views(&item);

        assert!(
            request
                .raw
                .starts_with("POST /upload HTTP/2\nHost: example.com\n"),
            "unexpected HTTP/2 request view:\n{}",
            request.raw
        );
        assert!(
            response.raw.starts_with("HTTP/2 200\n"),
            "unexpected HTTP/2 response view:\n{}",
            response.raw
        );
    }

    #[test]
    fn header_names_use_burp_title_case() {
        let item = item_with_bodies(Vec::new(), Vec::new());
        let request = request_views(&item);
        let response = response_views(&item);

        assert!(request.raw.contains("\nHost: example.com\n"));
        assert!(
            request
                .raw
                .contains("\nContent-Type: application/octet-stream\n")
        );
        assert!(request.raw.contains("\nX-Custom-Header: demo\n"));
        assert!(request.pretty.contains("\nHost: example.com\n"));
        assert!(response.raw.contains("\nContent-Type: image/png\n"));
        assert!(response.pretty.contains("\nContent-Type: image/png\n"));
    }

    #[test]
    fn acronym_headers_keep_their_conventional_casing() {
        assert_eq!(display_header_name("etag"), "ETag");
        assert_eq!(display_header_name("www-authenticate"), "WWW-Authenticate");
        assert_eq!(display_header_name("content-md5"), "Content-MD5");
        assert_eq!(display_header_name("content-dpr"), "Content-DPR");
        assert_eq!(display_header_name("dnt"), "DNT");
        assert_eq!(display_header_name("dpr"), "DPR");
        assert_eq!(display_header_name("ect"), "ECT");
        assert_eq!(display_header_name("rtt"), "RTT");
        assert_eq!(display_header_name("te"), "TE");
        assert_eq!(display_header_name("accept-ch"), "Accept-CH");
        assert_eq!(display_header_name("critical-ch"), "Critical-CH");
        assert_eq!(display_header_name("cdn-loop"), "CDN-Loop");
        assert_eq!(display_header_name("cf-ray"), "CF-Ray");
        assert_eq!(display_header_name("x-real-ip"), "X-Real-IP");
        assert_eq!(
            display_header_name("x-http-method-override"),
            "X-HTTP-Method-Override"
        );
        assert_eq!(display_header_name("x-xss-protection"), "X-XSS-Protection");
        assert_eq!(display_header_name("x-ua-compatible"), "X-UA-Compatible");
        assert_eq!(
            display_header_name("x-dns-prefetch-control"),
            "X-DNS-Prefetch-Control"
        );
        assert_eq!(display_header_name("sec-ch-ua-mobile"), "Sec-CH-UA-Mobile");
        assert_eq!(
            display_header_name("sec-websocket-key"),
            "Sec-WebSocket-Key"
        );
        assert_eq!(display_header_name("x-custom-header"), "X-Custom-Header");
    }

    #[test]
    fn raw_views_keep_text_bodies_readable() {
        let item = item_with_bodies(b"name=Perseid".to_vec(), b"hello".to_vec());

        let request = request_views(&item);
        let response = response_views(&item);

        assert!(request.raw.ends_with("name=Perseid\n"));
        assert!(response.raw.ends_with("hello\n"));
        assert!(!request.raw.contains("[Perseid"));
        assert!(!response.raw.contains("[Perseid"));
    }

    #[test]
    fn raw_binary_body_stays_visible() {
        // A NUL-heavy binary body used to render as a blank Raw panel because
        // egui has no glyphs for those characters.
        let body = CapturedBody::from_bytes(vec![
            0x1f, 0x8b, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0xff, 0xfe, b'h', b'i',
        ]);
        let raw = format_raw_body_with_decoded(&body, &Ok(None));

        assert!(!raw.contains('\0'));
        assert!(raw.contains('·'));
        assert!(raw.contains("hi"));
        assert!(!raw.contains("[Perseid"));
    }

    #[test]
    fn all_views_show_brotli_decoded_body() {
        let mut item = item_with_bodies(Vec::new(), BROTLI_HTML.to_vec());
        let headers = &mut item.response.as_mut().unwrap().headers;
        headers.push(("content-encoding".to_string(), "br".to_string()));
        headers.push(("content-length".to_string(), BROTLI_HTML.len().to_string()));
        headers.push(("content-type".to_string(), "text/html".to_string()));

        let views = response_views(&item);

        assert!(
            views.raw.contains("<h1>Hello, Perseid</h1>"),
            "Raw response was not decoded:\n{}",
            views.raw
        );
        assert!(!views.raw.contains("[Perseid"));
        assert!(!views.raw.to_ascii_lowercase().contains("content-encoding"));
        assert!(
            views
                .raw
                .contains(&format!("Content-Length: {}", HTML.len()))
        );
        assert!(
            views.pretty.contains("<h1>") && views.pretty.contains("Hello, Perseid"),
            "Pretty response was not formatted:\n{}",
            views.pretty
        );
        assert!(!views.pretty.contains("[Perseid"));
        assert!(
            contains(&hexdump_bytes(&views.hex), HTML),
            "Hex should show the decoded response body:\n{}",
            views.hex
        );
        assert!(!views.hex.contains("[Perseid"));
    }

    #[test]
    fn repeater_response_views_decode_compressed_bodies() {
        let body = CapturedBody::from_bytes(GZIP_HTML.to_vec());
        let headers = vec![
            ("content-encoding".to_string(), "gzip".to_string()),
            ("content-length".to_string(), GZIP_HTML.len().to_string()),
            ("content-type".to_string(), "text/html".to_string()),
        ];

        let views = response_views_from_parts("HTTP/1.1", 200, "OK", &headers, &body);

        assert!(views.raw.starts_with("HTTP/1.1 200 OK\n"));
        assert!(views.raw.contains("<h1>Hello, Perseid</h1>"));
        assert!(!views.raw.to_ascii_lowercase().contains("content-encoding"));
        assert!(
            views
                .raw
                .contains(&format!("Content-Length: {}", HTML.len()))
        );
        assert!(views.pretty.contains("<h1>"));
        assert!(contains(&hexdump_bytes(&views.hex), HTML));

        let h2 = response_views_from_parts("HTTP/2", 200, "OK", &[], &CapturedBody::default());
        assert!(h2.raw.starts_with("HTTP/2 200\n"));
    }

    #[test]
    fn all_views_show_gzip_decoded_request_body() {
        let mut item = item_with_bodies(GZIP_HTML.to_vec(), b"ok".to_vec());
        item.request
            .headers
            .push(("content-encoding".to_string(), "gzip".to_string()));

        let views = request_views(&item);

        assert!(views.raw.contains("<h1>Hello, Perseid</h1>"));
        assert!(!views.raw.contains("[Perseid"));
        assert!(!views.raw.contains('\0'));
        assert!(!views.raw.to_ascii_lowercase().contains("content-encoding"));
        assert!(
            contains(&hexdump_bytes(&views.hex), HTML),
            "Hex should show the decoded request body:\n{}",
            views.hex
        );
        assert!(!views.hex.contains("1f 8b"));
    }

    #[test]
    fn unsupported_encoding_falls_back_to_wire_bytes() {
        let mut item = item_with_bodies(GZIP_HTML.to_vec(), b"ok".to_vec());
        item.request
            .headers
            .push(("content-encoding".to_string(), "zstd".to_string()));

        let views = request_views(&item);

        assert!(!views.raw.contains("[Perseid"));
        assert!(views.raw.to_ascii_lowercase().contains("content-encoding"));
        assert!(views.hex.contains("1f 8b"));
    }

    #[test]
    fn empty_bodies_have_no_annotations() {
        let item = item_with_bodies(Vec::new(), Vec::new());
        let request = request_views(&item);
        let response = response_views(&item);

        for view in [
            &request.pretty,
            &request.raw,
            &request.hex,
            &response.pretty,
            &response.raw,
            &response.hex,
        ] {
            assert!(!view.contains("[Perseid"));
            assert!(!view.contains("empty body"));
            assert!(!view.contains("no body"));
        }
    }

    #[test]
    fn pretty_view_formats_json_response_body() {
        let mut item = item_with_bodies(Vec::new(), br#"{"items":[{"id":1},{"id":2}]}"#.to_vec());
        let headers = &mut item.response.as_mut().unwrap().headers;
        headers.retain(|(name, _)| !name.eq_ignore_ascii_case("content-type"));
        headers.push(("content-type".to_string(), "application/json".to_string()));

        let views = response_views(&item);

        assert!(
            views
                .pretty
                .contains("{\n  \"items\": [\n    {\n      \"id\": 1\n    },"),
            "JSON response was not pretty-printed:\n{}",
            views.pretty
        );
        assert!(views.raw.contains(r#"{"items":[{"id":1},{"id":2}]}"#));
    }

    #[test]
    fn response_view_has_no_body_annotations() {
        let mut item = item_with_bodies(Vec::new(), Vec::new());
        item.request.method = "GET".to_string();
        let response = item.response.as_mut().unwrap();
        response.status = 304;
        response.reason = "Not Modified".to_string();

        let views = response_views(&item);

        assert!(views.raw.contains("304 Not Modified"));
        assert!(!views.raw.contains("cached copy"));
        assert!(!views.raw.contains("(empty body)"));
        assert!(!views.raw.contains("[Perseid"));
    }

    #[test]
    fn intercept_binary_preview_remains_hex() {
        let preview = format_binary_preview(&CapturedBody::from_bytes(vec![
            0x89, b'P', b'N', b'G', 0, 1,
        ]));
        assert!(preview.starts_with("00000000  89 50 4e 47 00 01"));
    }

    #[test]
    fn human_bytes_scales_units() {
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(1023), "1023 B");
        assert_eq!(human_bytes(2048), "2.0 KiB");
        assert_eq!(human_bytes(5 * 1024 * 1024), "5.0 MiB");
    }

    #[test]
    fn hexdump_renders_offset_hex_and_ascii() {
        let dump = hexdump(b"Hello, world!");
        assert!(dump.starts_with("00000000  48 65 6c 6c 6f 2c 20 77  6f 72 6c 64 21"));
        assert!(dump.trim_end().ends_with("|Hello, world!|"));
    }

    #[test]
    fn binary_detection() {
        assert!(!looks_binary(b"plain text"));
        assert!(looks_binary(&[0x89, 0x50, 0x4e, 0x47, 0x00, 0x0d])); // PNG-ish
        assert!(!looks_binary("héllo".as_bytes()));
        // A multibyte char cut at the sample boundary is not "binary".
        let mut data = vec![b'a'; 4095];
        data.extend_from_slice("é".as_bytes());
        assert!(!looks_binary(&data));
    }
}
