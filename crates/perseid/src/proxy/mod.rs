//! The Perseid interception proxy, embedded in the main crate.
//!
//! The proxy is available to Rust callers as `perseid::proxy`. One call to
//! [`Proxy::start`] binds a listener, waits for the background runtime to
//! become ready, and returns; the proxy runs on its own Tokio runtime in a
//! dedicated thread so the caller (typically the egui UI thread) never blocks
//! on network activity afterward.
//!
//! Captured traffic is delivered as [`ProxyEvent`]s over a bounded channel.
//! Small and immutable by design: bodies are capped by
//! [`ProxyConfig::capture_limit`], the queue is capped by
//! [`ProxyConfig::event_queue`] (overflow is counted, not buffered), concurrent
//! work is capped by [`ProxyConfig::max_connections`] and
//! [`ProxyConfig::max_tunnels`], and the consumer owns the resulting history.
//!
//! Interception uses a separate bounded queue of [`InterceptItem`]s. The
//! proxy waits for each item to be forwarded or dropped, and the producer
//! applies backpressure instead of dropping a held exchange. Bodies are
//! buffered only when their size is known and within
//! [`ProxyConfig::intercept_limit`]; larger or streaming messages are held for
//! header edits while their bodies pass through untouched.
//!
//! [`Repeater`] is a separate direct HTTP/HTTPS client for manual replay. It
//! has its own Tokio runtime and connection pool, so it continues to work when
//! the proxy is stopped, and it does not pass traffic through the proxy's
//! interception, scope, or rewrite policy. [`Intruder`] reuses that direct
//! client for payload attacks and applies an optional scope list from
//! [`IntruderPlan`] to each generated request.
//!
//! # Current scope
//!
//! - HTTP/1.1 and HTTP/2 proxying with full request/response capture. The
//!   plain listener accepts h2c prior knowledge; intercepted TLS sessions and
//!   HTTPS origins negotiate `h2` through ALPN, with HTTP/1.1 as the fallback.
//!   Cleartext origins use HTTP/1.1.
//! - Request and response interception with editable heads and buffered UTF-8
//!   bodies, plus explicit forward and drop decisions.
//! - Opt-in scope enforcement for HTTP, HTTPS, and CONNECT targets, and
//!   request/response match/replace rules compiled from
//!   [`ProxySettings`].
//! - HTTPS via CONNECT either passes through as an opaque tunnel or is decrypted
//!   when [`ProxySettings::tls_intercept`] is on and a
//!   [`CertificateAuthority`] is supplied in [`ProxyConfig::tls`]. Each
//!   decrypted request travels the same interception, scope, rewriting, and
//!   capture pipeline as plain HTTP.
//! - A local certificate page at the listener address (and `http://perseid/`)
//!   downloads the public CA certificate. Portal requests are not captured.
//! - Direct Repeater replay with its own runtime and HTTP/HTTPS client. Edited
//!   requests can target the connection authority independently of a preserved
//!   virtual `Host` header, and response bodies are bounded by
//!   [`RepeaterConfig::capture_limit`]. WebSocket replay is not supported yet.
//! - Direct Intruder attacks using the same client. [`AttackType`] implements
//!   Sniper, Battering ram, Pitchfork, and Cluster bomb with `$$` markers,
//!   lazy request generation, and bounded result queues. The engine clamps
//!   concurrency to [`MAX_INTRUDER_CONCURRENCY`].
//!
//! # Example
//!
//! ```no_run
//! let proxy = perseid::proxy::Proxy::start(Default::default())?;
//! println!("listening on {}", proxy.local_addr());
//! while let Some(event) = proxy.try_recv() {
//!     println!("{event:?}");
//! }
//! # Ok::<(), perseid::proxy::StartError>(())
//! ```

mod capture;
mod config;
mod events;
mod forward;
mod intercept;
mod intruder;
mod message;
mod policy;
mod portal;
mod repeater;
mod rules;
mod server;
mod tls;
mod tunnel;
mod util;

pub use config::{
    DEFAULT_CAPTURE_LIMIT, DEFAULT_CONNECT_TIMEOUT, DEFAULT_EVENT_QUEUE, DEFAULT_INTERCEPT_LIMIT,
    DEFAULT_LISTEN, DEFAULT_MAX_CONNECTIONS, DEFAULT_MAX_TUNNELS, ProxyConfig, ProxySettings,
    TlsConfig,
};
pub use events::{
    CapturedBody, ExchangeEnd, ExchangeResult, HeaderList, ProxyEvent, RequestInfo, ResponseHead,
};
pub use intercept::{InterceptError, InterceptItem, InterceptKey, InterceptKind};
pub use intruder::{
    AttackType, Intruder, IntruderConfig, IntruderError, IntruderEvent, IntruderPlan,
    IntruderResult, IntruderStartError, MARKER, MAX_INTRUDER_CONCURRENCY, auto_mark, clear_markers,
    marker_ranges, toggle_marker, update_host_header,
};
pub use policy::host_in_scope;
pub use repeater::{
    DEFAULT_REPEATER_TIMEOUT, Repeater, RepeaterConfig, RepeaterError, RepeaterRequest,
    RepeaterResponse, RepeaterResult, RepeaterStartError,
};
pub use rules::{MatchReplaceRule, RuleDirection, RuleError, RulePart};
pub use tls::{CaError, CertificateAuthority, UpstreamTlsError};

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender};
use std::sync::{Arc, RwLock};
use std::thread;
use std::time::Duration;

use hyper_rustls::{DefaultServerNameResolver, HttpsConnector};
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;
use tokio::net::TcpListener;
use tokio::sync::{Semaphore, mpsc, oneshot};

use crate::proxy::policy::PolicySnapshot;
use crate::proxy::util::ProxyBody;

/// Failure to start the proxy itself (the listener, not individual requests).
#[derive(Debug, thiserror::Error)]
pub enum StartError {
    #[error("could not bind {addr}: {source}")]
    Bind {
        addr: SocketAddr,
        #[source]
        source: std::io::Error,
    },
    #[error("could not start the proxy thread: {0}")]
    Spawn(#[source] std::io::Error),
    #[error("could not initialize the proxy runtime: {0}")]
    Runtime(#[source] std::io::Error),
    #[error(transparent)]
    Tls(#[from] UpstreamTlsError),
    #[error(transparent)]
    Rule(#[from] RuleError),
    #[error("the proxy thread stopped before it was ready")]
    Startup,
}

/// A running proxy.
///
/// Dropping the handle asks the proxy to shut down and waits for its thread.
#[derive(Debug)]
pub struct Proxy {
    addr: SocketAddr,
    events: Receiver<ProxyEvent>,
    intercepts: mpsc::Receiver<InterceptItem>,
    policy: Arc<RwLock<PolicySnapshot>>,
    tls_ca: Arc<RwLock<Option<Arc<CertificateAuthority>>>>,
    dropped: Arc<AtomicU64>,
    paused: Arc<AtomicBool>,
    blocked_by_scope: Arc<AtomicU64>,
    rejected_connections: Arc<AtomicU64>,
    rejected_tunnels: Arc<AtomicU64>,
    shutdown: Option<oneshot::Sender<()>>,
    thread: Option<thread::JoinHandle<()>>,
}

impl Proxy {
    /// Bind `config.listen` and start forwarding traffic in the background.
    ///
    /// Binding happens synchronously, so address conflicts (e.g. another tool
    /// already on port 8080) are reported with [`StartError`] right here. The
    /// call also waits for the runtime thread to become ready, so a successful
    /// return means the proxy is actually accepting connections.
    pub fn start(config: ProxyConfig) -> Result<Self, StartError> {
        config.settings.validate()?;
        let policy = Arc::new(RwLock::new(PolicySnapshot::from_settings(
            &config.settings,
        )?));

        let listener =
            std::net::TcpListener::bind(config.listen).map_err(|source| StartError::Bind {
                addr: config.listen,
                source,
            })?;
        listener
            .set_nonblocking(true)
            .map_err(|source| StartError::Bind {
                addr: config.listen,
                source,
            })?;
        let addr = listener.local_addr().map_err(|source| StartError::Bind {
            addr: config.listen,
            source,
        })?;

        let (tx, events) = std::sync::mpsc::sync_channel(config.event_queue.max(1));
        let (intercept_tx, intercepts) = mpsc::channel(config.max_connections.max(1));
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let (startup_tx, startup_rx) = std::sync::mpsc::sync_channel(1);

        let dropped = Arc::new(AtomicU64::new(0));
        let paused = Arc::new(AtomicBool::new(false));
        let blocked_by_scope = Arc::new(AtomicU64::new(0));
        let rejected_connections = Arc::new(AtomicU64::new(0));
        let rejected_tunnels = Arc::new(AtomicU64::new(0));
        let connection_slots = Arc::new(Semaphore::new(config.max_connections.max(1)));
        let tunnel_slots = Arc::new(Semaphore::new(config.max_tunnels.max(1)));

        let upstream_tls = tls::upstream_client_config(&config.tls.extra_roots_pem)?;
        let mut connector = HttpConnector::new();
        connector.set_connect_timeout(Some(config.connect_timeout));
        // Scheme enforcement lives in the HTTPS connector, which knows how to
        // route `http` and `https` targets.
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
                .build(connector);

        let tls_ca = Arc::new(RwLock::new(config.tls.authority.clone()));
        let ctx = Arc::new(server::Ctx {
            client,
            tls_ca: tls_ca.clone(),
            listen: addr,
            tx: tx as SyncSender<ProxyEvent>,
            notify: config.notify.clone(),
            dropped: dropped.clone(),
            paused: paused.clone(),
            capture_limit: config.capture_limit,
            intercept_limit: config.intercept_limit,
            policy: policy.clone(),
            intercepts: intercept_tx,
            blocked_by_scope: blocked_by_scope.clone(),
            connect_timeout: config.connect_timeout,
            connection_slots,
            tunnel_slots,
            rejected_connections: rejected_connections.clone(),
            rejected_tunnels: rejected_tunnels.clone(),
        });

        let thread = thread::Builder::new()
            .name("perseid-proxy".to_string())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(err) => {
                        let _ = startup_tx.send(Err(StartError::Runtime(err)));
                        return;
                    }
                };
                runtime.block_on(async move {
                    let listener = match TcpListener::from_std(listener) {
                        Ok(listener) => listener,
                        Err(err) => {
                            let _ = startup_tx.send(Err(StartError::Runtime(err)));
                            return;
                        }
                    };
                    if startup_tx.send(Ok(())).is_err() {
                        return;
                    }
                    drop(startup_tx);
                    server::serve(listener, ctx, shutdown_rx).await;
                });
            })
            .map_err(StartError::Spawn)?;

        match startup_rx.recv() {
            Ok(Ok(())) => {}
            Ok(Err(err)) => {
                let _ = thread.join();
                return Err(err);
            }
            Err(_) => {
                let _ = thread.join();
                return Err(StartError::Startup);
            }
        }

        Ok(Self {
            addr,
            events,
            intercepts,
            policy,
            tls_ca,
            dropped,
            paused,
            blocked_by_scope,
            rejected_connections,
            rejected_tunnels,
            shutdown: Some(shutdown_tx),
            thread: Some(thread),
        })
    }

    /// The bound address, e.g. `127.0.0.1:8080`.
    pub fn local_addr(&self) -> SocketAddr {
        self.addr
    }

    /// Whether the proxy runtime thread is still running.
    ///
    /// A successful [`Proxy::start`] guarantees this is true initially. If it
    /// becomes false, the proxy stopped unexpectedly and the handle should be
    /// dropped and restarted.
    pub fn is_running(&self) -> bool {
        self.thread
            .as_ref()
            .is_some_and(|thread| !thread.is_finished())
    }

    /// Take the next capture event, if any.
    ///
    /// `None` means the queue is empty; use [`Proxy::is_running`] to tell that
    /// apart from a stopped proxy.
    pub fn try_recv(&self) -> Option<ProxyEvent> {
        self.events.try_recv().ok()
    }

    /// Blocking receive, mainly useful for tests and headless tooling.
    pub fn recv_timeout(&self, timeout: Duration) -> Result<ProxyEvent, RecvTimeoutError> {
        self.events.recv_timeout(timeout)
    }

    /// Take the next message waiting for an interception decision.
    pub fn try_recv_intercept(&mut self) -> Option<InterceptItem> {
        self.intercepts.try_recv().ok()
    }

    /// Await the next message waiting for an interception decision.
    ///
    /// This is mainly useful for tests and headless consumers. The desktop UI
    /// uses [`Proxy::try_recv_intercept`] from its frame loop.
    pub async fn recv_intercept(&mut self) -> Option<InterceptItem> {
        self.intercepts.recv().await
    }

    /// Replace the runtime interception, scope, and match/replace policy.
    ///
    /// The previous policy stays in effect when a rule fails to compile.
    pub fn set_settings(&self, settings: ProxySettings) -> Result<(), RuleError> {
        let policy = PolicySnapshot::from_settings(&settings)?;
        *self
            .policy
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = policy;
        Ok(())
    }

    /// Replace the certificate authority used for TLS interception.
    ///
    /// Existing tunnels keep the authority they started with; new CONNECT
    /// requests pick this one up immediately. Pass `None` to fall back to
    /// passthrough without restarting the proxy.
    pub fn set_tls_authority(&self, authority: Option<Arc<CertificateAuthority>>) {
        *self
            .tls_ca
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = authority;
    }

    /// Exchanges blocked because they were outside the enforced scope.
    pub fn blocked_by_scope(&self) -> u64 {
        self.blocked_by_scope.load(Ordering::Relaxed)
    }

    /// Events dropped so far because the consumer did not keep up.
    pub fn dropped_events(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// Client connections closed because [`ProxyConfig::max_connections`] was
    /// reached.
    pub fn rejected_connections(&self) -> u64 {
        self.rejected_connections.load(Ordering::Relaxed)
    }

    /// CONNECT requests rejected because [`ProxyConfig::max_tunnels`] was
    /// reached.
    pub fn rejected_tunnels(&self) -> u64 {
        self.rejected_tunnels.load(Ordering::Relaxed)
    }

    /// While paused, the proxy keeps forwarding but does not start recording
    /// new exchanges. Exchanges already in flight still deliver their terminal
    /// events, so history never strands rows.
    pub fn set_paused(&self, paused: bool) {
        self.paused.store(paused, Ordering::Relaxed);
    }

    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::Relaxed)
    }
}

impl Drop for Proxy {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(thread) = self.thread.take()
            && thread.join().is_err()
        {
            log::error!("proxy thread panicked while shutting down");
        }
    }
}
