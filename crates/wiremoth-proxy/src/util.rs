//! Small helpers shared by the forwarding and tunneling code.

use std::convert::Infallible;

use bytes::Bytes;
use http::header::{CONNECTION, CONTENT_LENGTH, CONTENT_TYPE};
use http::{HeaderMap, HeaderName, HeaderValue, Response, StatusCode, Version};
use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt, Full};
use hyper::body::Body;

use crate::events::HeaderList;

pub(crate) type BoxError = Box<dyn std::error::Error + Send + Sync>;
pub(crate) type ProxyBody = BoxBody<Bytes, BoxError>;

/// `"HTTP/1.1"` etc. for display.
pub(crate) fn version_str(version: Version) -> String {
    format!("{version:?}")
}

/// Snapshot headers for the capture model, preserving order.
pub(crate) fn header_list(headers: &HeaderMap) -> HeaderList {
    headers
        .iter()
        .map(|(name, value)| {
            (
                name.as_str().to_string(),
                String::from_utf8_lossy(value.as_bytes()).into_owned(),
            )
        })
        .collect()
}

/// Hop-by-hop headers that must not be forwarded by a proxy
/// (RFC 9110 §7.6.1 plus the proxy-auth pair).
fn is_hop_by_hop(name: &HeaderName) -> bool {
    matches!(
        name.as_str(),
        "connection"
            | "proxy-connection"
            | "keep-alive"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
            | "proxy-authenticate"
            | "proxy-authorization"
    )
}

/// Headers to copy when forwarding a message in either direction.
///
/// Drops hop-by-hop headers, anything named by `Connection`, and
/// `Content-Length`/framing headers: the HTTP stack re-frames the body when it
/// serializes the message, and stale framing headers would risk smuggling.
pub(crate) fn forwarded_headers(headers: &HeaderMap) -> Vec<(HeaderName, HeaderValue)> {
    let mut connection_named: Vec<HeaderName> = Vec::new();
    for value in headers.get_all(CONNECTION) {
        if let Ok(text) = value.to_str() {
            for token in text.split(',') {
                let token = token.trim();
                if token.is_empty() {
                    continue;
                }
                if let Ok(name) = HeaderName::from_bytes(token.as_bytes())
                    && name != CONNECTION
                {
                    connection_named.push(name);
                }
            }
        }
    }

    headers
        .iter()
        .filter(|(name, _)| {
            *name != CONTENT_LENGTH
                && !is_hop_by_hop(name)
                && !connection_named.iter().any(|named| named == *name)
        })
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect()
}

/// Host for display: drop the default port for `http` targets.
pub(crate) fn display_host(authority: &http::uri::Authority) -> String {
    match (authority.host(), authority.port_u16()) {
        (host, Some(80)) | (host, None) => host.to_string(),
        (host, Some(port)) => format!("{host}:{port}"),
    }
}

fn boxed(body: impl Body<Data = Bytes, Error = Infallible> + Send + Sync + 'static) -> ProxyBody {
    body.map_err(|never| -> BoxError { match never {} }).boxed()
}

/// Lets any concrete body be type-erased into the response body every handler
/// returns.
pub(crate) trait IntoProxyBody: Body<Data = Bytes> + Send + Sync + 'static {
    fn boxed_response(self) -> ProxyBody;
}

impl<B> IntoProxyBody for B
where
    B: Body<Data = Bytes> + Send + Sync + 'static,
    B::Error: std::error::Error + Send + Sync + 'static,
{
    fn boxed_response(self) -> ProxyBody {
        self.map_err(|err| -> BoxError { Box::new(err) }).boxed()
    }
}

/// An empty body for responses that must not carry one (e.g. CONNECT 200).
pub(crate) fn empty_body() -> ProxyBody {
    boxed(http_body_util::Empty::<Bytes>::new())
}

/// A small plain-text body, used for proxy-generated error pages.
pub(crate) fn text_body(text: String) -> ProxyBody {
    boxed(Full::new(Bytes::from(text)))
}

/// Build a proxy-generated error response (400/501/502/504 pages).
pub(crate) fn text_response(status: StatusCode, message: &str) -> Response<ProxyBody> {
    Response::builder()
        .status(status)
        .header(CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(text_body(format!("wiremoth: {message}\n")))
        .expect("valid generated response")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_hop_by_hop_and_connection_named_headers() {
        let mut headers = HeaderMap::new();
        headers.insert("proxy-connection", "keep-alive".parse().unwrap());
        headers.insert(CONNECTION, "x-private, keep-alive".parse().unwrap());
        headers.insert("x-private", "secret".parse().unwrap());
        headers.insert("x-keep", "visible".parse().unwrap());
        headers.insert(CONTENT_LENGTH, "12".parse().unwrap());

        let forwarded = forwarded_headers(&headers);
        let names: Vec<&str> = forwarded.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, vec!["x-keep"]);
    }

    #[test]
    fn display_host_hides_default_port() {
        let authority: http::uri::Authority = "example.com:80".parse().unwrap();
        assert_eq!(display_host(&authority), "example.com");
        let authority: http::uri::Authority = "example.com:8080".parse().unwrap();
        assert_eq!(display_host(&authority), "example.com:8080");
    }
}
