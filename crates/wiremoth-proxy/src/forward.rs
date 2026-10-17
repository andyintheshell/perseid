//! Plain-HTTP forwarding: rewrite the request for the origin server, stream it
//! upstream while capturing, stream the response back while capturing.

use std::sync::Arc;
use std::time::{Instant, SystemTime};

use http::header::{CONTENT_LENGTH, HOST};
use http::{HeaderMap, HeaderValue, Request, Response, StatusCode, Uri, Version};
use hyper::body::Incoming;
use hyper::ext::ReasonPhrase;
use hyper_util::client::legacy::Error as ClientError;
use log::debug;

use crate::capture::{BodyBuffer, EndReason, TeeBody, new_buffer, snapshot};
use crate::events::{CapturedBody, ExchangeEnd, ExchangeResult, ProxyEvent, RequestInfo, ResponseHead};
use crate::server::Ctx;
use crate::util::{
    IntoProxyBody, ProxyBody, display_host, forwarded_headers, header_list, text_response,
    version_str,
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

pub(crate) async fn handle(ctx: Arc<Ctx>, req: Request<Incoming>) -> Response<ProxyBody> {
    let id = ctx.next_id();
    let started = Instant::now();
    let timestamp = SystemTime::now();
    let version = version_str(req.version());
    let headers = header_list(req.headers());
    let raw_uri = req.uri().clone();

    match resolve_target(&req) {
        Ok(target) => {
            let authority = target
                .authority()
                .expect("resolved target always carries an authority");
            let request = RequestInfo {
                method: req.method().to_string(),
                uri: raw_uri.to_string(),
                host: display_host(authority),
                path: target
                    .path_and_query()
                    .map(|pq| pq.as_str().to_string())
                    .unwrap_or_else(|| "/".to_string()),
                version,
                headers,
            };
            ctx.emit(ProxyEvent::Started {
                id,
                timestamp,
                request,
            });
            forward(ctx, id, started, target, req).await
        }
        Err(err) => {
            debug!("rejecting request: {}", err.message);
            let request = RequestInfo {
                method: req.method().to_string(),
                uri: raw_uri.to_string(),
                host: req
                    .headers()
                    .get(HOST)
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or_default()
                    .to_string(),
                path: raw_uri.path().to_string(),
                version,
                headers,
            };
            ctx.emit(ProxyEvent::Started {
                id,
                timestamp,
                request,
            });
            let end = ExchangeEnd {
                duration: started.elapsed(),
                request_body: CapturedBody::default(),
                result: ExchangeResult::Failed {
                    error: err.message.clone(),
                },
            };
            ctx.emit(ProxyEvent::Finished { id, end });
            text_response(err.status, &err.message)
        }
    }
}

/// Determine the upstream URI for a proxied request.
///
/// Browsers send absolute-form targets (`GET http://host/path`); origin-form
/// targets (`GET /path` plus `Host`) are also accepted for clients that do
/// not implement proxy request syntax.
fn resolve_target(req: &Request<Incoming>) -> Result<Uri, TargetError> {
    let uri = req.uri();

    if let Some(scheme) = uri.scheme() {
        if scheme.as_str() != "http" {
            return Err(TargetError {
                status: StatusCode::NOT_IMPLEMENTED,
                message: format!(
                    "absolute-form target with scheme `{scheme}` is not supported yet; \
                     HTTPS interception is future work, use CONNECT for tunnels"
                ),
            });
        }
        if uri.authority().is_none() {
            return Err(TargetError::bad_request(
                "absolute-form target is missing an authority",
            ));
        }
        return Ok(uri.clone());
    }

    if uri.path() == "*" {
        return Err(TargetError::bad_request("asterisk-form targets are not supported"));
    }
    if !uri.path().starts_with('/') {
        return Err(TargetError::bad_request("unsupported request target form"));
    }

    let host = req
        .headers()
        .get(HOST)
        .ok_or_else(|| TargetError::bad_request("origin-form request without a Host header"))?
        .to_str()
        .map_err(|_| TargetError::bad_request("Host header is not valid ASCII"))?;
    let path = uri
        .path_and_query()
        .map(|pq| pq.as_str())
        .unwrap_or("/");
    format!("http://{host}{path}")
        .parse::<Uri>()
        .map_err(|_| TargetError::bad_request("invalid Host header"))
}

async fn forward(
    ctx: Arc<Ctx>,
    id: u64,
    started: Instant,
    target: Uri,
    req: Request<Incoming>,
) -> Response<ProxyBody> {
    let host_header = target
        .authority()
        .map(|authority| {
            display_host(authority)
                .parse::<HeaderValue>()
                .expect("authority renders as a valid header value")
        })
        .expect("resolved target always carries an authority");

    let (parts, body) = req.into_parts();

    let request_buffer = new_buffer();
    let tee = TeeBody::new(body, ctx.capture_limit, request_buffer.clone(), None);

    let mut upstream = Request::new(tee);
    *upstream.method_mut() = parts.method;
    *upstream.uri_mut() = target;
    // Origin servers speak HTTP/1.1 to a proxy regardless of client version.
    *upstream.version_mut() = Version::HTTP_11;
    let mut headers = HeaderMap::new();
    for (name, value) in forwarded_headers(&parts.headers) {
        headers.append(name, value);
    }
    // `Content-Length` was stripped: the body's size hint drives framing.
    debug_assert!(headers.get(CONTENT_LENGTH).is_none());
    headers.insert(HOST, host_header);
    *upstream.headers_mut() = headers;

    match ctx.client.request(upstream).await {
        Ok(response) => relay(ctx, id, started, request_buffer, response),
        Err(err) => {
            let (status, message) = classify(&err);
            let end = ExchangeEnd {
                duration: started.elapsed(),
                request_body: snapshot(&request_buffer),
                result: ExchangeResult::Failed {
                    error: message.clone(),
                },
            };
            ctx.emit(ProxyEvent::Finished { id, end });
            text_response(status, &message)
        }
    }
}

/// Stream the upstream response to the client, capturing as it goes. The
/// `Finished` event fires when the last frame is relayed (or earlier if the
/// connection dies), from inside the body adapter.
fn relay(
    ctx: Arc<Ctx>,
    id: u64,
    started: Instant,
    request_buffer: BodyBuffer,
    response: Response<Incoming>,
) -> Response<ProxyBody> {
    let status = response.status();
    let head = ResponseHead {
        version: version_str(response.version()),
        status: status.as_u16(),
        reason: response
            .extensions()
            .get::<ReasonPhrase>()
            .map(|phrase| String::from_utf8_lossy(phrase.as_bytes()).into_owned())
            .unwrap_or_default(),
        headers: header_list(response.headers()),
    };
    ctx.emit(ProxyEvent::ResponseHead { id, head });

    let (parts, body) = response.into_parts();

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
                duration: started.elapsed(),
                request_body: snapshot(&request_buffer),
                result,
            };
            ctx.emit(ProxyEvent::Finished { id, end });
        }
    };

    let tee = TeeBody::new(body, ctx.capture_limit, response_buffer, Some(Box::new(completion)));

    let mut out = Response::new(tee.boxed_response());
    *out.status_mut() = parts.status;
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
