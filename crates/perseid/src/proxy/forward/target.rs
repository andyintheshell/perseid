//! Upstream target resolution and request/body helpers.

use super::*;
/// Cases where the proxy rejects a request before any upstream traffic happens.
#[derive(Debug)]
pub(super) struct TargetError {
    pub(super) status: StatusCode,
    pub(super) message: String,
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
#[derive(Debug)]
pub(super) struct Target {
    pub(super) uri: Uri,
    pub(super) authority: http::uri::Authority,
}

/// Determine the upstream target for a proxied request.
///
/// Browsers send absolute-form targets (`GET http://host/path`); origin-form
/// targets (`GET /path` plus `Host`) are also accepted for clients that do
/// not implement proxy request syntax. Inside an intercepted TLS tunnel the
/// origin-form path resolves against `https`, and the CONNECT authority is the
/// fallback when the client omits `Host`.
pub(super) fn resolve_target(
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

pub(super) fn build_request_info(
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

pub(super) fn captured_buffered(body: &BodySource, limit: usize) -> CapturedBody {
    body.buffered()
        .map(|bytes| CapturedBody::from_bytes_limited(bytes, limit))
        .unwrap_or_default()
}

pub(super) fn content_length(headers: &HeaderMap) -> Option<usize> {
    headers.get(CONTENT_LENGTH)?.to_str().ok()?.parse().ok()
}

pub(super) fn can_buffer(headers: &HeaderMap, body: &Incoming, limit: usize) -> bool {
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

/// Whether a body source can be buffered for interception or rewriting.
/// Already-buffered bodies are always ready.
pub(super) fn fits_intercept_limit(source: &BodySource, headers: &HeaderMap, limit: usize) -> bool {
    match source {
        BodySource::Incoming(body) => can_buffer(headers, body, limit),
        BodySource::Buffered(_) => true,
    }
}

/// Read and buffer a body that is known to fit within `limit`.
pub(super) async fn buffer_body(
    source: &mut BodySource,
    limit: usize,
    kind: &str,
) -> Result<(), String> {
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
