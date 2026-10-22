//! Pretty-printing for the message editor's Pretty tab.
//!
//! The Pretty tab is a readability aid, not a rewriter: the proxy still
//! forwards the original bytes. JSON, HTML/XML, and URL-encoded form bodies are
//! laid out with indentation or one field per line; anything we do not
//! recognise falls back to the raw text representation.

/// Input and formatted-output budgets are independent: indentation can amplify
/// a small, deeply nested document by orders of magnitude.
const MAX_FORMATTED_BYTES: usize = 256 * 1024;
const MAX_NESTING_DEPTH: usize = 128;

struct Output(String);

impl Output {
    fn new(input_len: usize) -> Self {
        Self(String::with_capacity(input_len.min(MAX_FORMATTED_BYTES)))
    }

    fn push_str(&mut self, text: &str) -> Option<()> {
        let required = self.0.len().checked_add(text.len())?;
        if required > MAX_FORMATTED_BYTES {
            return None;
        }
        if required > self.0.capacity() {
            let capacity = self
                .0
                .capacity()
                .max(64)
                .saturating_mul(2)
                .max(required)
                .min(MAX_FORMATTED_BYTES);
            self.0.try_reserve_exact(capacity - self.0.len()).ok()?;
        }
        self.0.push_str(text);
        Some(())
    }

    fn push(&mut self, ch: char) -> Option<()> {
        self.push_str(ch.encode_utf8(&mut [0; 4]))
    }

    fn indent(&mut self, depth: usize) -> Option<()> {
        if depth > MAX_NESTING_DEPTH {
            return None;
        }
        for _ in 0..depth {
            self.push_str("  ")?;
        }
        Some(())
    }
}

/// Format a body for the Pretty tab.
///
/// Returns `None` when the body is not text or does not match a format we know
/// how to lay out, or when formatting would exceed the output/depth budgets.
/// The caller can show the bounded raw representation instead.
pub fn format_body(content_type: Option<&str>, body: &[u8]) -> Option<String> {
    if body.len() > MAX_FORMATTED_BYTES {
        return None;
    }
    let text = std::str::from_utf8(body).ok()?;
    if text.trim().is_empty() {
        return None;
    }

    let media = media_type(content_type);
    if is_json(media)
        && let Some(out) = format_json(text)
    {
        return Some(out);
    }
    if is_markup(media)
        && let Some(out) = format_markup(text, !is_xml(media))
    {
        return Some(out);
    }
    if is_form(media)
        && let Some(out) = format_form(text)
    {
        return Some(out);
    }

    // Content-Type is often missing, incomplete, or wrong. Sniff the common
    // structured text formats instead of leaving an obvious JSON or HTML body
    // unformatted.
    let trimmed = text.trim_start();
    if (trimmed.starts_with('{') || trimmed.starts_with('['))
        && let Some(out) = format_json(text)
    {
        return Some(out);
    }
    if trimmed.starts_with('<')
        && let Some(out) = format_markup(text, !is_xml(media))
    {
        return Some(out);
    }
    None
}

fn media_type(content_type: Option<&str>) -> &str {
    content_type
        .and_then(|value| value.split(';').next())
        .map(str::trim)
        .unwrap_or("")
}

fn is_json(media: &str) -> bool {
    media.eq_ignore_ascii_case("application/json")
        || media.eq_ignore_ascii_case("text/json")
        || media.to_ascii_lowercase().ends_with("+json")
}

fn is_markup(media: &str) -> bool {
    let media = media.to_ascii_lowercase();
    media == "text/html"
        || media == "application/xhtml+xml"
        || media == "text/xml"
        || media == "application/xml"
        || media == "image/svg+xml"
        || media.ends_with("+xml")
}

fn is_xml(media: &str) -> bool {
    let media = media.to_ascii_lowercase();
    media == "text/xml" || media == "application/xml" || media.ends_with("+xml")
}

fn is_form(media: &str) -> bool {
    media.eq_ignore_ascii_case("application/x-www-form-urlencoded")
}

/// Indent every token in a JSON value without changing strings, numbers, or
/// member order.
fn format_json(text: &str) -> Option<String> {
    let tokens = json_tokens(text)?;
    let mut out = Output::new(text.len());
    let mut expected_closers: Vec<char> = Vec::new();
    let mut index = 0usize;

    while index < tokens.len() {
        match &tokens[index] {
            JsonToken::Punct(open @ ('{' | '[')) => {
                if expected_closers.len() == MAX_NESTING_DEPTH {
                    return None;
                }
                let closer = if *open == '{' { '}' } else { ']' };
                out.push(*open)?;
                if tokens.get(index + 1) == Some(&JsonToken::Punct(closer)) {
                    out.push(closer)?;
                    index += 2;
                    continue;
                }
                expected_closers.push(closer);
                newline_indent(&mut out, expected_closers.len())?;
            }
            JsonToken::Punct(closer @ ('}' | ']')) => {
                if expected_closers.pop() != Some(*closer) {
                    return None;
                }
                newline_indent(&mut out, expected_closers.len())?;
                out.push(*closer)?;
            }
            JsonToken::Punct(',') => {
                out.push(',')?;
                newline_indent(&mut out, expected_closers.len())?;
            }
            JsonToken::Punct(':') => out.push_str(": ")?,
            JsonToken::Punct(_) => return None,
            JsonToken::Value(value) => out.push_str(value)?,
        }
        index += 1;
    }

    expected_closers.is_empty().then_some(out.0)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum JsonToken<'a> {
    Value(&'a str),
    Punct(char),
}

fn json_tokens(text: &str) -> Option<Vec<JsonToken<'_>>> {
    let bytes = text.as_bytes();
    let mut tokens = Vec::new();
    let mut index = 0usize;

    while index < bytes.len() {
        let byte = bytes[index];
        if byte.is_ascii_whitespace() {
            index += 1;
            continue;
        }
        match byte {
            b'{' | b'}' | b'[' | b']' | b',' | b':' => {
                tokens.push(JsonToken::Punct(byte as char));
                index += 1;
            }
            b'"' => {
                let start = index;
                index += 1;
                loop {
                    if index >= bytes.len() {
                        return None;
                    }
                    match bytes[index] {
                        b'\\' => index += 2,
                        b'"' => {
                            index += 1;
                            break;
                        }
                        _ => index += 1,
                    }
                }
                tokens.push(JsonToken::Value(&text[start..index]));
            }
            _ => {
                let start = index;
                while index < bytes.len()
                    && !bytes[index].is_ascii_whitespace()
                    && !matches!(bytes[index], b'{' | b'}' | b'[' | b']' | b',' | b':' | b'"')
                {
                    index += 1;
                }
                if index == start {
                    return None;
                }
                tokens.push(JsonToken::Value(&text[start..index]));
            }
        }
    }

    match tokens.first() {
        Some(JsonToken::Punct('{')) | Some(JsonToken::Punct('[')) => Some(tokens),
        _ => None,
    }
}

fn newline_indent(out: &mut Output, depth: usize) -> Option<()> {
    out.push('\n')?;
    out.indent(depth)
}

#[derive(Debug, PartialEq, Eq)]
enum MarkupToken<'a> {
    Text(&'a str),
    Tag {
        raw: &'a str,
        name: String,
        closing: bool,
        self_closing: bool,
        special: bool,
    },
    RawText(&'a str),
}

/// Indent HTML or XML while leaving text, attributes, and raw-text element
/// contents intact enough to read.
fn format_markup(text: &str, html: bool) -> Option<String> {
    let tokens = markup_tokens(text, html);
    if !tokens
        .iter()
        .any(|token| matches!(token, MarkupToken::Tag { .. }))
    {
        return None;
    }

    let mut out = Output::new(text.len());
    let mut stack: Vec<String> = Vec::new();
    let mut wrote = false;

    for token in tokens {
        match token {
            MarkupToken::Text(text) => {
                let collapsed = collapse_whitespace(text);
                if !collapsed.is_empty() {
                    push_line(&mut out, stack.len(), &collapsed)?;
                    wrote = true;
                }
            }
            MarkupToken::Tag {
                raw,
                name,
                closing,
                self_closing,
                special,
            } => {
                if special || name.is_empty() {
                    push_line(&mut out, stack.len(), raw)?;
                    wrote = true;
                    continue;
                }

                if closing {
                    if let Some(position) = stack.iter().rposition(|open| open == &name) {
                        stack.truncate(position);
                    } else if !stack.is_empty() {
                        stack.pop();
                    }
                    push_line(&mut out, stack.len(), raw)?;
                } else {
                    push_line(&mut out, stack.len(), raw)?;
                    if !(self_closing || html && is_void_element(&name)) {
                        if stack.len() == MAX_NESTING_DEPTH {
                            return None;
                        }
                        stack.push(name);
                    }
                }
                wrote = true;
            }
            MarkupToken::RawText(text) => {
                push_raw_text(&mut out, stack.len(), text)?;
                wrote = true;
            }
        }
    }

    wrote.then_some(out.0)
}

fn push_line(out: &mut Output, depth: usize, line: &str) -> Option<()> {
    if !out.0.is_empty() && !out.0.ends_with('\n') {
        out.push('\n')?;
    }
    out.indent(depth)?;
    out.push_str(line)?;
    out.push('\n')
}

fn push_raw_text(out: &mut Output, depth: usize, text: &str) -> Option<()> {
    for line in text.trim_matches('\n').lines() {
        if line.trim().is_empty() {
            out.push('\n')?;
        } else {
            push_line(out, depth, line.trim_end())?;
        }
    }
    Some(())
}

fn collapse_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn markup_tokens(text: &str, html: bool) -> Vec<MarkupToken<'_>> {
    let bytes = text.as_bytes();
    let mut tokens = Vec::new();
    let mut index = 0usize;

    while index < bytes.len() {
        if bytes[index] == b'<'
            && let Some(end) = find_tag_end(text, index)
        {
            let raw = &text[index..end];
            let (name, closing, self_closing, special) = parse_tag(raw);

            // Script, style, pre, and textarea content is not markup. Keep
            // it verbatim so formatters do not treat comparison operators
            // or string literals as tags.
            if html
                && !closing
                && !self_closing
                && !special
                && is_raw_text_element(&name)
                && let Some(close_start) = find_closing_tag(text, end, &name)
                && let Some(close_end) = find_tag_end(text, close_start)
            {
                tokens.push(MarkupToken::Tag {
                    raw,
                    name: name.clone(),
                    closing: false,
                    self_closing: false,
                    special: false,
                });
                let content = &text[end..close_start];
                if !content.is_empty() {
                    tokens.push(MarkupToken::RawText(content));
                }
                tokens.push(MarkupToken::Tag {
                    raw: &text[close_start..close_end],
                    name,
                    closing: true,
                    self_closing: false,
                    special: false,
                });
                index = close_end;
                continue;
            }

            tokens.push(MarkupToken::Tag {
                raw,
                name,
                closing,
                self_closing,
                special,
            });
            index = end;
            continue;
        }

        let start = index;
        let search_from = if bytes[index] == b'<' {
            index + 1
        } else {
            index
        };
        let next = text[search_from..]
            .find('<')
            .map(|offset| search_from + offset)
            .unwrap_or(text.len());
        tokens.push(MarkupToken::Text(&text[start..next]));
        index = next;
    }

    tokens
}

fn parse_tag(raw: &str) -> (String, bool, bool, bool) {
    let inner = raw
        .strip_prefix('<')
        .unwrap_or(raw)
        .strip_suffix('>')
        .unwrap_or(raw);
    let trimmed = inner.trim();

    if trimmed.starts_with('!') || trimmed.starts_with('?') {
        return (String::new(), false, false, true);
    }

    let closing = trimmed.starts_with('/');
    let rest = trimmed.strip_prefix('/').unwrap_or(trimmed).trim_start();
    let self_closing = rest.ends_with('/');
    let name_end = rest
        .find(|ch: char| ch.is_whitespace() || ch == '/' || ch == '>')
        .unwrap_or(rest.len());
    let name = rest[..name_end].to_ascii_lowercase();

    if name.is_empty() {
        return (name, closing, self_closing, true);
    }
    (name, closing, self_closing, false)
}

fn find_tag_end(text: &str, start: usize) -> Option<usize> {
    let mut quote = None;
    for (offset, ch) in text[start..].char_indices() {
        match quote {
            Some(active) => {
                if ch == active {
                    quote = None;
                }
            }
            None => match ch {
                '"' | '\'' => quote = Some(ch),
                '>' => return Some(start + offset + ch.len_utf8()),
                _ => {}
            },
        }
    }
    None
}

fn find_closing_tag(text: &str, from: usize, name: &str) -> Option<usize> {
    let lower = text[from..].to_ascii_lowercase();
    let needle = format!("</{name}");
    let mut search = 0usize;

    while let Some(offset) = lower[search..].find(&needle) {
        let position = search + offset;
        let after = position + needle.len();
        if lower[after..]
            .chars()
            .next()
            .is_none_or(|ch| ch.is_whitespace() || ch == '>')
        {
            return Some(from + position);
        }
        search = position + needle.len();
    }
    None
}

fn is_void_element(name: &str) -> bool {
    matches!(
        name,
        "area"
            | "base"
            | "br"
            | "col"
            | "embed"
            | "hr"
            | "img"
            | "input"
            | "link"
            | "meta"
            | "param"
            | "source"
            | "track"
            | "wbr"
    )
}

fn is_raw_text_element(name: &str) -> bool {
    matches!(name, "script" | "style" | "pre" | "textarea")
}

/// Put one form field per line and decode percent escapes for readability.
fn format_form(text: &str) -> Option<String> {
    if !text.contains('=') {
        return None;
    }

    let mut out = Output::new(text.len());
    let mut wrote = false;
    for (name, value) in form_urlencoded::parse(text.as_bytes()) {
        out.push_str(name.as_ref())?;
        out.push_str(" = ")?;
        out.push_str(value.as_ref())?;
        out.push('\n')?;
        wrote = true;
    }
    wrote.then_some(out.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_deeply_nested_documents_without_amplifying_them() {
        let json = format!("{}0{}", "[".repeat(2_000), "]".repeat(2_000));
        assert!(format_body(Some("application/json"), json.as_bytes()).is_none());
        let xml = format!("{}text{}", "<a>".repeat(2_000), "</a>".repeat(2_000));
        assert!(format_body(Some("application/xml"), xml.as_bytes()).is_none());
    }

    #[test]
    fn output_budget_applies_to_json_markup_and_forms() {
        let json = format!("[{}0]", "0,".repeat(MAX_FORMATTED_BYTES / 3));
        let xml = format!("<a>{}</a>", "<b>x</b>".repeat(MAX_FORMATTED_BYTES / 9));
        let form = "a=1&".repeat(MAX_FORMATTED_BYTES / 5);
        for (media, body) in [
            ("application/json", json),
            ("application/xml", xml),
            ("application/x-www-form-urlencoded", form),
        ] {
            assert!(body.len() < MAX_FORMATTED_BYTES);
            assert!(
                format_body(Some(media), body.as_bytes()).is_none(),
                "{media}"
            );
        }
    }

    #[test]
    fn output_budget_checks_utf8_bytes_before_appending() {
        let mut out = Output::new(0);
        out.push_str(&"x".repeat(MAX_FORMATTED_BYTES - 1)).unwrap();
        assert!(out.push('é').is_none());
        assert_eq!(out.0.len(), MAX_FORMATTED_BYTES - 1);
        out.push('x').unwrap();
        assert!(out.push('x').is_none());
        assert!(out.0.capacity() <= MAX_FORMATTED_BYTES);
    }

    #[test]
    fn formats_json_without_rewriting_values() {
        let body = br#"{"items":[{"id":1,"name":"Router"},{"id":2}],"ok":true}"#;
        let formatted = format_body(Some("application/json; charset=utf-8"), body).unwrap();

        assert_eq!(
            formatted,
            "{\n  \"items\": [\n    {\n      \"id\": 1,\n      \"name\": \"Router\"\n    },\n    {\n      \"id\": 2\n    }\n  ],\n  \"ok\": true\n}"
        );
    }

    #[test]
    fn formats_empty_json_containers_on_one_line() {
        let formatted = format_body(None, br#"{"empty":{},"list":[]}"#).unwrap();
        assert_eq!(formatted, "{\n  \"empty\": {},\n  \"list\": []\n}");
    }

    #[test]
    fn keeps_braces_and_escaped_quotes_inside_json_strings() {
        let formatted = format_body(None, br#"{"text":"a { b } \"quoted\""}"#).unwrap();
        assert!(formatted.contains(r#""text": "a { b } \"quoted\"""#));
    }

    #[test]
    fn formats_html_with_nesting_and_void_elements() {
        let body = b"<!DOCTYPE html><html><body><h1>Demo</h1><img src=\"x\"><p>Hello <b>world</b></p></body></html>";
        let formatted = format_body(Some("text/html"), body).unwrap();

        assert_eq!(
            formatted,
            "<!DOCTYPE html>\n\
             <html>\n\
             \x20\x20<body>\n\
             \x20\x20\x20\x20<h1>\n\
             \x20\x20\x20\x20\x20\x20Demo\n\
             \x20\x20\x20\x20</h1>\n\
             \x20\x20\x20\x20<img src=\"x\">\n\
             \x20\x20\x20\x20<p>\n\
             \x20\x20\x20\x20\x20\x20Hello\n\
             \x20\x20\x20\x20\x20\x20<b>\n\
             \x20\x20\x20\x20\x20\x20\x20\x20world\n\
             \x20\x20\x20\x20\x20\x20</b>\n\
             \x20\x20\x20\x20</p>\n\
             \x20\x20</body>\n\
             </html>\n"
        );
    }

    #[test]
    fn keeps_script_contents_out_of_markup_formatting() {
        let body = b"<html><script>if (a < b) { call(); }</script><p>ok</p></html>";
        let formatted = format_body(Some("text/html"), body).unwrap();

        assert!(formatted.contains("if (a < b) { call(); }"));
        assert!(formatted.contains("<p>"));
    }

    #[test]
    fn formats_xml_self_closing_elements() {
        let body = b"<root><item id=\"1\" /><item>two</item></root>";
        let formatted = format_body(Some("application/xml"), body).unwrap();

        assert!(formatted.starts_with("<root>\n  <item id=\"1\" />\n  <item>\n"));
        assert!(formatted.ends_with("</root>\n"));
    }

    #[test]
    fn formats_url_encoded_forms_one_field_per_line() {
        let formatted = format_body(
            Some("application/x-www-form-urlencoded"),
            b"name=Perseid+User&role=admin%2Fowner",
        )
        .unwrap();

        assert_eq!(formatted, "name = Perseid User\nrole = admin/owner\n");
    }

    #[test]
    fn form_formatting_handles_empty_fields_and_malformed_escapes() {
        let formatted = format_body(
            Some("application/x-www-form-urlencoded"),
            b"&&a=1&bad=%2&plus=a+b",
        )
        .unwrap();

        assert_eq!(formatted, "a = 1\nbad = %2\nplus = a b\n");
    }

    #[test]
    fn leaves_unrecognised_bodies_to_the_raw_view() {
        assert!(format_body(Some("text/plain"), b"plain text").is_none());
        assert!(format_body(Some("text/html"), b"plain text").is_none());
        assert!(format_body(Some("application/json"), b"not json").is_none());
        assert!(format_body(None, &[0xff, 0x00, 0x01]).is_none());
    }
}
