//! Small, line-oriented syntax highlighting for the message editor.
//!
//! HTTP messages only need a handful of grammars, and the views are rendered
//! one line at a time so the line numbers stay aligned with the text. A
//! purpose-built highlighter keeps the dependency and binary-size cost low
//! compared with pulling in a full TextMate-style highlighting engine.

use egui::text::LayoutJob;
use egui::{Color32, FontId, TextFormat};

use crate::theme;

/// Message text is a little smaller than the UI's default monospace style so
/// more of a request or response fits in the detail panes.
pub const MESSAGE_FONT_SIZE: f32 = 11.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Language {
    #[default]
    Plain,
    Json,
    Markup,
    Form,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineRole {
    Start,
    Header,
    Blank,
    Body,
}

/// The parts of a rendered message, derived from the first blank line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Syntax {
    /// Index of the blank line between the head and the body, or the line
    /// count when the message has no body.
    pub header_end: usize,
    pub language: Language,
}

impl Syntax {
    pub fn from_lines(lines: &[&str]) -> Self {
        let header_end = lines
            .iter()
            .skip(1)
            .position(|line| line.is_empty())
            .map_or(lines.len(), |offset| offset + 1);
        let body = lines.get(header_end + 1..).unwrap_or_default();
        Self {
            header_end,
            language: detect_language(body),
        }
    }

    pub fn role(&self, index: usize) -> LineRole {
        if index == 0 {
            LineRole::Start
        } else if index < self.header_end {
            LineRole::Header
        } else if index == self.header_end {
            LineRole::Blank
        } else {
            LineRole::Body
        }
    }
}

fn detect_language(lines: &[&str]) -> Language {
    let mut first = None;
    let mut all_have_equals = true;

    for line in lines {
        if line.trim().is_empty() {
            continue;
        }
        if first.is_none() {
            first = Some(*line);
        }
        // The form heuristic should not claim masked binary text just because
        // a stray `=` survived the Raw view's control-byte masking.
        if !line.contains('=') || line.contains('·') {
            all_have_equals = false;
        }
    }

    let Some(first) = first else {
        return Language::Plain;
    };
    let trimmed = first.trim_start();
    if trimmed.starts_with('{') || trimmed.starts_with('[') {
        Language::Json
    } else if trimmed.starts_with('<') {
        Language::Markup
    } else if all_have_equals {
        Language::Form
    } else {
        Language::Plain
    }
}

/// Build a one-line layout job. Every branch appends the source text verbatim,
/// so selection and copying see exactly the line that is displayed.
pub fn line_job(line: &str, role: LineRole, language: Language, font: &FontId) -> LayoutJob {
    let mut job = LayoutJob::default();
    job.wrap.max_width = f32::INFINITY;
    match role {
        LineRole::Start => start_line(&mut job, line, font),
        LineRole::Header => header_line(&mut job, line, font),
        LineRole::Blank => {}
        LineRole::Body => body_line(&mut job, line, language, font),
    }
    job
}

/// Build the full message layout for a read-only text editor.
///
/// Unlike the Hex row renderer this lays out the whole message, but `egui`
/// memoizes layout jobs, so the expensive shaping pass is only repeated when
/// the text or wrap width changes. The job must preserve every byte of the
/// input because it is also the buffer used for selection and copying.
pub fn message_job(text: &str, wrap_width: f32, font: &FontId) -> LayoutJob {
    let lines: Vec<&str> = text.lines().collect();
    let syntax = Syntax::from_lines(&lines);
    layout_text(text, wrap_width, font, |line, index| {
        line_job(line, syntax.role(index), syntax.language, font)
    })
}

fn layout_text(
    text: &str,
    wrap_width: f32,
    font: &FontId,
    mut style_line: impl FnMut(&str, usize) -> LayoutJob,
) -> LayoutJob {
    let mut job = LayoutJob::default();
    job.wrap.max_width = wrap_width;
    job.wrap.break_anywhere = true;
    job.keep_trailing_whitespace = true;

    let mut segments = text.split('\n').peekable();
    let mut index = 0;
    while let Some(segment) = segments.next() {
        let (line, carriage_return) = segment
            .strip_suffix('\r')
            .map_or((segment, ""), |line| (line, "\r"));
        let line_job = style_line(line, index);
        append_job(&mut job, line_job);
        if !carriage_return.is_empty() {
            job.append(carriage_return, 0.0, plain_format(font));
        }
        if segments.peek().is_some() {
            job.append("\n", 0.0, plain_format(font));
        }
        index += 1;
    }
    job
}

fn append_job(destination: &mut LayoutJob, source: LayoutJob) {
    let LayoutJob { text, sections, .. } = source;
    for section in sections {
        destination.append(
            &text[section.byte_range.start.0..section.byte_range.end.0],
            section.leading_space,
            section.format,
        );
    }
}

fn plain_format(font: &FontId) -> TextFormat {
    TextFormat {
        font_id: font.clone(),
        color: theme::TEXT,
        ..Default::default()
    }
}

fn start_line(job: &mut LayoutJob, line: &str, font: &FontId) {
    let mut parts = line.splitn(3, ' ');
    let first = parts.next().unwrap_or_default();

    if first.starts_with("HTTP/") {
        push(job, font, first, theme::TEXT_MUTED);
        push(job, font, " ", theme::TEXT);
        if let Some(status) = parts.next() {
            push(job, font, status, status_color(status));
        }
        if let Some(reason) = parts.next() {
            push(job, font, " ", theme::TEXT);
            push(job, font, reason, theme::TEXT);
        }
    } else {
        push(job, font, first, theme::BLUE);
        if let Some(target) = parts.next() {
            push(job, font, " ", theme::TEXT);
            push(job, font, target, theme::TEXT);
        }
        if let Some(version) = parts.next() {
            push(job, font, " ", theme::TEXT);
            push(job, font, version, theme::TEXT_MUTED);
        }
    }
}

fn status_color(status: &str) -> Color32 {
    match status.parse::<u16>().map(|status| status / 100) {
        Ok(2) => theme::GREEN,
        Ok(3) => theme::BLUE,
        Ok(4) => theme::AMBER,
        Ok(5) => theme::RED,
        _ => theme::TEXT_MUTED,
    }
}

fn header_line(job: &mut LayoutJob, line: &str, font: &FontId) {
    if let Some((name, rest)) = line.split_once(':') {
        push(job, font, name, theme::ACCENT_DARK);
        push(job, font, ":", theme::GRAY);
        push(job, font, rest, theme::TEXT);
    } else {
        push(job, font, line, theme::TEXT);
    }
}

fn body_line(job: &mut LayoutJob, line: &str, language: Language, font: &FontId) {
    match language {
        Language::Json => json_line(job, line, font),
        Language::Markup => markup_line(job, line, font),
        Language::Form => form_line(job, line, font),
        Language::Plain => push(job, font, line, theme::TEXT),
    }
}

fn form_line(job: &mut LayoutJob, line: &str, font: &FontId) {
    let pair = line
        .split_once(" = ")
        .map(|(name, value)| (name, " = ", value))
        .or_else(|| line.split_once('=').map(|(name, value)| (name, "=", value)));

    if let Some((name, separator, value)) = pair {
        push(job, font, name, theme::ACCENT_DARK);
        push(job, font, separator, theme::GRAY);
        push(job, font, value, theme::GREEN);
    } else {
        push(job, font, line, theme::TEXT);
    }
}

fn json_line(job: &mut LayoutJob, line: &str, font: &FontId) {
    let bytes = line.as_bytes();
    let mut index = 0;

    while index < bytes.len() {
        match bytes[index] {
            b'"' => {
                let start = index;
                index += 1;
                while index < bytes.len() {
                    match bytes[index] {
                        b'\\' => index = (index + 2).min(bytes.len()),
                        b'"' => {
                            index += 1;
                            break;
                        }
                        _ => index += 1,
                    }
                }
                let is_key = matches!(next_non_space(bytes, index), Some(b':'));
                push(
                    job,
                    font,
                    &line[start..index],
                    if is_key {
                        theme::ACCENT_DARK
                    } else {
                        theme::GREEN
                    },
                );
            }
            b'-' | b'0'..=b'9' => {
                let start = index;
                index += 1;
                while index < bytes.len()
                    && matches!(bytes[index], b'0'..=b'9' | b'.' | b'e' | b'E' | b'+' | b'-')
                {
                    index += 1;
                }
                push(job, font, &line[start..index], theme::BLUE);
            }
            b't' if line[index..].starts_with("true") => {
                push(job, font, "true", theme::AMBER);
                index += 4;
            }
            b'f' if line[index..].starts_with("false") => {
                push(job, font, "false", theme::AMBER);
                index += 5;
            }
            b'n' if line[index..].starts_with("null") => {
                push(job, font, "null", theme::AMBER);
                index += 4;
            }
            b'{' | b'}' | b'[' | b']' | b',' | b':' => {
                push(job, font, &line[index..index + 1], theme::GRAY);
                index += 1;
            }
            byte if byte.is_ascii_whitespace() => {
                let start = index;
                while index < bytes.len() && bytes[index].is_ascii_whitespace() {
                    index += 1;
                }
                push(job, font, &line[start..index], theme::TEXT);
            }
            _ => {
                let start = index;
                while index < bytes.len()
                    && !bytes[index].is_ascii_whitespace()
                    && !matches!(bytes[index], b'{' | b'}' | b'[' | b']' | b',' | b':' | b'"')
                {
                    index += 1;
                }
                push(job, font, &line[start..index], theme::TEXT);
            }
        }
    }
}

fn next_non_space(bytes: &[u8], mut index: usize) -> Option<u8> {
    while index < bytes.len() && bytes[index].is_ascii_whitespace() {
        index += 1;
    }
    bytes.get(index).copied()
}

fn markup_line(job: &mut LayoutJob, line: &str, font: &FontId) {
    let bytes = line.as_bytes();
    let mut index = 0;

    while index < bytes.len() {
        if bytes[index] == b'<' && looks_like_tag_start(bytes, index) {
            if line[index..].starts_with("<!--") {
                let end = line[index..]
                    .find("-->")
                    .map_or(bytes.len(), |offset| index + offset + 3);
                push(job, font, &line[index..end], theme::TEXT_MUTED);
                index = end;
            } else {
                let end = find_tag_end(line, index);
                highlight_tag(job, &line[index..end], font);
                index = end;
            }
        } else {
            // A `<` that does not start a tag is plain text. Resume the search
            // after it so the loop always advances, and keep the slice on a
            // char boundary for non-ASCII body text.
            let search_from = if bytes[index] == b'<' {
                index + 1
            } else {
                index
            };
            let end = line[search_from..]
                .find('<')
                .map_or(bytes.len(), |offset| search_from + offset);
            push(job, font, &line[index..end], theme::TEXT);
            index = end;
        }
    }
}

fn looks_like_tag_start(bytes: &[u8], index: usize) -> bool {
    match bytes.get(index + 1) {
        Some(b'/') => bytes.get(index + 2).is_some_and(u8::is_ascii_alphabetic),
        Some(b'!') | Some(b'?') => true,
        Some(byte) => byte.is_ascii_alphabetic(),
        None => false,
    }
}

fn find_tag_end(line: &str, start: usize) -> usize {
    let mut quote = None;
    for (offset, ch) in line[start..].char_indices() {
        match quote {
            Some(active) => {
                if ch == active {
                    quote = None;
                }
            }
            None => match ch {
                '"' | '\'' => quote = Some(ch),
                '>' => return start + offset + ch.len_utf8(),
                _ => {}
            },
        }
    }
    line.len()
}

fn highlight_tag(job: &mut LayoutJob, raw: &str, font: &FontId) {
    if raw.starts_with("<!") || raw.starts_with("<?") {
        push(job, font, raw, theme::TEXT_MUTED);
        return;
    }

    let bytes = raw.as_bytes();
    let mut index = 0;
    let mut expect_value = false;

    while index < bytes.len() {
        match bytes[index] {
            b'<' => {
                push(job, font, "<", theme::GRAY);
                index += 1;
                if bytes.get(index) == Some(&b'/') {
                    push(job, font, "/", theme::GRAY);
                    index += 1;
                }
                let start = index;
                while index < bytes.len()
                    && !bytes[index].is_ascii_whitespace()
                    && !matches!(bytes[index], b'>' | b'/')
                {
                    index += 1;
                }
                push(job, font, &raw[start..index], theme::BLUE);
            }
            b'>' => {
                push(job, font, ">", theme::GRAY);
                index += 1;
            }
            b'/' if bytes.get(index + 1) == Some(&b'>') => {
                push(job, font, "/>", theme::GRAY);
                index += 2;
            }
            b'/' => {
                push(job, font, "/", theme::GRAY);
                index += 1;
            }
            b'=' => {
                push(job, font, "=", theme::GRAY);
                expect_value = true;
                index += 1;
            }
            b'"' | b'\'' => {
                let quote = bytes[index];
                let start = index;
                index += 1;
                while index < bytes.len() {
                    match bytes[index] {
                        b'\\' => index = (index + 2).min(bytes.len()),
                        byte if byte == quote => {
                            index += 1;
                            break;
                        }
                        _ => index += 1,
                    }
                }
                push(job, font, &raw[start..index], theme::GREEN);
                expect_value = false;
            }
            byte if byte.is_ascii_whitespace() => {
                let start = index;
                while index < bytes.len() && bytes[index].is_ascii_whitespace() {
                    index += 1;
                }
                push(job, font, &raw[start..index], theme::TEXT);
            }
            _ => {
                let start = index;
                while index < bytes.len()
                    && !bytes[index].is_ascii_whitespace()
                    && !matches!(bytes[index], b'>' | b'/' | b'=' | b'"' | b'\'')
                {
                    index += 1;
                }
                push(
                    job,
                    font,
                    &raw[start..index],
                    if expect_value {
                        theme::GREEN
                    } else {
                        theme::ACCENT_DARK
                    },
                );
                expect_value = false;
            }
        }
    }
}

fn push(job: &mut LayoutJob, font: &FontId, text: &str, color: Color32) {
    if text.is_empty() {
        return;
    }
    job.append(
        text,
        0.0,
        TextFormat {
            font_id: font.clone(),
            color,
            ..Default::default()
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &str) -> Vec<&str> {
        text.lines().collect()
    }

    #[test]
    fn splits_head_and_body_at_the_first_blank_line() {
        let text = "HTTP/1.1 200 OK\nContent-Type: application/json\n\n{\n  \"ok\": true\n}";
        let syntax = Syntax::from_lines(&lines(text));

        assert_eq!(syntax.header_end, 2);
        assert_eq!(syntax.role(0), LineRole::Start);
        assert_eq!(syntax.role(1), LineRole::Header);
        assert_eq!(syntax.role(2), LineRole::Blank);
        assert_eq!(syntax.role(3), LineRole::Body);
        assert_eq!(syntax.language, Language::Json);
    }

    #[test]
    fn detects_markup_and_form_bodies() {
        let markup = Syntax::from_lines(&lines("HTTP/1.1 200 OK\n\n<!DOCTYPE html>\n<html>"));
        assert_eq!(markup.language, Language::Markup);

        let form = Syntax::from_lines(&lines(
            "POST /login HTTP/1.1\nHost: example.com\n\nuser=admin&password=hunter2",
        ));
        assert_eq!(form.language, Language::Form);

        let pretty_form =
            Syntax::from_lines(&lines("HTTP/1.1 200 OK\n\nname = Perseid\nrole = admin"));
        assert_eq!(pretty_form.language, Language::Form);

        let masked_binary = Syntax::from_lines(&lines("HTTP/1.1 200 OK\n\n··=··"));
        assert_eq!(masked_binary.language, Language::Plain);
    }

    #[test]
    fn head_only_messages_have_no_body_language() {
        let syntax = Syntax::from_lines(&lines("HTTP/1.1 204 No Content\nServer: perseid"));

        assert_eq!(syntax.header_end, 2);
        assert_eq!(syntax.language, Language::Plain);
        assert_eq!(syntax.role(1), LineRole::Header);
    }

    #[test]
    fn message_jobs_preserve_every_byte() {
        let font = FontId::monospace(MESSAGE_FONT_SIZE);
        let cases = [
            "GET / HTTP/1.1\nHost: example.com\n\nbody",
            "GET / HTTP/1.1\r\nHost: example.com\r\n\r\nbody\r\n",
            "GET / HTTP/1.1\n\n",
            "",
            "Привет\nмир",
        ];

        for text in cases {
            let job = message_job(text, 120.0, &font);
            assert_eq!(job.text, text);
        }
    }

    #[test]
    fn message_jobs_wrap_long_unbroken_lines() {
        let ctx = egui::Context::default();
        let font = FontId::monospace(MESSAGE_FONT_SIZE);
        let text = "GET / HTTP/1.1\nHost: example.com\n\n\
                    {\"value\":\"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\"}";
        let mut rows = 0;

        ctx.run_ui(egui::RawInput::default(), |ui| {
            let job = message_job(text, 80.0, &font);
            let galley = ui.fonts_mut(|fonts| fonts.layout_job(job));
            rows = galley.rows.len();
        })
        .drop_without_applying_deltas();

        assert!(rows > text.lines().count(), "long lines should wrap");
    }

    #[test]
    fn jobs_preserve_the_source_line() {
        let font = FontId::monospace(MESSAGE_FONT_SIZE);
        let cases = [
            ("GET /index.html HTTP/1.1", LineRole::Start, Language::Plain),
            ("HTTP/1.1 404 Not Found", LineRole::Start, Language::Plain),
            ("Content-Type: text/html", LineRole::Header, Language::Plain),
            (
                "  \"items\": [1, true, null],",
                LineRole::Body,
                Language::Json,
            ),
            (
                "<a href=\"/x\">Perseid</a>",
                LineRole::Body,
                Language::Markup,
            ),
            ("Привет <b>мир</b>", LineRole::Body, Language::Markup),
            ("name = Perseid", LineRole::Body, Language::Form),
            ("plain text", LineRole::Body, Language::Plain),
        ];

        for (line, role, language) in cases {
            let job = line_job(line, role, language, &font);
            assert_eq!(job.text, line);
        }
    }

    #[test]
    fn start_line_and_headers_use_accent_colors() {
        let font = FontId::monospace(MESSAGE_FONT_SIZE);

        let start = line_job("GET / HTTP/1.1", LineRole::Start, Language::Plain, &font);
        assert_eq!(start.sections[0].format.color, theme::BLUE);

        let header = line_job(
            "Content-Type: text/html",
            LineRole::Header,
            Language::Plain,
            &font,
        );
        assert_eq!(header.sections[0].format.color, theme::ACCENT_DARK);
        assert_eq!(
            &header.text
                [header.sections[0].byte_range.start.0..header.sections[0].byte_range.end.0],
            "Content-Type"
        );
    }

    #[test]
    fn status_colors_follow_the_status_class() {
        assert_eq!(status_color("200"), theme::GREEN);
        assert_eq!(status_color("301"), theme::BLUE);
        assert_eq!(status_color("404"), theme::AMBER);
        assert_eq!(status_color("500"), theme::RED);
        assert_eq!(status_color("nope"), theme::TEXT_MUTED);
    }
}
