//! Copy a captured HTTP request as a runnable POSIX-shell `curl` command.

use base64::Engine as _;

use super::util::request_url;
use crate::history::HistoryItem;

const PYTHON_BASE64_DECODER: &str =
    "import base64,sys;sys.stdout.buffer.write(base64.b64decode(sys.stdin.buffer.read()))";
const BODY_HEREDOC: &str = "PERSEID_CURL_BODY";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CurlError {
    ConnectTunnel,
    IncompleteBody,
    TruncatedBody,
    InvalidUrl,
}

impl CurlError {
    pub(super) fn message(self) -> &'static str {
        match self {
            Self::ConnectTunnel => "CONNECT tunnels are not HTTP requests",
            Self::IncompleteBody => "The request body has not finished capturing",
            Self::TruncatedBody => "The request body was truncated by the capture limit",
            Self::InvalidUrl => "The captured request does not have a valid HTTP or HTTPS URL",
        }
    }
}

pub(super) fn validate(item: &HistoryItem) -> Result<(), CurlError> {
    if item.is_tunnel() {
        return Err(CurlError::ConnectTunnel);
    }
    if !item.request_body.is_complete() {
        return Err(CurlError::IncompleteBody);
    }
    if item.request_body.is_truncated() {
        return Err(CurlError::TruncatedBody);
    }

    let url = request_url(item);
    let uri = url
        .parse::<http::Uri>()
        .map_err(|_| CurlError::InvalidUrl)?;
    if !matches!(uri.scheme_str(), Some("http" | "https")) || uri.authority().is_none() {
        return Err(CurlError::InvalidUrl);
    }
    Ok(())
}

pub(super) fn generate(item: &HistoryItem) -> Result<String, CurlError> {
    validate(item)?;

    let body = item.request_body.data();
    let has_body = !item.request_body.is_empty();
    let url = request_url(item);
    let mut options = vec![
        option("--request", &item.request.method),
        option("--url", &url),
    ];
    if let Some(version_option) = version_option(&item.request.version) {
        options.push(version_option.to_string());
    }
    options.extend(item.request.headers.iter().filter_map(|(name, value)| {
        // The copied body can differ from the originally declared length (for
        // example after editing). Let curl frame its input rather than replaying
        // stale HTTP/1 transfer metadata.
        if name.eq_ignore_ascii_case("transfer-encoding")
            || (has_body && name.eq_ignore_ascii_case("content-length"))
        {
            None
        } else {
            Some(option("--header", &format!("{name}: {value}")))
        }
    }));

    // `--data-binary` otherwise adds an implicit form content type that may
    // not have appeared in the captured request.
    if has_body
        && !item
            .request
            .headers
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("content-type"))
    {
        options.push(option("--header", "Content-Type:"));
    }
    if has_body {
        options.push(option("--data-binary", "@-"));
    }

    let mut command = if has_body {
        format!(
            "python3 -c {} <<'{BODY_HEREDOC}' | curl \\\n",
            shell_quote(PYTHON_BASE64_DECODER)
        )
    } else {
        "curl \\\n".to_string()
    };
    for (index, option) in options.iter().enumerate() {
        command.push_str("  ");
        command.push_str(option);
        if index + 1 < options.len() {
            command.push_str(" \\\n");
        } else {
            command.push('\n');
        }
    }

    if has_body {
        command.push_str(&base64::engine::general_purpose::STANDARD.encode(body));
        command.push('\n');
        command.push_str(BODY_HEREDOC);
        command.push('\n');
    }
    Ok(command)
}

fn version_option(version: &str) -> Option<&'static str> {
    match version {
        "HTTP/0.9" => Some("--http0.9"),
        "HTTP/1.0" => Some("--http1.0"),
        "HTTP/1.1" => Some("--http1.1"),
        "HTTP/2" | "HTTP/2.0" => Some("--http2"),
        "HTTP/3" => Some("--http3"),
        _ => None,
    }
}

fn option(name: &str, value: &str) -> String {
    format!("{name} {}", shell_quote(value))
}

/// POSIX single-quote escaping. Literal newlines stay within the quoted word;
/// embedded apostrophes close the quote, emit one quoted apostrophe, and reopen.
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime};

    use crate::history::{HistoryItem, Phase};
    use crate::proxy::{CapturedBody, ExchangeResult, RequestInfo};

    use super::*;

    fn item(method: &str, uri: &str, scheme: &str, host: &str) -> HistoryItem {
        HistoryItem {
            id: 1,
            number: 1,
            timestamp: SystemTime::now(),
            request: RequestInfo {
                method: method.to_string(),
                uri: uri.to_string(),
                scheme: scheme.to_string(),
                host: host.to_string(),
                path: uri.to_string(),
                version: "HTTP/1.1".to_string(),
                headers: Vec::new(),
            },
            request_body: CapturedBody::from_bytes(Vec::new()),
            response: None,
            phase: Phase::Finished {
                duration: Duration::ZERO,
                result: ExchangeResult::Dropped,
            },
        }
    }

    #[test]
    fn methods_urls_versions_and_no_proxy_are_explicit() {
        for method in ["GET", "POST", "PUT", "PATCH", "DELETE"] {
            let request = item(
                method,
                "/search?q=one&tag=two",
                "https",
                "example.test:8443",
            );
            let command = generate(&request).expect("request should convert");
            assert!(command.contains(&format!("--request '{}'", method)));
            assert!(command.contains("--url 'https://example.test:8443/search?q=one&tag=two'"));
            assert!(command.contains("--http1.1"));
            assert!(!command.contains("--proxy"));
            assert!(!command.contains("--data-binary"));
        }
    }

    #[test]
    fn shell_quotes_single_quotes_and_literal_newlines() {
        assert_eq!(
            shell_quote("one 'quoted'\ntwo"),
            "'one '\"'\"'quoted'\"'\"'\ntwo'"
        );
    }

    #[test]
    fn duplicate_headers_are_emitted_in_captured_order() {
        let mut request = item("GET", "/", "http", "example.test");
        request.request.headers = vec![
            ("x-repeat".into(), "first".into()),
            ("x-repeat".into(), "second".into()),
        ];
        let command = generate(&request).expect("request should convert");
        let first = command.find("--header 'x-repeat: first'").unwrap();
        let second = command.find("--header 'x-repeat: second'").unwrap();
        assert!(first < second);
        assert_eq!(command.matches("--header 'x-repeat:").count(), 2);
    }

    #[test]
    fn curl_regenerates_body_framing_and_suppresses_an_implicit_content_type() {
        let mut request = item("POST", "/upload", "http", "example.test");
        request.request_body = CapturedBody::from_bytes(b"body".to_vec());
        request.request.headers = vec![
            ("content-length".into(), "999".into()),
            ("transfer-encoding".into(), "chunked".into()),
        ];
        let command = generate(&request).expect("request should convert");
        assert!(!command.contains("content-length"));
        assert!(!command.contains("transfer-encoding"));
        assert!(command.contains("--header 'Content-Type:'"));
        assert!(command.contains("--data-binary '@-'"));
    }

    #[test]
    fn arbitrary_body_bytes_are_base64_piped_without_utf8_conversion() {
        let bytes = [0, 0xff, b'\n', b'\'', 0x80];
        let mut request = item("POST", "/upload", "https", "example.test");
        request.request_body = CapturedBody::from_bytes(bytes.to_vec());
        request.request.headers = vec![("content-type".into(), "application/octet-stream".into())];

        let command = generate(&request).expect("request should convert");
        let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
        assert!(command.contains("python3 -c "));
        assert!(command.contains("--data-binary '@-'"));
        assert!(command.contains(&format!("\n{encoded}\n{BODY_HEREDOC}\n")));
        assert!(!command.as_bytes().contains(&0));
        assert!(!command.as_bytes().contains(&0xff));
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(encoded)
                .unwrap(),
            bytes
        );
    }

    #[test]
    fn bodyless_requests_do_not_get_a_data_option_or_python_wrapper() {
        let request = item("POST", "/empty", "http", "example.test");
        let command = generate(&request).expect("empty body should convert");
        assert!(command.contains("--request 'POST'"));
        assert!(!command.contains("--data-binary"));
        assert!(!command.contains("python3"));
    }

    #[cfg(unix)]
    #[test]
    fn generated_commands_parse_as_posix_shell() {
        let mut request = item("POST", "/upload", "https", "example.test");
        request.request_body = CapturedBody::from_bytes(vec![0, 0xff, b'\n']);
        request.request.headers = vec![("x-test".into(), "one 'quoted'\ntwo".into())];
        let command = generate(&request).expect("request should convert");
        let output = std::process::Command::new("sh")
            .args(["-n", "-c", &command])
            .output()
            .expect("POSIX shell should be installed on Unix test runners");
        assert!(
            output.status.success(),
            "generated command did not parse: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn http2_is_explicit() {
        let mut request = item("GET", "/", "https", "example.test");
        request.request.version = "HTTP/2".into();
        assert!(generate(&request).unwrap().contains("--http2"));
    }

    #[test]
    fn incomplete_and_truncated_bodies_are_refused() {
        let mut request = item("POST", "/", "https", "example.test");
        request.request_body = CapturedBody::default();
        assert_eq!(validate(&request), Err(CurlError::IncompleteBody));

        request.request_body = CapturedBody::from_bytes_limited(b"payload", 3);
        assert_eq!(validate(&request), Err(CurlError::TruncatedBody));
    }
}
