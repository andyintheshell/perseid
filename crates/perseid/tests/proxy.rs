//! End-to-end tests: real TCP client -> Perseid proxy -> local origin server.

use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use http::{HeaderMap, Method, Response, StatusCode, Uri, Version};
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper_util::rt::{TokioExecutor, TokioIo};
use perseid::proxy::{
    CertificateAuthority, ExchangeResult, InterceptItem, InterceptKind, MatchReplaceRule, Proxy,
    ProxyConfig, ProxyEvent, ProxySettings, RequestInfo, RuleDirection, RulePart, StartError,
    TlsConfig, WebSocketDirection, WebSocketOpcode,
};
use rustls::pki_types::ServerName;
use rustls::{ClientConfig, RootCertStore};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot};
use tokio_rustls::{TlsAcceptor, TlsConnector};

mod common;

use common::{origin_certificate, origin_certificate_h2, read_request};

const TIMEOUT: Duration = Duration::from_secs(5);

fn init_logging() {
    let _ = env_logger::builder()
        .is_test(true)
        .filter_level(log::LevelFilter::Debug)
        .try_init();
}

fn test_proxy(capture_limit: usize, event_queue: usize) -> Proxy {
    init_logging();
    Proxy::start(ProxyConfig {
        listen: "127.0.0.1:0".parse().expect("valid test address"),
        capture_limit,
        event_queue,
        ..ProxyConfig::default()
    })
    .expect("proxy should start on an ephemeral port")
}

fn test_proxy_with(settings: ProxySettings, capture_limit: usize, intercept_limit: usize) -> Proxy {
    init_logging();
    Proxy::start(ProxyConfig {
        listen: "127.0.0.1:0".parse().expect("valid test address"),
        capture_limit,
        intercept_limit,
        settings,
        event_queue: 64,
        ..ProxyConfig::default()
    })
    .expect("proxy should start on an ephemeral port")
}

/// Origin that answers every request with the same raw response.
async fn spawn_origin(response: String) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let response = response.clone();
            tokio::spawn(async move {
                let _ = tokio::time::timeout(TIMEOUT, read_request(&mut stream)).await;
                let _ = stream.write_all(response.as_bytes()).await;
                let _ = stream.shutdown().await;
            });
        }
    });
    addr
}

/// Origin that accepts one connection, records the request, and responds.
async fn spawn_capture_origin(response: String) -> (SocketAddr, oneshot::Receiver<Vec<u8>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (request_tx, request_rx) = oneshot::channel();
    tokio::spawn(async move {
        if let Ok((mut stream, _)) = listener.accept().await {
            let request = read_request(&mut stream).await;
            let _ = request_tx.send(request);
            let _ = stream.write_all(response.as_bytes()).await;
            let _ = stream.shutdown().await;
        }
    });
    (addr, request_rx)
}

/// Send a raw request, return the raw response (client sends `Connection: close`).
async fn raw_exchange(proxy: SocketAddr, request: &str) -> Vec<u8> {
    let mut stream = TcpStream::connect(proxy).await.unwrap();
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut response = Vec::new();
    tokio::time::timeout(TIMEOUT, stream.read_to_end(&mut response))
        .await
        .expect("proxy should answer before the test timeout")
        .unwrap();
    response
}

fn spawn_raw_client(proxy: SocketAddr, request: String) -> tokio::task::JoinHandle<Vec<u8>> {
    tokio::spawn(async move { raw_exchange(proxy, &request).await })
}

/// Send one HTTP/2 request over an already-connected transport and read the
/// complete response. Used for h2c and for HTTP/2 inside an intercepted TLS
/// tunnel.
async fn h2_exchange<I>(io: I, uri: String) -> (Version, StatusCode, HeaderMap, Vec<u8>)
where
    I: hyper::rt::Read + hyper::rt::Write + Unpin + Send + 'static,
{
    h2_exchange_with(io, Method::GET, uri, Bytes::new()).await
}

async fn h2_exchange_with<I>(
    io: I,
    method: Method,
    uri: String,
    body: Bytes,
) -> (Version, StatusCode, HeaderMap, Vec<u8>)
where
    I: hyper::rt::Read + hyper::rt::Write + Unpin + Send + 'static,
{
    let (mut sender, connection) = hyper::client::conn::http2::handshake(TokioExecutor::new(), io)
        .await
        .expect("HTTP/2 handshake");
    tokio::spawn(async move {
        let _ = connection.await;
    });

    let request = hyper::Request::builder()
        .method(method)
        .uri(uri)
        .version(Version::HTTP_2)
        .body(Full::new(body))
        .expect("HTTP/2 request");
    let response = tokio::time::timeout(TIMEOUT, sender.send_request(request))
        .await
        .expect("proxy should answer the HTTP/2 request before the test timeout")
        .expect("HTTP/2 request should succeed");
    let version = response.version();
    let status = response.status();
    let headers = response.headers().clone();
    let body = tokio::time::timeout(TIMEOUT, response.into_body().collect())
        .await
        .expect("proxy should finish the HTTP/2 response")
        .expect("HTTP/2 response body")
        .to_bytes()
        .to_vec();
    (version, status, headers, body)
}

async fn next_intercept(proxy: &mut Proxy) -> InterceptItem {
    tokio::time::timeout(TIMEOUT, proxy.recv_intercept())
        .await
        .expect("an intercepted message before the test timeout")
        .expect("the intercept channel should stay open")
}

async fn read_http_head<S: AsyncRead + Unpin>(stream: &mut S) -> String {
    let mut buf = Vec::new();
    let mut byte = [0u8; 1];
    let deadline = tokio::time::Instant::now() + TIMEOUT;
    while buf.len() < 8192 {
        tokio::time::timeout_at(deadline, stream.read_exact(&mut byte))
            .await
            .expect("proxy should answer before the test timeout")
            .unwrap();
        buf.push(byte[0]);
        if buf.ends_with(b"\r\n\r\n") {
            break;
        }
    }
    String::from_utf8_lossy(&buf).into_owned()
}

/// Read one WebSocket frame and return its FIN bit, opcode, and unmasked
/// payload. Test frames are small enough to read completely.
async fn read_websocket_frame<S: AsyncRead + Unpin>(stream: &mut S) -> (bool, u8, Vec<u8>) {
    let mut head = [0u8; 2];
    tokio::time::timeout(TIMEOUT, stream.read_exact(&mut head))
        .await
        .expect("WebSocket frame header should arrive")
        .expect("WebSocket frame header");
    let fin = head[0] & 0x80 != 0;
    let opcode = head[0] & 0x0f;
    let masked = head[1] & 0x80 != 0;
    let mut length = u64::from(head[1] & 0x7f);
    if length == 126 {
        let mut extended = [0u8; 2];
        stream.read_exact(&mut extended).await.unwrap();
        length = u64::from(u16::from_be_bytes(extended));
    } else if length == 127 {
        let mut extended = [0u8; 8];
        stream.read_exact(&mut extended).await.unwrap();
        length = u64::from_be_bytes(extended);
    }
    let mut mask = [0u8; 4];
    if masked {
        stream.read_exact(&mut mask).await.unwrap();
    }
    let mut payload = vec![0u8; length as usize];
    stream.read_exact(&mut payload).await.unwrap();
    if masked {
        for (index, byte) in payload.iter_mut().enumerate() {
            *byte ^= mask[index % 4];
        }
    }
    (fin, opcode, payload)
}

/// Client-to-server frame. Browsers always mask these.
fn websocket_masked_frame(opcode: u8, payload: &[u8]) -> Vec<u8> {
    let mask = [0x0a, 0x1b, 0x2c, 0x3d];
    let mut frame = Vec::with_capacity(payload.len() + 14);
    frame.push(0x80 | opcode);
    if payload.len() < 126 {
        frame.push(0x80 | payload.len() as u8);
    } else {
        frame.push(0x80 | 126);
        frame.extend_from_slice(&(payload.len() as u16).to_be_bytes());
    }
    frame.extend_from_slice(&mask);
    for (index, byte) in payload.iter().enumerate() {
        frame.push(byte ^ mask[index % 4]);
    }
    frame
}

/// Server-to-client frame. Servers do not mask.
fn websocket_server_frame(opcode: u8, payload: &[u8]) -> Vec<u8> {
    let mut frame = Vec::with_capacity(payload.len() + 10);
    frame.push(0x80 | opcode);
    if payload.len() < 126 {
        frame.push(payload.len() as u8);
    } else {
        frame.push(126);
        frame.extend_from_slice(&(payload.len() as u16).to_be_bytes());
    }
    frame.extend_from_slice(payload);
    frame
}

/// Read until the accumulated response contains `needle`.
async fn read_until_contains(stream: &mut TcpStream, needle: &[u8]) -> Vec<u8> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    while !buf.windows(needle.len()).any(|window| window == needle) {
        let n = tokio::time::timeout(TIMEOUT, stream.read(&mut chunk))
            .await
            .expect("proxy should answer before the test timeout")
            .expect("proxy connection should remain readable");
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    buf
}

/// Origin that sends a response head plus `hello`, waits for a release signal,
/// then sends `world`. This gives tests a deterministic in-flight exchange.
async fn spawn_chunked_origin() -> (SocketAddr, oneshot::Sender<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (release_tx, release_rx) = oneshot::channel();
    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let _ = read_request(&mut stream).await;
        let _ = stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\nhello")
            .await;
        let _ = release_rx.await;
        let _ = stream.write_all(b"world").await;
        let _ = stream.shutdown().await;
    });
    (addr, release_tx)
}

/// Collect events until the exchange reports `Finished`.
fn drain_until_finished(proxy: &Proxy) -> Vec<ProxyEvent> {
    let deadline = Instant::now() + TIMEOUT;
    let mut events = Vec::new();
    while Instant::now() < deadline {
        match proxy.recv_timeout(Duration::from_millis(100)) {
            Ok(event) => {
                let finished = matches!(event, ProxyEvent::Finished { .. });
                events.push(event);
                if finished {
                    return events;
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    panic!(
        "no Finished event within the test timeout; got {} events",
        events.len()
    );
}

fn started_of(events: &[ProxyEvent]) -> &RequestInfo {
    events
        .iter()
        .find_map(|event| match event {
            ProxyEvent::Started { request, .. } => Some(request),
            _ => None,
        })
        .expect("a Started event")
}

fn finished_of(events: &[ProxyEvent]) -> &perseid::proxy::ExchangeEnd {
    events
        .iter()
        .find_map(|event| match event {
            ProxyEvent::Finished { end, .. } => Some(end),
            _ => None,
        })
        .expect("a Finished event")
}

fn head_of(events: &[ProxyEvent]) -> &perseid::proxy::ResponseHead {
    events
        .iter()
        .find_map(|event| match event {
            ProxyEvent::ResponseHead { head, .. } => Some(head),
            _ => None,
        })
        .expect("a ResponseHead event")
}

fn body_after_head(response: &[u8]) -> &[u8] {
    let head_end = response
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("response should contain a header terminator")
        + 4;
    &response[head_end..]
}

#[tokio::test(flavor = "multi_thread")]
async fn forwards_and_captures_plain_http() {
    // Non-canonical reason phrase: exercises preservation in both capture and
    // the response written back to the client.
    let origin = spawn_origin(
        "HTTP/1.1 200 Yo\r\nX-Origin: yes\r\nContent-Length: 11\r\n\r\nhello world".to_string(),
    )
    .await;
    let proxy = test_proxy(64 * 1024, 64);
    let request = format!(
        "GET http://{origin}/hello?x=1 HTTP/1.1\r\nHost: {origin}\r\nProxy-Connection: keep-alive\r\nConnection: close\r\n\r\n"
    );

    let response = raw_exchange(proxy.local_addr(), &request).await;
    let text = String::from_utf8_lossy(&response);
    assert!(
        text.starts_with("HTTP/1.1 200 Yo"),
        "raw response was: {text}"
    );
    assert!(text.to_ascii_lowercase().contains("x-origin: yes"));
    assert_eq!(body_after_head(&response), b"hello world");

    let events = drain_until_finished(&proxy);
    let request = started_of(&events);
    assert_eq!(request.method, "GET");
    assert_eq!(request.host, origin.to_string());
    assert_eq!(request.path, "/hello?x=1");
    assert_eq!(request.version, "HTTP/1.1");

    let head = head_of(&events);
    assert_eq!(head.status, 200);
    assert_eq!(head.reason, "Yo");

    match &finished_of(&events).result {
        ExchangeResult::Response { body } => {
            assert_eq!(body.data(), b"hello world");
            assert_eq!(body.total_len(), 11);
            assert!(body.is_complete());
            assert!(!body.is_truncated());
        }
        other => panic!("expected a response result, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn serves_the_ca_download_page_on_the_listener_address() {
    let ca = Arc::new(CertificateAuthority::generate().expect("CA"));
    let proxy = Proxy::start(ProxyConfig {
        listen: "127.0.0.1:0".parse().expect("address"),
        tls: TlsConfig {
            authority: Some(ca.clone()),
            ..TlsConfig::default()
        },
        ..ProxyConfig::default()
    })
    .expect("proxy starts");
    let addr = proxy.local_addr();

    let request = format!("GET / HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
    let response = raw_exchange(addr, &request).await;
    let text = String::from_utf8_lossy(&response);
    assert!(text.starts_with("HTTP/1.1 200"), "response was: {text}");
    assert!(text.contains("/perseid-ca.pem"), "response was: {text}");
    assert!(text.contains("Firefox"), "response was: {text}");
    assert!(text.contains("Perseid CA"), "response was: {text}");

    let request =
        format!("GET /perseid-ca.pem HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
    let response = raw_exchange(addr, &request).await;
    let text = String::from_utf8_lossy(&response);
    assert!(text.starts_with("HTTP/1.1 200"), "response was: {text}");
    assert!(
        text.to_ascii_lowercase()
            .contains("content-type: application/x-pem-file"),
        "response was: {text}"
    );
    assert_eq!(body_after_head(&response), ca.cert_pem().as_bytes());

    // Browsers that send loopback traffic through the proxy use absolute-form;
    // serve the portal instead of trying to forward the request to the proxy
    // itself.
    let request =
        format!("GET http://{addr}/ HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
    let response = raw_exchange(addr, &request).await;
    assert!(
        String::from_utf8_lossy(&response).contains("Trust Perseid's HTTPS certificate"),
        "response was: {}",
        String::from_utf8_lossy(&response)
    );

    // `http://perseid/` is the short address to give users once they have
    // configured the proxy.
    let request = "GET http://perseid/ HTTP/1.1\r\nHost: perseid\r\nConnection: close\r\n\r\n";
    let response = raw_exchange(addr, request).await;
    assert!(
        String::from_utf8_lossy(&response).contains("Trust Perseid's HTTPS certificate"),
        "response was: {}",
        String::from_utf8_lossy(&response)
    );

    assert!(
        proxy.try_recv().is_none(),
        "the certificate page must not appear in HTTP history"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn captures_chunked_responses_without_interception() {
    let origin = spawn_origin(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nTransfer-Encoding: chunked\r\n\r\n\
         5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n"
            .to_string(),
    )
    .await;
    let proxy = test_proxy(64 * 1024, 64);
    let request = format!(
        "GET http://{origin}/chunked HTTP/1.1\r\nHost: {origin}\r\nConnection: close\r\n\r\n"
    );

    let response = raw_exchange(proxy.local_addr(), &request).await;
    let text = String::from_utf8_lossy(&response);
    assert!(text.contains("hello"), "raw response was: {text}");

    let events = drain_until_finished(&proxy);
    match &finished_of(&events).result {
        ExchangeResult::Response { body } => {
            assert_eq!(body.data(), b"hello world");
            assert_eq!(body.total_len(), 11);
            assert!(body.is_complete());
        }
        other => panic!("expected a response result, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn captures_request_bodies() {
    let origin = spawn_origin("HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok".to_string()).await;
    let proxy = test_proxy(64 * 1024, 64);
    let body = "abc=123";
    let request = format!(
        "POST http://{origin}/submit HTTP/1.1\r\nHost: {origin}\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );

    let response = raw_exchange(proxy.local_addr(), &request).await;
    assert_eq!(body_after_head(&response), b"ok");

    let events = drain_until_finished(&proxy);
    let request = started_of(&events);
    assert_eq!(request.method, "POST");
    assert_eq!(request.path, "/submit");

    let end = finished_of(&events);
    assert_eq!(end.request_body.data(), body.as_bytes());
    assert_eq!(end.request_body.total_len(), body.len() as u64);
    assert!(end.request_body.is_complete());
}

#[tokio::test(flavor = "multi_thread")]
async fn forwards_bodies_larger_than_the_capture_limit() {
    let big = "A".repeat(256 * 1024);
    let origin = spawn_origin(format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{big}",
        big.len()
    ))
    .await;
    let proxy = test_proxy(1024, 64);
    let request =
        format!("GET http://{origin}/big HTTP/1.1\r\nHost: {origin}\r\nConnection: close\r\n\r\n");

    let response = raw_exchange(proxy.local_addr(), &request).await;
    let received = body_after_head(&response);
    assert_eq!(
        received.len(),
        big.len(),
        "every byte must still reach the client"
    );
    assert!(received.iter().all(|&b| b == b'A'));

    let events = drain_until_finished(&proxy);
    match &finished_of(&events).result {
        ExchangeResult::Response { body } => {
            assert_eq!(body.data().len(), 1024);
            assert_eq!(body.data(), &vec![b'A'; 1024][..]);
            assert_eq!(body.total_len(), big.len() as u64);
            assert!(body.is_truncated());
            assert!(body.is_complete());
        }
        other => panic!("expected a response result, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn connect_tunnel_relays_bytes_and_reports_stats() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let echo_addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            tokio::spawn(async move {
                let (mut read_half, mut write_half) = stream.split();
                let _ = tokio::io::copy(&mut read_half, &mut write_half).await;
            });
        }
    });

    let proxy = test_proxy(64 * 1024, 64);
    let mut stream = TcpStream::connect(proxy.local_addr()).await.unwrap();
    let connect = format!("CONNECT {echo_addr} HTTP/1.1\r\nHost: {echo_addr}\r\n\r\n");
    stream.write_all(connect.as_bytes()).await.unwrap();

    let head = read_http_head(&mut stream).await;
    assert!(
        head.starts_with("HTTP/1.1 200"),
        "unexpected CONNECT response: {head}"
    );

    stream.write_all(b"ping").await.unwrap();
    let mut echoed = [0u8; 4];
    tokio::time::timeout(TIMEOUT, stream.read_exact(&mut echoed))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&echoed, b"ping");
    stream.shutdown().await.unwrap();

    let events = drain_until_finished(&proxy);
    let request = started_of(&events);
    assert_eq!(request.method, "CONNECT");
    assert_eq!(request.host, echo_addr.to_string());
    assert_eq!(head_of(&events).status, 200);
    match finished_of(&events).result {
        ExchangeResult::Tunnel {
            client_to_server,
            server_to_client,
        } => {
            assert_eq!(client_to_server, 4);
            assert_eq!(server_to_client, 4);
        }
        ref other => panic!("expected a tunnel result, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn failed_upstream_connect_yields_502_and_failed_event() {
    // Nothing listens here: bind a port, then drop the listener.
    let dead = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let dead_addr = dead.local_addr().unwrap();
    drop(dead);

    let proxy = test_proxy(64 * 1024, 64);
    let request = format!(
        "GET http://{dead_addr}/ HTTP/1.1\r\nHost: {dead_addr}\r\nConnection: close\r\n\r\n"
    );
    let response = raw_exchange(proxy.local_addr(), &request).await;
    let text = String::from_utf8_lossy(&response);
    assert!(text.starts_with("HTTP/1.1 502"), "raw response was: {text}");

    let events = drain_until_finished(&proxy);
    match &finished_of(&events).result {
        ExchangeResult::Failed { error } => {
            assert!(error.contains("connect"), "error was: {error}")
        }
        other => panic!("expected a failed result, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn pausing_capture_does_not_strand_an_in_flight_exchange() {
    let (origin, release) = spawn_chunked_origin().await;
    let proxy = test_proxy(64 * 1024, 64);
    let mut stream = TcpStream::connect(proxy.local_addr()).await.unwrap();
    let request =
        format!("GET http://{origin}/slow HTTP/1.1\r\nHost: {origin}\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).await.unwrap();

    let seen = read_until_contains(&mut stream, b"hello").await;
    assert!(
        seen.windows(5).any(|window| window == b"hello"),
        "response did not contain the first chunk"
    );

    // Consume the start of the exchange, then pause before its body finishes.
    let mut early = Vec::new();
    let deadline = Instant::now() + TIMEOUT;
    while Instant::now() < deadline {
        let event = proxy
            .recv_timeout(Duration::from_millis(100))
            .expect("started/head events should arrive");
        let saw_head = matches!(event, ProxyEvent::ResponseHead { .. });
        early.push(event);
        if saw_head {
            break;
        }
    }
    assert!(
        early
            .iter()
            .any(|event| matches!(event, ProxyEvent::Started { .. }))
    );
    assert!(
        early
            .iter()
            .any(|event| matches!(event, ProxyEvent::ResponseHead { .. }))
    );

    proxy.set_paused(true);
    assert!(proxy.is_paused());
    release.send(()).unwrap();

    let mut rest = Vec::new();
    tokio::time::timeout(TIMEOUT, stream.read_to_end(&mut rest))
        .await
        .expect("proxy should finish forwarding after release")
        .unwrap();
    assert!(rest.ends_with(b"world"), "rest was: {rest:?}");

    let finished = drain_until_finished(&proxy);
    match &finished_of(&finished).result {
        ExchangeResult::Response { body } => {
            assert_eq!(body.data(), b"helloworld");
            assert_eq!(body.total_len(), 10);
            assert!(body.is_complete());
        }
        other => panic!("expected a response result, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn paused_capture_ignores_new_exchanges_until_resumed() {
    let origin = spawn_origin("HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok".to_string()).await;
    let proxy = test_proxy(64 * 1024, 64);
    proxy.set_paused(true);

    let request = format!(
        "GET http://{origin}/ignored HTTP/1.1\r\nHost: {origin}\r\nConnection: close\r\n\r\n"
    );
    let response = raw_exchange(proxy.local_addr(), &request).await;
    assert_eq!(body_after_head(&response), b"ok");
    assert!(
        proxy
            .recv_timeout(Duration::from_millis(250))
            .is_err_and(|err| err == std::sync::mpsc::RecvTimeoutError::Timeout),
        "paused capture should not emit events for a new exchange"
    );
    assert_eq!(proxy.dropped_events(), 0);

    proxy.set_paused(false);
    let request = format!(
        "GET http://{origin}/resumed HTTP/1.1\r\nHost: {origin}\r\nConnection: close\r\n\r\n"
    );
    let response = raw_exchange(proxy.local_addr(), &request).await;
    assert_eq!(body_after_head(&response), b"ok");

    let events = drain_until_finished(&proxy);
    assert_eq!(started_of(&events).path, "/resumed");
}

#[tokio::test(flavor = "multi_thread")]
async fn rejects_connections_above_the_limit() {
    // Keep the first downstream TCP connection open after its exchange
    // completes; the connection permit is held for that socket's lifetime.
    let (origin_addr, origin_request) =
        spawn_capture_origin("HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok".to_string()).await;

    let proxy = Proxy::start(ProxyConfig {
        listen: "127.0.0.1:0".parse().expect("valid test address"),
        max_connections: 1,
        max_tunnels: 1,
        ..ProxyConfig::default()
    })
    .expect("proxy should start on an ephemeral port");

    let mut first = TcpStream::connect(proxy.local_addr()).await.unwrap();
    let request = format!("GET http://{origin_addr}/hold HTTP/1.1\r\nHost: {origin_addr}\r\n\r\n");
    first.write_all(request.as_bytes()).await.unwrap();
    let first_event = proxy
        .recv_timeout(TIMEOUT)
        .expect("first request should start");
    assert!(matches!(first_event, ProxyEvent::Started { .. }));
    let captured_request = tokio::time::timeout(TIMEOUT, origin_request)
        .await
        .expect("first request should reach the origin")
        .expect("the origin should capture the first request");
    assert!(String::from_utf8_lossy(&captured_request).contains("/hold"));
    assert!(
        read_http_head(&mut first)
            .await
            .starts_with("HTTP/1.1 200 OK")
    );
    let mut body = [0; 2];
    tokio::time::timeout(TIMEOUT, first.read_exact(&mut body))
        .await
        .expect("the first response body should arrive")
        .expect("the first response body should be readable");
    assert_eq!(&body, b"ok");

    let mut second = TcpStream::connect(proxy.local_addr()).await.unwrap();
    second
        .write_all(b"GET / HTTP/1.1\r\nHost: example.com\r\n\r\n")
        .await
        .unwrap();
    let mut buf = Vec::new();
    let _ = tokio::time::timeout(TIMEOUT, second.read_to_end(&mut buf))
        .await
        .expect("rejected connection should close promptly");
    assert!(
        buf.is_empty(),
        "rejected connection should not receive a response"
    );
    assert_eq!(proxy.rejected_connections(), 1);

    drop(first);
}

#[tokio::test(flavor = "multi_thread")]
async fn rejects_connect_tunnels_above_the_limit() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let echo_addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            tokio::spawn(async move {
                let (mut read_half, mut write_half) = stream.split();
                let _ = tokio::io::copy(&mut read_half, &mut write_half).await;
            });
        }
    });

    let proxy = Proxy::start(ProxyConfig {
        listen: "127.0.0.1:0".parse().expect("valid test address"),
        max_connections: 4,
        max_tunnels: 1,
        ..ProxyConfig::default()
    })
    .expect("proxy should start on an ephemeral port");

    let mut first = TcpStream::connect(proxy.local_addr()).await.unwrap();
    let connect = format!("CONNECT {echo_addr} HTTP/1.1\r\nHost: {echo_addr}\r\n\r\n");
    first.write_all(connect.as_bytes()).await.unwrap();
    let first_head = read_http_head(&mut first).await;
    assert!(
        first_head.starts_with("HTTP/1.1 200"),
        "unexpected first CONNECT response: {first_head}"
    );
    first.write_all(b"ping").await.unwrap();
    let mut echoed = [0u8; 4];
    tokio::time::timeout(TIMEOUT, first.read_exact(&mut echoed))
        .await
        .expect("first tunnel should relay bytes")
        .unwrap();
    assert_eq!(&echoed, b"ping");

    let mut second = TcpStream::connect(proxy.local_addr()).await.unwrap();
    second.write_all(connect.as_bytes()).await.unwrap();
    let second_head = read_http_head(&mut second).await;
    assert!(
        second_head.starts_with("HTTP/1.1 503"),
        "unexpected limited CONNECT response: {second_head}"
    );
    assert_eq!(proxy.rejected_tunnels(), 1);

    let events = drain_until_finished(&proxy);
    match &finished_of(&events).result {
        ExchangeResult::Failed { error } => {
            assert!(error.contains("tunnel limit"), "error was: {error}")
        }
        other => panic!("expected a failed result, got {other:?}"),
    }

    drop(first);
}

#[tokio::test(flavor = "multi_thread")]
async fn malformed_targets_get_proxy_errors_instead_of_panics() {
    let proxy = test_proxy(64 * 1024, 64);
    let response = raw_exchange(
        proxy.local_addr(),
        "GET / HTTP/1.1\r\nHost: bad host\r\nConnection: close\r\n\r\n",
    )
    .await;
    let text = String::from_utf8_lossy(&response);
    assert!(text.starts_with("HTTP/1.1 400"), "response was: {text}");
    let events = drain_until_finished(&proxy);
    match &finished_of(&events).result {
        ExchangeResult::Failed { error } => assert!(error.contains("Host")),
        other => panic!("expected a failed result, got {other:?}"),
    }

    let response = raw_exchange(
        proxy.local_addr(),
        "GET ftp://example.com/ HTTP/1.1\r\nHost: example.com\r\nConnection: close\r\n\r\n",
    )
    .await;
    let text = String::from_utf8_lossy(&response);
    assert!(text.starts_with("HTTP/1.1 501"), "response was: {text}");
    let events = drain_until_finished(&proxy);
    assert!(matches!(
        finished_of(&events).result,
        ExchangeResult::Failed { .. }
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn intercepts_and_forwards_an_edited_request() {
    let (origin, origin_request) =
        spawn_capture_origin("HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok".to_string()).await;
    let mut proxy = test_proxy_with(
        ProxySettings {
            intercept_requests: true,
            ..ProxySettings::default()
        },
        64 * 1024,
        64 * 1024,
    );
    let request = format!(
        "POST http://{origin}/before HTTP/1.1\r\nHost: {origin}\r\nX-Test: original\r\nContent-Length: 4\r\nConnection: close\r\n\r\nold!"
    );
    let client = spawn_raw_client(proxy.local_addr(), request);

    let mut item = next_intercept(&mut proxy).await;
    assert_eq!(item.kind(), InterceptKind::Request);
    assert_eq!(item.request().path, "/before");
    assert_eq!(item.body(), Some(b"old!".as_slice()));
    assert!(
        item.head()
            .to_ascii_lowercase()
            .contains("x-test: original")
    );

    let edited = format!(
        "POST /after HTTP/1.1\r\nHost: {origin}\r\nX-Edited: yes\r\nContent-Length: 3\r\nConnection: close\r\n\r\n"
    );
    item.forward(&edited, Some(b"new".to_vec()))
        .expect("edited request should be accepted");

    let response = client.await.unwrap();
    assert_eq!(body_after_head(&response), b"ok");
    let captured = tokio::time::timeout(TIMEOUT, origin_request)
        .await
        .expect("origin should receive the edited request")
        .unwrap();
    let captured_text = String::from_utf8_lossy(&captured);
    assert!(
        captured_text.starts_with("POST /after HTTP/1.1"),
        "origin request was: {captured_text}"
    );
    assert!(captured_text.to_ascii_lowercase().contains("x-edited: yes"));
    assert_eq!(body_after_head(&captured), b"new");

    let events = drain_until_finished(&proxy);
    let started = started_of(&events);
    assert_eq!(started.method, "POST");
    assert_eq!(started.path, "/after");
    assert!(
        started
            .headers
            .iter()
            .any(|(name, value)| name == "x-edited" && value == "yes")
    );
    assert_eq!(finished_of(&events).request_body.data(), b"new");
}

#[tokio::test(flavor = "multi_thread")]
async fn match_replace_applies_to_an_unchanged_intercepted_body() {
    let (origin, origin_request) =
        spawn_capture_origin("HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok".to_string()).await;
    let mut proxy = test_proxy_with(
        ProxySettings {
            intercept_requests: true,
            match_replace: vec![MatchReplaceRule {
                enabled: true,
                direction: RuleDirection::Request,
                part: RulePart::Body,
                pattern: "secret".to_string(),
                replacement: "redacted".to_string(),
                regex: false,
            }],
            ..ProxySettings::default()
        },
        64 * 1024,
        64 * 1024,
    );
    let request = format!(
        "POST http://{origin}/rules HTTP/1.1\r\nHost: {origin}\r\nContent-Length: 6\r\nConnection: close\r\n\r\nsecret"
    );
    let client = spawn_raw_client(proxy.local_addr(), request);

    let mut item = next_intercept(&mut proxy).await;
    assert_eq!(item.body(), Some(b"secret".as_slice()));
    let unchanged_head = item.head().to_string();
    item.forward(&unchanged_head, None)
        .expect("forwarding an unchanged body should apply body rules");

    let response = client.await.unwrap();
    assert_eq!(body_after_head(&response), b"ok");
    let captured = tokio::time::timeout(TIMEOUT, origin_request)
        .await
        .expect("origin should receive the rewritten request")
        .unwrap();
    assert_eq!(body_after_head(&captured), b"redacted");

    let events = drain_until_finished(&proxy);
    assert_eq!(finished_of(&events).request_body.data(), b"redacted");
}

#[tokio::test(flavor = "multi_thread")]
async fn intercepts_and_forwards_an_edited_response() {
    let origin = spawn_origin(
        "HTTP/1.1 200 OK\r\nX-Origin: old\r\nContent-Length: 3\r\n\r\nold".to_string(),
    )
    .await;
    let mut proxy = test_proxy_with(
        ProxySettings {
            intercept_responses: true,
            ..ProxySettings::default()
        },
        64 * 1024,
        64 * 1024,
    );
    let request =
        format!("GET http://{origin}/resp HTTP/1.1\r\nHost: {origin}\r\nConnection: close\r\n\r\n");
    let client = spawn_raw_client(proxy.local_addr(), request);

    let mut item = next_intercept(&mut proxy).await;
    assert_eq!(item.kind(), InterceptKind::Response);
    assert_eq!(item.request().path, "/resp");
    assert_eq!(item.body(), Some(b"old".as_slice()));
    let edited = "HTTP/1.1 201 Created\r\nX-Edited: yes\r\nContent-Length: 3\r\n\r\n";
    item.forward(edited, Some(b"new".to_vec()))
        .expect("edited response should be accepted");

    let response = client.await.unwrap();
    let text = String::from_utf8_lossy(&response);
    assert!(
        text.starts_with("HTTP/1.1 201 Created"),
        "raw response was: {text}"
    );
    assert!(text.to_ascii_lowercase().contains("x-edited: yes"));
    assert_eq!(body_after_head(&response), b"new");

    let events = drain_until_finished(&proxy);
    let head = head_of(&events);
    assert_eq!(head.status, 201);
    assert_eq!(head.reason, "Created");
    match &finished_of(&events).result {
        ExchangeResult::Response { body } => assert_eq!(body.data(), b"new"),
        other => panic!("expected a response result, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn one_interception_policy_can_hold_both_directions() {
    let origin = spawn_origin("HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok".to_string()).await;
    let mut proxy = test_proxy_with(
        ProxySettings {
            intercept_requests: true,
            intercept_responses: true,
            ..ProxySettings::default()
        },
        64 * 1024,
        64 * 1024,
    );
    let request =
        format!("GET http://{origin}/both HTTP/1.1\r\nHost: {origin}\r\nConnection: close\r\n\r\n");
    let client = spawn_raw_client(proxy.local_addr(), request);

    let mut held_request = next_intercept(&mut proxy).await;
    assert_eq!(held_request.kind(), InterceptKind::Request);
    let request_head = held_request.head().to_string();
    held_request
        .forward(&request_head, None)
        .expect("request should forward");

    let mut held_response = next_intercept(&mut proxy).await;
    assert_eq!(held_response.kind(), InterceptKind::Response);
    assert_eq!(held_response.request().path, "/both");
    let response_head = held_response.head().to_string();
    held_response
        .forward(&response_head, None)
        .expect("response should forward");

    let response = client.await.unwrap();
    assert_eq!(body_after_head(&response), b"ok");
    let events = drain_until_finished(&proxy);
    assert_eq!(started_of(&events).path, "/both");
    match &finished_of(&events).result {
        ExchangeResult::Response { body } => assert_eq!(body.data(), b"ok"),
        other => panic!("expected a response result, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn dropping_an_intercepted_request_records_it_as_dropped() {
    let (origin, origin_request) =
        spawn_capture_origin("HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok".to_string()).await;
    let mut proxy = test_proxy_with(
        ProxySettings {
            intercept_requests: true,
            ..ProxySettings::default()
        },
        64 * 1024,
        64 * 1024,
    );
    let request = format!(
        "POST http://{origin}/drop HTTP/1.1\r\nHost: {origin}\r\nContent-Length: 4\r\nConnection: close\r\n\r\nold!"
    );
    let client = spawn_raw_client(proxy.local_addr(), request);

    let mut item = next_intercept(&mut proxy).await;
    item.drop_message().expect("request should drop cleanly");
    let response = client.await.unwrap();
    let text = String::from_utf8_lossy(&response);
    assert!(text.starts_with("HTTP/1.1 403"), "raw response was: {text}");
    assert!(
        tokio::time::timeout(Duration::from_millis(250), origin_request)
            .await
            .is_err(),
        "a dropped request must not reach the origin"
    );

    let events = drain_until_finished(&proxy);
    assert!(matches!(
        finished_of(&events).result,
        ExchangeResult::Dropped
    ));
    assert_eq!(finished_of(&events).request_body.data(), b"old!");
}

#[tokio::test(flavor = "multi_thread")]
async fn dropping_an_intercepted_response_records_it_as_dropped() {
    let origin = spawn_origin("HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\nold".to_string()).await;
    let mut proxy = test_proxy_with(
        ProxySettings {
            intercept_responses: true,
            ..ProxySettings::default()
        },
        64 * 1024,
        64 * 1024,
    );
    let request = format!(
        "GET http://{origin}/drop-response HTTP/1.1\r\nHost: {origin}\r\nConnection: close\r\n\r\n"
    );
    let client = spawn_raw_client(proxy.local_addr(), request);

    let mut item = next_intercept(&mut proxy).await;
    item.drop_message().expect("response should drop cleanly");
    let response = client.await.unwrap();
    let text = String::from_utf8_lossy(&response);
    assert!(text.starts_with("HTTP/1.1 502"), "raw response was: {text}");

    let events = drain_until_finished(&proxy);
    assert!(matches!(
        finished_of(&events).result,
        ExchangeResult::Dropped
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn scope_enforcement_blocks_requests_until_rules_allow_them() {
    let origin = spawn_origin("HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok".to_string()).await;
    let proxy = test_proxy_with(
        ProxySettings {
            scope_enforce: true,
            scope_rules: vec!["allowed.test".to_string()],
            ..ProxySettings::default()
        },
        64 * 1024,
        64 * 1024,
    );

    let blocked = format!(
        "GET http://{origin}/blocked HTTP/1.1\r\nHost: {origin}\r\nConnection: close\r\n\r\n"
    );
    let response = raw_exchange(proxy.local_addr(), &blocked).await;
    let text = String::from_utf8_lossy(&response);
    assert!(text.starts_with("HTTP/1.1 403"), "raw response was: {text}");
    assert_eq!(proxy.blocked_by_scope(), 1);
    let events = drain_until_finished(&proxy);
    match &finished_of(&events).result {
        ExchangeResult::Failed { error } => assert!(error.contains("scope")),
        other => panic!("expected a failed result, got {other:?}"),
    }

    proxy
        .set_settings(ProxySettings {
            scope_enforce: true,
            scope_rules: Vec::new(),
            ..ProxySettings::default()
        })
        .expect("clearing the scope should update the policy");
    let allowed = format!(
        "GET http://{origin}/allowed HTTP/1.1\r\nHost: {origin}\r\nConnection: close\r\n\r\n"
    );
    let response = raw_exchange(proxy.local_addr(), &allowed).await;
    assert_eq!(body_after_head(&response), b"ok");
    let events = drain_until_finished(&proxy);
    assert_eq!(started_of(&events).path, "/allowed");
    assert_eq!(proxy.blocked_by_scope(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn scope_is_rechecked_after_request_edits() {
    let (origin, origin_request) =
        spawn_capture_origin("HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok".to_string()).await;
    let mut proxy = test_proxy_with(
        ProxySettings {
            intercept_requests: true,
            scope_enforce: true,
            scope_rules: vec!["127.0.0.1".to_string()],
            ..ProxySettings::default()
        },
        64 * 1024,
        64 * 1024,
    );
    let request = format!(
        "GET http://{origin}/inside HTTP/1.1\r\nHost: {origin}\r\nConnection: close\r\n\r\n"
    );
    let client = spawn_raw_client(proxy.local_addr(), request);

    let mut item = next_intercept(&mut proxy).await;
    let edited = "GET http://blocked.test/outside HTTP/1.1\r\nHost: blocked.test\r\nConnection: close\r\n\r\n";
    item.forward(edited, None)
        .expect("edited head should be accepted before the scope check");

    let response = client.await.unwrap();
    let text = String::from_utf8_lossy(&response);
    assert!(text.starts_with("HTTP/1.1 403"), "raw response was: {text}");
    assert_eq!(proxy.blocked_by_scope(), 1);
    assert!(
        tokio::time::timeout(Duration::from_millis(250), origin_request)
            .await
            .is_err(),
        "an edited request moved outside scope must not reach the origin"
    );

    let events = drain_until_finished(&proxy);
    match &finished_of(&events).result {
        ExchangeResult::Failed { error } => assert!(error.contains("scope")),
        other => panic!("expected a failed result, got {other:?}"),
    }
    assert_eq!(started_of(&events).path, "/outside");
}

#[tokio::test(flavor = "multi_thread")]
async fn scope_enforcement_blocks_connect_tunnels() {
    let proxy = test_proxy_with(
        ProxySettings {
            scope_enforce: true,
            scope_rules: vec!["allowed.test".to_string()],
            ..ProxySettings::default()
        },
        64 * 1024,
        64 * 1024,
    );
    let mut stream = TcpStream::connect(proxy.local_addr()).await.unwrap();
    stream
        .write_all(b"CONNECT 127.0.0.1:9 HTTP/1.1\r\nHost: 127.0.0.1:9\r\n\r\n")
        .await
        .unwrap();
    let head = read_http_head(&mut stream).await;
    assert!(
        head.starts_with("HTTP/1.1 403"),
        "unexpected CONNECT response: {head}"
    );
    assert_eq!(proxy.blocked_by_scope(), 1);

    let events = drain_until_finished(&proxy);
    let started = started_of(&events);
    assert_eq!(started.method, "CONNECT");
    assert_eq!(started.host, "127.0.0.1:9");
    match &finished_of(&events).result {
        ExchangeResult::Failed { error } => assert!(error.contains("scope")),
        other => panic!("expected a failed result, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn match_replace_rewrites_request_and_response() {
    let (origin, origin_request) = spawn_capture_origin(
        "HTTP/1.1 200 OK\r\nX-Origin: old\r\nContent-Length: 8\r\n\r\nold body".to_string(),
    )
    .await;
    let settings = ProxySettings {
        match_replace: vec![
            MatchReplaceRule {
                enabled: true,
                direction: RuleDirection::Request,
                part: RulePart::Head,
                pattern: "original".to_string(),
                replacement: "rewritten".to_string(),
                regex: false,
            },
            MatchReplaceRule {
                enabled: true,
                direction: RuleDirection::Request,
                part: RulePart::Body,
                pattern: "secret".to_string(),
                replacement: "redacted".to_string(),
                regex: false,
            },
            MatchReplaceRule {
                enabled: true,
                direction: RuleDirection::Response,
                part: RulePart::Head,
                pattern: "old".to_string(),
                replacement: "new".to_string(),
                regex: false,
            },
            MatchReplaceRule {
                enabled: true,
                direction: RuleDirection::Response,
                part: RulePart::Body,
                pattern: "old body".to_string(),
                replacement: "new body".to_string(),
                regex: false,
            },
        ],
        ..ProxySettings::default()
    };
    let proxy = test_proxy_with(settings, 64 * 1024, 64 * 1024);
    let request = format!(
        "POST http://{origin}/rewrite HTTP/1.1\r\nHost: {origin}\r\nX-Test: original\r\nContent-Length: 6\r\nConnection: close\r\n\r\nsecret"
    );
    let response = raw_exchange(proxy.local_addr(), &request).await;
    let text = String::from_utf8_lossy(&response);
    assert!(text.to_ascii_lowercase().contains("x-origin: new"));
    assert_eq!(body_after_head(&response), b"new body");

    let captured = tokio::time::timeout(TIMEOUT, origin_request)
        .await
        .expect("origin should receive the rewritten request")
        .unwrap();
    let captured_text = String::from_utf8_lossy(&captured);
    assert!(
        captured_text
            .to_ascii_lowercase()
            .contains("x-test: rewritten")
    );
    assert_eq!(body_after_head(&captured), b"redacted");

    let events = drain_until_finished(&proxy);
    assert_eq!(finished_of(&events).request_body.data(), b"redacted");
    assert_eq!(head_of(&events).status, 200);
    assert!(
        head_of(&events)
            .headers
            .iter()
            .any(|(name, value)| name == "x-origin" && value == "new")
    );
    match &finished_of(&events).result {
        ExchangeResult::Response { body } => assert_eq!(body.data(), b"new body"),
        other => panic!("expected a response result, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn oversized_bodies_are_held_for_head_edits_only() {
    let expected_body = "A".repeat(100);
    let origin = spawn_origin(format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{expected_body}",
        expected_body.len()
    ))
    .await;
    let mut proxy = test_proxy_with(
        ProxySettings {
            intercept_responses: true,
            ..ProxySettings::default()
        },
        64 * 1024,
        8,
    );
    let request = format!(
        "GET http://{origin}/large HTTP/1.1\r\nHost: {origin}\r\nConnection: close\r\n\r\n"
    );
    let client = spawn_raw_client(proxy.local_addr(), request);

    let mut item = next_intercept(&mut proxy).await;
    assert_eq!(item.kind(), InterceptKind::Response);
    assert!(item.body().is_none());
    assert!(!item.body_is_text());
    let edited = "HTTP/1.1 200 OK\r\nX-Edited: yes\r\nContent-Length: 100\r\n\r\n";
    item.forward(edited, None)
        .expect("head-only interception should accept an unchanged body");

    let response = client.await.unwrap();
    let text = String::from_utf8_lossy(&response);
    assert!(text.to_ascii_lowercase().contains("x-edited: yes"));
    assert_eq!(body_after_head(&response), expected_body.as_bytes());

    let events = drain_until_finished(&proxy);
    match &finished_of(&events).result {
        ExchangeResult::Response { body } => {
            assert_eq!(body.data(), expected_body.as_bytes());
        }
        other => panic!("expected a response result, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn streaming_bodies_are_held_for_head_edits_only() {
    let origin = spawn_origin(
        "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n0\r\n\r\n".to_string(),
    )
    .await;
    let mut proxy = test_proxy_with(
        ProxySettings {
            intercept_responses: true,
            ..ProxySettings::default()
        },
        64 * 1024,
        64 * 1024,
    );
    let request = format!(
        "GET http://{origin}/chunked HTTP/1.1\r\nHost: {origin}\r\nConnection: close\r\n\r\n"
    );
    let client = spawn_raw_client(proxy.local_addr(), request);

    let mut item = next_intercept(&mut proxy).await;
    assert_eq!(item.kind(), InterceptKind::Response);
    assert!(
        item.body().is_none(),
        "chunked responses have no trustworthy length and must stay streaming"
    );
    let edited = "HTTP/1.1 200 OK\r\nX-Edited: yes\r\nTransfer-Encoding: chunked\r\n\r\n";
    item.forward(edited, None)
        .expect("head-only interception should accept a streaming body");

    let response = client.await.unwrap();
    let text = String::from_utf8_lossy(&response);
    assert!(text.to_ascii_lowercase().contains("x-edited: yes"));
    assert!(text.contains("hello"));

    let events = drain_until_finished(&proxy);
    match &finished_of(&events).result {
        ExchangeResult::Response { body } => assert_eq!(body.data(), b"hello"),
        other => panic!("expected a response result, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn strips_unsupported_accept_encoding_before_forwarding() {
    let (origin, request_rx) =
        spawn_capture_origin("HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok".to_string()).await;
    let proxy = test_proxy(64 * 1024, 64);
    let request = format!(
        "GET http://{origin}/compressed HTTP/1.1\r\n\
         Host: {origin}\r\n\
         Accept-Encoding: gzip, zstd, br;q=0.9\r\n\
         Connection: close\r\n\r\n"
    );

    let response = raw_exchange(proxy.local_addr(), &request).await;
    assert!(response.starts_with(b"HTTP/1.1 200 OK"));

    let upstream = String::from_utf8_lossy(
        &request_rx
            .await
            .expect("origin should capture the upstream request"),
    )
    .into_owned();
    assert!(
        upstream
            .to_ascii_lowercase()
            .contains("accept-encoding: gzip, br;q=0.9"),
        "upstream request did not sanitize Accept-Encoding:\n{upstream}"
    );
    assert!(
        !upstream.to_ascii_lowercase().contains("zstd"),
        "upstream request kept an unsupported coding:\n{upstream}"
    );
}

#[test]
fn invalid_rules_prevent_start() {
    let settings = ProxySettings {
        match_replace: vec![MatchReplaceRule {
            enabled: true,
            direction: RuleDirection::Request,
            part: RulePart::Head,
            pattern: "(".to_string(),
            replacement: "x".to_string(),
            regex: true,
        }],
        ..ProxySettings::default()
    };
    let error = Proxy::start(ProxyConfig {
        listen: "127.0.0.1:0".parse().expect("valid test address"),
        settings,
        ..ProxyConfig::default()
    })
    .unwrap_err();
    assert!(matches!(error, StartError::Rule(_)));
}

#[test]
fn start_reports_bind_conflicts() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let error = Proxy::start(ProxyConfig::new(addr)).unwrap_err();
    assert!(matches!(error, StartError::Bind { addr: bound, .. } if bound == addr));
}

#[test]
fn start_returns_only_after_the_proxy_is_running() {
    let proxy = Proxy::start(ProxyConfig {
        listen: "127.0.0.1:0".parse().expect("valid test address"),
        ..ProxyConfig::default()
    })
    .expect("proxy should start on an ephemeral port");
    assert!(proxy.is_running());
    assert!(proxy.local_addr().port() > 0);
}

// ---------------------------------------------------------------------------
// TLS interception
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct CapturedH2Request {
    version: Version,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Vec<u8>,
}

/// TLS origin that only offers `h2` through ALPN and reports every request.
async fn spawn_h2_tls_origin(
    name: &str,
    body: &'static [u8],
) -> (
    SocketAddr,
    String,
    mpsc::UnboundedReceiver<CapturedH2Request>,
) {
    let (config, cert_pem) = origin_certificate_h2(name);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (request_tx, request_rx) = mpsc::unbounded_channel();
    let config = Arc::new(config);
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            let acceptor = TlsAcceptor::from(config.clone());
            let request_tx = request_tx.clone();
            tokio::spawn(async move {
                let Ok(tls) = acceptor.accept(stream).await else {
                    return;
                };
                assert_eq!(tls.get_ref().1.alpn_protocol(), Some(b"h2".as_slice()));
                let service = service_fn(move |req: hyper::Request<Incoming>| {
                    let request_tx = request_tx.clone();
                    async move {
                        let (parts, body_stream) = req.into_parts();
                        let request_body = body_stream
                            .collect()
                            .await
                            .map(|body| body.to_bytes().to_vec())
                            .unwrap_or_default();
                        let _ = request_tx.send(CapturedH2Request {
                            version: parts.version,
                            method: parts.method,
                            uri: parts.uri,
                            headers: parts.headers,
                            body: request_body,
                        });
                        Ok::<_, Infallible>(
                            Response::builder()
                                .status(StatusCode::OK)
                                .header("content-type", "text/plain")
                                .header("content-length", body.len())
                                .body(Full::new(Bytes::from_static(body)))
                                .expect("HTTP/2 origin response"),
                        )
                    }
                });
                let _ = hyper::server::conn::http2::Builder::new(TokioExecutor::new())
                    .serve_connection(TokioIo::new(tls), service)
                    .await;
            });
        }
    });
    (addr, cert_pem, request_rx)
}

/// TLS origin that speaks HTTP/1.1 and reports every request it receives.
async fn spawn_tls_origin(
    name: &str,
    response: String,
) -> (SocketAddr, String, mpsc::UnboundedReceiver<Vec<u8>>) {
    let (config, cert_pem) = origin_certificate(name);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (request_tx, request_rx) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            let acceptor = TlsAcceptor::from(Arc::new(config.clone()));
            let response = response.clone();
            let request_tx = request_tx.clone();
            tokio::spawn(async move {
                let Ok(mut tls) = acceptor.accept(stream).await else {
                    return;
                };
                loop {
                    let request = tokio::time::timeout(TIMEOUT, read_request(&mut tls))
                        .await
                        .unwrap_or_default();
                    if request.is_empty() {
                        break;
                    }
                    let _ = request_tx.send(request);
                    if tls.write_all(response.as_bytes()).await.is_err() {
                        break;
                    }
                    // Honor the fixture's advertised close. Waiting for another
                    // request instead races its idle timeout against the client
                    // read timeout and can produce a TCP reset on macOS.
                    if response
                        .lines()
                        .take_while(|line| !line.is_empty())
                        .any(|line| {
                            line.split_once(':').is_some_and(|(name, value)| {
                                name.eq_ignore_ascii_case("connection")
                                    && value.trim().eq_ignore_ascii_case("close")
                            })
                        })
                    {
                        break;
                    }
                }
                let _ = tls.shutdown().await;
            });
        }
    });
    (addr, cert_pem, request_rx)
}

/// Trust anchors built from PEM certificates.
fn pem_roots(pems: &[&str]) -> RootCertStore {
    let mut roots = RootCertStore::empty();
    for pem in pems {
        for cert in rustls_pemfile::certs(&mut pem.as_bytes()) {
            roots
                .add(cert.expect("a PEM certificate"))
                .expect("a trustable certificate");
        }
    }
    roots
}

fn proxy_with_tls(
    authority: Option<Arc<CertificateAuthority>>,
    extra_roots_pem: Vec<String>,
    settings: ProxySettings,
) -> Proxy {
    init_logging();
    Proxy::start(ProxyConfig {
        listen: "127.0.0.1:0".parse().expect("valid test address"),
        settings,
        tls: TlsConfig {
            authority,
            extra_roots_pem,
        },
        event_queue: 64,
        ..ProxyConfig::default()
    })
    .expect("proxy should start on an ephemeral port")
}

/// CONNECT to `target`, then hand the tunnel to `roots`-trusting TLS and send
/// `request`. Returns the CONNECT response head and the response inside TLS.
async fn tls_exchange(
    proxy: SocketAddr,
    target: SocketAddr,
    server_name: &str,
    roots: RootCertStore,
    request: &str,
) -> (String, Vec<u8>) {
    let mut stream = TcpStream::connect(proxy).await.unwrap();
    let connect = format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n\r\n");
    stream.write_all(connect.as_bytes()).await.unwrap();
    let head = read_http_head(&mut stream).await;
    assert!(
        head.starts_with("HTTP/1.1 200"),
        "unexpected CONNECT response: {head}"
    );

    let config = ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    let connector = TlsConnector::from(Arc::new(config));
    let name = ServerName::try_from(server_name.to_string()).expect("valid server name");
    let mut tls = connector
        .connect(name, stream)
        .await
        .expect("client TLS handshake should succeed");
    tls.write_all(request.as_bytes()).await.unwrap();
    let mut response = Vec::new();
    let _ = tokio::time::timeout(TIMEOUT, tls.read_to_end(&mut response)).await;
    (head, response)
}

/// Collect events until `finishes` exchanges report `Finished`.
fn drain_events(proxy: &Proxy, finishes: usize) -> Vec<ProxyEvent> {
    let deadline = Instant::now() + TIMEOUT;
    let mut events = Vec::new();
    let mut finished = 0;
    while Instant::now() < deadline {
        match proxy.recv_timeout(Duration::from_millis(100)) {
            Ok(event) => {
                if matches!(event, ProxyEvent::Finished { .. }) {
                    finished += 1;
                }
                events.push(event);
                if finished >= finishes {
                    return events;
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    panic!(
        "only {finished} of {finishes} exchanges finished within the test timeout; got {} events",
        events.len()
    );
}

fn started_events(events: &[ProxyEvent]) -> Vec<&RequestInfo> {
    events
        .iter()
        .filter_map(|event| match event {
            ProxyEvent::Started { request, .. } => Some(request),
            _ => None,
        })
        .collect()
}

fn results(events: &[ProxyEvent]) -> Vec<ExchangeResult> {
    events
        .iter()
        .filter_map(|event| match event {
            ProxyEvent::Finished { end, .. } => Some(end.result.clone()),
            _ => None,
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn intercepts_https_and_captures_the_decrypted_exchange() {
    let (origin, origin_pem, mut origin_requests) = spawn_tls_origin(
        "127.0.0.1",
        "HTTP/1.1 200 OK\r\nContent-Length: 12\r\nConnection: close\r\n\r\nhello secure"
            .to_string(),
    )
    .await;
    let ca = Arc::new(CertificateAuthority::generate().expect("CA"));
    let proxy = proxy_with_tls(
        Some(ca.clone()),
        vec![origin_pem],
        ProxySettings {
            tls_intercept: true,
            match_replace: vec![MatchReplaceRule {
                enabled: true,
                direction: RuleDirection::Request,
                part: RulePart::Head,
                pattern: "original".to_string(),
                replacement: "rewritten".to_string(),
                regex: false,
            }],
            ..ProxySettings::default()
        },
    );

    let request = format!(
        "GET /secure HTTP/1.1\r\nHost: {origin}\r\nX-Test: original\r\nConnection: close\r\n\r\n"
    );
    let (_head, response) = tls_exchange(
        proxy.local_addr(),
        origin,
        "localhost",
        pem_roots(&[ca.cert_pem()]),
        &request,
    )
    .await;
    let text = String::from_utf8_lossy(&response);
    assert!(
        text.starts_with("HTTP/1.1 200 OK"),
        "response inside TLS was: {text}"
    );
    assert_eq!(body_after_head(&response), b"hello secure");

    // The origin sees a plain origin-form request, rewritten by the rule.
    let upstream = String::from_utf8_lossy(
        &origin_requests
            .recv()
            .await
            .expect("origin should capture the decrypted request"),
    )
    .into_owned();
    assert!(
        upstream.starts_with("GET /secure HTTP/1.1"),
        "upstream request was: {upstream}"
    );
    assert!(
        upstream.to_ascii_lowercase().contains("x-test: rewritten"),
        "match/replace did not run on the decrypted request:\n{upstream}"
    );
    assert!(
        upstream
            .to_ascii_lowercase()
            .contains(&format!("host: {origin}")),
        "upstream request lost the Host header:\n{upstream}"
    );

    let events = drain_events(&proxy, 2);
    let started = started_events(&events);
    assert_eq!(started.len(), 2, "CONNECT plus one decrypted request");
    let connect = started
        .iter()
        .find(|request| request.method == "CONNECT")
        .expect("a CONNECT exchange");
    assert_eq!(connect.scheme, "https");
    let inner = started
        .iter()
        .find(|request| request.method == "GET")
        .expect("a decrypted GET exchange");
    assert_eq!(inner.scheme, "https");
    assert_eq!(inner.uri, "/secure");
    assert_eq!(inner.host, origin.to_string());
    assert_eq!(inner.path, "/secure");

    let results = results(&events);
    assert!(
        results
            .iter()
            .any(|result| matches!(result, ExchangeResult::InterceptedTls { requests: 1 })),
        "expected an intercepted tunnel result, got {results:?}"
    );
    assert!(
        results.iter().any(|result| matches!(
            result,
            ExchangeResult::Response { body } if body.data() == b"hello secure"
        )),
        "expected a captured decrypted response, got {results:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn intercepted_requests_without_host_fall_back_to_the_connect_authority() {
    let (origin, origin_pem, mut origin_requests) = spawn_tls_origin(
        "127.0.0.1",
        "HTTP/1.1 200 OK\r\nContent-Length: 7\r\nConnection: close\r\n\r\nnomatch".to_string(),
    )
    .await;
    let ca = Arc::new(CertificateAuthority::generate().expect("CA"));
    let proxy = proxy_with_tls(
        Some(ca.clone()),
        vec![origin_pem],
        ProxySettings {
            tls_intercept: true,
            ..ProxySettings::default()
        },
    );

    // An HTTP/1.0 request without a Host header: only the CONNECT authority
    // (including its non-default port) identifies the target.
    let request = "GET /nohost HTTP/1.0\r\nConnection: close\r\n\r\n";
    let (_head, response) = tls_exchange(
        proxy.local_addr(),
        origin,
        "127.0.0.1",
        pem_roots(&[ca.cert_pem()]),
        request,
    )
    .await;
    assert_eq!(body_after_head(&response), b"nomatch");

    let upstream = String::from_utf8_lossy(
        &origin_requests
            .recv()
            .await
            .expect("origin should capture the decrypted request"),
    )
    .into_owned();
    assert!(
        upstream
            .to_ascii_lowercase()
            .contains(&format!("host: {origin}")),
        "fallback lost the CONNECT port:\n{upstream}"
    );

    let events = drain_events(&proxy, 2);
    assert!(
        results(&events)
            .iter()
            .any(|result| matches!(result, ExchangeResult::Response { .. })),
        "expected the decrypted response to be captured"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn tls_passthrough_hosts_stay_encrypted() {
    let (origin, origin_pem, _origin_requests) = spawn_tls_origin(
        "127.0.0.1",
        "HTTP/1.1 200 OK\r\nContent-Length: 9\r\nConnection: close\r\n\r\ntunnel me".to_string(),
    )
    .await;
    let ca = Arc::new(CertificateAuthority::generate().expect("CA"));
    let proxy = proxy_with_tls(
        Some(ca),
        Vec::new(),
        ProxySettings {
            tls_intercept: true,
            tls_passthrough: vec!["127.0.0.1".to_string()],
            ..ProxySettings::default()
        },
    );

    // Trusting the origin directly only works when the tunnel stays opaque.
    let request = format!("GET /raw HTTP/1.1\r\nHost: {origin}\r\nConnection: close\r\n\r\n");
    let (_head, response) = tls_exchange(
        proxy.local_addr(),
        origin,
        "127.0.0.1",
        pem_roots(&[&origin_pem]),
        &request,
    )
    .await;
    assert_eq!(body_after_head(&response), b"tunnel me");

    let events = drain_events(&proxy, 1);
    assert_eq!(
        started_events(&events).len(),
        1,
        "only the CONNECT exchange"
    );
    let results = results(&events);
    assert!(
        matches!(results.as_slice(), [ExchangeResult::Tunnel { .. }]),
        "expected a passthrough tunnel result, got {results:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn tls_interception_is_off_by_default_even_with_a_ca() {
    let (origin, origin_pem, _origin_requests) = spawn_tls_origin(
        "127.0.0.1",
        "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok".to_string(),
    )
    .await;
    let ca = Arc::new(CertificateAuthority::generate().expect("CA"));
    let proxy = proxy_with_tls(Some(ca), Vec::new(), ProxySettings::default());

    let request = format!("GET /default HTTP/1.1\r\nHost: {origin}\r\nConnection: close\r\n\r\n");
    let (_head, response) = tls_exchange(
        proxy.local_addr(),
        origin,
        "127.0.0.1",
        pem_roots(&[&origin_pem]),
        &request,
    )
    .await;
    assert_eq!(body_after_head(&response), b"ok");

    let events = drain_events(&proxy, 1);
    let results = results(&events);
    assert!(
        matches!(results.as_slice(), [ExchangeResult::Tunnel { .. }]),
        "expected a passthrough tunnel result, got {results:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn tls_authority_can_be_added_at_runtime() {
    let (origin, origin_pem, mut origin_requests) = spawn_tls_origin(
        "127.0.0.1",
        "HTTP/1.1 200 OK\r\nContent-Length: 7\r\nConnection: close\r\n\r\nruntime".to_string(),
    )
    .await;
    // Interception is requested but no CA is configured yet: passthrough.
    let proxy = proxy_with_tls(
        None,
        vec![origin_pem.clone()],
        ProxySettings {
            tls_intercept: true,
            ..ProxySettings::default()
        },
    );

    let request = format!("GET /first HTTP/1.1\r\nHost: {origin}\r\nConnection: close\r\n\r\n");
    let (_head, response) = tls_exchange(
        proxy.local_addr(),
        origin,
        "127.0.0.1",
        pem_roots(&[&origin_pem]),
        &request,
    )
    .await;
    assert_eq!(body_after_head(&response), b"runtime");
    let events = drain_events(&proxy, 1);
    assert!(matches!(
        results(&events).as_slice(),
        [ExchangeResult::Tunnel { .. }]
    ));
    let _ = origin_requests.recv().await;

    // Adding the CA at runtime sends new tunnels through interception.
    let ca = Arc::new(CertificateAuthority::generate().expect("CA"));
    proxy.set_tls_authority(Some(ca.clone()));
    let request = format!("GET /second HTTP/1.1\r\nHost: {origin}\r\nConnection: close\r\n\r\n");
    let (_head, response) = tls_exchange(
        proxy.local_addr(),
        origin,
        "127.0.0.1",
        pem_roots(&[ca.cert_pem()]),
        &request,
    )
    .await;
    assert_eq!(body_after_head(&response), b"runtime");

    let events = drain_events(&proxy, 2);
    let outcomes = results(&events);
    assert!(
        outcomes
            .iter()
            .any(|result| matches!(result, ExchangeResult::InterceptedTls { requests: 1 })),
        "expected an intercepted tunnel result, got {outcomes:?}"
    );

    // Removing it again falls back to passthrough for the next tunnel.
    proxy.set_tls_authority(None);
    let request = format!("GET /third HTTP/1.1\r\nHost: {origin}\r\nConnection: close\r\n\r\n");
    let (_head, response) = tls_exchange(
        proxy.local_addr(),
        origin,
        "127.0.0.1",
        pem_roots(&[&origin_pem]),
        &request,
    )
    .await;
    assert_eq!(body_after_head(&response), b"runtime");
    let events = drain_events(&proxy, 1);
    assert!(matches!(
        results(&events).as_slice(),
        [ExchangeResult::Tunnel { .. }]
    ));
}

// ---------------------------------------------------------------------------
// HTTP/2
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn forwards_and_captures_h2c_requests() {
    let (origin, origin_request) =
        spawn_capture_origin("HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok".to_string()).await;
    let proxy = test_proxy(64 * 1024, 64);

    let stream = TcpStream::connect(proxy.local_addr()).await.unwrap();
    let (version, status, _headers, body) =
        h2_exchange(TokioIo::new(stream), format!("http://{origin}/h2c")).await;
    assert_eq!(version, Version::HTTP_2);
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, b"ok");

    let upstream = String::from_utf8_lossy(
        &origin_request
            .await
            .expect("origin should receive the forwarded h2c request"),
    )
    .into_owned();
    assert!(
        upstream.starts_with("GET /h2c HTTP/1.1"),
        "upstream request was: {upstream}"
    );

    let events = drain_until_finished(&proxy);
    let request = started_of(&events);
    assert_eq!(request.version, "HTTP/2");
    assert_eq!(request.path, "/h2c");
    let response = head_of(&events);
    assert_eq!(response.version, "HTTP/1.1");
    assert_eq!(finished_of(&events).request_body.data(), b"");
}

#[tokio::test(flavor = "multi_thread")]
async fn forwards_https_requests_over_an_h2_upstream() {
    let (origin, origin_pem, mut origin_requests) =
        spawn_h2_tls_origin("127.0.0.1", b"hello h2").await;
    let proxy = proxy_with_tls(None, vec![origin_pem], ProxySettings::default());

    let request =
        format!("GET https://{origin}/up HTTP/1.1\r\nHost: {origin}\r\nConnection: close\r\n\r\n");
    let response = raw_exchange(proxy.local_addr(), &request).await;
    let text = String::from_utf8_lossy(&response);
    assert!(
        text.starts_with("HTTP/1.1 200 OK"),
        "HTTP/1.1 client response was: {text}"
    );
    assert_eq!(body_after_head(&response), b"hello h2");

    let upstream = tokio::time::timeout(TIMEOUT, origin_requests.recv())
        .await
        .expect("origin should receive the request")
        .expect("origin request channel");
    assert_eq!(upstream.version, Version::HTTP_2);
    assert_eq!(upstream.method, Method::GET);
    assert_eq!(upstream.uri.path(), "/up");
    let expected_authority = origin.to_string();
    assert_eq!(
        upstream.uri.authority().map(|authority| authority.as_str()),
        Some(expected_authority.as_str())
    );
    assert!(
        upstream.headers.get("host").is_none(),
        "HTTP/2 upstream request must use :authority, not Host: {:?}",
        upstream.headers
    );
    assert_eq!(upstream.body, b"");

    let events = drain_until_finished(&proxy);
    assert_eq!(started_of(&events).version, "HTTP/1.1");
    let response = head_of(&events);
    assert_eq!(response.version, "HTTP/2");
    assert_eq!(response.status, 200);
    assert!(response.reason.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn intercepts_h2_inside_a_tls_tunnel_and_forwards_it_as_h2() {
    let (origin, origin_pem, mut origin_requests) =
        spawn_h2_tls_origin("127.0.0.1", b"hello h2").await;
    let ca = Arc::new(CertificateAuthority::generate().expect("CA"));
    let proxy = proxy_with_tls(
        Some(ca.clone()),
        vec![origin_pem],
        ProxySettings {
            tls_intercept: true,
            ..ProxySettings::default()
        },
    );

    let mut stream = TcpStream::connect(proxy.local_addr()).await.unwrap();
    let connect = format!("CONNECT {origin} HTTP/1.1\r\nHost: {origin}\r\n\r\n");
    stream.write_all(connect.as_bytes()).await.unwrap();
    let head = read_http_head(&mut stream).await;
    assert!(
        head.starts_with("HTTP/1.1 200"),
        "unexpected CONNECT response: {head}"
    );

    let mut config = ClientConfig::builder()
        .with_root_certificates(pem_roots(&[ca.cert_pem()]))
        .with_no_client_auth();
    config.alpn_protocols = vec![b"h2".to_vec()];
    let connector = TlsConnector::from(Arc::new(config));
    let name = ServerName::try_from("localhost".to_string()).expect("valid server name");
    let tls = connector
        .connect(name, stream)
        .await
        .expect("client TLS handshake should negotiate h2");
    assert_eq!(tls.get_ref().1.alpn_protocol(), Some(b"h2".as_slice()));

    let (version, status, _headers, body) =
        h2_exchange(TokioIo::new(tls), format!("https://{origin}/secure")).await;
    assert_eq!(version, Version::HTTP_2);
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, b"hello h2");

    let upstream = tokio::time::timeout(TIMEOUT, origin_requests.recv())
        .await
        .expect("origin should receive the decrypted HTTP/2 request")
        .expect("origin request channel");
    assert_eq!(upstream.version, Version::HTTP_2);
    assert_eq!(upstream.uri.path(), "/secure");

    let events = drain_events(&proxy, 2);
    let started = started_events(&events);
    let inner = started
        .iter()
        .find(|request| request.method == "GET")
        .expect("a decrypted GET exchange");
    assert_eq!(inner.scheme, "https");
    assert_eq!(inner.version, "HTTP/2");
    assert_eq!(inner.path, "/secure");
    let response = events
        .iter()
        .find_map(|event| match event {
            ProxyEvent::ResponseHead { head, .. } if head.version == "HTTP/2" => Some(head),
            _ => None,
        })
        .expect("an upstream HTTP/2 response head");
    assert_eq!(response.status, 200);
    assert!(
        results(&events)
            .iter()
            .any(|result| matches!(result, ExchangeResult::InterceptedTls { requests: 1 })),
        "expected one intercepted HTTP/2 exchange"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn intercepts_and_forwards_an_edited_h2_request() {
    let (origin, origin_request) =
        spawn_capture_origin("HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok".to_string()).await;
    let mut proxy = test_proxy_with(
        ProxySettings {
            intercept_requests: true,
            ..ProxySettings::default()
        },
        64 * 1024,
        64 * 1024,
    );

    let proxy_addr = proxy.local_addr();
    let client = tokio::spawn(async move {
        let stream = TcpStream::connect(proxy_addr).await.unwrap();
        h2_exchange(TokioIo::new(stream), format!("http://{origin}/held")).await
    });

    let mut held = next_intercept(&mut proxy).await;
    assert_eq!(held.kind(), InterceptKind::Request);
    assert_eq!(held.request().version, "HTTP/2");
    let head = held.head().to_string();
    assert!(
        head.starts_with("GET /held HTTP/2\r\n"),
        "unexpected HTTP/2 head: {head:?}"
    );
    assert!(
        head.to_ascii_lowercase()
            .contains(&format!("host: {origin}\r\n")),
        "HTTP/2 head lost :authority: {head:?}"
    );
    let edited = head.replace("GET /held HTTP/2", "GET /edited HTTP/2");
    held.forward(&edited, None)
        .expect("HTTP/2 request should forward");

    let (version, status, _headers, body) = client.await.expect("h2c client task");
    assert_eq!(version, Version::HTTP_2);
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, b"ok");

    let upstream = String::from_utf8_lossy(
        &origin_request
            .await
            .expect("origin should receive the edited request"),
    )
    .into_owned();
    assert!(
        upstream.starts_with("GET /edited HTTP/1.1"),
        "upstream request was: {upstream}"
    );

    let events = drain_until_finished(&proxy);
    assert_eq!(started_of(&events).version, "HTTP/2");
}

#[tokio::test(flavor = "multi_thread")]
async fn intercepts_and_forwards_an_edited_h2_response() {
    let (origin, origin_pem, mut origin_requests) =
        spawn_h2_tls_origin("127.0.0.1", b"hello h2").await;
    let mut proxy = proxy_with_tls(
        None,
        vec![origin_pem],
        ProxySettings {
            intercept_responses: true,
            ..ProxySettings::default()
        },
    );

    let proxy_addr = proxy.local_addr();
    let client = tokio::spawn(async move {
        let stream = TcpStream::connect(proxy_addr).await.unwrap();
        h2_exchange(TokioIo::new(stream), format!("https://{origin}/edit")).await
    });

    let mut held = next_intercept(&mut proxy).await;
    assert_eq!(held.kind(), InterceptKind::Response);
    let head = held.head().to_string();
    assert!(
        head.starts_with("HTTP/2 200\r\n"),
        "unexpected HTTP/2 response head: {head:?}"
    );
    assert!(
        !head.contains("OK"),
        "HTTP/2 response head should not have a reason phrase: {head:?}"
    );
    held.forward("HTTP/2 201\r\nX-Edited: yes\r\n\r\n", None)
        .expect("HTTP/2 response should forward");

    let (version, status, headers, body) = client.await.expect("h2c client task");
    assert_eq!(version, Version::HTTP_2);
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(headers.get("x-edited").unwrap(), "yes");
    assert_eq!(body, b"hello h2");

    let upstream = tokio::time::timeout(TIMEOUT, origin_requests.recv())
        .await
        .expect("origin should receive the request")
        .expect("origin request channel");
    assert_eq!(upstream.uri.path(), "/edit");

    let events = drain_until_finished(&proxy);
    let response = head_of(&events);
    assert_eq!(response.version, "HTTP/2");
    assert_eq!(response.status, 201);
    assert!(response.reason.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn h2c_absolute_https_requests_keep_their_scheme_after_head_rewrites() {
    let (origin, origin_pem, mut origin_requests) =
        spawn_h2_tls_origin("127.0.0.1", b"hello h2").await;
    let proxy = proxy_with_tls(
        None,
        vec![origin_pem],
        ProxySettings {
            match_replace: vec![MatchReplaceRule {
                enabled: true,
                direction: RuleDirection::Request,
                part: RulePart::Head,
                pattern: "/rewritten".to_string(),
                replacement: "/target".to_string(),
                regex: false,
            }],
            ..ProxySettings::default()
        },
    );

    let stream = TcpStream::connect(proxy.local_addr()).await.unwrap();
    let (version, status, _headers, body) =
        h2_exchange(TokioIo::new(stream), format!("https://{origin}/rewritten")).await;
    assert_eq!(version, Version::HTTP_2);
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, b"hello h2");

    let upstream = tokio::time::timeout(TIMEOUT, origin_requests.recv())
        .await
        .expect("origin should receive the rewritten request")
        .expect("origin request channel");
    assert_eq!(upstream.version, Version::HTTP_2);
    assert_eq!(upstream.uri.path(), "/target");

    let events = drain_until_finished(&proxy);
    let request = started_of(&events);
    assert_eq!(request.scheme, "https");
    assert_eq!(request.version, "HTTP/2");
    assert_eq!(request.path, "/target");
    assert_eq!(head_of(&events).version, "HTTP/2");
}

#[tokio::test(flavor = "multi_thread")]
async fn websocket_handshake_relays_frames_and_captures_messages() {
    init_logging();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = listener.local_addr().unwrap();
    let (head_tx, head_rx) = oneshot::channel();
    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.expect("WebSocket origin accept");
        let head = read_request(&mut stream).await;
        let _ = head_tx.send(head);
        stream
            .write_all(
                b"HTTP/1.1 101 Switching Protocols\r\n\
                  Upgrade: websocket\r\n\
                  Connection: Upgrade\r\n\
                  Sec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo=\r\n\
                  Sec-WebSocket-Protocol: chat\r\n\
                  \r\n",
            )
            .await
            .expect("101 response");
        for expected in [
            b"hello".as_slice(),
            b"paused".as_slice(),
            b"again".as_slice(),
        ] {
            let (fin, opcode, payload) = read_websocket_frame(&mut stream).await;
            assert!(fin);
            assert_eq!(opcode, 1);
            assert_eq!(payload, expected);
            stream
                .write_all(&websocket_server_frame(1, &payload))
                .await
                .expect("echo frame");
        }
        let _ = tokio::time::timeout(Duration::from_millis(200), stream.read(&mut [0u8; 1])).await;
    });

    let proxy = test_proxy(1024, 64);
    let mut stream = TcpStream::connect(proxy.local_addr()).await.unwrap();
    let request = format!(
        "GET http://{origin}/socket HTTP/1.1\r\n\
         Host: {origin}\r\n\
         Upgrade: websocket\r\n\
         Connection: Upgrade\r\n\
         Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
         Sec-WebSocket-Version: 13\r\n\
         Sec-WebSocket-Protocol: chat\r\n\
         Sec-WebSocket-Extensions: permessage-deflate\r\n\
         \r\n"
    );
    stream.write_all(request.as_bytes()).await.unwrap();
    let response = read_http_head(&mut stream).await;
    assert!(
        response.starts_with("HTTP/1.1 101"),
        "unexpected handshake response: {response:?}"
    );
    let lower = response.to_ascii_lowercase();
    assert!(lower.contains("upgrade: websocket"));
    assert!(lower.contains("connection: upgrade"));
    assert!(lower.contains("sec-websocket-protocol: chat"));
    assert!(
        !lower.contains("sec-websocket-extensions"),
        "the proxy must not negotiate a compression extension it cannot decode"
    );

    stream
        .write_all(&websocket_masked_frame(1, b"hello"))
        .await
        .unwrap();
    let (fin, opcode, payload) = read_websocket_frame(&mut stream).await;
    assert!(fin);
    assert_eq!(opcode, 1);
    assert_eq!(payload, b"hello");

    // The proxy forwards a frame before emitting its capture event. Wait for
    // both directions before pausing so the pause transition cannot race the
    // final capture callback for this exchange.
    let mut messages = Vec::new();
    let deadline = Instant::now() + TIMEOUT;
    while messages.len() < 2 && Instant::now() < deadline {
        match proxy.recv_timeout(Duration::from_millis(100)) {
            Ok(ProxyEvent::WebSocketMessage {
                connection,
                direction,
                opcode,
                payload,
                ..
            }) => messages.push((connection, direction, opcode, payload)),
            Ok(event) => panic!("unexpected HTTP history event for a WebSocket: {event:?}"),
            Err(_) => {}
        }
    }
    assert_eq!(
        messages.len(),
        2,
        "both hello frames should be captured before pausing"
    );

    // A persistent WebSocket stays open while capture is paused. The paused
    // message must be forwarded but omitted, and capture must resume without a
    // new handshake.
    proxy.set_paused(true);
    stream
        .write_all(&websocket_masked_frame(1, b"paused"))
        .await
        .unwrap();
    let (_, opcode, payload) = read_websocket_frame(&mut stream).await;
    assert_eq!(opcode, 1);
    assert_eq!(payload, b"paused");
    tokio::time::sleep(Duration::from_millis(150)).await;
    proxy.set_paused(false);
    stream
        .write_all(&websocket_masked_frame(1, b"again"))
        .await
        .unwrap();
    let (_, opcode, payload) = read_websocket_frame(&mut stream).await;
    assert_eq!(opcode, 1);
    assert_eq!(payload, b"again");

    let upstream_head = tokio::time::timeout(TIMEOUT, head_rx)
        .await
        .expect("origin should receive the handshake")
        .expect("handshake request channel");
    let upstream_lower = String::from_utf8_lossy(&upstream_head).to_ascii_lowercase();
    assert!(upstream_lower.contains("connection: upgrade"));
    assert!(upstream_lower.contains("upgrade: websocket"));
    assert!(
        !upstream_lower.contains("sec-websocket-extensions"),
        "extension offers should be stripped before reaching the origin"
    );

    let deadline = Instant::now() + TIMEOUT;
    while messages.len() < 4 && Instant::now() < deadline {
        match proxy.recv_timeout(Duration::from_millis(100)) {
            Ok(ProxyEvent::WebSocketMessage {
                connection,
                direction,
                opcode,
                payload,
                ..
            }) => messages.push((connection, direction, opcode, payload)),
            Ok(event) => panic!("unexpected HTTP history event for a WebSocket: {event:?}"),
            Err(_) => {}
        }
    }
    assert_eq!(
        messages.len(),
        4,
        "capture should resume after it is unpaused"
    );
    assert!(
        messages
            .iter()
            .all(|(_, _, _, payload)| payload.data() != b"paused"),
        "a message completed while capture was paused must not enter history"
    );

    let find = |direction, payload: &[u8]| {
        messages
            .iter()
            .find(|(_, candidate_direction, _, candidate)| {
                *candidate_direction == direction && candidate.data() == payload
            })
            .unwrap_or_else(|| panic!("missing {direction:?} {payload:?} message"))
    };
    let (connection, direction, opcode, payload) = find(WebSocketDirection::ToServer, b"hello");
    assert_eq!(*direction, WebSocketDirection::ToServer);
    assert_eq!(*opcode, WebSocketOpcode::Text);
    assert_eq!(payload.data(), b"hello");
    assert_eq!(connection.url, format!("ws://{origin}/socket"));
    assert_eq!(connection.host, origin.to_string());
    assert!(!connection.tls);
    assert_eq!(connection.listener_port, proxy.local_addr().port());
    assert_eq!(connection.protocol.as_deref(), Some("chat"));
    let (downstream, direction, opcode, payload) = find(WebSocketDirection::ToClient, b"hello");
    assert_eq!(*direction, WebSocketDirection::ToClient);
    assert_eq!(*opcode, WebSocketOpcode::Text);
    assert_eq!(payload.data(), b"hello");
    assert_eq!(downstream.id, connection.id);
    let (again_upstream, _, _, _) = find(WebSocketDirection::ToServer, b"again");
    let (again_downstream, _, _, _) = find(WebSocketDirection::ToClient, b"again");
    assert_eq!(again_upstream.id, connection.id);
    assert_eq!(again_downstream.id, connection.id);
}

#[tokio::test(flavor = "multi_thread")]
async fn rejected_websocket_handshake_stays_in_http_history() {
    init_logging();
    let origin = spawn_origin(
        "HTTP/1.1 403 Forbidden\r\nContent-Type: text/plain\r\nContent-Length: 4\r\n\r\nnope"
            .to_string(),
    )
    .await;
    let proxy = test_proxy(1024, 64);
    let mut stream = TcpStream::connect(proxy.local_addr()).await.unwrap();
    let request = format!(
        "GET http://{origin}/socket HTTP/1.1\r\n\
         Host: {origin}\r\n\
         Upgrade: websocket\r\n\
         Connection: Upgrade\r\n\
         Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
         Sec-WebSocket-Version: 13\r\n\
         \r\n"
    );
    stream.write_all(request.as_bytes()).await.unwrap();
    let response = read_http_head(&mut stream).await;
    assert!(
        response.starts_with("HTTP/1.1 403"),
        "unexpected rejected handshake response: {response:?}"
    );
    drop(stream);

    let events = drain_until_finished(&proxy);
    let request = started_of(&events);
    assert_eq!(request.method, "GET");
    assert_eq!(request.host, origin.to_string());
    assert_eq!(head_of(&events).status, 403);
    assert!(
        events
            .iter()
            .all(|event| !matches!(event, ProxyEvent::WebSocketMessage { .. })),
        "a non-101 handshake must not create WebSocket history rows"
    );
    let ExchangeResult::Response { body } = &finished_of(&events).result else {
        panic!("rejected handshake should finish as a regular HTTP response");
    };
    assert_eq!(
        body.data(),
        b"nope",
        "the rejected handshake response body should be captured as HTTP"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn intercepts_wss_and_captures_decrypted_messages() {
    init_logging();
    let (config, origin_pem) = origin_certificate("127.0.0.1");
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = listener.local_addr().unwrap();
    let config = Arc::new(config);
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("WSS origin accept");
        let acceptor = TlsAcceptor::from(config);
        let mut tls = acceptor.accept(stream).await.expect("origin TLS handshake");
        let head = read_request(&mut tls).await;
        assert!(
            String::from_utf8_lossy(&head)
                .to_ascii_lowercase()
                .contains("upgrade: websocket"),
            "origin did not receive an upgrade request"
        );
        tls.write_all(
            b"HTTP/1.1 101 Switching Protocols\r\n\
              Upgrade: websocket\r\n\
              Connection: Upgrade\r\n\
              Sec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo=\r\n\
              Sec-WebSocket-Protocol: chat\r\n\
              \r\n",
        )
        .await
        .expect("101 response");
        let (fin, opcode, payload) = read_websocket_frame(&mut tls).await;
        assert!(fin);
        assert_eq!(opcode, 1);
        assert_eq!(payload, b"secure hello");
        tls.write_all(&websocket_server_frame(1, &payload))
            .await
            .expect("echo frame");
        let _ = tokio::time::timeout(Duration::from_millis(200), tls.read(&mut [0u8; 1])).await;
    });

    let ca = Arc::new(CertificateAuthority::generate().expect("CA"));
    let proxy = proxy_with_tls(
        Some(ca.clone()),
        vec![origin_pem],
        ProxySettings {
            tls_intercept: true,
            ..ProxySettings::default()
        },
    );

    let mut stream = TcpStream::connect(proxy.local_addr()).await.unwrap();
    let connect = format!("CONNECT {origin} HTTP/1.1\r\nHost: {origin}\r\n\r\n");
    stream.write_all(connect.as_bytes()).await.unwrap();
    let connect_head = read_http_head(&mut stream).await;
    assert!(
        connect_head.starts_with("HTTP/1.1 200"),
        "unexpected CONNECT response: {connect_head:?}"
    );

    let mut tls_config = ClientConfig::builder()
        .with_root_certificates(pem_roots(&[ca.cert_pem()]))
        .with_no_client_auth();
    tls_config.alpn_protocols = vec![b"http/1.1".to_vec()];
    let connector = TlsConnector::from(Arc::new(tls_config));
    let name = ServerName::try_from("localhost".to_string()).expect("valid server name");
    let mut tls = connector
        .connect(name, stream)
        .await
        .expect("client TLS handshake through the proxy");
    let request = format!(
        "GET /chat HTTP/1.1\r\n\
         Host: {origin}\r\n\
         Upgrade: websocket\r\n\
         Connection: Upgrade\r\n\
         Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
         Sec-WebSocket-Version: 13\r\n\
         Sec-WebSocket-Protocol: chat\r\n\
         \r\n"
    );
    tls.write_all(request.as_bytes()).await.unwrap();
    let response = read_http_head(&mut tls).await;
    assert!(
        response.starts_with("HTTP/1.1 101"),
        "unexpected decrypted handshake response: {response:?}"
    );
    tls.write_all(&websocket_masked_frame(1, b"secure hello"))
        .await
        .unwrap();
    let (fin, opcode, payload) = read_websocket_frame(&mut tls).await;
    assert!(fin);
    assert_eq!(opcode, 1);
    assert_eq!(payload, b"secure hello");

    let mut messages = Vec::new();
    let deadline = Instant::now() + TIMEOUT;
    while messages.len() < 2 && Instant::now() < deadline {
        match proxy.recv_timeout(Duration::from_millis(100)) {
            Ok(ProxyEvent::WebSocketMessage {
                connection,
                direction,
                opcode,
                payload,
                ..
            }) => messages.push((connection, direction, opcode, payload)),
            Ok(ProxyEvent::Started { request, .. }) => {
                assert_ne!(
                    request.method, "GET",
                    "a successful WebSocket handshake must not create an HTTP history row"
                );
            }
            Ok(_) => {}
            Err(_) => {}
        }
    }
    assert_eq!(messages.len(), 2, "both WSS directions should be captured");
    let (connection, direction, opcode, payload) = &messages[0];
    assert_eq!(*direction, WebSocketDirection::ToServer);
    assert_eq!(*opcode, WebSocketOpcode::Text);
    assert_eq!(payload.data(), b"secure hello");
    assert_eq!(connection.url, format!("wss://{origin}/chat"));
    assert_eq!(connection.host, origin.to_string());
    assert!(connection.tls);
    assert_eq!(connection.listener_port, proxy.local_addr().port());
    assert_eq!(connection.protocol.as_deref(), Some("chat"));

    let (connection, direction, opcode, payload) = &messages[1];
    assert_eq!(*direction, WebSocketDirection::ToClient);
    assert_eq!(*opcode, WebSocketOpcode::Text);
    assert_eq!(payload.data(), b"secure hello");
    assert_eq!(connection.id, messages[0].0.id);
}

#[tokio::test(flavor = "multi_thread")]
async fn intercepts_and_forwards_an_edited_h2_request_body() {
    let (origin, origin_pem, mut origin_requests) = spawn_h2_tls_origin("127.0.0.1", b"ok").await;
    let mut proxy = proxy_with_tls(
        None,
        vec![origin_pem],
        ProxySettings {
            intercept_requests: true,
            ..ProxySettings::default()
        },
    );

    let proxy_addr = proxy.local_addr();
    let client = tokio::spawn(async move {
        let stream = TcpStream::connect(proxy_addr).await.unwrap();
        h2_exchange_with(
            TokioIo::new(stream),
            Method::POST,
            format!("https://{origin}/body"),
            Bytes::from_static(b"hello"),
        )
        .await
    });

    let mut held = next_intercept(&mut proxy).await;
    assert_eq!(held.kind(), InterceptKind::Request);
    assert_eq!(
        held.body(),
        Some(b"hello".as_slice()),
        "the HTTP/2 request body should be buffered for editing"
    );
    let head = held.head().to_string();
    assert!(
        head.to_ascii_lowercase().contains("content-length: 5\r\n"),
        "HTTP/2 body length was not represented in the head: {head:?}"
    );
    held.forward(&head, Some(b"world".to_vec()))
        .expect("HTTP/2 request body should forward");

    let (version, status, _headers, body) = client.await.expect("h2c client task");
    assert_eq!(version, Version::HTTP_2);
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, b"ok");

    let upstream = tokio::time::timeout(TIMEOUT, origin_requests.recv())
        .await
        .expect("origin should receive the edited body")
        .expect("origin request channel");
    assert_eq!(upstream.version, Version::HTTP_2);
    assert_eq!(upstream.method, Method::POST);
    assert_eq!(upstream.uri.path(), "/body");
    assert_eq!(upstream.body, b"world");

    let events = drain_until_finished(&proxy);
    assert_eq!(started_of(&events).scheme, "https");
    assert_eq!(finished_of(&events).request_body.data(), b"world");
    assert_eq!(head_of(&events).version, "HTTP/2");
}
