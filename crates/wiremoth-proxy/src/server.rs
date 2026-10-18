//! TCP accept loop and per-connection HTTP/1 service.

use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{SyncSender, TrySendError};
use std::time::Duration;

use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Method, Request, Response};
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioIo;
use log::{debug, info, warn};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;

use crate::capture::TeeBody;
use crate::events::ProxyEvent;
use crate::util::ProxyBody;
use crate::{forward, tunnel};

/// Shared per-proxy state handed to every connection task.
pub(crate) struct Ctx {
    pub(crate) client: Client<HttpConnector, TeeBody<Incoming>>,
    pub(crate) tx: SyncSender<ProxyEvent>,
    pub(crate) notify: Option<Arc<dyn Fn() + Send + Sync>>,
    pub(crate) next_id: AtomicU64,
    pub(crate) dropped: Arc<AtomicU64>,
    pub(crate) paused: Arc<AtomicBool>,
    pub(crate) capture_limit: usize,
    pub(crate) connect_timeout: Duration,
}

impl Ctx {
    pub(crate) fn next_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    /// Enqueue a capture event. Never blocks and never grows memory: when the
    /// consumer is too slow, the event is dropped and counted.
    pub(crate) fn emit(&self, event: ProxyEvent) {
        if self.paused.load(Ordering::Relaxed) {
            return;
        }
        match self.tx.try_send(event) {
            Ok(()) => {
                if let Some(notify) = &self.notify {
                    notify();
                }
            }
            Err(TrySendError::Full(_)) => {
                self.dropped.fetch_add(1, Ordering::Relaxed);
            }
            Err(TrySendError::Disconnected(_)) => {}
        }
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
            Ok((stream, peer)) => spawn_connection(stream, peer, ctx.clone()),
            Err(err) => {
                warn!("accept failed: {err}");
                // Back off briefly so a persistent error cannot spin the loop.
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
    }
}

fn spawn_connection(stream: TcpStream, peer: SocketAddr, ctx: Arc<Ctx>) {
    let _ = stream.set_nodelay(true);
    tokio::spawn(async move {
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
    } else {
        forward::handle(ctx, req).await
    }
}
