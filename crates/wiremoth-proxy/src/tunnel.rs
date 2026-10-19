//! CONNECT tunneling (HTTPS passthrough).
//!
//! Wiremoth does not yet perform TLS interception: an intercepted `CONNECT`
//! results in a raw byte tunnel between client and origin. The tunnel is
//! visible in history, but its contents are not.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Instant, SystemTime};

use http::{Request, Response, StatusCode};
use hyper::body::Incoming;
use hyper_util::rt::TokioIo;
use log::debug;
use tokio::net::TcpStream;

use crate::events::{
    CapturedBody, ExchangeEnd, ExchangeResult, ProxyEvent, RequestInfo, ResponseHead,
};
use crate::server::Ctx;
use crate::util::{ProxyBody, empty_body, header_list, text_response, version_str};

pub(crate) async fn handle(ctx: Arc<Ctx>, req: Request<Incoming>) -> Response<ProxyBody> {
    let id = ctx.next_id();
    let started = Instant::now();
    let timestamp = SystemTime::now();
    let capture = ctx.capture_enabled();
    let version = version_str(req.version());
    let raw_uri = req.uri().clone();

    let Some(authority) = raw_uri.authority().cloned() else {
        let message = "CONNECT requires a host:port target".to_string();
        if capture {
            ctx.emit(ProxyEvent::Started {
                id,
                timestamp,
                request: RequestInfo {
                    method: "CONNECT".to_string(),
                    uri: raw_uri.to_string(),
                    host: raw_uri.to_string(),
                    path: String::new(),
                    version,
                    headers: header_list(req.headers()),
                },
            });
            finish_failed(&ctx, id, started, message.clone());
        }
        return text_response(StatusCode::BAD_REQUEST, &message);
    };

    let host = authority.host().to_string();
    let port = authority.port_u16().unwrap_or(443);
    let policy = ctx.policy();

    if policy.scope_enforced() && !policy.in_scope(authority.as_ref()) {
        ctx.block_by_scope();
        let message = "target is outside the configured scope".to_string();
        if capture {
            ctx.emit(ProxyEvent::Started {
                id,
                timestamp,
                request: RequestInfo {
                    method: "CONNECT".to_string(),
                    uri: raw_uri.to_string(),
                    host: authority.to_string(),
                    path: String::new(),
                    version: version.clone(),
                    headers: header_list(req.headers()),
                },
            });
            finish_failed(&ctx, id, started, message.clone());
        }
        return text_response(StatusCode::FORBIDDEN, &message);
    }

    if capture {
        ctx.emit(ProxyEvent::Started {
            id,
            timestamp,
            request: RequestInfo {
                method: "CONNECT".to_string(),
                uri: raw_uri.to_string(),
                host: authority.to_string(),
                path: String::new(),
                version: version.clone(),
                headers: header_list(req.headers()),
            },
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
