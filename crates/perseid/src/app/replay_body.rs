//! Captured replay bytes are independent of every display representation.

use crate::project::BodyWorkspace;
use crate::proxy::split_raw_request;

pub(super) fn bytes<'a>(body: &'a Option<BodyWorkspace>, raw: &'a str) -> Result<&'a [u8], String> {
    match body {
        Some(body) if body.truncated || !body.complete => {
            Err("Acknowledge the incomplete captured body or replace it before sending".into())
        }
        Some(body) => {
            if !split_raw_request(raw).1.is_empty() {
                return Err("Replace the preserved body before entering body text".into());
            }
            Ok(&body.data)
        }
        None => Ok(split_raw_request(raw).1.as_bytes()),
    }
}

pub(super) fn controls(ui: &mut egui::Ui, body: &mut Option<BodyWorkspace>, raw: &mut String) {
    let Some(captured) = body else { return };
    ui.label(format!(
        "Preserved captured body: {} bytes (not shown in editor)",
        captured.data.len()
    ));
    if captured.truncated || !captured.complete {
        ui.colored_label(
            crate::theme::AMBER,
            "Incomplete capture: sending is blocked.",
        );
        if ui
            .button("Use captured prefix (acknowledge missing bytes)")
            .clicked()
        {
            captured.truncated = false;
            captured.complete = true;
        }
    }
    if ui
        .button("Replace captured body with empty editable text")
        .clicked()
    {
        // This is a replacement, not an implicit lossy/decompressed conversion.
        let (head, _) = split_raw_request(raw);
        let newline = if head.contains("\r\n") { "\r\n" } else { "\n" };
        *raw = head
            .trim_end_matches(['\r', '\n'])
            .lines()
            .filter(|line| {
                !line.split_once(':').is_some_and(|(name, _)| {
                    name.eq_ignore_ascii_case("content-encoding")
                        || name.eq_ignore_ascii_case("content-length")
                })
            })
            .collect::<Vec<_>>()
            .join(newline);
        raw.push_str(newline);
        raw.push_str(newline);
        *body = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proxy::CapturedBody;

    fn click(body: &mut Option<BodyWorkspace>, raw: &mut String, label: &str) {
        let ctx = egui::Context::default();
        let input = |events| egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1200.0, 600.0),
            )),
            events,
            ..Default::default()
        };
        let output = ctx.run_ui(input(vec![]), |ui| controls(ui, body, raw));
        let pos = output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.job.text == label => {
                    Some(text.pos + egui::vec2(3.0, 3.0))
                }
                _ => None,
            })
            .expect("button visible");
        output.drop_without_applying_deltas();
        for pressed in [true, false] {
            ctx.run_ui(
                input(vec![
                    egui::Event::PointerMoved(pos),
                    egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ]),
                |ui| controls(ui, body, raw),
            )
            .drop_without_applying_deltas();
        }
    }

    #[test]
    fn acknowledgement_and_replacement_are_explicit_and_persisted() {
        let mut body = Some(BodyWorkspace {
            data: vec![0, 255],
            total_len: 20,
            truncated: true,
            complete: false,
        });
        let mut raw = "POST / HTTP/1.1\r\nContent-Encoding: gzip\r\nContent-Length: 20\r\nContent-Type: application/octet-stream\r\n\r\n".to_string();
        assert!(bytes(&body, &raw).is_err());
        click(
            &mut body,
            &mut raw,
            "Use captured prefix (acknowledge missing bytes)",
        );
        let saved = serde_json::to_vec(&body).unwrap();
        body = serde_json::from_slice(&saved).unwrap();
        assert_eq!(bytes(&body, &raw).unwrap(), vec![0, 255]);
        assert!(raw.contains("Content-Encoding: gzip"));
        click(
            &mut body,
            &mut raw,
            "Replace captured body with empty editable text",
        );
        assert!(body.is_none());
        assert!(!raw.contains("Content-Encoding"));
        assert!(!raw.contains("Content-Length"));
        assert!(raw.contains("Content-Type: application/octet-stream"));
        assert!(bytes(&body, &raw).unwrap().is_empty());
        raw.push_str("a=1\r\n");
        assert_eq!(bytes(&body, &raw).unwrap(), b"a=1\r\n");
    }

    #[test]
    fn replacement_preserves_head_line_endings_and_entire_multipart_body() {
        for newline in ["\r\n", "\n"] {
            let mut body = Some(BodyWorkspace::from_body(&CapturedBody::from_bytes(vec![
                255,
            ])));
            let mut raw = format!(
                "POST / HTTP/1.1{newline}Content-Encoding: gzip{newline}Content-Length: 1{newline}Content-Type: multipart/form-data; boundary=test{newline}{newline}"
            );
            click(
                &mut body,
                &mut raw,
                "Replace captured body with empty editable text",
            );
            assert_eq!(
                raw,
                format!(
                    "POST / HTTP/1.1{newline}Content-Type: multipart/form-data; boundary=test{newline}{newline}"
                )
            );
            let multipart = "--test\r\nContent-Disposition: form-data; name=\"field\"\r\n\r\nvalue\r\n--test--\r\n";
            raw.push_str(multipart);
            assert_eq!(bytes(&body, &raw).unwrap(), multipart.as_bytes());
            let (head, _) = split_raw_request(&raw);
            let request = crate::proxy::RepeaterRequest::parse(
                head,
                bytes(&body, &raw).unwrap().to_vec(),
                "http",
                "example.com",
            )
            .unwrap();
            assert_eq!(request.body, multipart.as_bytes());
        }
    }

    #[test]
    fn preserved_mode_never_silently_ignores_entered_body_text() {
        let body = Some(BodyWorkspace::from_body(&CapturedBody::from_bytes(vec![
            255,
        ])));
        assert!(bytes(&body, "POST / HTTP/1.1\n\nreplacement").is_err());
        assert_eq!(bytes(&body, "POST / HTTP/1.1\n\n").unwrap(), vec![255]);
    }
}
