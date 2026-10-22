//! CONNECT handling: passthrough tunnels and TLS interception.
//!
//! With a certificate authority configured and [`ProxySettings::tls_intercept`]
//! enabled, Perseid terminates the client TLS session with a certificate
//! minted for the requested host, serves HTTP/2 or HTTP/1.1 inside the tunnel
//! according to ALPN, and sends each decrypted request through the same
//! interception, scope, rewriting, and capture pipeline as plain HTTP. Without
//! interception, or for hosts on the pass-through list, the CONNECT stays an
//! opaque byte tunnel.
//!
//! [`ProxySettings::tls_intercept`]: crate::proxy::ProxySettings::tls_intercept

use std::convert::Infallible;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Instant, SystemTime};

use http::{Request, Response, StatusCode};
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto;
use log::{debug, warn};
use rustls::ServerConfig;
use tokio::net::TcpStream;
use tokio::sync::OwnedSemaphorePermit;
use tokio_rustls::TlsAcceptor;

use crate::proxy::events::{
    CapturedBody, ExchangeEnd, ExchangeResult, HeaderList, ProxyEvent, RequestInfo, ResponseHead,
};
use crate::proxy::forward;
use crate::proxy::server::Ctx;
use crate::proxy::tls::CertificateAuthority;
use crate::proxy::util::{ProxyBody, empty_body, header_list, text_response, version_str};

pub(crate) async fn handle(ctx: Arc<Ctx>, req: Request<Incoming>) -> Response<ProxyBody> {
    let id = ctx.next_id();
    let started = Instant::now();
    let timestamp = SystemTime::now();
    let capture = ctx.capture_enabled();
    let version = version_str(req.version());
    let raw_uri = req.uri().clone();
    let headers = header_list(req.headers());

    let Some(authority) = raw_uri.authority().cloned() else {
        let message = "CONNECT requires a host:port target".to_string();
        if capture {
            ctx.emit(ProxyEvent::Started {
                id,
                timestamp,
                request: connect_request_info(&raw_uri, version.clone(), &headers),
            });
            finish_failed(&ctx, id, started, message.clone());
        }
        return text_response(StatusCode::BAD_REQUEST, &message);
    };

    let host = authority.host().to_string();
    let authority_text = authority.to_string();
    let port = authority.port_u16().unwrap_or(443);
    let policy = ctx.policy();

    if policy.scope_enforced() && !policy.in_scope(authority.as_ref()) {
        ctx.block_by_scope();
        let message = "target is outside the configured scope".to_string();
        if capture {
            ctx.emit(ProxyEvent::Started {
                id,
                timestamp,
                request: connect_request_info(&raw_uri, version.clone(), &headers),
            });
            finish_failed(&ctx, id, started, message.clone());
        }
        return text_response(StatusCode::FORBIDDEN, &message);
    }

    if capture {
        ctx.emit(ProxyEvent::Started {
            id,
            timestamp,
            request: connect_request_info(&raw_uri, version.clone(), &headers),
        });
    }

    let tunnel_permit = match ctx.tunnel_slots.clone().try_acquire_owned() {
        Ok(permit) => permit,
        Err(_) => {
            ctx.rejected_tunnels.fetch_add(1, Ordering::Relaxed);
            let message = "CONNECT tunnel limit reached".to_string();
            if capture {
                finish_failed(&ctx, id, started, message.clone());
            }
            return text_response(StatusCode::SERVICE_UNAVAILABLE, &message);
        }
    };

    if let Some(ca) = ctx.tls_authority()
        && policy.tls_intercept()
        && !policy.tls_passthrough(authority.as_ref())
    {
        return intercept(
            ctx,
            req,
            ca,
            host,
            authority_text,
            tunnel_permit,
            id,
            started,
            capture,
            version,
        );
    }

    // Connect upstream before answering 200 so failures surface as a clean
    // proxy error rather than a tunnel that dies immediately.
    let upstream = match tokio::time::timeout(
        ctx.connect_timeout,
        TcpStream::connect((host.as_str(), port)),
    )
    .await
    {
        Ok(Ok(stream)) => stream,
        Ok(Err(err)) => {
            let message = format!("could not connect to {host}:{port}: {err}");
            if capture {
                finish_failed(&ctx, id, started, message.clone());
            }
            return text_response(StatusCode::BAD_GATEWAY, &message);
        }
        Err(_) => {
            let message = format!("connection to {host}:{port} timed out");
            if capture {
                finish_failed(&ctx, id, started, message.clone());
            }
            return text_response(StatusCode::GATEWAY_TIMEOUT, &message);
        }
    };
    let _ = upstream.set_nodelay(true);

    if capture {
        ctx.emit(ProxyEvent::ResponseHead {
            id,
            head: ResponseHead {
                version,
                status: 200,
                reason: "Connection Established".to_string(),
                headers: Vec::new(),
            },
        });
    }

    tokio::spawn(async move {
        // The tunnel permit stays with the task that owns the upgraded socket.
        let _permit = tunnel_permit;
        let result = match hyper::upgrade::on(req).await {
            Ok(upgraded) => {
                debug!("CONNECT upgrade resolved for {id}");
                match pump(TokioIo::new(upgraded), upstream, id).await {
                    Ok((client_to_server, server_to_client)) => {
                        debug!(
                            "CONNECT tunnel {id} closed: {client_to_server} up, {server_to_client} down"
                        );
                        ExchangeResult::Tunnel {
                            client_to_server,
                            server_to_client,
                        }
                    }
                    Err(err) => ExchangeResult::Failed {
                        error: format!("tunnel error: {err}"),
                    },
                }
            }
            Err(err) => ExchangeResult::Failed {
                error: format!("client upgrade failed: {err}"),
            },
        };
        if capture {
            let end = ExchangeEnd {
                duration: started.elapsed(),
                request_body: CapturedBody::default(),
                result,
            };
            ctx.emit(ProxyEvent::Finished { id, end });
        }
    });

    let mut response = Response::new(empty_body());
    *response.status_mut() = StatusCode::OK;
    response
}

/// Answer the CONNECT, upgrade, then terminate TLS and serve HTTP/2 or HTTP/1.1
/// inside the tunnel. The upgrade work happens in a task: the intercepted
/// exchange's lifetime is the TLS session, which can carry many requests.
#[allow(clippy::too_many_arguments)]
fn intercept(
    ctx: Arc<Ctx>,
    req: Request<Incoming>,
    ca: Arc<CertificateAuthority>,
    host: String,
    authority: String,
    tunnel_permit: OwnedSemaphorePermit,
    id: u64,
    started: Instant,
    capture: bool,
    version: String,
) -> Response<ProxyBody> {
    debug!("intercepting TLS for CONNECT {id} ({authority})");
    let server_config = ca.server_config(Some(host));

    if capture {
        ctx.emit(ProxyEvent::ResponseHead {
            id,
            head: ResponseHead {
                version,
                status: 200,
                reason: "Connection Established".to_string(),
                headers: Vec::new(),
            },
        });
    }

    tokio::spawn(async move {
        // The tunnel permit stays with the task that owns the TLS session.
        let _permit = tunnel_permit;
        let requests = Arc::new(AtomicU64::new(0));
        let outcome =
            serve_intercepted(ctx.clone(), req, server_config, authority, requests.clone()).await;
        let count = requests.load(Ordering::Relaxed);
        let result = match outcome {
            Ok(()) => ExchangeResult::InterceptedTls { requests: count },
            // A session that carried requests ended when the client went away;
            // that is normal browsing, not a proxy failure.
            Err(err) if count > 0 => {
                debug!("CONNECT interception {id} ended after {count} requests: {err}");
                ExchangeResult::InterceptedTls { requests: count }
            }
            Err(err) => {
                warn!("CONNECT interception {id} failed: {err}");
                ExchangeResult::Failed { error: err }
            }
        };
        if capture {
            let end = ExchangeEnd {
                duration: started.elapsed(),
                request_body: CapturedBody::default(),
                result,
            };
            ctx.emit(ProxyEvent::Finished { id, end });
        }
    });

    let mut response = Response::new(empty_body());
    *response.status_mut() = StatusCode::OK;
    response
}

/// Complete the client upgrade, perform the TLS handshake, and serve requests
/// until the client closes the tunnel. `authority` is the CONNECT target and
/// becomes the fallback when a decrypted request omits its `Host` header.
async fn serve_intercepted(
    ctx: Arc<Ctx>,
    req: Request<Incoming>,
    server_config: Arc<ServerConfig>,
    authority: String,
    requests: Arc<AtomicU64>,
) -> Result<(), String> {
    let upgraded = hyper::upgrade::on(req)
        .await
        .map_err(|err| format!("client upgrade failed: {err}"))?;
    let acceptor = TlsAcceptor::from(server_config);
    let tls = acceptor
        .accept(TokioIo::new(upgraded))
        .await
        .map_err(|err| format!("TLS handshake failed: {err}"))?;
    let io = TokioIo::new(tls);
    debug!("TLS intercepted session established for {authority}");

    let service = service_fn(move |req: Request<Incoming>| {
        let ctx = ctx.clone();
        let authority = authority.clone();
        let requests = requests.clone();
        async move {
            requests.fetch_add(1, Ordering::Relaxed);
            Ok::<_, Infallible>(forward::handle_intercepted(ctx, req, authority).await)
        }
    });

    // ALPN normally tells the client which protocol to use; the auto builder
    // also detects the HTTP/2 preface for clients that do not negotiate ALPN.
    // Upgrades are required for decrypted WebSocket handshakes (`wss://`);
    // HTTP/2 does not use this path, so the auto builder still serves h2.
    auto::Builder::new(TokioExecutor::new())
        .preserve_header_case(true)
        .serve_connection_with_upgrades(io, service)
        .await
        .map_err(|err| format!("intercepted connection error: {err}"))
}

fn connect_request_info(uri: &http::Uri, version: String, headers: &HeaderList) -> RequestInfo {
    RequestInfo {
        method: "CONNECT".to_string(),
        uri: uri.to_string(),
        scheme: "https".to_string(),
        host: uri.to_string(),
        path: String::new(),
        version,
        headers: headers.clone(),
    }
}

fn finish_failed(ctx: &Ctx, id: u64, started: Instant, message: String) {
    debug!("CONNECT failed: {message}");
    let end = ExchangeEnd {
        duration: started.elapsed(),
        request_body: CapturedBody::default(),
        result: ExchangeResult::Failed { error: message },
    };
    ctx.emit(ProxyEvent::Finished { id, end });
}

/// Relay bytes in both directions between the client and the origin until
/// both sides have closed. Each direction is an independent copy task joined
/// together, so a slow or stalled direction cannot block the other.
async fn pump<A, B>(client: A, server: B, id: u64) -> std::io::Result<(u64, u64)>
where
    A: tokio::io::AsyncRead + tokio::io::AsyncWrite,
    B: tokio::io::AsyncRead + tokio::io::AsyncWrite,
{
    use tokio::io::{AsyncWriteExt, copy};

    let (mut client_read, mut client_write) = tokio::io::split(client);
    let (mut server_read, mut server_write) = tokio::io::split(server);

    let upstream = async {
        let bytes = copy(&mut client_read, &mut server_write).await?;
        debug!("CONNECT {id}: client->server done after {bytes} bytes");
        server_write.shutdown().await?;
        debug!("CONNECT {id}: client->server write half shut down");
        Ok::<u64, std::io::Error>(bytes)
    };
    let downstream = async {
        let bytes = copy(&mut server_read, &mut client_write).await?;
        debug!("CONNECT {id}: server->client done after {bytes} bytes");
        let _ = client_write.shutdown().await;
        Ok::<u64, std::io::Error>(bytes)
    };

    let (client_to_server, server_to_client) = tokio::join!(upstream, downstream);
    Ok((client_to_server?, server_to_client?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    #[tokio::test]
    async fn pump_propagates_half_close_and_counts_bytes() {
        // Echo origin: replies once, then must observe EOF.
        let echo_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let echo_addr = echo_listener.local_addr().unwrap();
        let echo = tokio::spawn(async move {
            let (mut stream, _) = echo_listener.accept().await.unwrap();
            let mut buf = [0u8; 64];
            let n = stream.read(&mut buf).await.unwrap();
            stream.write_all(&buf[..n]).await.unwrap();
            let n = stream.read(&mut buf).await.unwrap();
            assert_eq!(n, 0, "echo should observe EOF after the client half-closes");
        });

        // Stand-in for the upgraded client IO.
        let client_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client_addr = client_listener.local_addr().unwrap();
        let test_client = tokio::spawn(async move {
            let mut stream = TcpStream::connect(client_addr).await.unwrap();
            stream.write_all(b"ping").await.unwrap();
            let mut buf = [0u8; 4];
            stream.read_exact(&mut buf).await.unwrap();
            assert_eq!(&buf, b"ping");
            stream.shutdown().await.unwrap();
        });

        let (client_side, _) = client_listener.accept().await.unwrap();
        let server_side = TcpStream::connect(echo_addr).await.unwrap();

        let counts = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            pump(client_side, server_side, 7),
        )
        .await
        .expect("pump should finish after both sides close")
        .unwrap();
        assert_eq!(counts, (4, 4));
        test_client.await.unwrap();
        echo.await.unwrap();
    }
}
