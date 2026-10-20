//! Direct replay tests for the Repeater engine.

use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use http::{Response, StatusCode, Version};
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper_util::rt::{TokioExecutor, TokioIo};
use perseid_proxy::{Repeater, RepeaterConfig, RepeaterRequest, RepeaterResult};
use rcgen::{CertificateParams, KeyPair};
use rustls::ServerConfig;
use rustls::pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::{mpsc, oneshot};
use tokio_rustls::TlsAcceptor;

const TIMEOUT: Duration = Duration::from_secs(5);

fn origin_certificate(name: &str) -> (ServerConfig, String) {
    let key = KeyPair::generate().expect("origin key");
    let params = CertificateParams::new(vec![name.to_string()]).expect("origin params");
    let cert = params.self_signed(&key).expect("origin certificate");
    let key_der = PrivatePkcs8KeyDer::from(key.serialize_der());
    let config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![cert.der().clone()], PrivateKeyDer::Pkcs8(key_der))
        .expect("origin server config");
    (config, cert.pem())
}

fn origin_certificate_h2(name: &str) -> (ServerConfig, String) {
    let (mut config, pem) = origin_certificate(name);
    config.alpn_protocols = vec![b"h2".to_vec()];
    (config, pem)
}

async fn read_request<S: AsyncRead + Unpin>(stream: &mut S) -> Vec<u8> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let n = stream.read(&mut chunk).await.unwrap();
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(head_end) = buf.windows(4).position(|window| window == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&buf[..head_end]);
            let length = head
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().ok())
                        .flatten()
                })
                .unwrap_or(0);
            if buf.len() >= head_end + 4 + length {
                break;
            }
        }
    }
    buf
}

#[tokio::test]
async fn sends_an_edited_request_directly_to_the_origin() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (request_tx, request_rx) = oneshot::channel();

    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let request = read_request(&mut stream).await;
        let _ = request_tx.send(request);
        let _ = stream
            .write_all(b"HTTP/1.1 201 Created\r\nContent-Length: 5\r\nX-Replayed: yes\r\n\r\nhello")
            .await;
        let _ = stream.shutdown().await;
    });

    let repeater = Repeater::start(RepeaterConfig {
        timeout: TIMEOUT,
        ..RepeaterConfig::default()
    })
    .unwrap();

    let request = RepeaterRequest::parse(
        "POST /submit?q=1 HTTP/1.1\r\nHost: virtual.example\r\nContent-Length: 3\r\n\r\n",
        b"abc".to_vec(),
        "http",
        &addr.to_string(),
    )
    .unwrap();
    let id = repeater.send(request).unwrap();

    let response = tokio::time::timeout(TIMEOUT, async {
        loop {
            if let Some(response) = repeater.try_recv() {
                break response;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("Repeater should answer before the test timeout");

    assert_eq!(response.id, id);
    match response.result {
        RepeaterResult::Response {
            version,
            status,
            reason,
            headers,
            body,
            body_error,
        } => {
            assert_eq!(version, "HTTP/1.1");
            assert_eq!(status, 201);
            assert_eq!(reason, "Created");
            assert!(body_error.is_none());
            assert_eq!(body.data(), b"hello");
            assert!(
                headers
                    .iter()
                    .any(|(name, value)| name == "x-replayed" && value == "yes")
            );
        }
        RepeaterResult::Failed { error } => panic!("Repeater failed: {error}"),
    }

    let sent = tokio::time::timeout(TIMEOUT, request_rx)
        .await
        .expect("origin should receive the request")
        .expect("origin request channel");
    let sent = String::from_utf8_lossy(&sent);
    assert!(sent.starts_with("POST /submit?q=1 HTTP/1.1\r\n"));
    assert!(sent.contains("host: virtual.example\r\n"));
    assert!(sent.ends_with("\r\n\r\nabc"));
}

#[tokio::test]
async fn sends_over_tls_with_a_configured_trust_root() {
    let (server_config, certificate) = origin_certificate("127.0.0.1");
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let acceptor = TlsAcceptor::from(Arc::new(server_config));
    let (request_tx, request_rx) = oneshot::channel();

    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut stream = acceptor.accept(stream).await.unwrap();
        let request = read_request(&mut stream).await;
        let _ = request_tx.send(request);
        let _ = stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 6\r\n\r\nsecure")
            .await;
        let _ = stream.shutdown().await;
    });

    let repeater = Repeater::start(RepeaterConfig {
        timeout: TIMEOUT,
        extra_roots_pem: vec![certificate],
        ..RepeaterConfig::default()
    })
    .unwrap();

    let request = RepeaterRequest::parse(
        "GET /tls HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n",
        Vec::new(),
        "https",
        &format!("127.0.0.1:{}", addr.port()),
    )
    .unwrap();
    let id = repeater.send(request).unwrap();

    let response = tokio::time::timeout(TIMEOUT, async {
        loop {
            if let Some(response) = repeater.try_recv() {
                break response;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("TLS Repeater should answer before the test timeout");

    assert_eq!(response.id, id);
    match response.result {
        RepeaterResult::Response {
            status,
            body,
            body_error,
            ..
        } => {
            assert_eq!(status, 200);
            assert!(body_error.is_none());
            assert_eq!(body.data(), b"secure");
        }
        RepeaterResult::Failed { error } => panic!("TLS Repeater failed: {error}"),
    }

    let sent = request_rx.await.expect("origin request channel");
    assert!(String::from_utf8_lossy(&sent).starts_with("GET /tls HTTP/1.1\r\n"));
}

#[tokio::test]
async fn negotiates_http2_over_tls() {
    let (server_config, certificate) = origin_certificate_h2("127.0.0.1");
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let acceptor = TlsAcceptor::from(Arc::new(server_config));
    let (request_tx, mut request_rx) = mpsc::unbounded_channel();

    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let tls = acceptor.accept(stream).await.unwrap();
        assert_eq!(tls.get_ref().1.alpn_protocol(), Some(b"h2".as_slice()));
        let service = service_fn(move |request: hyper::Request<Incoming>| {
            let request_tx = request_tx.clone();
            async move {
                let (parts, body) = request.into_parts();
                let body = body.collect().await.unwrap().to_bytes();
                let _ =
                    request_tx.send((parts.version, parts.method, parts.uri, parts.headers, body));
                Ok::<_, Infallible>(
                    Response::builder()
                        .status(StatusCode::OK)
                        .header("content-length", "9")
                        .body(Full::new(Bytes::from_static(b"h2 secure")))
                        .unwrap(),
                )
            }
        });
        let _ = hyper::server::conn::http2::Builder::new(TokioExecutor::new())
            .serve_connection(TokioIo::new(tls), service)
            .await;
    });

    let repeater = Repeater::start(RepeaterConfig {
        timeout: TIMEOUT,
        extra_roots_pem: vec![certificate],
        ..RepeaterConfig::default()
    })
    .unwrap();

    let request = RepeaterRequest::parse(
        "GET /h2 HTTP/1.1\r\nHost: virtual.example\r\n\r\n",
        Vec::new(),
        "https",
        &format!("127.0.0.1:{}", addr.port()),
    )
    .unwrap();
    repeater.send(request).unwrap();

    let response = tokio::time::timeout(TIMEOUT, async {
        loop {
            if let Some(response) = repeater.try_recv() {
                break response;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("HTTP/2 Repeater should answer before the test timeout");

    match response.result {
        RepeaterResult::Response {
            version,
            status,
            body,
            ..
        } => {
            assert_eq!(version, "HTTP/2");
            assert_eq!(status, 200);
            assert_eq!(body.data(), b"h2 secure");
        }
        RepeaterResult::Failed { error } => panic!("HTTP/2 Repeater failed: {error}"),
    }

    let (version, method, uri, headers, body) = request_rx.recv().await.expect("origin request");
    assert_eq!(version, Version::HTTP_2);
    assert_eq!(method, http::Method::GET);
    assert_eq!(uri.path(), "/h2");
    assert!(
        uri.authority()
            .is_some_and(|authority| authority.as_str().starts_with("127.0.0.1:"))
    );
    assert_eq!(
        headers.get("host").and_then(|v| v.to_str().ok()),
        Some("virtual.example")
    );
    assert!(body.is_empty());
}

#[tokio::test]
async fn bounds_the_retained_response_body() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let _ = read_request(&mut stream).await;
        let _ = stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\n0123456789")
            .await;
        let _ = stream.shutdown().await;
    });

    let repeater = Repeater::start(RepeaterConfig {
        capture_limit: 4,
        timeout: TIMEOUT,
        ..RepeaterConfig::default()
    })
    .unwrap();

    let request = RepeaterRequest::parse(
        "GET /large HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n",
        Vec::new(),
        "http",
        &addr.to_string(),
    )
    .unwrap();
    repeater.send(request).unwrap();

    let response = tokio::time::timeout(TIMEOUT, async {
        loop {
            if let Some(response) = repeater.try_recv() {
                break response;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("Repeater should answer before the test timeout");

    match response.result {
        RepeaterResult::Response { body, .. } => {
            assert_eq!(body.data(), b"0123");
            assert_eq!(body.total_len(), 10);
            assert!(body.is_truncated());
            assert!(body.is_complete());
        }
        RepeaterResult::Failed { error } => panic!("Repeater failed: {error}"),
    }
}

#[test]
fn rejects_requests_without_a_usable_target() {
    let error = RepeaterRequest::parse("GET * HTTP/1.1\n\n", Vec::new(), "https", "")
        .expect_err("asterisk-form targets are not supported");
    assert!(error.to_string().contains("asterisk-form"));
}
