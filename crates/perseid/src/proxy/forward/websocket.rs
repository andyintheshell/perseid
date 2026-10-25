//! WebSocket handshake forwarding and upgrade helpers.

use super::*;
use crate::proxy::websocket;
/// Forward a WebSocket handshake and, on 101, hand both upgraded sockets to
/// the frame relay. The handshake itself does not create an HTTP history row;
/// only complete WebSocket messages enter the WebSockets history.
pub(super) async fn forward_websocket(
    ctx: Arc<Ctx>,
    exchange: Exchange,
    target: Target,
    parts: http::request::Parts,
    body_source: BodySource,
    client_upgrade: Option<OnUpgrade>,
) -> Response<ProxyBody> {
    let Exchange {
        start,
        capture,
        policy,
        request,
        downstream_version,
    } = exchange;

    if client_upgrade.is_none() || downstream_version != Version::HTTP_11 {
        let message = "WebSocket upgrades require an HTTP/1.1 client connection".to_string();
        if capture {
            ctx.emit(ProxyEvent::Started {
                id: start.id,
                timestamp: start.timestamp,
                request,
            });
            finish_failed(&ctx, start.id, start.started, None, message.clone());
        }
        return text_response(StatusCode::NOT_IMPLEMENTED, &message);
    }

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
    *upstream.version_mut() = Version::HTTP_11;
    *upstream.headers_mut() = websocket_upstream_headers(&parts.headers);

    let mut response = match ctx.ws_client.request(upstream).await {
        Ok(response) => response,
        Err(err) => {
            let (status, message) = classify(&err);
            if capture {
                ctx.emit(ProxyEvent::Started {
                    id: start.id,
                    timestamp: start.timestamp,
                    request,
                });
                finish_failed(
                    &ctx,
                    start.id,
                    start.started,
                    Some(snapshot(&request_buffer)),
                    message.clone(),
                );
            }
            return text_response(status, &message);
        }
    };

    if response.status() != StatusCode::SWITCHING_PROTOCOLS {
        // A rejected or redirected handshake is ordinary HTTP. Preserve the
        // normal capture path so failed upgrades remain visible.
        if capture {
            ctx.emit(ProxyEvent::Started {
                id: start.id,
                timestamp: start.timestamp,
                request: request.clone(),
            });
        }
        return relay(
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
        .await;
    }

    let upstream_upgrade = hyper::upgrade::on(&mut response);
    let (response_parts, _) = response.into_parts();
    let client_upgrade = client_upgrade.expect("checked above");
    let tls = request_is_tls(&request);
    let connection = Arc::new(WebSocketConnectionInfo {
        id: start.id,
        url: websocket_url(&request),
        host: request.host.clone(),
        tls,
        listener_port: ctx.listen.port(),
        protocol: response_parts
            .headers
            .get(SEC_WEBSOCKET_PROTOCOL)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned),
    });

    let connection_id = connection.id;
    let relay_ctx = ctx.clone();
    tokio::spawn(async move {
        let client = match client_upgrade.await {
            Ok(upgraded) => TokioIo::new(upgraded),
            Err(err) => {
                warn!("WebSocket {connection_id} client upgrade failed: {err}");
                return;
            }
        };
        let server = match upstream_upgrade.await {
            Ok(upgraded) => TokioIo::new(upgraded),
            Err(err) => {
                warn!("WebSocket {connection_id} upstream upgrade failed: {err}");
                return;
            }
        };
        match websocket::relay(relay_ctx, connection, client, server).await {
            Ok((client_to_server, server_to_client)) => debug!(
                "WebSocket {connection_id} closed after {client_to_server} up and \
                 {server_to_client} down bytes"
            ),
            Err(err) => debug!("WebSocket {connection_id} relay error: {err}"),
        }
    });

    let mut out = Response::new(empty_body());
    *out.status_mut() = StatusCode::SWITCHING_PROTOCOLS;
    *out.version_mut() = downstream_version;
    if let Some(phrase) = response_parts.extensions.get::<ReasonPhrase>() {
        out.extensions_mut().insert(phrase.clone());
    }
    let mut headers = HeaderMap::new();
    for (name, value) in forwarded_headers(&response_parts.headers) {
        // Compression was not offered upstream, so the history must not
        // advertise an extension it cannot decode.
        if name == SEC_WEBSOCKET_EXTENSIONS {
            continue;
        }
        headers.append(name, value);
    }
    headers.insert(CONNECTION, HeaderValue::from_static("Upgrade"));
    headers.insert(UPGRADE, HeaderValue::from_static("websocket"));
    *out.headers_mut() = headers;
    out
}

pub(super) fn is_websocket_upgrade(parts: &http::request::Parts) -> bool {
    parts.method == Method::GET
        && parts.version == Version::HTTP_11
        && header_has_token(&parts.headers, CONNECTION, "upgrade")
        && header_has_token(&parts.headers, UPGRADE, "websocket")
}

pub(super) fn header_has_token(
    headers: &HeaderMap,
    name: http::header::HeaderName,
    token: &str,
) -> bool {
    headers.get_all(name).iter().any(|value| {
        value.to_str().is_ok_and(|text| {
            text.split(',')
                .any(|part| part.trim().eq_ignore_ascii_case(token))
        })
    })
}

pub(super) fn websocket_upstream_headers(headers: &HeaderMap) -> HeaderMap {
    let mut out = HeaderMap::new();
    for (name, value) in forwarded_headers(headers) {
        if name == HOST || name == SEC_WEBSOCKET_EXTENSIONS {
            continue;
        }
        out.append(name, value);
    }
    out.insert(CONNECTION, HeaderValue::from_static("Upgrade"));
    out.insert(UPGRADE, HeaderValue::from_static("websocket"));
    out
}

fn request_is_tls(request: &RequestInfo) -> bool {
    matches!(
        request.scheme.to_ascii_lowercase().as_str(),
        "https" | "wss"
    )
}

fn websocket_url(request: &RequestInfo) -> String {
    let scheme = if request_is_tls(request) { "wss" } else { "ws" };
    let path = if request.path.is_empty() {
        "/"
    } else {
        request.path.as_str()
    };
    format!("{scheme}://{}{path}", request.host)
}
