//! End-to-end tests: real TCP client -> wiremoth proxy -> local origin server.

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
use wiremoth_proxy::{
    ExchangeResult, InterceptItem, InterceptKind, MatchReplaceRule, Proxy, ProxyConfig, ProxyEvent,
    ProxySettings, RequestInfo, RuleDirection, RulePart, StartError,
};

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

/// Read a request head plus declared body.
async fn read_request(stream: &mut TcpStream) -> Vec<u8> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let n = match stream.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        buf.extend_from_slice(&chunk[..n]);
        if let Some(head_end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&buf[..head_end]);
            let content_length = head
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().ok())
                        .flatten()
                })
                .unwrap_or(0);
            if buf.len() >= head_end + 4 + content_length {
                break;
            }
        }
    }
    buf
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

async fn next_intercept(proxy: &mut Proxy) -> InterceptItem {
    tokio::time::timeout(TIMEOUT, proxy.recv_intercept())
        .await
        .expect("an intercepted message before the test timeout")
        .expect("the intercept channel should stay open")
}

async fn read_http_head(stream: &mut TcpStream) -> String {
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

fn finished_of(events: &[ProxyEvent]) -> &wiremoth_proxy::ExchangeEnd {
    events
        .iter()
        .find_map(|event| match event {
            ProxyEvent::Finished { end, .. } => Some(end),
            _ => None,
        })
        .expect("a Finished event")
}

fn head_of(events: &[ProxyEvent]) -> &wiremoth_proxy::ResponseHead {
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
    let origin = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin_addr = origin.local_addr().unwrap();
    let origin_task = tokio::spawn(async move {
        let (_stream, _) = origin.accept().await.unwrap();
        tokio::time::sleep(TIMEOUT * 2).await;
    });

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
    origin_task.abort();
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
async fn dropping_an_intercepted_request_fails_the_exchange() {
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
    match &finished_of(&events).result {
        ExchangeResult::Failed { error } => assert!(error.contains("dropped")),
        other => panic!("expected a failed result, got {other:?}"),
    }
    assert_eq!(finished_of(&events).request_body.data(), b"old!");
}

#[tokio::test(flavor = "multi_thread")]
async fn dropping_an_intercepted_response_fails_the_exchange() {
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
    match &finished_of(&events).result {
        ExchangeResult::Failed { error } => assert!(error.contains("dropped")),
        other => panic!("expected a failed result, got {other:?}"),
    }
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
