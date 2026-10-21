//! Parsing and serializing editable HTTP/1 and HTTP/2 message heads.
//!
//! HTTP/2 has no request line or textual head on the wire. Interception uses a
//! synthetic representation: an origin-form request line with the `Host`
//! derived from `:authority`, and a status line without a reason phrase. The
//! parser accepts both `HTTP/2` and the older `HTTP/2.0` spelling.

use std::borrow::Cow;

use http::header::{HOST, HeaderName, HeaderValue};
use http::{HeaderMap, Method, StatusCode, Uri, Version};
use hyper::ext::ReasonPhrase;

use crate::proxy::util::version_str;

pub(crate) struct ParsedRequestHead {
    pub(crate) method: Method,
    pub(crate) uri: Uri,
    pub(crate) version: Version,
    pub(crate) headers: HeaderMap,
}

pub(crate) struct ParsedResponseHead {
    pub(crate) version: Version,
    pub(crate) status: StatusCode,
    pub(crate) reason: Option<ReasonPhrase>,
    pub(crate) headers: HeaderMap,
}

/// The visible request line and headers, terminated by a blank line.
///
/// HTTP/2 requests use an origin-form target and a synthetic `Host` header so
/// the editor can represent the `:scheme`, `:authority`, and `:path`
/// pseudo-headers. `fallback_host` is the CONNECT authority for intercepted
/// requests whose URI has no authority.
pub(crate) fn serialize_request_head(
    method: &Method,
    uri: &Uri,
    version: Version,
    headers: &HeaderMap,
    fallback_host: Option<&str>,
) -> String {
    let target = if version == Version::HTTP_2 {
        Cow::Borrowed(
            uri.path_and_query()
                .map(|path| path.as_str())
                .unwrap_or("/"),
        )
    } else {
        Cow::Owned(uri.to_string())
    };
    let mut out = format!("{} {} {}\r\n", method, target, version_str(version));

    if version == Version::HTTP_2 {
        let mut headers = headers.clone();
        if let Some(authority) = uri.authority().map(|a| a.as_str()).or(fallback_host)
            && let Ok(value) = HeaderValue::from_str(authority)
        {
            headers.insert(HOST, value);
        }
        push_headers(&mut out, &headers);
    } else {
        push_headers(&mut out, headers);
    }
    out.push_str("\r\n");
    out
}

/// The visible response status line and headers, terminated by a blank line.
///
/// HTTP/2 status lines have no reason phrase.
pub(crate) fn serialize_response_head(
    version: Version,
    status: StatusCode,
    reason: &str,
    headers: &HeaderMap,
) -> String {
    let mut out = if version == Version::HTTP_2 {
        format!("{} {}\r\n", version_str(version), status.as_u16())
    } else {
        format!(
            "{} {} {}\r\n",
            version_str(version),
            status.as_u16(),
            reason
        )
    };
    push_headers(&mut out, headers);
    out.push_str("\r\n");
    out
}

fn push_headers(out: &mut String, headers: &HeaderMap) {
    for (name, value) in headers {
        out.push_str(name.as_str());
        out.push_str(": ");
        out.push_str(&String::from_utf8_lossy(value.as_bytes()));
        out.push_str("\r\n");
    }
}

/// Parse an edited request head. LF-only line endings are accepted so a
/// text-editor draft does not need platform-specific line endings.
pub(crate) fn parse_request_head(text: &str) -> Result<ParsedRequestHead, String> {
    let (first, headers) = split_head(text, "request")?;
    let mut parts = first.split_whitespace();
    let method = parts
        .next()
        .ok_or_else(|| "request line is missing a method".to_string())?;
    let target = parts
        .next()
        .ok_or_else(|| "request line is missing a target".to_string())?;
    let version = parts
        .next()
        .ok_or_else(|| "request line is missing an HTTP version".to_string())?;
    if parts.next().is_some() {
        return Err("request line has too many fields".to_string());
    }

    let method = Method::from_bytes(method.as_bytes())
        .map_err(|_| format!("invalid HTTP method `{method}`"))?;
    let uri = target
        .parse::<Uri>()
        .map_err(|_| format!("invalid request target `{target}`"))?;
    let version = parse_version(version)?;

    Ok(ParsedRequestHead {
        method,
        uri,
        version,
        headers: parse_headers(headers)?,
    })
}

/// Parse an edited response head.
pub(crate) fn parse_response_head(text: &str) -> Result<ParsedResponseHead, String> {
    let (first, headers) = split_head(text, "response")?;
    let mut parts = first.splitn(3, ' ');
    let version = parts
        .next()
        .ok_or_else(|| "status line is missing an HTTP version".to_string())?;
    let status = parts
        .next()
        .ok_or_else(|| "status line is missing a status code".to_string())?;
    let reason = parts.next().unwrap_or_default();

    let version = parse_version(version)?;
    let status = status
        .parse::<u16>()
        .map_err(|_| format!("invalid HTTP status code `{status}`"))?;
    if !(100..=599).contains(&status) {
        return Err(format!("HTTP status code {status} is out of range"));
    }
    let status =
        StatusCode::from_u16(status).map_err(|_| format!("invalid HTTP status code `{status}`"))?;
    let reason = if reason.is_empty() {
        None
    } else {
        Some(
            ReasonPhrase::try_from(reason.as_bytes())
                .map_err(|_| "invalid HTTP reason phrase".to_string())?,
        )
    };

    Ok(ParsedResponseHead {
        version,
        status,
        reason,
        headers: parse_headers(headers)?,
    })
}

/// A reason phrase suitable for the editor, falling back to the canonical
/// phrase when hyper did not preserve one.
pub(crate) fn editor_reason(status: StatusCode, extensions: &http::Extensions) -> String {
    extensions
        .get::<ReasonPhrase>()
        .map(|phrase| String::from_utf8_lossy(phrase.as_bytes()).into_owned())
        .or_else(|| status.canonical_reason().map(ToString::to_string))
        .unwrap_or_default()
}

fn split_head(text: &str, kind: &str) -> Result<(String, Vec<String>), String> {
    let normalized = text.replace("\r\n", "\n");
    if normalized.contains('\r') {
        return Err(format!("{kind} head contains a stray carriage return"));
    }

    let mut lines = normalized.lines();
    let first = lines
        .next()
        .filter(|line| !line.trim().is_empty())
        .ok_or_else(|| format!("{kind} head is empty"))?;

    let mut headers = Vec::new();
    for line in lines {
        if line.trim().is_empty() {
            break;
        }
        if line.starts_with(' ') || line.starts_with('\t') {
            return Err("folded header lines are not supported".to_string());
        }
        headers.push(line.to_string());
    }

    Ok((first.to_string(), headers))
}

fn parse_headers(lines: Vec<String>) -> Result<HeaderMap, String> {
    let mut headers = HeaderMap::new();
    for line in lines {
        let (name, value) = line
            .split_once(':')
            .ok_or_else(|| format!("header line is missing a colon: `{line}`"))?;
        let name = name.trim();
        if name.is_empty() {
            return Err("header name is empty".to_string());
        }
        let name = HeaderName::from_bytes(name.as_bytes())
            .map_err(|_| format!("invalid header name `{name}`"))?;
        let value = value.trim_matches([' ', '\t']);
        let value = HeaderValue::from_str(value)
            .map_err(|_| format!("invalid value for header `{name}`"))?;
        headers.append(name, value);
    }
    Ok(headers)
}

fn parse_version(version: &str) -> Result<Version, String> {
    match version {
        "HTTP/1.0" => Ok(Version::HTTP_10),
        "HTTP/1.1" => Ok(Version::HTTP_11),
        "HTTP/2" | "HTTP/2.0" => Ok(Version::HTTP_2),
        _ => Err(format!("unsupported HTTP version `{version}`")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_lf_only_request_heads() {
        let parsed =
            parse_request_head("POST /submit HTTP/1.1\nHost: example.com\nX-Test: one\n\n")
                .unwrap();
        assert_eq!(parsed.method, Method::POST);
        assert_eq!(parsed.uri, "/submit");
        assert_eq!(parsed.version, Version::HTTP_11);
        assert_eq!(parsed.headers.get("host").unwrap(), "example.com");
        assert_eq!(parsed.headers.get("x-test").unwrap(), "one");
    }

    #[test]
    fn parses_response_heads_and_preserves_reason() {
        let parsed = parse_response_head("HTTP/1.1 418 Teapot\r\nX-Test: yes\r\n\r\n").unwrap();
        assert_eq!(parsed.status, StatusCode::IM_A_TEAPOT);
        assert_eq!(parsed.reason.unwrap().as_bytes(), "Teapot".as_bytes());
        assert_eq!(parsed.headers.get("x-test").unwrap(), "yes");
    }

    #[test]
    fn parses_http2_heads_without_pseudo_headers() {
        let parsed = parse_request_head("GET /h2 HTTP/2\r\nHost: example.com\r\n\r\n").unwrap();
        assert_eq!(parsed.version, Version::HTTP_2);
        assert_eq!(parsed.uri, "/h2");
        assert_eq!(parsed.headers.get("host").unwrap(), "example.com");

        let parsed = parse_response_head("HTTP/2 200\r\nX-Test: yes\r\n\r\n").unwrap();
        assert_eq!(parsed.version, Version::HTTP_2);
        assert_eq!(parsed.status, StatusCode::OK);
        assert!(parsed.reason.is_none());
    }

    #[test]
    fn rejects_malformed_heads() {
        assert!(parse_request_head("GET / HTTP/1.1\r\nBroken\r\n\r\n").is_err());
        assert!(parse_response_head("HTTP/1.1 999 Nope\r\n\r\n").is_err());
        assert!(parse_request_head("GET / HTTP/3\r\n\r\n").is_err());
        assert!(parse_request_head("GET / HTTP/2\r\n:method: GET\r\n\r\n").is_err());
        assert!(parse_request_head("GET / HTTP/1.1\r\n Host: example.com\r\n\r\n").is_err());
    }

    #[test]
    fn serialized_heads_parse_back() {
        let mut headers = HeaderMap::new();
        headers.insert("host", HeaderValue::from_static("example.com"));
        headers.insert("x-test", HeaderValue::from_static("yes"));
        let head = serialize_request_head(
            &Method::GET,
            &"/path?q=1".parse().unwrap(),
            Version::HTTP_11,
            &headers,
            None,
        );
        let parsed = parse_request_head(&head).unwrap();
        assert_eq!(parsed.method, Method::GET);
        assert_eq!(parsed.uri, "/path?q=1");
        assert_eq!(parsed.headers.get("x-test").unwrap(), "yes");
    }

    #[test]
    fn http2_requests_are_shown_with_origin_form_and_authority() {
        let headers = HeaderMap::new();
        let head = serialize_request_head(
            &Method::GET,
            &"https://example.com:8443/path?q=1".parse().unwrap(),
            Version::HTTP_2,
            &headers,
            None,
        );
        assert!(
            head.starts_with("GET /path?q=1 HTTP/2\r\nhost: example.com:8443\r\n"),
            "unexpected head: {head:?}"
        );
        let parsed = parse_request_head(&head).unwrap();
        assert_eq!(parsed.version, Version::HTTP_2);
        assert_eq!(parsed.uri, "/path?q=1");
        assert_eq!(parsed.headers.get("host").unwrap(), "example.com:8443");
    }

    #[test]
    fn http2_responses_have_no_reason_phrase() {
        let headers = HeaderMap::new();
        let head = serialize_response_head(
            Version::HTTP_2,
            StatusCode::NO_CONTENT,
            "No Content",
            &headers,
        );
        assert_eq!(head, "HTTP/2 204\r\n\r\n");
    }
}
