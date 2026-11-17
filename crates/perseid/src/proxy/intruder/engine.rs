//! Intruder runtime: the background engine, attack dispatch, and events.

use std::sync::Arc;
#[cfg(test)]
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use hyper_rustls::{DefaultServerNameResolver, HttpsConnector};
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;
use tokio::sync::{Semaphore, mpsc};
use tokio::task::JoinSet;

use super::plan::{IntruderError, IntruderPlan, MAX_INTRUDER_CONCURRENCY, split_raw_request};
use crate::proxy::policy::host_in_scope;
use crate::proxy::repeater::{
    DEFAULT_REPEATER_TIMEOUT, RepeaterRequest, RepeaterResult, execute as execute_repeater,
};
use crate::proxy::tls::{self, UpstreamTlsError};
use crate::proxy::util::ProxyBody;

/// Bound on completed results waiting for the UI to collect them.
const MAX_QUEUED_RESULTS: usize = 64;
/// Retained bytes allowed across queued results before the engine applies
/// backpressure. The desktop UI keeps a separate bounded result history.
const MAX_QUEUED_RESULT_BYTES: usize = 64 * 1024 * 1024;

/// Runtime settings shared by every attack in one engine.
#[derive(Clone)]
pub struct IntruderConfig {
    /// Optional shared, in-memory session cookies.
    pub cookie_jar: Option<crate::proxy::cookies::SharedCookieJar>,
    /// Maximum response body bytes retained for each result.
    pub capture_limit: usize,
    /// End-to-end timeout for one request.
    pub timeout: Duration,
    /// Extra roots trusted for upstream TLS, mainly for tests and private CAs.
    pub extra_roots_pem: Vec<String>,
    /// Called when an event is ready. The desktop UI uses this to repaint.
    pub notify: Option<Arc<dyn Fn() + Send + Sync>>,
    /// Test-only high-water mark of task entries retained during an attack.
    #[cfg(test)]
    pub(crate) max_tracked_tasks: Option<Arc<AtomicUsize>>,
}

impl Default for IntruderConfig {
    fn default() -> Self {
        Self {
            cookie_jar: None,
            capture_limit: crate::proxy::DEFAULT_CAPTURE_LIMIT,
            timeout: DEFAULT_REPEATER_TIMEOUT,
            extra_roots_pem: Vec::new(),
            notify: None,
            #[cfg(test)]
            max_tracked_tasks: None,
        }
    }
}

/// Failure to start the Intruder engine.
#[derive(Debug, thiserror::Error)]
pub enum IntruderStartError {
    #[error("the Intruder capture limit is too large")]
    CaptureLimit,
    #[error("could not start the Intruder runtime: {0}")]
    Runtime(#[source] std::io::Error),
    #[error("could not spawn the Intruder thread: {0}")]
    Spawn(#[source] std::io::Error),
    #[error(transparent)]
    Tls(#[from] UpstreamTlsError),
    #[error("the Intruder thread stopped before it was ready")]
    Startup,
}

/// One completed request in an attack.
#[derive(Debug, Clone)]
pub struct IntruderResult {
    pub attack_id: u64,
    pub sequence: u64,
    pub payloads: Vec<String>,
    pub duration: Duration,
    pub result: RepeaterResult,
}

/// Events produced by the engine.
#[derive(Debug, Clone)]
pub enum IntruderEvent {
    Result(IntruderResult),
    Finished {
        attack_id: u64,
        completed: u64,
        total: u64,
        cancelled: bool,
    },
}

enum Command {
    Start {
        id: u64,
        plan: IntruderPlan,
        cancel: Arc<AtomicBool>,
    },
    Stop,
    Shutdown,
}

/// A direct HTTP/HTTPS attack engine with a small background runtime.
pub struct Intruder {
    commands: mpsc::UnboundedSender<Command>,
    results: mpsc::Receiver<IntruderEvent>,
    running: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

impl Intruder {
    /// Start the attack runtime.
    pub fn start(config: IntruderConfig) -> Result<Self, IntruderStartError> {
        if config.capture_limit > MAX_QUEUED_RESULT_BYTES {
            return Err(IntruderStartError::CaptureLimit);
        }
        let capture_limit = config.capture_limit.max(1);
        let timeout = config.timeout.max(Duration::from_millis(1));
        let notify = config.notify.clone();
        let upstream_tls = tls::upstream_client_config(&config.extra_roots_pem)?;

        // Reserve room for the response body plus a little header/payload
        // overhead. At least one queued result must always fit.
        let result_bytes = capture_limit.saturating_add(64 * 1024);
        let result_capacity = (MAX_QUEUED_RESULT_BYTES / result_bytes).clamp(1, MAX_QUEUED_RESULTS);

        let (commands, mut command_rx) = mpsc::unbounded_channel();
        let (result_tx, results) = mpsc::channel(result_capacity);
        let (startup_tx, startup_rx) = std::sync::mpsc::sync_channel(1);
        let running = Arc::new(AtomicBool::new(false));
        let running_thread = running.clone();

        let thread = thread::Builder::new()
            .name("perseid-intruder".to_string())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(err) => {
                        let _ = startup_tx.send(Err(IntruderStartError::Runtime(err)));
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
                        .http1_max_buf_size(64 * 1024)
                        .http2_max_header_list_size((64 * 1024) as u32)
                        .build(connector);

                runtime.block_on(async move {
                    let _ = startup_tx.send(Ok(()));
                    let mut current_cancel: Option<Arc<AtomicBool>> = None;
                    while let Some(command) = command_rx.recv().await {
                        match command {
                            Command::Start { id, plan, cancel } => {
                                if let Some(old) = current_cancel.replace(cancel.clone()) {
                                    old.store(true, Ordering::Relaxed);
                                }
                                running_thread.store(true, Ordering::Relaxed);
                                tokio::spawn(run_attack(
                                    client.clone(),
                                    id,
                                    plan,
                                    capture_limit,
                                    timeout,
                                    result_tx.clone(),
                                    cancel,
                                    notify.clone(),
                                    running_thread.clone(),
                                    config.cookie_jar.clone(),
                                    #[cfg(test)]
                                    config.max_tracked_tasks.clone(),
                                ));
                            }
                            Command::Stop => {
                                if let Some(cancel) = &current_cancel {
                                    cancel.store(true, Ordering::Relaxed);
                                }
                            }
                            Command::Shutdown => {
                                if let Some(cancel) = &current_cancel {
                                    cancel.store(true, Ordering::Relaxed);
                                }
                                break;
                            }
                        }
                    }
                });
            })
            .map_err(IntruderStartError::Spawn)?;

        match startup_rx.recv() {
            Ok(Ok(())) => {}
            Ok(Err(err)) => {
                let _ = thread.join();
                return Err(err);
            }
            Err(_) => {
                let _ = thread.join();
                return Err(IntruderStartError::Startup);
            }
        }

        Ok(Self {
            commands,
            results,
            running,
            thread: Some(thread),
        })
    }

    /// Queue one attack. Only one attack may run at a time; collect
    /// [`IntruderEvent::Finished`] before starting the next.
    pub fn start_attack(&self, plan: IntruderPlan) -> Result<u64, IntruderError> {
        plan.prepare()?;
        if self
            .running
            .compare_exchange(false, true, Ordering::Relaxed, Ordering::Relaxed)
            .is_err()
        {
            return Err(IntruderError::Busy);
        }
        static NEXT_ID: AtomicU64 = AtomicU64::new(1);
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let cancel = Arc::new(AtomicBool::new(false));
        if self
            .commands
            .send(Command::Start { id, plan, cancel })
            .is_err()
        {
            self.running.store(false, Ordering::Relaxed);
            return Err(IntruderError::Closed);
        }
        Ok(id)
    }

    /// Ask the running attack to stop scheduling new requests. Requests that
    /// are already in flight finish and still produce results.
    pub fn stop(&self) {
        let _ = self.commands.send(Command::Stop);
    }

    /// Take the next result or completion event, if any.
    pub fn try_recv(&mut self) -> Option<IntruderEvent> {
        let event = self.results.try_recv().ok()?;
        if matches!(event, IntruderEvent::Finished { .. }) {
            self.running.store(false, Ordering::Relaxed);
        }
        Some(event)
    }

    /// Whether the engine thread is still alive.
    pub fn is_running(&self) -> bool {
        self.thread
            .as_ref()
            .is_some_and(|thread| !thread.is_finished())
    }
}

impl Drop for Intruder {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Shutdown);
        if let Some(thread) = self.thread.take()
            && thread.join().is_err()
        {
            log::error!("Intruder thread panicked while shutting down");
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_attack(
    client: Client<HttpsConnector<HttpConnector>, ProxyBody>,
    attack_id: u64,
    plan: IntruderPlan,
    capture_limit: usize,
    timeout: Duration,
    results: mpsc::Sender<IntruderEvent>,
    cancel: Arc<AtomicBool>,
    notify: Option<Arc<dyn Fn() + Send + Sync>>,
    running: Arc<AtomicBool>,
    cookie_jar: Option<crate::proxy::cookies::SharedCookieJar>,
    #[cfg(test)] max_tracked_tasks: Option<Arc<AtomicUsize>>,
) {
    let prepared = match plan.prepare() {
        Ok(prepared) => prepared,
        Err(_) => {
            running.store(false, Ordering::Relaxed);
            let _ = results
                .send(IntruderEvent::Finished {
                    attack_id,
                    completed: 0,
                    total: 0,
                    cancelled: false,
                })
                .await;
            return;
        }
    };
    let total = prepared.total();
    let concurrency = plan.concurrency.clamp(1, MAX_INTRUDER_CONCURRENCY);
    let semaphore = Arc::new(Semaphore::new(concurrency));
    let mut tasks = JoinSet::new();
    let mut sequence = 1u64;
    let mut cancelled = false;

    while sequence <= total {
        if cancel.load(Ordering::Relaxed) {
            cancelled = true;
            break;
        }
        // Wait for capacity while joining tasks that finish in the meantime.
        // A long attack must not retain one completed JoinSet entry per
        // request until scheduling ends.
        let permit = loop {
            tokio::select! {
                permit = semaphore.clone().acquire_owned() => break permit,
                _ = tasks.join_next(), if !tasks.is_empty() => {}
            }
        };
        let Ok(permit) = permit else {
            break;
        };
        // A completing task releases its permit just before JoinSet records
        // the task as finished, so reap again before adding another entry.
        reap_completed(&mut tasks);
        // Stop may arrive while this task waits for an in-flight request to
        // release a permit. Recheck after admission so stopping cannot schedule
        // one more request.
        if cancel.load(Ordering::Relaxed) {
            cancelled = true;
            drop(permit);
            break;
        }
        let rendered = match prepared.render(sequence) {
            Ok(rendered) => rendered,
            Err(err) => {
                let event = result_event(
                    attack_id,
                    sequence,
                    Vec::new(),
                    Duration::ZERO,
                    RepeaterResult::Failed {
                        error: err.to_string(),
                    },
                );
                if send_event(&results, event, &notify).await.is_err() {
                    drop(permit);
                    break;
                }
                sequence += 1;
                drop(permit);
                continue;
            }
        };
        let payloads = rendered.payloads;
        let (head, _) = split_raw_request(&rendered.raw);
        let request = match plan.body_bytes(&rendered.raw).and_then(|body| {
            RepeaterRequest::parse(head, body, &plan.scheme, &plan.host)
                .map_err(|error| error.to_string())
        }) {
            Ok(request) => request,
            Err(err) => {
                let event = result_event(
                    attack_id,
                    sequence,
                    payloads,
                    Duration::ZERO,
                    RepeaterResult::Failed {
                        error: err.to_string(),
                    },
                );
                if send_event(&results, event, &notify).await.is_err() {
                    drop(permit);
                    break;
                }
                sequence = sequence.saturating_add(1);
                drop(permit);
                continue;
            }
        };
        if let Some(error) = out_of_scope_target(&request, &plan.scope) {
            let event = result_event(
                attack_id,
                sequence,
                payloads,
                Duration::ZERO,
                RepeaterResult::Failed { error },
            );
            if send_event(&results, event, &notify).await.is_err() {
                drop(permit);
                break;
            }
            sequence = sequence.saturating_add(1);
            drop(permit);
            continue;
        }
        let client = client.clone();
        let results = results.clone();
        let notify = notify.clone();
        let cookie_jar = cookie_jar.clone();
        tasks.spawn(async move {
            let started = Instant::now();
            let result =
                execute_repeater(&client, request, capture_limit, timeout, cookie_jar, true).await;
            let event = result_event(attack_id, sequence, payloads, started.elapsed(), result);
            let _ = send_event(&results, event, &notify).await;
            drop(permit);
        });
        #[cfg(test)]
        record_tracked_tasks(&max_tracked_tasks, &tasks);
        reap_completed(&mut tasks);
        sequence = sequence.saturating_add(1);
    }

    while tasks.join_next().await.is_some() {}
    running.store(false, Ordering::Relaxed);
    let _ = send_event(
        &results,
        IntruderEvent::Finished {
            attack_id,
            completed: total.min(sequence.saturating_sub(1)),
            total,
            cancelled,
        },
        &notify,
    )
    .await;
}

/// Join every task that has already finished without waiting for more.
///
/// A completed entry keeps its task bookkeeping until it is joined, so the
/// scheduling loop reaps continuously instead of only after the last request.
fn reap_completed(tasks: &mut JoinSet<()>) {
    while tasks.try_join_next().is_some() {}
}

#[cfg(test)]
fn record_tracked_tasks(max_tracked_tasks: &Option<Arc<AtomicUsize>>, tasks: &JoinSet<()>) {
    if let Some(max_tracked_tasks) = max_tracked_tasks {
        max_tracked_tasks.fetch_max(tasks.len(), Ordering::Relaxed);
    }
}

/// Reject a generated request whose effective target is outside the plan's
/// scope. The desktop workspace performs the same check before starting, but a
/// payload can still change an absolute-form request line, so every generated
/// request is checked again.
fn out_of_scope_target(request: &RepeaterRequest, scope: &[String]) -> Option<String> {
    if scope.is_empty() {
        return None;
    }
    let Some(host) = request
        .uri
        .parse::<http::Uri>()
        .ok()
        .and_then(|uri| uri.host().map(str::to_string))
    else {
        return Some(
            "the generated request target could not be checked against the enforced scope"
                .to_string(),
        );
    };
    if host_in_scope(&host, scope.iter().map(String::as_str)) {
        None
    } else {
        Some(format!(
            "{host} is outside the enforced scope; the generated request was not sent"
        ))
    }
}

fn result_event(
    attack_id: u64,
    sequence: u64,
    payloads: Vec<String>,
    duration: Duration,
    result: RepeaterResult,
) -> IntruderEvent {
    IntruderEvent::Result(IntruderResult {
        attack_id,
        sequence,
        payloads,
        duration,
        result,
    })
}

async fn send_event(
    results: &mpsc::Sender<IntruderEvent>,
    event: IntruderEvent,
    notify: &Option<Arc<dyn Fn() + Send + Sync>>,
) -> Result<(), ()> {
    results.send(event).await.map_err(|_| ())?;
    if let Some(notify) = notify {
        notify();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::convert::Infallible;

    use bytes::Bytes;
    use http::{Request, Response};
    use http_body_util::Full;
    use hyper::body::Incoming;
    use hyper::service::service_fn;
    use hyper_util::rt::TokioIo;
    use tokio::net::TcpListener;

    use super::super::plan::AttackType;
    use super::*;

    #[tokio::test]
    async fn long_attack_reaps_completed_tasks_while_scheduling() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                tokio::spawn(async move {
                    let service = service_fn(|_request: Request<Incoming>| async {
                        Ok::<_, Infallible>(Response::new(Full::new(Bytes::new())))
                    });
                    let _ = hyper::server::conn::http1::Builder::new()
                        .serve_connection(TokioIo::new(stream), service)
                        .await;
                });
            }
        });

        const TOTAL: u64 = 1_000;
        const CONCURRENCY: usize = 8;
        let max_tracked = Arc::new(AtomicUsize::new(0));
        let mut intruder = Intruder::start(IntruderConfig {
            capture_limit: 1024,
            timeout: Duration::from_secs(5),
            max_tracked_tasks: Some(max_tracked.clone()),
            ..Default::default()
        })
        .unwrap();
        intruder
            .start_attack(IntruderPlan {
                body_escaped: false,
                preserved_body: None,
                attack: AttackType::Sniper,
                scheme: "http".to_string(),
                host: address.to_string(),
                template: "GET /?n=§n§ HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n".to_string(),
                payload_sets: vec![(0..TOTAL).map(|value| value.to_string()).collect()],
                concurrency: CONCURRENCY,
                scope: Vec::new(),
            })
            .unwrap();

        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        let mut received = 0;
        loop {
            assert!(
                tokio::time::Instant::now() < deadline,
                "the long attack did not finish before the test timeout"
            );
            match intruder.try_recv() {
                Some(IntruderEvent::Result(_)) => received += 1,
                Some(IntruderEvent::Finished {
                    completed,
                    total,
                    cancelled,
                    ..
                }) => {
                    assert_eq!(completed, TOTAL);
                    assert_eq!(total, TOTAL);
                    assert!(!cancelled);
                    break;
                }
                None => tokio::time::sleep(Duration::from_millis(1)).await,
            }
        }
        assert_eq!(received, TOTAL);

        // The high-water mark stays near the configured concurrency instead of
        // growing with the number of completed requests.
        let peak = max_tracked.load(Ordering::Relaxed);
        assert!(peak > 0, "the attack never recorded its task bookkeeping");
        assert!(
            peak <= CONCURRENCY * 2,
            "retained {peak} task entries while running a {TOTAL}-request attack at concurrency {CONCURRENCY}"
        );
    }
}
