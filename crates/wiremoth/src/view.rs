//! Formatting and rendering helpers for the history and detail views.

use std::time::{Duration, SystemTime};

use egui::Color32;

use crate::history::{HistoryItem, Phase};
use crate::theme;
use wiremoth_proxy::CapturedBody;

/// Maximum body bytes rendered as text.
pub const TEXT_PREVIEW_LIMIT: usize = 256 * 1024;
/// Maximum body bytes rendered as a hex dump (hex is far bulkier than text).
pub const HEX_PREVIEW_LIMIT: usize = 64 * 1024;

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

/// The request as it was (or will be) sent to the origin server: origin-form
/// target, like the raw view in other interception tools.
pub fn format_request(item: &HistoryItem) -> String {
    let mut out = format_request_head(item);
    out.push('\n');

    if item.request_body.total_len() == 0 && !item.request_body.is_complete() {
        out.push_str(if item.is_in_flight() {
            "(request body still streaming…)\n"
        } else {
            "(no request body captured)\n"
        });
    } else {
        out.push_str(&format_body(&item.request_body));
    }
    out
}

/// Request line and headers only.
pub fn format_request_head(item: &HistoryItem) -> String {
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
    push_headers(&mut out, &item.request.headers);
    out
}

pub fn format_response(item: &HistoryItem) -> String {
    let mut out = format_response_head(item);
    if item.response.is_some() {
        out.push('\n');
        match item.response_body() {
            Some(body) => out.push_str(&format_body(body)),
            None => match &item.phase {
                Phase::InFlight => out.push_str("(response body streaming…)\n"),
                Phase::Finished { .. } => match item.tunnel_stats() {
                    Some((up, down)) => out.push_str(&format!(
                        "[wiremoth: TLS passthrough tunnel; relayed {} to server, {} to client; payload not decrypted]\n",
                        human_bytes(up),
                        human_bytes(down),
                    )),
                    None => out.push_str("(no response body captured)\n"),
                },
            },
        }
    }
    if let Some(error) = item.error() {
        out.push_str(&format!("\n[wiremoth: exchange failed: {error}]\n"));
    }
    out
}

/// Status line and headers only.
pub fn format_response_head(item: &HistoryItem) -> String {
    let mut out = String::new();
    match &item.response {
        Some(head) => {
            out.push_str(&format!(
                "{} {} {}\n",
                head.version, head.status, head.reason
            ));
            push_headers(&mut out, &head.headers);
        }
        None => match &item.phase {
            Phase::InFlight => out.push_str("(awaiting response…)\n"),
            Phase::Finished { .. } => out.push_str("(no response head captured)\n"),
        },
    }
    out
}

pub fn request_views(item: &HistoryItem) -> MessageViews {
    let head = format_request_head(item);
    let raw = format_request(item);
    let mut bytes = head.as_bytes().to_vec();
    bytes.push(b'\n');
    bytes.extend_from_slice(item.request_body.data());
    let mut hex = hex_preview(&bytes);
    append_body_notes(&mut hex, &item.request_body, item.is_in_flight());
    MessageViews { raw, head, hex }
}

pub fn response_views(item: &HistoryItem) -> MessageViews {
    let head = format_response_head(item);
    let raw = format_response(item);
    let mut hex = if item.response.is_some() {
        let mut bytes = head.as_bytes().to_vec();
        bytes.push(b'\n');
        if let Some(body) = item.response_body() {
            bytes.extend_from_slice(body.data());
        }
        hex_preview(&bytes)
    } else {
        raw.clone()
    };
    if let Some(body) = item.response_body() {
        append_body_notes(&mut hex, body, item.is_in_flight());
    }
    if let Some((up, down)) = item.tunnel_stats() {
        hex.push_str(&format!(
            "\n[wiremoth: TLS passthrough tunnel; {up} bytes to server, {down} bytes to client; payload not decrypted]\n"
        ));
    }
    MessageViews { raw, head, hex }
}

fn hex_preview(bytes: &[u8]) -> String {
    let shown = &bytes[..bytes.len().min(HEX_PREVIEW_LIMIT)];
    let mut out = hexdump(shown);
    if shown.len() < bytes.len() {
        out.push_str(&format!(
            "\n[wiremoth: hex preview truncated to {} of {} bytes]\n",
            human_bytes(shown.len() as u64),
            human_bytes(bytes.len() as u64)
        ));
    }
    out
}

fn append_body_notes(out: &mut String, body: &CapturedBody, in_flight: bool) {
    if body.total_len() == 0 && !body.is_complete() {
        out.push_str(if in_flight {
            "\n[wiremoth: body still streaming]\n"
        } else {
            "\n[wiremoth: no body captured]\n"
        });
    }
    if body.is_truncated() {
        out.push_str(&format!(
            "\n[wiremoth: capture limit reached; {} bytes on the wire]\n",
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

/// Render a captured body: text when it looks like text, hex otherwise, with
/// notes for any limit that hides data.
pub fn format_body(body: &CapturedBody) -> String {
    if body.total_len() == 0 {
        return if body.is_complete() {
            "(empty body)\n".to_string()
        } else {
            "(body not read)\n".to_string()
        };
    }

    let data = body.data();
    let binary = looks_binary(data);
    let limit = if binary {
        HEX_PREVIEW_LIMIT
    } else {
        TEXT_PREVIEW_LIMIT
    };
    let shown = &data[..data.len().min(limit)];

    let mut out = if binary {
        hexdump(shown)
    } else {
        let mut text = String::from_utf8_lossy(shown).into_owned();
        text.push('\n');
        text
    };

    if shown.len() < data.len() {
        out.push_str(&format!(
            "\n[wiremoth: preview truncated to {} of {} captured bytes]\n",
            human_bytes(shown.len() as u64),
            human_bytes(data.len() as u64)
        ));
    }
    if body.is_truncated() {
        out.push_str(&format!(
            "\n[wiremoth: capture limit reached; {} bytes on the wire]\n",
            human_bytes(body.total_len())
        ));
    }
    if !body.is_complete() {
        out.push_str("\n[wiremoth: body did not finish]\n");
    }
    out
}

/// Bodies with NUL bytes or invalid UTF-8 (that is not just a truncated final
/// character) are rendered as hex.
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
