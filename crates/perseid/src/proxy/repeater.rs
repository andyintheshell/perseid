//! Direct request replay for the Repeater tool.
//!
//! Repeater is deliberately independent of the proxy: it uses its own Tokio
//! runtime and HTTP client, so replay works even when the proxy is stopped. The
//! request is still sent directly to the origin, exactly as the Repeater target
//! says, rather than through the local listener.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender};
use std::thread;
use std::time::{Duration, Instant};

use bytes::Bytes;
use http::header::{ACCEPT_ENCODING, HOST, HeaderValue};
use http::{HeaderMap, HeaderName, Method, Request, Uri, Version};
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::ext::ReasonPhrase;
use hyper_rustls::{DefaultServerNameResolver, HttpsConnector};
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc, oneshot};

use crate::proxy::events::{CapturedBody, HeaderList};
use crate::proxy::message::parse_request_head;
use crate::proxy::tls::{self, UpstreamTlsError};
use crate::proxy::util::{
    IntoProxyBody, ProxyBody, forwarded_headers, header_list, sanitized_accept_encoding,
    version_str,
};

/// How long one Repeater request may take, from connect to the last response
/// byte. The proxy's per-connection timeout is shorter because it is only a
/// connect timeout; manual replay should allow slower origins to finish.
pub const DEFAULT_REPEATER_TIMEOUT: Duration = Duration::from_secs(60);

/// Upper bound on simultaneous tab sends. The UI only prevents duplicate
/// sends from one tab, so this keeps a burst across many tabs bounded.
const MAX_CONCURRENT_REQUESTS: usize = 8;
/// Admission covers queued jobs, active requests, and unread results together.
const MAX_PENDING_REQUESTS: usize = 32;
const MAX_PENDING_BYTES: usize = 64 * 1024 * 1024;
const MAX_REQUEST_BYTES: usize = 8 * 1024 * 1024;
const MAX_HEAD_BYTES: usize = 64 * 1024;

/// Runtime settings for the direct replay engine.
#[derive(Clone)]
pub struct RepeaterConfig {
    /// Maximum response body bytes retained for display.
    pub capture_limit: usize,
    /// End-to-end timeout for one request.
    pub timeout: Duration,
    /// Extra roots trusted for upstream TLS, mainly for tests and private CAs.
    pub extra_roots_pem: Vec<String>,
    /// Called when a result is ready. The desktop UI uses this to repaint.
    pub notify: Option<Arc<dyn Fn() + Send + Sync>>,
}

impl Default for RepeaterConfig {
    fn default() -> Self {
        Self {
            capture_limit: crate::proxy::DEFAULT_CAPTURE_LIMIT,
            timeout: DEFAULT_REPEATER_TIMEOUT,
            extra_roots_pem: Vec::new(),
            notify: None,
        }
    }
}

/// A parsed request ready to be sent directly to an origin server.
///
/// The target is stored as an absolute URI. The `Host` header is preserved
/// separately, so a tester can connect to one authority while sending a
/// different virtual host, which is a common access-control test.
#[derive(Debug, Clone)]
pub struct RepeaterRequest {
    pub method: String,
    /// Absolute target URI, for example `https://example.com:8443/a?b=c`.
    pub uri: String,
    /// The edited request-line version. The transport still negotiates
    /// HTTP/2 or HTTP/1.1 through ALPN, matching the proxy's upstream client.
    pub version: String,
    pub headers: HeaderList,
    pub body: Vec<u8>,
}

impl RepeaterRequest {
    /// Parse an edited raw request head and body.
    ///
    /// Origin-form request targets (`GET /path HTTP/1.1` plus `Host`) use
    /// `default_scheme` and `default_host` as the connection target. An
    /// absolute-form target (`GET https://example.com/path HTTP/1.1`) carries
    /// its own scheme and authority.
    pub fn parse(
        head: &str,
        body: Vec<u8>,
        default_scheme: &str,
        default_host: &str,
    ) -> Result<Self, RepeaterError> {
        if head
            .len()
            .saturating_add(default_host.len())
            .saturating_add(default_scheme.len())
            > MAX_HEAD_BYTES
            || head.len().saturating_add(body.len()) > MAX_REQUEST_BYTES
        {
            return Err(RepeaterError::RequestTooLarge);
        }
        let parsed = parse_request_head(head).map_err(RepeaterError::InvalidRequest)?;
        let (scheme, authority, path) =
            resolve_target(&parsed.uri, default_scheme, default_host, &parsed.headers)?;
        let uri_text = format!("{scheme}://{authority}{path}");
        let uri = uri_text.parse::<Uri>().map_err(|_| {
            RepeaterError::InvalidRequest(format!("invalid target URI `{uri_text}`"))
        })?;

        let mut headers = HeaderMap::new();
        for (name, value) in forwarded_headers(&parsed.headers) {
            headers.append(name, value);
        }

        // Match the proxy's readable-capture behavior: keep codings the
        // inspection views can unpack, and drop unsupported ones (notably
        // zstd) so a normal browser request does not produce an opaque
        // response in Repeater.
        let accept_encoding = sanitized_accept_encoding(&headers);
        headers.remove(ACCEPT_ENCODING);
        if let Some(value) = accept_encoding {
            headers.insert(ACCEPT_ENCODING, value);
        }

        // Keep a user-supplied Host header for virtual-host testing. Only fill
        // it from the target when the editor left it out or blank.
        let user_host = headers
            .get(HOST)
            .filter(|value| !value.as_bytes().is_empty())
            .cloned();
        headers.remove(HOST);
        let host = match user_host {
            Some(host) => host,
            None => HeaderValue::from_str(uri.authority().map(|a| a.as_str()).unwrap_or(""))
                .map_err(|_| {
                    RepeaterError::InvalidRequest("target authority cannot be a Host header".into())
                })?,
        };
        headers.insert(HOST, host);

        Ok(Self {
            method: parsed.method.to_string(),
            uri: uri.to_string(),
            version: version_str(parsed.version),
            headers: header_list(&headers),
            body,
        })
    }
}

fn resolve_target(
    uri: &Uri,
    default_scheme: &str,
    default_host: &str,
    headers: &HeaderMap,
) -> Result<(String, String, String), RepeaterError> {
    if let Some(scheme) = uri.scheme_str() {
        if !matches!(scheme, "http" | "https") {
            return Err(RepeaterError::InvalidRequest(format!(
                "absolute-form target with scheme `{scheme}` is not supported"
            )));
        }
        let authority = uri
            .authority()
            .ok_or_else(|| {
                RepeaterError::InvalidRequest(
                    "absolute-form target is missing an authority".to_string(),
                )
            })?
            .as_str()
            .to_string();
        let path = uri.path_and_query().map(|p| p.as_str()).unwrap_or("/");
        if path == "*" {
            return Err(RepeaterError::InvalidRequest(
                "asterisk-form targets are not supported".to_string(),
            ));
        }
        return Ok((scheme.to_string(), authority, path.to_string()));
    }

    let path = uri.path_and_query().map(|p| p.as_str()).unwrap_or("/");
    if path == "*" {
        return Err(RepeaterError::InvalidRequest(
            "asterisk-form targets are not supported".to_string(),
        ));
    }
    if !path.starts_with('/') {
        return Err(RepeaterError::InvalidRequest(
            "origin-form target must start with `/`".to_string(),
        ));
    }

    let (scheme, authority) = match default_target(default_scheme, default_host) {
        Ok(target) => target,
        Err(err) if default_host.trim().is_empty() => {
            // A blank target bar can still work when the editor has a Host
            // header. This is useful for a fresh tab and keeps the raw request
            // usable without filling in a separate field first.
            let host = headers
                .get(HOST)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default();
            default_target(default_scheme, host).map_err(|_| err)?
        }
        Err(err) => return Err(err),
    };
    Ok((scheme, authority, path.to_string()))
}

fn default_target(scheme: &str, host: &str) -> Result<(String, String), RepeaterError> {
    let host = host.trim();
    if host.is_empty() {
        return Err(RepeaterError::InvalidRequest(
            "the Repeater target is missing a host".to_string(),
        ));
    }

    // Accept a full URI in the host field as a convenience. The UI normally
    // keeps scheme and host separate, but pasted targets should work too.
    if host.contains("://") {
        let uri = host.parse::<Uri>().map_err(|_| {
            RepeaterError::InvalidRequest(format!("invalid Repeater target `{host}`"))
        })?;
        let scheme = uri.scheme_str().unwrap_or_default();
        if !matches!(scheme, "http" | "https") {
            return Err(RepeaterError::InvalidRequest(format!(
                "Repeater target scheme `{scheme}` is not supported"
            )));
        }
        let authority = uri
            .authority()
            .ok_or_else(|| {
                RepeaterError::InvalidRequest("Repeater target is missing an authority".to_string())
            })?
            .as_str()
            .to_string();
        if uri.path() != "/" {
            return Err(RepeaterError::InvalidRequest(
                "the Repeater target must be a host, not a path".to_string(),
            ));
        }
        return Ok((scheme.to_string(), authority));
    }

    let scheme = scheme.trim().to_ascii_lowercase();
    if !matches!(scheme.as_str(), "http" | "https") {
        return Err(RepeaterError::InvalidRequest(format!(
            "Repeater target scheme `{scheme}` is not supported"
        )));
    }
    let uri = format!("{scheme}://{host}")
        .parse::<Uri>()
        .map_err(|_| RepeaterError::InvalidRequest(format!("invalid Repeater target `{host}`")))?;
    let authority = uri
        .authority()
        .ok_or_else(|| {
            RepeaterError::InvalidRequest("Repeater target is missing an authority".to_string())
        })?
        .as_str()
        .to_string();
    if uri.path() != "/" {
        return Err(RepeaterError::InvalidRequest(
            "the Repeater target must be a host, not a path".to_string(),
        ));
    }
    Ok((scheme, authority))
}

/// Why a request could not be parsed or queued.
#[derive(Debug, thiserror::Error)]
pub enum RepeaterError {
    #[error("invalid request: {0}")]
    InvalidRequest(String),
    #[error("the Repeater request exceeds the 8 MiB request or 64 KiB head limit")]
    RequestTooLarge,
    #[error("the Repeater pending-work budget is full; collect responses before retrying")]
    Busy,
    #[error("the Repeater engine is not running")]
    Closed,
}

/// Failure to start the direct replay engine.
#[derive(Debug, thiserror::Error)]
pub enum RepeaterStartError {
    #[error("the Repeater capture limit exceeds the {limit}-byte pending-budget maximum")]
    CaptureLimit { limit: usize },
    #[error("could not start the Repeater runtime: {0}")]
    Runtime(#[source] std::io::Error),
    #[error("could not spawn the Repeater thread: {0}")]
    Spawn(#[source] std::io::Error),
    #[error(transparent)]
    Tls(#[from] UpstreamTlsError),
    #[error("the Repeater thread stopped before it was ready")]
    Startup,
}

/// One response from the direct replay engine.
#[derive(Debug, Clone)]
pub struct RepeaterResponse {
    /// Matches the id returned by [`Repeater::send`].
    pub id: u64,
    pub duration: Duration,
    pub result: RepeaterResult,
}

#[derive(Debug, Clone)]
pub enum RepeaterResult {
    Response {
        version: String,
        status: u16,
        reason: String,
        headers: HeaderList,
        body: CapturedBody,
        /// Set when the response head arrived but the body transfer failed.
        body_error: Option<String>,
    },
    Failed {
        error: String,
    },
}

/// A direct HTTP client with a small background runtime.
///
/// The handle is cheap to keep alive and can be dropped independently of the
/// proxy. Results are polled with [`Repeater::try_recv`].
pub struct Repeater {
    jobs: mpsc::Sender<Job>,
    results: Receiver<QueuedResponse>,
    budget: PendingBudget,
    shutdown: Option<oneshot::Sender<()>>,
    thread: Option<thread::JoinHandle<()>>,
}

struct Job {
    id: u64,
    request: RepeaterRequest,
    reservation: Reservation,
}

struct QueuedResponse {
    response: RepeaterResponse,
    _reservation: Reservation,
}

/// A reservation stays alive until the consumer takes the result, not merely
/// until network work finishes. Thus unread responses cannot bypass admission.
struct Reservation {
    _slot: OwnedSemaphorePermit,
    _bytes: OwnedSemaphorePermit,
}

struct PendingBudget {
    slots: Arc<Semaphore>,
    bytes: Arc<Semaphore>,
    response_bytes: usize,
}

impl PendingBudget {
    fn new(capture_limit: usize) -> Self {
        Self {
            slots: Arc::new(Semaphore::new(MAX_PENDING_REQUESTS)),
            bytes: Arc::new(Semaphore::new(MAX_PENDING_BYTES)),
            // Lossy UTF-8 conversion can triple header bytes. Reserve before
            // starting work; the HTTP parser independently caps the wire head.
            response_bytes: capture_limit.saturating_add(3 * MAX_HEAD_BYTES),
        }
    }

    fn reserve(&self, request: &RepeaterRequest) -> Result<Reservation, RepeaterError> {
        let mut request_bytes = request
            .method
            .capacity()
            .saturating_add(request.uri.capacity())
            .saturating_add(request.version.capacity())
            .saturating_add(request.body.capacity())
            .saturating_add(
                request
                    .headers
                    .capacity()
                    .saturating_mul(std::mem::size_of::<(String, String)>()),
            );
        let mut head_bytes = request
            .method
            .len()
            .saturating_add(request.uri.len())
            .saturating_add(request.version.len());
        for (name, value) in &request.headers {
            request_bytes = request_bytes
                .saturating_add(name.capacity())
                .saturating_add(value.capacity());
            head_bytes = head_bytes
                .saturating_add(name.len())
                .saturating_add(value.len())
                .saturating_add(4);
        }
        if request_bytes > MAX_REQUEST_BYTES || head_bytes > MAX_HEAD_BYTES {
            return Err(RepeaterError::RequestTooLarge);
        }
        let retained = request_bytes.saturating_add(self.response_bytes);
        if retained > MAX_PENDING_BYTES {
            return Err(RepeaterError::Busy);
        }
        let slot = self
            .slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| RepeaterError::Busy)?;
        let bytes = self
            .bytes
            .clone()
            .try_acquire_many_owned(retained as u32)
            .map_err(|_| RepeaterError::Busy)?;
        Ok(Reservation {
            _slot: slot,
            _bytes: bytes,
        })
    }
}

impl Repeater {
    /// Start the replay runtime.
    pub fn start(config: RepeaterConfig) -> Result<Self, RepeaterStartError> {
        let capture_limit = config.capture_limit.max(1);
        let max_capture = MAX_PENDING_BYTES - MAX_REQUEST_BYTES - 3 * MAX_HEAD_BYTES;
        if capture_limit > max_capture {
            return Err(RepeaterStartError::CaptureLimit { limit: max_capture });
        }
        let timeout = config.timeout.max(Duration::from_millis(1));
        let notify = config.notify.clone();
        let upstream_tls = tls::upstream_client_config(&config.extra_roots_pem)?;

        let budget = PendingBudget::new(capture_limit);
        let (jobs, job_rx) = mpsc::channel(MAX_PENDING_REQUESTS);
        let (result_tx, results) = std::sync::mpsc::sync_channel(MAX_PENDING_REQUESTS);
        let (shutdown, shutdown_rx) = oneshot::channel();
        let (startup_tx, startup_rx) = std::sync::mpsc::sync_channel(1);

        let thread = thread::Builder::new()
            .name("perseid-repeater".to_string())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(err) => {
                        let _ = startup_tx.send(Err(RepeaterStartError::Runtime(err)));
                        return;
                    }
                };

                let mut connector = HttpConnector::new();
                connector.set_connect_timeout(Some(timeout));
                connector.enforce_http(false);
                let connector = HttpsConnector::new(
                    connector,
                    upstream_tls,
                    false,
                    Arc::new(DefaultServerNameResolver::default()),
                );
                let client: Client<HttpsConnector<HttpConnector>, ProxyBody> =
                    Client::builder(TokioExecutor::new())
                        .http1_preserve_header_case(true)
                        .http1_max_buf_size(MAX_HEAD_BYTES)
                        .http2_max_header_list_size(MAX_HEAD_BYTES as u32)
                        .build(connector);

                runtime.spawn(async move {
                    let _ = startup_tx.send(Ok(()));
                    worker(client, job_rx, result_tx, capture_limit, timeout, notify).await;
                });

                runtime.block_on(async {
                    let _ = shutdown_rx.await;
                });
            })
            .map_err(RepeaterStartError::Spawn)?;

        match startup_rx.recv() {
            Ok(Ok(())) => {}
            Ok(Err(err)) => {
                let _ = thread.join();
                return Err(err);
            }
            Err(_) => {
                let _ = thread.join();
                return Err(RepeaterStartError::Startup);
            }
        }

        Ok(Self {
            jobs,
            results,
            budget,
            shutdown: Some(shutdown),
            thread: Some(thread),
        })
    }

    /// Queue one request without blocking. The returned id is echoed in
    /// [`RepeaterResponse`]. At most 32 jobs/results and 64 MiB of retained
    /// payload reservations may be pending, including unread responses. Requests
    /// larger than 8 MiB are rejected. Collect results before retrying `Busy`.
    pub fn send(&self, request: RepeaterRequest) -> Result<u64, RepeaterError> {
        if self.jobs.is_closed() {
            return Err(RepeaterError::Closed);
        }
        let reservation = self.budget.reserve(&request)?;
        static NEXT_ID: AtomicU64 = AtomicU64::new(1);
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        self.jobs
            .try_send(Job {
                id,
                request,
                reservation,
            })
            .map_err(|err| match err {
                mpsc::error::TrySendError::Full(_) => RepeaterError::Busy,
                mpsc::error::TrySendError::Closed(_) => RepeaterError::Closed,
            })?;
        Ok(id)
    }

    /// Take the next completed response, if any.
    pub fn try_recv(&self) -> Option<RepeaterResponse> {
        self.results.try_recv().ok().map(|queued| queued.response)
    }

    /// Whether the replay runtime thread is still alive.
    pub fn is_running(&self) -> bool {
        self.thread
            .as_ref()
            .is_some_and(|thread| !thread.is_finished())
    }
}

impl Drop for Repeater {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(thread) = self.thread.take()
            && thread.join().is_err()
        {
            log::error!("Repeater thread panicked while shutting down");
        }
    }
}

async fn worker(
    client: Client<HttpsConnector<HttpConnector>, ProxyBody>,
    mut jobs: mpsc::Receiver<Job>,
    results: SyncSender<QueuedResponse>,
    capture_limit: usize,
    timeout: Duration,
    notify: Option<Arc<dyn Fn() + Send + Sync>>,
) {
    let concurrency = Arc::new(Semaphore::new(MAX_CONCURRENT_REQUESTS));
    while let Some(job) = jobs.recv().await {
        let Ok(permit) = concurrency.clone().acquire_owned().await else {
            break;
        };
        let client = client.clone();
        let results = results.clone();
        let notify = notify.clone();
        tokio::spawn(async move {
            let Job {
                id,
                request,
                reservation,
            } = job;
            let started = Instant::now();
            let result = execute(&client, request, capture_limit, timeout).await;
            let response = RepeaterResponse {
                id,
                duration: started.elapsed(),
                result,
            };
            // This cannot block: every result holds an admission slot, and
            // the result channel has capacity for every admitted job together.
            if results
                .send(QueuedResponse {
                    response,
                    _reservation: reservation,
                })
                .is_ok()
                && let Some(notify) = &notify
            {
                notify();
            }
            drop(permit);
        });
    }
}

async fn execute(
    client: &Client<HttpsConnector<HttpConnector>, ProxyBody>,
    request: RepeaterRequest,
    capture_limit: usize,
    timeout: Duration,
) -> RepeaterResult {
    let future = async move {
        let request = build_request(request)?;
        let response = client
            .request(request)
            .await
            .map_err(|err| format!("upstream request failed: {err}"))?;
        let (parts, body) = response.into_parts();
        let (body, body_error) = read_body(body, capture_limit).await;
        let reason = parts
            .extensions
            .get::<ReasonPhrase>()
            .map(|phrase| String::from_utf8_lossy(phrase.as_bytes()).into_owned())
            .or_else(|| parts.status.canonical_reason().map(ToString::to_string))
            .unwrap_or_default();
        Ok::<_, String>(RepeaterResult::Response {
            version: version_str(parts.version),
            status: parts.status.as_u16(),
            reason,
            headers: header_list(&parts.headers),
            body,
            body_error,
        })
    };

    match tokio::time::timeout(timeout, future).await {
        Ok(Ok(result)) => result,
        Ok(Err(error)) => RepeaterResult::Failed { error },
        Err(_) => RepeaterResult::Failed {
            error: format!(
                "request timed out after {:.1} seconds",
                timeout.as_secs_f64()
            ),
        },
    }
}

fn build_request(request: RepeaterRequest) -> Result<Request<ProxyBody>, String> {
    let method = Method::from_bytes(request.method.as_bytes())
        .map_err(|_| format!("invalid HTTP method `{}`", request.method))?;
    let uri = request
        .uri
        .parse::<Uri>()
        .map_err(|_| format!("invalid target URI `{}`", request.uri))?;
    if uri.authority().is_none() {
        return Err("the request target is missing an authority".to_string());
    }

    let mut headers = HeaderMap::new();
    for (name, value) in request.headers {
        let name = HeaderName::from_bytes(name.as_bytes())
            .map_err(|_| format!("invalid header name `{name}`"))?;
        let value =
            HeaderValue::from_str(&value).map_err(|_| format!("invalid value for `{name}`"))?;
        headers.append(name, value);
    }

    // Let hyper derive Host/:authority from the URI when the editor kept the
    // default host. Sending an explicit Host alongside HTTP/2 :authority is
    // redundant and is rejected by strict origins. A non-default Host is kept
    // below for virtual-host testing.
    let default_host = uri.authority().is_some_and(|authority| {
        headers.get(HOST).is_some_and(|host| {
            host.as_bytes()
                .eq_ignore_ascii_case(authority.as_str().as_bytes())
        })
    });
    if default_host {
        headers.remove(HOST);
    }

    let body = Full::new(Bytes::from(request.body)).boxed_response();
    let mut upstream = Request::new(body);
    *upstream.method_mut() = method;
    *upstream.uri_mut() = uri;
    // Let the connector choose HTTP/2 through ALPN. For cleartext origins the
    // legacy client stays on HTTP/1.1.
    *upstream.version_mut() = Version::HTTP_11;
    *upstream.headers_mut() = headers;
    Ok(upstream)
}

async fn read_body(mut body: Incoming, limit: usize) -> (CapturedBody, Option<String>) {
    let mut captured = CapturedBody::default();
    loop {
        match body.frame().await {
            None => {
                captured.mark_complete();
                return (captured, None);
            }
            Some(Ok(frame)) => {
                if let Some(data) = frame.data_ref() {
                    captured.append(data, limit);
                }
            }
            Some(Err(err)) => {
                return (
                    captured,
                    Some(format!("response body could not be read: {err}")),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn small_request() -> RepeaterRequest {
        RepeaterRequest::parse("GET / HTTP/1.1\n\n", Vec::new(), "http", "127.0.0.1:1").unwrap()
    }

    #[test]
    fn pending_budget_bounds_slots_until_the_result_is_consumed() {
        let budget = PendingBudget::new(1);
        let request = small_request();
        let mut reservations = Vec::new();
        for _ in 0..MAX_PENDING_REQUESTS {
            reservations.push(budget.reserve(&request).unwrap());
        }
        assert!(matches!(budget.reserve(&request), Err(RepeaterError::Busy)));
        drop(reservations.pop());
        assert!(budget.reserve(&request).is_ok());
        drop(reservations);
        assert_eq!(budget.bytes.available_permits(), MAX_PENDING_BYTES);
    }

    #[test]
    fn pending_budget_reserves_response_bytes_before_starting_work() {
        let budget = PendingBudget::new(MAX_PENDING_BYTES / 2);
        let request = small_request();
        let first = budget.reserve(&request).unwrap();
        assert!(matches!(budget.reserve(&request), Err(RepeaterError::Busy)));
        // A rejected byte reservation must release its count reservation.
        assert_eq!(budget.slots.available_permits(), MAX_PENDING_REQUESTS - 1);
        drop(first);
        assert_eq!(budget.bytes.available_permits(), MAX_PENDING_BYTES);
    }

    #[test]
    fn oversized_request_buffers_and_heads_are_rejected() {
        let budget = PendingBudget::new(1);
        let mut request = small_request();
        request.body = Vec::with_capacity(MAX_REQUEST_BYTES + 1);
        assert!(matches!(
            budget.reserve(&request),
            Err(RepeaterError::RequestTooLarge)
        ));
        assert!(matches!(
            RepeaterRequest::parse(
                &"x".repeat(MAX_HEAD_BYTES + 1),
                Vec::new(),
                "http",
                "localhost"
            ),
            Err(RepeaterError::RequestTooLarge)
        ));
        assert_eq!(budget.slots.available_permits(), MAX_PENDING_REQUESTS);
    }

    #[test]
    fn capture_limits_that_can_never_fit_are_rejected_at_startup() {
        assert!(matches!(
            Repeater::start(RepeaterConfig {
                capture_limit: usize::MAX,
                ..RepeaterConfig::default()
            }),
            Err(RepeaterStartError::CaptureLimit { .. })
        ));
    }

    #[test]
    fn unread_results_apply_backpressure_without_blocking_shutdown() {
        let repeater = Repeater::start(RepeaterConfig::default()).unwrap();
        for _ in 0..MAX_PENDING_REQUESTS {
            let mut request = small_request();
            // A local validation failure gives a result without depending on
            // any particular port being unused on the test machine.
            request.method = "invalid method".to_string();
            repeater.send(request).unwrap();
        }
        assert!(matches!(
            repeater.send(small_request()),
            Err(RepeaterError::Busy)
        ));
        let queued = repeater
            .results
            .recv_timeout(Duration::from_secs(5))
            .expect("Repeater should report a validation failure");
        assert!(matches!(
            repeater.send(small_request()),
            Err(RepeaterError::Busy)
        ));
        drop(queued);
        assert!(repeater.send(small_request()).is_ok());
        drop(repeater);
    }

    #[test]
    fn origin_form_uses_the_target_and_keeps_a_virtual_host() {
        let request = RepeaterRequest::parse(
            "GET /admin HTTP/1.1\nHost: virtual.example\n\n",
            Vec::new(),
            "http",
            "127.0.0.1:8080",
        )
        .unwrap();

        assert_eq!(request.method, "GET");
        assert_eq!(request.uri, "http://127.0.0.1:8080/admin");
        assert_eq!(request.version, "HTTP/1.1");
        assert!(
            request
                .headers
                .iter()
                .any(|(name, value)| name == "host" && value == "virtual.example")
        );
    }

    #[test]
    fn missing_host_uses_the_target_authority() {
        let request = RepeaterRequest::parse(
            "POST /submit HTTP/1.1\nContent-Length: 999\n\n",
            b"hello".to_vec(),
            "https",
            "example.com:8443",
        )
        .unwrap();

        assert_eq!(request.uri, "https://example.com:8443/submit");
        assert_eq!(request.body, b"hello");
        assert!(
            request
                .headers
                .iter()
                .any(|(name, value)| name == "host" && value == "example.com:8443")
        );
        assert!(
            !request
                .headers
                .iter()
                .any(|(name, _)| name == "content-length")
        );
    }

    #[test]
    fn absolute_form_target_wins_over_the_default() {
        let request = RepeaterRequest::parse(
            "PUT http://other.example:9000/path?q=1 HTTP/2\nHost: virtual.example\n\n",
            Vec::new(),
            "https",
            "ignored.example",
        )
        .unwrap();

        assert_eq!(request.method, "PUT");
        assert_eq!(request.uri, "http://other.example:9000/path?q=1");
        assert_eq!(request.version, "HTTP/2");
    }

    #[test]
    fn keeps_only_supported_accept_encodings() {
        let request = RepeaterRequest::parse(
            "GET / HTTP/1.1\nHost: example.com\nAccept-Encoding: gzip, zstd, br\n\n",
            Vec::new(),
            "https",
            "example.com",
        )
        .unwrap();

        let values: Vec<&str> = request
            .headers
            .iter()
            .filter(|(name, _)| name == "accept-encoding")
            .map(|(_, value)| value.as_str())
            .collect();
        assert_eq!(values, vec!["gzip, br"]);
    }

    #[test]
    fn rejects_asterisk_form_and_missing_targets() {
        assert!(
            RepeaterRequest::parse("OPTIONS * HTTP/1.1\n\n", Vec::new(), "http", "example.com")
                .is_err()
        );
        assert!(RepeaterRequest::parse("GET / HTTP/1.1\n\n", Vec::new(), "http", "").is_err());
    }

    #[test]
    fn lets_hyper_add_the_default_host_but_keeps_a_virtual_one() {
        let default = RepeaterRequest::parse(
            "GET / HTTP/1.1\nHost: example.com\n\n",
            Vec::new(),
            "https",
            "example.com",
        )
        .unwrap();
        let built = build_request(default).unwrap();
        assert!(built.headers().get("host").is_none());

        let virtual_host = RepeaterRequest::parse(
            "GET / HTTP/1.1\nHost: virtual.example\n\n",
            Vec::new(),
            "https",
            "example.com",
        )
        .unwrap();
        let built = build_request(virtual_host).unwrap();
        assert_eq!(built.headers().get("host").unwrap(), "virtual.example");
    }

    #[test]
    fn parses_an_empty_editor_body_as_no_body() {
        let request = RepeaterRequest::parse(
            "GET / HTTP/1.1\r\nHost: example.com\r\n\r\n",
            Vec::new(),
            "http",
            "",
        )
        .unwrap();
        assert!(request.body.is_empty());
        assert_eq!(request.uri, "http://example.com/");
    }
}
