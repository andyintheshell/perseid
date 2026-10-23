//! Plain-HTTP forwarding: rewrite the request for the origin server, stream it
//! upstream while capturing, stream the response back while capturing.
//!
//! Interception and match/replace operate on a message-level copy before it is
//! sent. Bodies up to the interception limit are buffered so they can be
//! edited; larger or streaming bodies still park for header edits, but their
//! body bytes pass through untouched.

mod error;
mod reject;
mod response;
mod target;
mod websocket;

use self::error::classify;
use self::reject::{finish, finish_failed, reject_dropped_request, reject_request};
use self::response::relay;
use self::target::{
    Target, buffer_body, build_request_info, captured_buffered, fits_intercept_limit,
    resolve_target,
};
use self::websocket::{forward_websocket, is_websocket_upgrade};

use std::sync::Arc;
use std::time::{Instant, SystemTime};

use bytes::Bytes;
use http::header::{
    ACCEPT_ENCODING, CONNECTION, CONTENT_LENGTH, HOST, SEC_WEBSOCKET_EXTENSIONS,
    SEC_WEBSOCKET_PROTOCOL, TRANSFER_ENCODING, UPGRADE,
};
use http::{HeaderMap, HeaderValue, Method, Request, Response, StatusCode, Uri, Version};
use http_body_util::{BodyExt, Full};
use hyper::body::{Body, Incoming};
use hyper::ext::ReasonPhrase;
use hyper::upgrade::OnUpgrade;
use hyper_util::client::legacy::Error as ClientError;
use hyper_util::rt::TokioIo;
use log::{debug, warn};
use tokio::sync::oneshot;

use crate::proxy::capture::{BodyBuffer, EndReason, TeeBody, new_buffer, snapshot};
use crate::proxy::events::{
    CapturedBody, ExchangeEnd, ExchangeResult, ProxyEvent, RequestInfo, ResponseHead,
    WebSocketConnectionInfo,
};
use crate::proxy::intercept::{InterceptDecision, InterceptItem};
use crate::proxy::message::{
    ParsedResponseHead, editor_reason, parse_request_head, parse_response_head,
    serialize_request_head, serialize_response_head,
};
use crate::proxy::policy::PolicySnapshot;
use crate::proxy::server::Ctx;
use crate::proxy::util::{
    IntoProxyBody, ProxyBody, display_host, empty_body, forwarded_headers, header_list,
    sanitized_accept_encoding, text_response, version_str,
};

/// The identity and timing of one exchange.
#[derive(Clone, Copy, Debug)]
struct ExchangeStart {
    id: u64,
    timestamp: SystemTime,
    started: Instant,
}

/// Everything carried from request parsing through response completion.
struct Exchange {
    start: ExchangeStart,
    capture: bool,
    policy: PolicySnapshot,
    request: RequestInfo,
    /// Protocol the client used, independent of the protocol negotiated with
    /// the origin. Responses must be framed for this side of the proxy.
    downstream_version: Version,
}

/// The transport a request arrived on. Origin-form targets are resolved
/// against the originating scheme, so a decrypted request reaches the origin
/// over TLS while a plain proxy request stays plain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ConnScheme {
    Http,
    Https,
}

impl ConnScheme {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Http => "http",
            Self::Https => "https",
        }
    }
}

/// Where the body bytes will come from once the message is sent.
enum BodySource {
    Incoming(Incoming),
    Buffered(Vec<u8>),
}

impl BodySource {
    fn buffered(&self) -> Option<&[u8]> {
        match self {
            Self::Incoming(_) => None,
            Self::Buffered(bytes) => Some(bytes),
        }
    }

    fn buffered_mut(&mut self) -> Option<&mut Vec<u8>> {
        match self {
            Self::Incoming(_) => None,
            Self::Buffered(bytes) => Some(bytes),
        }
    }
}

pub(crate) async fn handle(ctx: Arc<Ctx>, req: Request<Incoming>) -> Response<ProxyBody> {
    handle_with(ctx, req, ConnScheme::Http, None).await
}

/// Handle a request received inside an intercepted CONNECT tunnel.
///
/// `authority` is the CONNECT target; it backs up the `Host` header for
/// clients that omit one.
pub(crate) async fn handle_intercepted(
    ctx: Arc<Ctx>,
    req: Request<Incoming>,
    authority: String,
) -> Response<ProxyBody> {
    handle_with(ctx, req, ConnScheme::Https, Some(authority)).await
}

async fn handle_with(
    ctx: Arc<Ctx>,
    req: Request<Incoming>,
    conn_scheme: ConnScheme,
    fallback_authority: Option<String>,
) -> Response<ProxyBody> {
    let start = ExchangeStart {
        id: ctx.next_id(),
        timestamp: SystemTime::now(),
        started: Instant::now(),
    };
    let capture = ctx.capture_enabled();
    let policy = ctx.policy();
    let (mut parts, body) = req.into_parts();
    // Hyper stores the pending upgrade future in the extensions. Pull it out
    // before request editing so a 101 response can hand the live socket to the
    // WebSocket relay task.
    let client_upgrade = parts.extensions.remove::<OnUpgrade>();
    let downstream_version = parts.version;
    let raw_uri = parts.uri.clone();
    // HTTP/2 head editing uses an origin-form request line, so a later
    // resolution of the edited target cannot rely on the absolute URI alone.
    // Keep the scheme the client originally asked for; otherwise an h2c
    // request for `https://host/path` would be re-resolved as cleartext.
    let default_scheme = match raw_uri.scheme_str() {
        Some("https") => ConnScheme::Https,
        Some("http") => ConnScheme::Http,
        _ => conn_scheme,
    };

    let target = match resolve_target(
        &parts.uri,
        &parts.headers,
        default_scheme,
        fallback_authority.as_deref(),
    ) {
        Ok(target) => target,
        Err(err) => {
            debug!("rejecting request: {}", err.message);
            let request = RequestInfo {
                method: parts.method.to_string(),
                uri: raw_uri.to_string(),
                scheme: default_scheme.as_str().to_string(),
                host: parts
                    .headers
                    .get(HOST)
                    .and_then(|value| value.to_str().ok())
                    .unwrap_or_default()
                    .to_string(),
                path: raw_uri.path().to_string(),
                version: version_str(parts.version),
                headers: header_list(&parts.headers),
            };
            if capture {
                ctx.emit(ProxyEvent::Started {
                    id: start.id,
                    timestamp: start.timestamp,
                    request,
                });
                finish_failed(&ctx, start.id, start.started, None, err.message.clone());
            }
            return text_response(err.status, &err.message);
        }
    };

    let mut exchange = Exchange {
        start,
        capture,
        policy,
        request: build_request_info(&parts, &target, &raw_uri, default_scheme),
        downstream_version,
    };
    if exchange.policy.scope_enforced() && !exchange.policy.in_scope(&exchange.request.host) {
        ctx.block_by_scope();
        return reject_request(
            &ctx,
            exchange.start,
            exchange.capture,
            exchange.request,
            CapturedBody::default(),
            StatusCode::FORBIDDEN,
            "target is outside the configured scope".to_string(),
        );
    }

    let mut body_source = BodySource::Incoming(body);
    let needs_buffer =
        exchange.policy.intercept_requests() || exchange.policy.has_request_body_rules();
    if needs_buffer {
        let can_buffer = fits_intercept_limit(&body_source, &parts.headers, ctx.intercept_limit);
        if can_buffer
            && let Err(message) =
                buffer_body(&mut body_source, ctx.intercept_limit, "request").await
        {
            return reject_request(
                &ctx,
                exchange.start,
                exchange.capture,
                exchange.request,
                captured_buffered(&body_source, ctx.capture_limit),
                StatusCode::BAD_REQUEST,
                message,
            );
        }
    }

    if exchange.policy.intercept_requests() {
        let head = serialize_request_head(
            &parts.method,
            &parts.uri,
            parts.version,
            &parts.headers,
            fallback_authority.as_deref(),
        );
        let body = body_source.buffered().map(<[u8]>::to_vec);
        let (decision_tx, decision_rx) = oneshot::channel();
        let item = InterceptItem::new_request(
            exchange.start.id,
            exchange.request.clone(),
            head,
            body,
            decision_tx,
        );

        match ctx.ask_interceptor(item, decision_rx).await {
            InterceptDecision::Forward { mut head, mut body } => {
                if let Some(edited) = body.as_mut() {
                    exchange.policy.apply_request(&mut head, Some(edited));
                } else if let Some(original) = body_source.buffered_mut() {
                    exchange.policy.apply_request(&mut head, Some(original));
                } else {
                    exchange.policy.apply_request(&mut head, None);
                }
                let parsed = match parse_request_head(&head) {
                    Ok(parsed) => parsed,
                    Err(message) => {
                        return reject_request(
                            &ctx,
                            exchange.start,
                            exchange.capture,
                            exchange.request,
                            captured_buffered(&body_source, ctx.capture_limit),
                            StatusCode::BAD_REQUEST,
                            message,
                        );
                    }
                };
                if parsed.method == Method::CONNECT {
                    return reject_request(
                        &ctx,
                        exchange.start,
                        exchange.capture,
                        exchange.request,
                        captured_buffered(&body_source, ctx.capture_limit),
                        StatusCode::BAD_REQUEST,
                        "CONNECT cannot be produced by request interception".to_string(),
                    );
                }
                if let Some(edited) = body {
                    body_source = BodySource::Buffered(edited);
                }
                parts.method = parsed.method;
                parts.uri = parsed.uri;
                parts.version = parsed.version;
                parts.headers = parsed.headers;
            }
            InterceptDecision::Drop => {
                return reject_dropped_request(
                    &ctx,
                    exchange.start,
                    exchange.capture,
                    exchange.request,
                    captured_buffered(&body_source, ctx.capture_limit),
                    StatusCode::FORBIDDEN,
                    "request dropped in the interceptor".to_string(),
                );
            }
            InterceptDecision::Abort => {
                return reject_request(
                    &ctx,
                    exchange.start,
                    exchange.capture,
                    exchange.request,
                    captured_buffered(&body_source, ctx.capture_limit),
                    StatusCode::BAD_GATEWAY,
                    "the interceptor is no longer available".to_string(),
                );
            }
        }
    } else if exchange.policy.has_request_rules() {
        // Only round-trip the head when a rule can change it. Serializing an
        // untouched head would lossily rewrite non-UTF-8 header values.
        if exchange.policy.has_request_head_rules() {
            let mut head = serialize_request_head(
                &parts.method,
                &parts.uri,
                parts.version,
                &parts.headers,
                fallback_authority.as_deref(),
            );
            exchange.policy.apply_request_head(&mut head);
            let parsed = match parse_request_head(&head) {
                Ok(parsed) => parsed,
                Err(message) => {
                    return reject_request(
                        &ctx,
                        exchange.start,
                        exchange.capture,
                        exchange.request,
                        captured_buffered(&body_source, ctx.capture_limit),
                        StatusCode::BAD_REQUEST,
                        message,
                    );
                }
            };
            if parsed.method == Method::CONNECT {
                return reject_request(
                    &ctx,
                    exchange.start,
                    exchange.capture,
                    exchange.request,
                    captured_buffered(&body_source, ctx.capture_limit),
                    StatusCode::BAD_REQUEST,
                    "CONNECT cannot be produced by a rewrite rule".to_string(),
                );
            }
            parts.method = parsed.method;
            parts.uri = parsed.uri;
            parts.version = parsed.version;
            parts.headers = parsed.headers;
        }
        if let Some(body) = body_source.buffered_mut() {
            exchange.policy.apply_request_body(body);
        }
    }

    let target = match resolve_target(
        &parts.uri,
        &parts.headers,
        default_scheme,
        fallback_authority.as_deref(),
    ) {
        Ok(target) => target,
        Err(err) => {
            return reject_request(
                &ctx,
                exchange.start,
                exchange.capture,
                exchange.request,
                captured_buffered(&body_source, ctx.capture_limit),
                err.status,
                err.message,
            );
        }
    };
    exchange.request = build_request_info(&parts, &target, &parts.uri, default_scheme);

    if exchange.policy.scope_enforced() && !exchange.policy.in_scope(&exchange.request.host) {
        ctx.block_by_scope();
        return reject_request(
            &ctx,
            exchange.start,
            exchange.capture,
            exchange.request,
            captured_buffered(&body_source, ctx.capture_limit),
            StatusCode::FORBIDDEN,
            "target is outside the configured scope".to_string(),
        );
    }

    let websocket = is_websocket_upgrade(&parts);
    if exchange.capture && !websocket {
        ctx.emit(ProxyEvent::Started {
            id: exchange.start.id,
            timestamp: exchange.start.timestamp,
            request: exchange.request.clone(),
        });
    }

    forward(
        ctx,
        exchange,
        target,
        parts,
        body_source,
        websocket,
        client_upgrade,
    )
    .await
}

async fn forward(
    ctx: Arc<Ctx>,
    exchange: Exchange,
    target: Target,
    parts: http::request::Parts,
    body_source: BodySource,
    websocket: bool,
    client_upgrade: Option<OnUpgrade>,
) -> Response<ProxyBody> {
    if websocket {
        return forward_websocket(ctx, exchange, target, parts, body_source, client_upgrade).await;
    }

    let Exchange {
        start,
        capture,
        policy,
        request,
        downstream_version,
    } = exchange;

    let request_buffer = new_buffer();
    let request_limit = if capture { ctx.capture_limit } else { 0 };
    let body = if capture {
        match body_source {
            BodySource::Incoming(body) => {
                TeeBody::new(body, request_limit, request_buffer.clone(), None).boxed_response()
            }
            BodySource::Buffered(bytes) => TeeBody::new(
                Full::new(Bytes::from(bytes)),
                request_limit,
                request_buffer.clone(),
                None,
            )
            .boxed_response(),
        }
    } else {
        match body_source {
            BodySource::Incoming(body) => body.boxed_response(),
            BodySource::Buffered(bytes) => Full::new(Bytes::from(bytes)).boxed_response(),
        }
    };

    let mut upstream = Request::new(body);
    *upstream.method_mut() = parts.method;
    *upstream.uri_mut() = target.uri;
    // Keep HTTP/1.1 as the request version for the legacy client. If the
    // origin negotiates h2 over ALPN, hyper-util switches the pooled
    // connection to HTTP/2 and derives the pseudo-headers from the URI and
    // headers; if it stays on h1, the request is already in the right form.
    *upstream.version_mut() = Version::HTTP_11;
    let mut headers = HeaderMap::new();
    for (name, value) in forwarded_headers(&parts.headers) {
        // Let hyper derive Host for HTTP/1.1 and :authority for HTTP/2 from
        // the target URI. An explicit Host header alongside HTTP/2 :authority
        // is invalid and strict origins such as Google reject the request.
        if name == ACCEPT_ENCODING || name == HOST {
            continue;
        }
        headers.append(name, value);
    }
    if let Some(value) = sanitized_accept_encoding(&parts.headers) {
        headers.insert(ACCEPT_ENCODING, value);
    }
    // `Content-Length` was stripped: the body's size hint drives framing.
    debug_assert!(headers.get(CONTENT_LENGTH).is_none());
    *upstream.headers_mut() = headers;

    match ctx.client.request(upstream).await {
        Ok(response) => {
            relay(
                ctx,
                Exchange {
                    start,
                    capture,
                    policy,
                    request,
                    downstream_version,
                },
                request_buffer,
                response,
            )
            .await
        }
        Err(err) => {
            let (status, message) = classify(&err);
            if capture {
                finish_failed(
                    &ctx,
                    start.id,
                    start.started,
                    Some(snapshot(&request_buffer)),
                    message.clone(),
                );
            }
            text_response(status, &message)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fmt;

    use super::error::error_chain;
    use super::target::content_length;
    use super::websocket::{header_has_token, websocket_upstream_headers};
    use super::*;

    fn request_parts(
        method: Method,
        version: Version,
        headers: &[(&str, &'static str)],
    ) -> http::request::Parts {
        let mut builder = Request::builder().method(method).version(version);
        for (name, value) in headers {
            builder = builder.header(*name, *value);
        }
        builder.body(()).expect("request builds").into_parts().0
    }

    #[test]
    fn websocket_upgrade_requires_get_http11_and_both_tokens() {
        let upgrading = [
            ("connection", "keep-alive, Upgrade"),
            ("upgrade", "websocket"),
        ];
        assert!(is_websocket_upgrade(&request_parts(
            Method::GET,
            Version::HTTP_11,
            &upgrading
        )));
        assert!(!is_websocket_upgrade(&request_parts(
            Method::POST,
            Version::HTTP_11,
            &upgrading
        )));
        assert!(!is_websocket_upgrade(&request_parts(
            Method::GET,
            Version::HTTP_2,
            &upgrading
        )));
        let no_upgrade = [("connection", "keep-alive")];
        assert!(!is_websocket_upgrade(&request_parts(
            Method::GET,
            Version::HTTP_11,
            &no_upgrade
        )));
    }

    #[test]
    fn header_token_matching_ignores_case_and_spacing() {
        let mut headers = HeaderMap::new();
        headers.insert(
            CONNECTION,
            HeaderValue::from_static("keep-alive,  UpGradE "),
        );
        assert!(header_has_token(&headers, CONNECTION, "upgrade"));
        assert!(!header_has_token(&headers, CONNECTION, "close"));
        assert!(!header_has_token(&headers, UPGRADE, "websocket"));
    }

    #[test]
    fn header_token_scans_every_value_of_a_repeated_header() {
        let mut headers = HeaderMap::new();
        headers.append(CONNECTION, HeaderValue::from_static("keep-alive"));
        headers.append(CONNECTION, HeaderValue::from_static("upgrade"));
        assert!(header_has_token(&headers, CONNECTION, "upgrade"));
    }

    #[test]
    fn websocket_upstream_headers_drop_hop_by_hop_and_host_headers() {
        let mut headers = HeaderMap::new();
        headers.insert(HOST, HeaderValue::from_static("example.com"));
        headers.insert(CONNECTION, HeaderValue::from_static("keep-alive, Upgrade"));
        headers.insert(UPGRADE, HeaderValue::from_static("websocket"));
        headers.insert(SEC_WEBSOCKET_PROTOCOL, HeaderValue::from_static("chat"));
        headers.insert(
            SEC_WEBSOCKET_EXTENSIONS,
            HeaderValue::from_static("permessage-deflate"),
        );
        headers.insert(CONTENT_LENGTH, HeaderValue::from_static("12"));
        headers.insert(
            "sec-websocket-key",
            HeaderValue::from_static("dGhlIHNhbXBsZSBub25jZQ=="),
        );
        headers.insert("user-agent", HeaderValue::from_static("test"));

        let upstream = websocket_upstream_headers(&headers);
        assert_eq!(upstream.get(CONNECTION).expect("connection"), "Upgrade");
        assert_eq!(upstream.get(UPGRADE).expect("upgrade"), "websocket");
        assert!(upstream.get(HOST).is_none());
        assert!(upstream.get(SEC_WEBSOCKET_EXTENSIONS).is_none());
        assert!(upstream.get(CONTENT_LENGTH).is_none());
        assert_eq!(
            upstream.get(SEC_WEBSOCKET_PROTOCOL).expect("protocol"),
            "chat"
        );
        assert_eq!(
            upstream.get("sec-websocket-key").expect("key"),
            "dGhlIHNhbXBsZSBub25jZQ=="
        );
        assert_eq!(upstream.get("user-agent").expect("user-agent"), "test");
    }

    #[test]
    fn resolve_target_accepts_absolute_form_targets() {
        let uri: Uri = "http://example.com:8080/a?b=c".parse().expect("uri");
        let target =
            resolve_target(&uri, &HeaderMap::new(), ConnScheme::Http, None).expect("absolute form");
        assert_eq!(target.uri, uri);
        assert_eq!(target.authority.as_str(), "example.com:8080");
    }

    #[test]
    fn resolve_target_rejects_unsupported_absolute_schemes() {
        let uri: Uri = "ftp://example.com/file".parse().expect("uri");
        let err = resolve_target(&uri, &HeaderMap::new(), ConnScheme::Http, None)
            .expect_err("scheme is not proxied");
        assert_eq!(err.status, StatusCode::NOT_IMPLEMENTED);
        assert!(err.message.contains("ftp"));
    }

    #[test]
    fn resolve_target_builds_origin_form_targets_from_the_host_header() {
        let uri: Uri = "/a/b?c=d".parse().expect("uri");
        let mut headers = HeaderMap::new();
        headers.insert(HOST, HeaderValue::from_static("example.com:8443"));
        let target = resolve_target(&uri, &headers, ConnScheme::Https, None).expect("origin form");
        assert_eq!(target.uri.to_string(), "https://example.com:8443/a/b?c=d");
        assert_eq!(target.authority.as_str(), "example.com:8443");
    }

    #[test]
    fn resolve_target_resolves_origin_form_against_the_connection_scheme() {
        let uri: Uri = "/a".parse().expect("uri");
        let mut headers = HeaderMap::new();
        headers.insert(HOST, HeaderValue::from_static("example.com"));
        let target = resolve_target(&uri, &headers, ConnScheme::Http, None).expect("origin form");
        assert_eq!(target.uri.to_string(), "http://example.com/a");
    }

    #[test]
    fn resolve_target_uses_the_connect_authority_when_host_is_missing() {
        let uri: Uri = "/".parse().expect("uri");
        let target = resolve_target(
            &uri,
            &HeaderMap::new(),
            ConnScheme::Https,
            Some("example.com:443"),
        )
        .expect("fallback authority");
        assert_eq!(target.uri.to_string(), "https://example.com:443/");
        assert_eq!(target.authority.as_str(), "example.com:443");
    }

    #[test]
    fn resolve_target_rejects_origin_form_without_a_host() {
        let uri: Uri = "/a".parse().expect("uri");
        let err = resolve_target(&uri, &HeaderMap::new(), ConnScheme::Http, None)
            .expect_err("no host is available");
        assert_eq!(err.status, StatusCode::BAD_REQUEST);
    }

    #[test]
    fn resolve_target_rejects_asterisk_and_authority_forms() {
        let uri: Uri = "*".parse().expect("uri");
        let err =
            resolve_target(&uri, &HeaderMap::new(), ConnScheme::Http, None).expect_err("asterisk");
        assert_eq!(err.status, StatusCode::BAD_REQUEST);
        assert!(err.message.contains("asterisk"));

        let uri: Uri = "example.com:443".parse().expect("uri");
        let err = resolve_target(&uri, &HeaderMap::new(), ConnScheme::Http, None)
            .expect_err("authority form");
        assert_eq!(err.status, StatusCode::BAD_REQUEST);
    }

    #[test]
    fn content_length_parses_valid_values_only() {
        let mut headers = HeaderMap::new();
        assert_eq!(content_length(&headers), None);
        headers.insert(CONTENT_LENGTH, HeaderValue::from_static("42"));
        assert_eq!(content_length(&headers), Some(42));
        headers.insert(CONTENT_LENGTH, HeaderValue::from_static("4 2"));
        assert_eq!(content_length(&headers), None);
        headers.insert(CONTENT_LENGTH, HeaderValue::from_static("-1"));
        assert_eq!(content_length(&headers), None);
    }

    #[test]
    fn buffered_bodies_are_always_ready_for_interception() {
        // A transfer-coded message cannot be size-checked while streaming, but
        // an already-buffered body needs no such check.
        let mut headers = HeaderMap::new();
        headers.insert(TRANSFER_ENCODING, HeaderValue::from_static("chunked"));
        let source = BodySource::Buffered(b"body".to_vec());
        assert!(fits_intercept_limit(&source, &headers, 0));
    }

    /// An error with an optional cause, standing in for hyper-util's layered
    /// client errors.
    #[derive(Debug)]
    struct ChainedError {
        message: &'static str,
        source: Option<Box<ChainedError>>,
    }

    impl fmt::Display for ChainedError {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str(self.message)
        }
    }

    impl std::error::Error for ChainedError {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            self.source
                .as_deref()
                .map(|cause| cause as &(dyn std::error::Error + 'static))
        }
    }

    #[test]
    fn error_chain_flattens_every_source() {
        let plain = ChainedError {
            message: "tls failure",
            source: None,
        };
        assert_eq!(error_chain(&plain), "tls failure");

        let layered = ChainedError {
            message: "client error (SendRequest)",
            source: Some(Box::new(ChainedError {
                message: "connection",
                source: Some(Box::new(ChainedError {
                    message: "tls failure",
                    source: None,
                })),
            })),
        };
        assert_eq!(
            error_chain(&layered),
            "client error (SendRequest): connection: tls failure"
        );
    }

    #[test]
    fn request_info_keeps_the_wire_uri_and_takes_the_host_from_the_target() {
        let raw: Uri = "http://example.com:8080/a?b=c".parse().expect("uri");
        let request = Request::builder()
            .method(Method::GET)
            .uri(raw.clone())
            .header("host", "example.com:8080")
            .body(())
            .expect("request");
        let (parts, ()) = request.into_parts();
        let target = resolve_target(&raw, &parts.headers, ConnScheme::Http, None).expect("target");
        let info = build_request_info(&parts, &target, &raw, ConnScheme::Http);
        assert_eq!(info.uri, "http://example.com:8080/a?b=c");
        assert_eq!(info.scheme, "http");
        assert_eq!(info.host, "example.com:8080");
        assert_eq!(info.path, "/a?b=c");
        assert_eq!(info.version, "HTTP/1.1");
        assert_eq!(info.method, "GET");
    }

    #[test]
    fn request_info_reports_the_connection_scheme_for_origin_form() {
        let raw: Uri = "/a?b=c".parse().expect("uri");
        let mut headers = HeaderMap::new();
        headers.insert(HOST, HeaderValue::from_static("example.com"));
        let request = Request::builder()
            .method(Method::GET)
            .uri(raw.clone())
            .body(())
            .expect("request");
        let (parts, ()) = request.into_parts();
        let target = resolve_target(&raw, &headers, ConnScheme::Https, None).expect("target");
        let info = build_request_info(&parts, &target, &raw, ConnScheme::Https);
        assert_eq!(info.uri, "/a?b=c");
        assert_eq!(info.scheme, "https");
        assert_eq!(info.host, "example.com");
        assert_eq!(info.path, "/a?b=c");
    }
}
