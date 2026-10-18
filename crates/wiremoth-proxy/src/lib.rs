//! The Wiremoth interception proxy.
//!
//! One call to [`Proxy::start`] binds a listener and returns immediately; the
//! proxy runs on its own Tokio runtime in a dedicated thread so the caller
//! (typically the egui UI thread) never blocks on network activity.
//!
//! Captured traffic is delivered as [`ProxyEvent`]s over a bounded channel.
//! Small and immutable by design: bodies are capped by
//! [`ProxyConfig::capture_limit`], the queue is capped by
//! [`ProxyConfig::event_queue`] (overflow is counted, not buffered), and the
//! consumer owns the resulting history.
//!
//! # Current scope
//!
//! - Plain HTTP/1.1 proxying with full request/response capture.
//! - HTTPS via CONNECT **passthrough** (no TLS interception yet): history
//!   shows the tunnel, not its contents.
//!
//! # Example
//!
//! ```no_run
//! let proxy = wiremoth_proxy::Proxy::start(Default::default())?;
//! println!("listening on {}", proxy.local_addr());
//! while let Some(event) = proxy.try_recv() {
//!     println!("{event:?}");
//! }
//! # Ok::<(), wiremoth_proxy::StartError>(())
//! ```

mod capture;
mod config;
mod events;
mod forward;
mod server;
mod tunnel;
mod util;

pub use config::{DEFAULT_CAPTURE_LIMIT, DEFAULT_EVENT_QUEUE, DEFAULT_LISTEN, ProxyConfig};
pub use events::{
    CapturedBody, ExchangeEnd, ExchangeResult, HeaderList, ProxyEvent, RequestInfo, ResponseHead,
};

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender};
use std::thread;
use std::time::Duration;

use hyper::body::Incoming;
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;
use tokio::net::TcpListener;
use tokio::sync::oneshot;

use crate::capture::TeeBody;

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
}

/// A running proxy.
///
/// Dropping the handle asks the proxy to shut down and waits for its thread.
#[derive(Debug)]
pub struct Proxy {
    addr: SocketAddr,
    events: Receiver<ProxyEvent>,
    dropped: Arc<AtomicU64>,
    paused: Arc<AtomicBool>,
    shutdown: Option<oneshot::Sender<()>>,
    thread: Option<thread::JoinHandle<()>>,
}

impl Proxy {
    /// Bind `config.listen` and start forwarding traffic in the background.
    ///
    /// Binding happens synchronously, so address conflicts (e.g. another tool
    /// already on port 8080) are reported with [`StartError`] right here.
    pub fn start(config: ProxyConfig) -> Result<Self, StartError> {
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
        let (shutdown_tx, shutdown_rx) = oneshot::channel();

        let dropped = Arc::new(AtomicU64::new(0));
        let paused = Arc::new(AtomicBool::new(false));

        let mut connector = HttpConnector::new();
        connector.set_connect_timeout(Some(config.connect_timeout));
        connector.enforce_http(true);
        let client: Client<HttpConnector, TeeBody<Incoming>> =
            Client::builder(TokioExecutor::new())
                .http1_preserve_header_case(true)
                .build(connector);

        let ctx = Arc::new(server::Ctx {
            client,
            tx: tx as SyncSender<ProxyEvent>,
            notify: config.notify.clone(),
            next_id: AtomicU64::new(1),
            dropped: dropped.clone(),
            paused: paused.clone(),
            capture_limit: config.capture_limit,
            connect_timeout: config.connect_timeout,
        });

        let thread = thread::Builder::new()
            .name("wiremoth-proxy".to_string())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(err) => {
                        log::error!("could not start the proxy runtime: {err}");
                        return;
                    }
                };
                runtime.block_on(async move {
                    let listener = TcpListener::from_std(listener)
                        .expect("listener was bound in this process");
                    server::serve(listener, ctx, shutdown_rx).await;
                });
            })
            .map_err(StartError::Spawn)?;

        Ok(Self {
            addr,
            events,
            dropped,
            paused,
            shutdown: Some(shutdown_tx),
            thread: Some(thread),
        })
    }

    /// The bound address, e.g. `127.0.0.1:8080`.
    pub fn local_addr(&self) -> SocketAddr {
        self.addr
    }

    /// Take the next capture event, if any.
    pub fn try_recv(&self) -> Option<ProxyEvent> {
        self.events.try_recv().ok()
    }

    /// Blocking receive, mainly useful for tests and headless tooling.
    pub fn recv_timeout(&self, timeout: Duration) -> Result<ProxyEvent, RecvTimeoutError> {
        self.events.recv_timeout(timeout)
    }

    /// Events dropped so far because the consumer did not keep up.
    pub fn dropped_events(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// While paused, the proxy keeps forwarding but emits no capture events.
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
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
