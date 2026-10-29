//! Upstream response relaying, interception, and capture.

use super::*;
/// Stream the upstream response to the client, capturing as it goes. The
/// `Finished` event fires when the last frame is relayed (or earlier if the
/// connection dies), from inside the body adapter.
pub(super) async fn relay(
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
        let can_buffer = fits_intercept_limit(&body_source, &parts.headers, ctx.intercept_limit);
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
        let item = InterceptItem::new_response(start.id, request.clone(), head, body, decision_tx);

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
                    finish(
                        &ctx,
                        start.id,
                        start.started,
                        Some(snapshot(&request_buffer)),
                        ExchangeResult::Dropped,
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

    if let Some(jar) = &ctx.cookie_jar
        && let Ok(mut jar) = jar.lock()
    {
        let target = format!("{}://{}{}", request.scheme, request.host, request.path);
        jar.observe_proxy_response(&target, &header_list(&parts.headers));
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
