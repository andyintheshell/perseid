//! Upstream client failure classification.

use super::*;
/// Map an upstream client failure to a proxy error page.
pub(super) fn classify(err: &ClientError) -> (StatusCode, String) {
    let detail = error_chain(err);
    if err.is_connect() {
        (
            StatusCode::BAD_GATEWAY,
            format!("could not connect to the upstream server: {detail}"),
        )
    } else {
        (
            StatusCode::BAD_GATEWAY,
            format!("upstream request failed: {detail}"),
        )
    }
}

/// Flatten an error and its sources for the proxy error page and history.
///
/// hyper-util's display text is often just `client error (SendRequest)`; the
/// underlying HTTP/2 or TLS failure is one or more source links down.
pub(super) fn error_chain(err: &(dyn std::error::Error + 'static)) -> String {
    let mut message = err.to_string();
    let mut source = err.source();
    while let Some(cause) = source {
        message.push_str(": ");
        message.push_str(&cause.to_string());
        source = cause.source();
    }
    message
}
