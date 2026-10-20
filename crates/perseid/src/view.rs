//! Formatting and rendering helpers for the history and detail views.

use std::time::{Duration, SystemTime};

use egui::Color32;

use crate::decode::{self, DecodeError};
use crate::history::{HistoryItem, Phase};
use crate::theme;
use perseid_proxy::CapturedBody;

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
/// Request/Response tabs and their Raw/Headers/Hex views.
pub struct MessageViews {
    pub raw: String,
    pub head: String,
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

    if item.request_body.total_len() == 0 && !item.request_body.is_complete() {
        out.push_str(if item.is_in_flight() {
            "(request body still streaming…)\n"
        } else {
            "(no request body captured)\n"
        });
    } else {
        out.push_str(&format_raw_body_with_decoded(&item.request_body, decoded));
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
    let headers = display_headers(&item.request.headers, decoded, Some(&item.request_body));
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
        match item.response_body() {
            Some(body) if body.total_len() == 0 && body.is_complete() => {
                out.push_str(empty_response_note(item));
            }
            Some(body) => out.push_str(&format_raw_body_with_decoded(body, decoded)),
            None => match &item.phase {
                Phase::InFlight => out.push_str("(response body streaming…)\n"),
                Phase::Finished { .. } => match item.tls_intercepted_requests() {
                    Some(0) => out.push_str(
                        "[Perseid: TLS intercepted; the client closed before sending a request]\n",
                    ),
                    Some(requests) => out.push_str(&format!(
                        "[Perseid: TLS intercepted; {requests} HTTP request{} decrypted into separate history entries]\n",
                        if requests == 1 { "" } else { "s" },
                    )),
                    None => match item.tunnel_stats() {
                        Some((up, down)) => out.push_str(&format!(
                            "[Perseid: TLS passthrough tunnel; relayed {} to server, {} to client; payload not decrypted]\n",
                            human_bytes(up),
                            human_bytes(down),
                        )),
                        None => out.push_str("(no response body captured)\n"),
                    },
                },
            },
        }
    }
    if let Some(error) = item.error() {
        out.push_str(&format!("\n[Perseid: exchange failed: {error}]\n"));
    }
    out
}

fn empty_response_note(item: &HistoryItem) -> &'static str {
    match item.status() {
        Some(304) => "(no response body: 304 Not Modified; the client reuses its cached copy)\n",
        Some(204) => "(no response body: 204 No Content)\n",
        _ if item.request.method.eq_ignore_ascii_case("HEAD") => "(no response body for HEAD)\n",
        _ => "(empty body)\n",
    }
}

fn format_response_head_with(item: &HistoryItem, decoded: &DecodeResult) -> String {
    let mut out = String::new();
    match &item.response {
        Some(head) => {
            out.push_str(&format!(
                "{} {} {}\n",
                head.version, head.status, head.reason
            ));
            let headers = display_headers(&head.headers, decoded, item.response_body());
            push_headers(&mut out, &headers);
        }
        None => match &item.phase {
            Phase::InFlight => out.push_str("(awaiting response…)\n"),
            Phase::Finished { .. } => out.push_str("(no response head captured)\n"),
        },
    }
    out
}

pub fn request_views(item: &HistoryItem) -> MessageViews {
    let decoded = decode_request(item);
    let head = format_request_head_with(item, &decoded);
    let raw = format_request_with(item, &decoded);
    let mut bytes = head.as_bytes().to_vec();
    bytes.push(b'\n');
    bytes.extend_from_slice(display_body_bytes(&item.request_body, &decoded));
    let mut hex = hex_preview(&bytes);
    append_body_notes(&mut hex, &item.request_body, item.is_in_flight());
    append_decoded_hex_notes(&mut hex, &item.request_body, &decoded);
    MessageViews { raw, head, hex }
}

pub fn response_views(item: &HistoryItem) -> MessageViews {
    let decoded = decode_response(item);
    let head = format_response_head_with(item, &decoded);
    let raw = format_response_with(item, &decoded);
    let body = item.response_body();
    let mut hex = if item.response.is_some() {
        let mut bytes = head.as_bytes().to_vec();
        bytes.push(b'\n');
        if let Some(body) = body {
            bytes.extend_from_slice(display_body_bytes(body, &decoded));
        }
        hex_preview(&bytes)
    } else {
        raw.clone()
    };
    if let Some(body) = body {
        append_body_notes(&mut hex, body, item.is_in_flight());
        append_decoded_hex_notes(&mut hex, body, &decoded);
    }
    if let Some(requests) = item.tls_intercepted_requests() {
        hex.push_str(&format!(
            "\n[Perseid: TLS intercepted; {requests} HTTP request{} decrypted into separate history entries]\n",
            if requests == 1 { "" } else { "s" },
        ));
    } else if let Some((up, down)) = item.tunnel_stats() {
        hex.push_str(&format!(
            "\n[Perseid: TLS passthrough tunnel; {up} bytes to server, {down} bytes to client; payload not decrypted]\n"
        ));
    }
    MessageViews { raw, head, hex }
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
    let shown = &bytes[..bytes.len().min(HEX_PREVIEW_LIMIT)];
    let mut out = hexdump(shown);
    if shown.len() < bytes.len() {
        out.push_str(&format!(
            "\n[Perseid: hex preview truncated to {} of {}]\n",
            human_bytes(shown.len() as u64),
            human_bytes(bytes.len() as u64)
        ));
    }
    out
}

fn append_body_notes(out: &mut String, body: &CapturedBody, in_flight: bool) {
    if body.total_len() == 0 && !body.is_complete() {
        out.push_str(if in_flight {
            "\n[Perseid: body still streaming]\n"
        } else {
            "\n[Perseid: no body captured]\n"
        });
    }
    if body.is_truncated() {
        out.push_str(&format!(
            "\n[Perseid: capture limit reached; {} on the wire]\n",
            human_bytes(body.total_len())
        ));
    }
}

fn push_headers(out: &mut String, headers: &[(String, String)]) {
    for (name, value) in headers {
        out.push_str(name);
        out.push_str(": ");
        out.push_str(value);
        out.push('\n');
    }
}

/// Render a raw body using the same decoded representation as the other views.
///
/// Raw never switches to a hex dump, even for binary bodies: bytes are decoded
/// lossily, control bytes are shown as visible markers, and a note points at the
/// Hex view for the byte-offset representation. Requests and responses share
/// this path so both stay consistent.
fn format_raw_body_with_decoded(body: &CapturedBody, decoded: &DecodeResult) -> String {
    if body.total_len() == 0 {
        return if body.is_complete() {
            "(empty body)\n".to_string()
        } else {
            "(body not read)\n".to_string()
        };
    }

    match decoded {
        Ok(Some(decoded)) => format_decoded_raw_body(body, decoded),
        Ok(None) => format_wire_raw_body(body),
        Err(DecodeError::Unsupported(name)) => {
            let mut out = format_wire_raw_body(body);
            out.push_str(&format!(
                "\n[Perseid: content-encoding `{name}` is not decoded; showing wire bytes]\n"
            ));
            out
        }
        Err(DecodeError::Invalid { encoding, .. }) => {
            let mut out = format_wire_raw_body(body);
            out.push_str(&format!(
                "\n[Perseid: could not decode content-encoding `{encoding}`; showing wire bytes]\n"
            ));
            out
        }
    }
}

fn format_decoded_raw_body(body: &CapturedBody, decoded: &decode::DecodedBody) -> String {
    let mut out = visible_raw_text(&decoded.bytes);
    out.push('\n');
    if looks_binary(&decoded.bytes) {
        out.push_str(
            "[Perseid: decoded body shown as lossy text; use Hex for a byte-offset view]\n",
        );
    }
    if decoded.truncated {
        out.push_str(&format!(
            "[Perseid: decoded preview truncated to {}]\n",
            human_bytes(TEXT_PREVIEW_LIMIT as u64)
        ));
    }
    if decoded.incomplete {
        out.push_str(
            "[Perseid: compressed body ended before the decoder finished; preview is partial]\n",
        );
    }
    if body.is_truncated() {
        out.push_str(&format!(
            "[Perseid: capture limit reached; {} on the wire]\n",
            human_bytes(body.total_len())
        ));
    }
    if !body.is_complete() {
        out.push_str("[Perseid: body did not finish]\n");
    }
    out.push_str(&format!(
        "[Perseid: decoded from {} for display; {} on the wire]\n",
        decoded.encoding,
        human_bytes(body.total_len())
    ));
    out
}

fn append_decoded_hex_notes(out: &mut String, body: &CapturedBody, decoded: &DecodeResult) {
    match decoded {
        Ok(Some(decoded)) => {
            if decoded.truncated {
                out.push_str(&format!(
                    "\n[Perseid: decoded preview truncated to {}]\n",
                    human_bytes(TEXT_PREVIEW_LIMIT as u64)
                ));
            }
            if decoded.incomplete {
                out.push_str(
                    "\n[Perseid: compressed body ended before the decoder finished; preview is partial]\n",
                );
            }
            out.push_str(&format!(
                "\n[Perseid: body decoded from {} for display; {} on the wire]\n",
                decoded.encoding,
                human_bytes(body.total_len())
            ));
        }
        Ok(None) => {}
        Err(DecodeError::Unsupported(name)) => {
            out.push_str(&format!(
                "\n[Perseid: content-encoding `{name}` is not decoded; showing wire bytes]\n"
            ));
        }
        Err(DecodeError::Invalid { encoding, .. }) => {
            out.push_str(&format!(
                "\n[Perseid: could not decode content-encoding `{encoding}`; showing wire bytes]\n"
            ));
        }
    }
}

fn format_wire_raw_body(body: &CapturedBody) -> String {
    let data = body.data();
    let shown = &data[..data.len().min(TEXT_PREVIEW_LIMIT)];
    let mut out = visible_raw_text(shown);
    out.push('\n');
    if looks_binary(shown) {
        out.push_str(
            "[Perseid: binary body shown as lossy text; use Hex for a byte-offset view]\n",
        );
    }
    push_body_preview_notes(&mut out, body, shown.len(), data.len());
    out
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
        use perseid_proxy::{ExchangeResult, RequestInfo, ResponseHead};

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
                headers: vec![(
                    "content-type".to_string(),
                    "application/octet-stream".to_string(),
                )],
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
                raw.contains("binary body shown as lossy text"),
                "Raw view is missing the binary-body note:\n{raw}"
            );
        }
        assert!(request.raw.starts_with("POST /upload HTTP/1.1\n"));
        assert!(response.raw.starts_with("HTTP/1.1 200 OK\n"));
        assert!(request.hex.contains("00000000  "));
        assert!(response.hex.contains("00000000  "));
    }

    #[test]
    fn raw_views_keep_text_bodies_readable() {
        let item = item_with_bodies(b"name=Perseid".to_vec(), b"hello".to_vec());

        let request = request_views(&item);
        let response = response_views(&item);

        assert!(request.raw.ends_with("name=Perseid\n"));
        assert!(response.raw.ends_with("hello\n"));
        assert!(!request.raw.contains("binary body"));
        assert!(!response.raw.contains("binary body"));
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
        assert!(raw.contains("binary body shown as lossy text"));
    }

    #[test]
    fn all_views_show_brotli_decoded_body() {
        let mut item = item_with_bodies(Vec::new(), BROTLI_HTML.to_vec());
        let headers = &mut item.response.as_mut().unwrap().headers;
        headers.push(("content-encoding".to_string(), "br".to_string()));
        headers.push(("content-length".to_string(), BROTLI_HTML.len().to_string()));

        let views = response_views(&item);

        assert!(
            views.raw.contains("<h1>Hello, Perseid</h1>"),
            "Raw response was not decoded:\n{}",
            views.raw
        );
        assert!(views.raw.contains("decoded from br for display"));
        assert!(!views.raw.contains("Hex shows the"));
        assert!(!views.raw.contains("binary body"));
        assert!(!views.head.to_ascii_lowercase().contains("content-encoding"));
        assert!(
            views
                .head
                .contains(&format!("content-length: {}", HTML.len()))
        );
        assert!(
            contains(&hexdump_bytes(&views.hex), HTML),
            "Hex should show the decoded response body:\n{}",
            views.hex
        );
    }

    #[test]
    fn all_views_show_gzip_decoded_request_body() {
        let mut item = item_with_bodies(GZIP_HTML.to_vec(), b"ok".to_vec());
        item.request
            .headers
            .push(("content-encoding".to_string(), "gzip".to_string()));

        let views = request_views(&item);

        assert!(views.raw.contains("<h1>Hello, Perseid</h1>"));
        assert!(views.raw.contains("decoded from gzip for display"));
        assert!(!views.raw.contains('\0'));
        assert!(!views.head.to_ascii_lowercase().contains("content-encoding"));
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

        assert!(views.raw.contains("is not decoded; showing wire bytes"));
        assert!(views.head.to_ascii_lowercase().contains("content-encoding"));
        assert!(views.hex.contains("1f 8b"));
    }

    #[test]
    fn raw_response_labels_304_as_bodyless() {
        let mut item = item_with_bodies(Vec::new(), Vec::new());
        item.request.method = "GET".to_string();
        let response = item.response.as_mut().unwrap();
        response.status = 304;
        response.reason = "Not Modified".to_string();

        let views = response_views(&item);

        assert!(views.raw.contains("304 Not Modified"));
        assert!(views.raw.contains("cached copy"));
        assert!(!views.raw.contains("(empty body)"));
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
