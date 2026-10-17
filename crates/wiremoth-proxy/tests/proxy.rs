//! End-to-end tests: real TCP client -> wiremoth proxy -> local origin server.

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use wiremoth_proxy::{ExchangeResult, Proxy, ProxyConfig, ProxyEvent, RequestInfo};

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
    panic!("no Finished event within the test timeout; got {} events", events.len());
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
    let origin = spawn_origin("HTTP/1.1 200 Yo\r\nX-Origin: yes\r\nContent-Length: 11\r\n\r\nhello world".to_string()).await;
    let proxy = test_proxy(64 * 1024, 64);
    let request = format!(
        "GET http://{origin}/hello?x=1 HTTP/1.1\r\nHost: {origin}\r\nProxy-Connection: keep-alive\r\nConnection: close\r\n\r\n"
    );

    let response = raw_exchange(proxy.local_addr(), &request).await;
    let text = String::from_utf8_lossy(&response);
    assert!(text.starts_with("HTTP/1.1 200 Yo"), "raw response was: {text}");
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
    assert_eq!(received.len(), big.len(), "every byte must still reach the client");
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
    assert!(head.starts_with("HTTP/1.1 200"), "unexpected CONNECT response: {head}");

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
    let request =
        format!("GET http://{dead_addr}/ HTTP/1.1\r\nHost: {dead_addr}\r\nConnection: close\r\n\r\n");
    let response = raw_exchange(proxy.local_addr(), &request).await;
    let text = String::from_utf8_lossy(&response);
    assert!(text.starts_with("HTTP/1.1 502"), "raw response was: {text}");

    let events = drain_until_finished(&proxy);
    match &finished_of(&events).result {
        ExchangeResult::Failed { error } => assert!(error.contains("connect"), "error was: {error}"),
        other => panic!("expected a failed result, got {other:?}"),
    }
}
