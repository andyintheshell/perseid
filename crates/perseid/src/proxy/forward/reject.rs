//! Early-return helpers that record failed or rejected exchanges.

use super::*;
pub(super) fn finish(
    ctx: &Ctx,
    id: u64,
    started: Instant,
    request_body: Option<CapturedBody>,
    result: ExchangeResult,
) {
    let end = ExchangeEnd {
        duration: started.elapsed(),
        request_body: request_body.unwrap_or_default(),
        result,
    };
    ctx.emit(ProxyEvent::Finished { id, end });
}

pub(super) fn finish_failed(
    ctx: &Ctx,
    id: u64,
    started: Instant,
    request_body: Option<CapturedBody>,
    message: String,
) {
    finish(
        ctx,
        id,
        started,
        request_body,
        ExchangeResult::Failed { error: message },
    );
}

pub(super) fn reject_request(
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

pub(super) fn reject_dropped_request(
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
        finish(
            ctx,
            start.id,
            start.started,
            Some(request_body),
            ExchangeResult::Dropped,
        );
    }
    text_response(status, &message)
}
