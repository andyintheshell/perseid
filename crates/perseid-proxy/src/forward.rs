//! Plain-HTTP forwarding: rewrite the request for the origin server, stream it
//! upstream while capturing, stream the response back while capturing.
//!
//! Interception and match/replace operate on a message-level copy before it is
//! sent. Bodies up to the interception limit are buffered so they can be
//! edited; larger or streaming bodies still park for header edits, but their
//! body bytes pass through untouched.

use std::sync::Arc;
use std::time::{Instant, SystemTime};

use bytes::Bytes;
use http::header::{ACCEPT_ENCODING, CONTENT_LENGTH, HOST, TRANSFER_ENCODING};
use http::{HeaderMap, HeaderValue, Method, Request, Response, StatusCode, Uri, Version};
use http_body_util::{BodyExt, Full};
use hyper::body::{Body, Incoming};
use hyper::ext::ReasonPhrase;
use hyper_util::client::legacy::Error as ClientError;
use log::debug;
use tokio::sync::oneshot;

use crate::capture::{BodyBuffer, EndReason, TeeBody, new_buffer, snapshot};
use crate::events::{
    CapturedBody, ExchangeEnd, ExchangeResult, ProxyEvent, RequestInfo, ResponseHead,
};
use crate::intercept::{InterceptDecision, InterceptItem, InterceptRequest, InterceptResponse};
use crate::message::{
    ParsedResponseHead, editor_reason, parse_request_head, parse_response_head,
    serialize_request_head, serialize_response_head,
};
use crate::policy::PolicySnapshot;
use crate::server::Ctx;
use crate::util::{
    IntoProxyBody, ProxyBody, display_host, forwarded_headers, header_list,
    sanitized_accept_encoding, text_response, version_str,
};

/// Cases where the proxy rejects a request before any upstream traffic happens.
struct TargetError {
    status: StatusCode,
    message: String,
}

impl TargetError {
    fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: message.into(),
        }
    }
}

/// A validated upstream target. Keeping the authority alongside the URI avoids
/// later `expect`s on a value that originated in an untrusted request.
struct Target {
    uri: Uri,
    authority: http::uri::Authority,
}

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
        let can_buffer = match &body_source {
            BodySource::Incoming(body) => {
                can_buffer_request(&parts.headers, body, ctx.intercept_limit)
            }
            BodySource::Buffered(_) => true,
        };
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
        let item = InterceptItem::Request(InterceptRequest::new(
            exchange.start.id,
            exchange.request.clone(),
            head,
            body,
            decision_tx,
        ));

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
                return reject_request(
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

    if exchange.capture {
        ctx.emit(ProxyEvent::Started {
            id: exchange.start.id,
            timestamp: exchange.start.timestamp,
            request: exchange.request.clone(),
        });
    }

    forward(ctx, exchange, target, parts, body_source).await
}

async fn forward(
    ctx: Arc<Ctx>,
    exchange: Exchange,
    target: Target,
    parts: http::request::Parts,
    body_source: BodySource,
) -> Response<ProxyBody> {
    let Exchange {
        start,
        capture,
        policy,
        request,
        downstream_version,
    } = exchange;

    let host_header = match display_host(&target.authority).parse::<HeaderValue>() {
        Ok(value) => value,
        Err(_) => {
            let message = "invalid authority in request target".to_string();
            if capture {
                finish_failed(&ctx, start.id, start.started, None, message.clone());
            }
            return text_response(StatusCode::BAD_REQUEST, &message);
        }
    };

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
        if name == ACCEPT_ENCODING {
            continue;
        }
        headers.append(name, value);
    }
    if let Some(value) = sanitized_accept_encoding(&parts.headers) {
        headers.insert(ACCEPT_ENCODING, value);
    }
    // `Content-Length` was stripped: the body's size hint drives framing.
    debug_assert!(headers.get(CONTENT_LENGTH).is_none());
    headers.insert(HOST, host_header);
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

fn finish_failed(
    ctx: &Ctx,
    id: u64,
    started: Instant,
    request_body: Option<CapturedBody>,
    message: String,
) {
    let end = ExchangeEnd {
        duration: started.elapsed(),
        request_body: request_body.unwrap_or_default(),
        result: ExchangeResult::Failed { error: message },
    };
    ctx.emit(ProxyEvent::Finished { id, end });
}

fn reject_request(
    ctx: &Ctx,
    start: ExchangeStart,
    capture: bool,
    request: RequestInfo,
    request_body: CapturedBody,
    status: StatusCode,
    message: String,
) -> Response<ProxyBody> {
    if capture {
        ctx.emit(ProxyEvent::Started {
            id: start.id,
            timestamp: start.timestamp,
            request,
        });
        finish_failed(
            ctx,
            start.id,
            start.started,
            Some(request_body),
            message.clone(),
        );
    }
    text_response(status, &message)
}

/// Stream the upstream response to the client, capturing as it goes. The
/// `Finished` event fires when the last frame is relayed (or earlier if the
/// connection dies), from inside the body adapter.
async fn relay(
    ctx: Arc<Ctx>,
    exchange: Exchange,
    request_buffer: BodyBuffer,
    response: Response<Incoming>,
) -> Response<ProxyBody> {
    let Exchange {
        start,
        capture,
        policy,
        request,
        downstream_version,
    } = exchange;
    let (mut parts, body) = response.into_parts();
    let mut body_source = BodySource::Incoming(body);

    let needs_buffer = policy.intercept_responses() || policy.has_response_body_rules();
    if needs_buffer {
        let can_buffer = match &body_source {
            BodySource::Incoming(body) => {
                can_buffer_response(&parts.headers, body, ctx.intercept_limit)
            }
            BodySource::Buffered(_) => true,
        };
        if can_buffer
            && let Err(message) =
                buffer_body(&mut body_source, ctx.intercept_limit, "response").await
        {
            if capture {
                finish_failed(
                    &ctx,
                    start.id,
                    start.started,
                    Some(snapshot(&request_buffer)),
                    message.clone(),
                );
            }
            return text_response(StatusCode::BAD_GATEWAY, &message);
        }
    }

    if policy.intercept_responses() {
        let reason = editor_reason(parts.status, &parts.extensions);
        let head = serialize_response_head(parts.version, parts.status, &reason, &parts.headers);
        let body = body_source.buffered().map(<[u8]>::to_vec);
        let (decision_tx, decision_rx) = oneshot::channel();
        let item = InterceptItem::Response(InterceptResponse::new(
            start.id,
            request.clone(),
            head,
            body,
            decision_tx,
        ));

        match ctx.ask_interceptor(item, decision_rx).await {
            InterceptDecision::Forward { mut head, mut body } => {
                if let Some(edited) = body.as_mut() {
                    policy.apply_response(&mut head, Some(edited));
                } else if let Some(original) = body_source.buffered_mut() {
                    policy.apply_response(&mut head, Some(original));
                } else {
                    policy.apply_response(&mut head, None);
                }
                let parsed = match parse_response_head(&head) {
                    Ok(parsed) => parsed,
                    Err(message) => {
                        if capture {
                            finish_failed(
                                &ctx,
                                start.id,
                                start.started,
                                Some(snapshot(&request_buffer)),
                                message.clone(),
                            );
                        }
                        return text_response(StatusCode::BAD_GATEWAY, &message);
                    }
                };
                apply_response_head(&mut parts, parsed);
                if let Some(edited) = body {
                    body_source = BodySource::Buffered(edited);
                }
            }
            InterceptDecision::Drop => {
                let message = "response dropped in the interceptor".to_string();
                if capture {
                    finish_failed(
                        &ctx,
                        start.id,
                        start.started,
                        Some(snapshot(&request_buffer)),
                        message.clone(),
                    );
                }
                return text_response(StatusCode::BAD_GATEWAY, &message);
            }
            InterceptDecision::Abort => {
                let message = "the interceptor is no longer available".to_string();
                if capture {
                    finish_failed(
                        &ctx,
                        start.id,
                        start.started,
                        Some(snapshot(&request_buffer)),
                        message.clone(),
                    );
                }
                return text_response(StatusCode::BAD_GATEWAY, &message);
            }
        }
    } else if policy.has_response_rules() {
        if policy.has_response_head_rules() {
            let reason = editor_reason(parts.status, &parts.extensions);
            let mut head =
                serialize_response_head(parts.version, parts.status, &reason, &parts.headers);
            policy.apply_response_head(&mut head);
            let parsed = match parse_response_head(&head) {
                Ok(parsed) => parsed,
                Err(message) => {
                    if capture {
                        finish_failed(
                            &ctx,
                            start.id,
                            start.started,
                            Some(snapshot(&request_buffer)),
                            message.clone(),
                        );
                    }
                    return text_response(StatusCode::BAD_GATEWAY, &message);
                }
            };
            apply_response_head(&mut parts, parsed);
        }
        if let Some(body) = body_source.buffered_mut() {
            policy.apply_response_body(body);
        }
    }

    if capture {
        let head = ResponseHead {
            version: version_str(parts.version),
            status: parts.status.as_u16(),
            reason: parts
                .extensions
                .get::<ReasonPhrase>()
                .map(|phrase| String::from_utf8_lossy(phrase.as_bytes()).into_owned())
                .unwrap_or_default(),
            headers: header_list(&parts.headers),
        };
        ctx.emit(ProxyEvent::ResponseHead { id: start.id, head });
    }

    let body = if capture {
        let response_buffer = new_buffer();
        let completion = {
            let ctx = ctx.clone();
            let request_buffer = request_buffer.clone();
            let response_buffer = response_buffer.clone();
            move |reason: EndReason| {
                let result = match reason {
                    EndReason::Complete => ExchangeResult::Response {
                        body: snapshot(&response_buffer),
                    },
                    EndReason::Failed(err) => ExchangeResult::Failed {
                        error: format!("upstream body error: {err}"),
                    },
                    EndReason::Dropped => ExchangeResult::Failed {
                        error: "connection closed before the response body finished".to_string(),
                    },
                };
                let end = ExchangeEnd {
                    duration: start.started.elapsed(),
                    request_body: snapshot(&request_buffer),
                    result,
                };
                ctx.emit(ProxyEvent::Finished { id: start.id, end });
            }
        };

        match body_source {
            BodySource::Incoming(body) => TeeBody::new(
                body,
                ctx.capture_limit,
                response_buffer,
                Some(Box::new(completion)),
            )
            .boxed_response(),
            BodySource::Buffered(bytes) => TeeBody::new(
                Full::new(Bytes::from(bytes)),
                ctx.capture_limit,
                response_buffer,
                Some(Box::new(completion)),
            )
            .boxed_response(),
        }
    } else {
        match body_source {
            BodySource::Incoming(body) => body.boxed_response(),
            BodySource::Buffered(bytes) => Full::new(Bytes::from(bytes)).boxed_response(),
        }
    };

    let mut out = Response::new(body);
    *out.status_mut() = parts.status;
    // The response head is captured with the origin's protocol, but the
    // connection back to the client may have negotiated a different one.
    *out.version_mut() = downstream_version;
    if let Some(phrase) = parts.extensions.get::<ReasonPhrase>() {
        out.extensions_mut().insert(phrase.clone());
    }
    let mut headers = HeaderMap::new();
    for (name, value) in forwarded_headers(&parts.headers) {
        headers.append(name, value);
    }
    *out.headers_mut() = headers;
    out
}

fn apply_response_head(parts: &mut http::response::Parts, parsed: ParsedResponseHead) {
    parts.version = parsed.version;
    parts.status = parsed.status;
    parts.headers = parsed.headers;
    parts.extensions.remove::<ReasonPhrase>();
    if let Some(reason) = parsed.reason {
        parts.extensions.insert(reason);
    }
}

/// Determine the upstream target for a proxied request.
///
/// Browsers send absolute-form targets (`GET http://host/path`); origin-form
/// targets (`GET /path` plus `Host`) are also accepted for clients that do
/// not implement proxy request syntax. Inside an intercepted TLS tunnel the
/// origin-form path resolves against `https`, and the CONNECT authority is the
/// fallback when the client omits `Host`.
fn resolve_target(
    uri: &Uri,
    headers: &HeaderMap,
    conn_scheme: ConnScheme,
    fallback_authority: Option<&str>,
) -> Result<Target, TargetError> {
    if let Some(scheme) = uri.scheme() {
        match scheme.as_str() {
            "http" | "https" => {}
            other => {
                return Err(TargetError {
                    status: StatusCode::NOT_IMPLEMENTED,
                    message: format!("absolute-form target with scheme `{other}` is not supported"),
                });
            }
        }
        let authority = uri.authority().cloned().ok_or_else(|| {
            TargetError::bad_request("absolute-form target is missing an authority")
        })?;
        return Ok(Target {
            uri: uri.clone(),
            authority,
        });
    }

    if uri.path() == "*" {
        return Err(TargetError::bad_request(
            "asterisk-form targets are not supported",
        ));
    }
    if !uri.path().starts_with('/') {
        return Err(TargetError::bad_request("unsupported request target form"));
    }

    let host = match headers.get(HOST) {
        Some(value) => value
            .to_str()
            .map_err(|_| TargetError::bad_request("Host header is not valid ASCII"))?
            .to_string(),
        None => fallback_authority
            .map(str::to_string)
            .ok_or_else(|| TargetError::bad_request("origin-form request without a Host header"))?,
    };
    let path = uri.path_and_query().map(|pq| pq.as_str()).unwrap_or("/");
    let uri = format!("{}://{host}{path}", conn_scheme.as_str())
        .parse::<Uri>()
        .map_err(|_| TargetError::bad_request("invalid Host header"))?;
    let authority = uri
        .authority()
        .cloned()
        .ok_or_else(|| TargetError::bad_request("invalid Host header"))?;
    Ok(Target { uri, authority })
}

fn build_request_info(
    parts: &http::request::Parts,
    target: &Target,
    raw_uri: &Uri,
    conn_scheme: ConnScheme,
) -> RequestInfo {
    RequestInfo {
        method: parts.method.to_string(),
        uri: raw_uri.to_string(),
        scheme: raw_uri
            .scheme()
            .map(|scheme| scheme.as_str().to_string())
            .unwrap_or_else(|| conn_scheme.as_str().to_string()),
        host: display_host(&target.authority),
        path: target
            .uri
            .path_and_query()
            .map(|pq| pq.as_str().to_string())
            .unwrap_or_else(|| "/".to_string()),
        version: version_str(parts.version),
        headers: header_list(&parts.headers),
    }
}

fn captured_buffered(body: &BodySource, limit: usize) -> CapturedBody {
    body.buffered()
        .map(|bytes| CapturedBody::from_bytes_limited(bytes, limit))
        .unwrap_or_default()
}

fn content_length(headers: &HeaderMap) -> Option<usize> {
    headers.get(CONTENT_LENGTH)?.to_str().ok()?.parse().ok()
}

fn can_buffer_request(headers: &HeaderMap, body: &Incoming, limit: usize) -> bool {
    // A message with transfer coding does not have a trustworthy byte length,
    // even if a Content-Length is also present.
    if headers.contains_key(TRANSFER_ENCODING) {
        return false;
    }
    if let Some(length) = content_length(headers) {
        return length <= limit;
    }
    body.is_end_stream()
}

fn can_buffer_response(headers: &HeaderMap, body: &Incoming, limit: usize) -> bool {
    if headers.contains_key(TRANSFER_ENCODING) {
        return false;
    }
    if let Some(length) = content_length(headers) {
        return length <= limit;
    }
    body.is_end_stream()
}

/// Read and buffer a body that is known to fit within `limit`.
async fn buffer_body(source: &mut BodySource, limit: usize, kind: &str) -> Result<(), String> {
    let current = std::mem::replace(source, BodySource::Buffered(Vec::new()));
    let BodySource::Incoming(body) = current else {
        *source = current;
        return Ok(());
    };
    let collected = body
        .collect()
        .await
        .map_err(|err| format!("could not read the {kind} body: {err}"))?;
    let bytes = collected.to_bytes();
    if bytes.len() > limit {
        return Err(format!(
            "the {kind} body exceeded the {limit} byte interception limit"
        ));
    }
    *source = BodySource::Buffered(bytes.to_vec());
    Ok(())
}

/// Map an upstream client failure to a proxy error page.
fn classify(err: &ClientError) -> (StatusCode, String) {
    if err.is_connect() {
        (
            StatusCode::BAD_GATEWAY,
            format!("could not connect to the upstream server: {err}"),
        )
    } else {
        (
            StatusCode::BAD_GATEWAY,
            format!("upstream request failed: {err}"),
        )
    }
}
