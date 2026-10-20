//! TCP accept loop and per-connection HTTP/1 service.

use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{SyncSender, TrySendError};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Method, Request, Response};
use hyper_rustls::HttpsConnector;
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioIo;
use log::{debug, info, warn};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc, oneshot};

use crate::events::ProxyEvent;
use crate::intercept::{InterceptDecision, InterceptItem};
use crate::policy::{Policy, PolicySnapshot};
use crate::tls::CertificateAuthority;
use crate::util::ProxyBody;
use crate::{forward, portal, tunnel};

/// Shared per-proxy state handed to every connection task.
pub(crate) struct Ctx {
    pub(crate) client: Client<HttpsConnector<HttpConnector>, ProxyBody>,
    /// The local CA that mints interception certificates. `None` keeps CONNECT
    /// traffic on the passthrough path even when interception is enabled.
    /// Swapped at runtime by [`crate::Proxy::set_tls_authority`].
    pub(crate) tls_ca: Arc<RwLock<Option<Arc<CertificateAuthority>>>>,
    /// Address this proxy is listening on, used to recognise the local CA
    /// download page without forwarding the request to itself.
    pub(crate) listen: SocketAddr,
    pub(crate) tx: SyncSender<ProxyEvent>,
    pub(crate) notify: Option<Arc<dyn Fn() + Send + Sync>>,
    pub(crate) dropped: Arc<AtomicU64>,
    pub(crate) paused: Arc<AtomicBool>,
    pub(crate) capture_limit: usize,
    pub(crate) intercept_limit: usize,
    pub(crate) policy: Arc<RwLock<Policy>>,
    pub(crate) intercepts: mpsc::Sender<InterceptItem>,
    pub(crate) blocked_by_scope: Arc<AtomicU64>,
    pub(crate) connect_timeout: Duration,
    pub(crate) connection_slots: Arc<Semaphore>,
    pub(crate) tunnel_slots: Arc<Semaphore>,
    pub(crate) rejected_connections: Arc<AtomicU64>,
    pub(crate) rejected_tunnels: Arc<AtomicU64>,
}

impl Ctx {
    /// Exchange ids are unique for the lifetime of the process, so a retained
    /// history stays coherent when the proxy is restarted.
    pub(crate) fn next_id(&self) -> u64 {
        static NEXT_ID: AtomicU64 = AtomicU64::new(1);
        NEXT_ID.fetch_add(1, Ordering::Relaxed)
    }

    /// Capture is decided when an exchange starts, so pausing never strands
    /// an exchange that is already in flight.
    pub(crate) fn capture_enabled(&self) -> bool {
        !self.paused.load(Ordering::Relaxed)
    }

    /// Enqueue a capture event. Never blocks and never grows memory: when the
    /// consumer is too slow, the event is dropped and counted.
    ///
    /// Callers gate events for exchanges that started while capture was
    /// paused; this method deliberately does not re-check the pause state.
    pub(crate) fn emit(&self, event: ProxyEvent) {
        match self.tx.try_send(event) {
            Ok(()) => self.wake(),
            Err(TrySendError::Full(_)) => {
                self.dropped.fetch_add(1, Ordering::Relaxed);
            }
            Err(TrySendError::Disconnected(_)) => {}
        }
    }

    pub(crate) fn wake(&self) {
        if let Some(notify) = &self.notify {
            notify();
        }
    }

    pub(crate) fn policy(&self) -> PolicySnapshot {
        self.policy
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .snapshot()
    }

    /// Snapshot the certificate authority for one CONNECT decision.
    pub(crate) fn tls_authority(&self) -> Option<Arc<CertificateAuthority>> {
        self.tls_ca
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// Hold a message for a UI decision. The intercept channel applies
    /// backpressure when the UI is not keeping up, instead of dropping the
    /// message and hanging the connection forever.
    pub(crate) async fn ask_interceptor(
        &self,
        item: InterceptItem,
        decision: oneshot::Receiver<InterceptDecision>,
    ) -> InterceptDecision {
        match self.intercepts.send(item).await {
            Ok(()) => {
                self.wake();
                decision.await.unwrap_or(InterceptDecision::Abort)
            }
            Err(_) => InterceptDecision::Abort,
        }
    }

    pub(crate) fn block_by_scope(&self) {
        self.blocked_by_scope.fetch_add(1, Ordering::Relaxed);
    }
}

/// Accept connections until `shutdown` fires.
pub(crate) async fn serve(
    listener: TcpListener,
    ctx: Arc<Ctx>,
    mut shutdown: oneshot::Receiver<()>,
) {
    info!(
        "proxy listening on {}",
        listener
            .local_addr()
            .map(|a| a.to_string())
            .unwrap_or_else(|_| "?".into())
    );
    loop {
        let accepted = tokio::select! {
            _ = &mut shutdown => {
                info!("proxy shutting down");
                return;
            }
            accepted = listener.accept() => accepted,
        };
        match accepted {
            Ok((stream, peer)) => {
                let Some(permit) = acquire_connection(&ctx, peer) else {
                    continue;
                };
                spawn_connection(stream, peer, ctx.clone(), permit);
            }
            Err(err) => {
                warn!("accept failed: {err}");
                // Back off briefly so a persistent error cannot spin the loop.
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
    }
}

fn acquire_connection(ctx: &Ctx, peer: SocketAddr) -> Option<OwnedSemaphorePermit> {
    match ctx.connection_slots.clone().try_acquire_owned() {
        Ok(permit) => Some(permit),
        Err(_) => {
            ctx.rejected_connections.fetch_add(1, Ordering::Relaxed);
            debug!("connection from {peer} rejected: concurrent connection limit reached");
            None
        }
    }
}

fn spawn_connection(
    stream: TcpStream,
    peer: SocketAddr,
    ctx: Arc<Ctx>,
    permit: OwnedSemaphorePermit,
) {
    let _ = stream.set_nodelay(true);
    tokio::spawn(async move {
        // Holding the permit for the lifetime of the connection task is what
        // enforces the concurrent-connection limit.
        let _permit = permit;
        let io = TokioIo::new(stream);
        let service = service_fn(move |req| {
            let ctx = ctx.clone();
            async move { Ok::<_, Infallible>(dispatch(ctx, req).await) }
        });
        let connection = http1::Builder::new()
            .preserve_header_case(true)
            .serve_connection(io, service)
            .with_upgrades();
        if let Err(err) = connection.await {
            debug!("connection from {peer} closed with error: {err}");
        }
    });
}

async fn dispatch(ctx: Arc<Ctx>, req: Request<Incoming>) -> Response<ProxyBody> {
    if req.method() == Method::CONNECT {
        tunnel::handle(ctx, req).await
    } else if portal::matches(&req, ctx.listen) {
        portal::handle(&ctx, req)
    } else {
        forward::handle(ctx, req).await
    }
}
