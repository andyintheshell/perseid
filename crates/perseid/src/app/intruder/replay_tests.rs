//! Synthetic capture -> workspace -> real transport -> local receiver.

use super::*;
use crate::history::{HistoryItem, Phase};
use crate::proxy::{CapturedBody, RequestInfo};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::mpsc;
use std::time::{Instant, SystemTime};

type Receiver = (
    String,
    mpsc::Receiver<(String, Vec<u8>)>,
    std::thread::JoinHandle<()>,
);

fn receiver() -> Receiver {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let host = listener.local_addr().unwrap().to_string();
    listener.set_nonblocking(true).unwrap();
    let (tx, rx) = mpsc::channel();
    let task = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(Instant::now() < deadline, "replay did not connect");
                    std::thread::sleep(Duration::from_millis(2));
                }
                Err(error) => panic!("accept: {error}"),
            }
        };
        stream.set_nonblocking(false).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let mut head = Vec::new();
        while !head.ends_with(b"\r\n\r\n") {
            let mut byte = [0];
            stream.read_exact(&mut byte).unwrap();
            head.push(byte[0]);
            assert!(head.len() < 65536);
        }
        let head = String::from_utf8(head).unwrap();
        let len: usize = head
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse().unwrap())
            })
            .unwrap_or(0);
        let mut body = vec![0; len];
        stream.read_exact(&mut body).unwrap();
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .unwrap();
        tx.send((head, body)).unwrap();
    });
    (host, rx, task)
}

fn item(host: &str, body: Vec<u8>, encoding: Option<&str>, content_type: &str) -> HistoryItem {
    let mut headers = vec![
        ("Host".into(), host.into()),
        ("Content-Length".into(), body.len().to_string()),
        ("Content-Type".into(), content_type.into()),
    ];
    if let Some(encoding) = encoding {
        headers.push(("Content-Encoding".into(), encoding.into()));
    }
    HistoryItem {
        id: 1,
        number: 1,
        timestamp: SystemTime::now(),
        request: RequestInfo {
            method: "POST".into(),
            uri: format!("http://{host}/?q=base"),
            scheme: "http".into(),
            host: host.into(),
            path: "/?q=base".into(),
            version: "HTTP/1.1".into(),
            headers,
        },
        request_body: CapturedBody::from_bytes(body),
        response: None,
        phase: Phase::InFlight,
    }
}

#[test]
fn captured_bodies_round_trip_both_tools_and_workspace_json() {
    let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    gzip.write_all(b"a=1\r\n").unwrap();
    let cases = vec![
        (b"a=1".to_vec(), None, "application/x-www-form-urlencoded"),
        (b"a=1\r\n".to_vec(), None, "text/plain"),
        (vec![0, 255, 128, 13, 10], None, "application/octet-stream"),
        (gzip.finish().unwrap(), Some("gzip"), "text/plain"),
        (b"--boundary\r\nContent-Disposition: form-data; name=\"file\"; filename=\"a.bin\"\r\n\r\n\0\xff\r\n--boundary--\r\n".to_vec(), None, "multipart/form-data; boundary=boundary"),
        (vec![b'x'; crate::view::TEXT_PREVIEW_LIMIT + 1024], None, "text/plain"),
    ];
    for (body, encoding, content_type) in cases {
        for intruder in [false, true] {
            let (host, received, task) = receiver();
            let capture = item(&host, body.clone(), encoding, content_type);
            let mut app = PerseidApp::test_app();
            app.scope_enforce = false;
            if intruder {
                app.send_history_to_intruder(&capture);
                app.intruder.request = app.intruder.request.replace("q=base", "q=$$base$$");
                app.intruder.payload_sets[0].text = "base".into();
                let saved = serde_json::to_vec(&app.intruder_workspace()).unwrap();
                app.apply_intruder_workspace(&serde_json::from_slice(&saved).unwrap());
                app.start_intruder_attack();
                assert!(app.intruder.error.is_none(), "{:?}", app.intruder.error);
            } else {
                app.send_history_to_repeater(&capture);
                let saved = serde_json::to_vec(&app.repeater_workspace()).unwrap();
                app.apply_repeater_workspace(
                    &serde_json::from_slice::<Vec<crate::project::RepeaterWorkspace>>(&saved)
                        .unwrap(),
                    0,
                );
                app.send_active_repeater();
                assert!(app.repeater_workspace()[0].error.is_none());
            }
            let (head, actual) = received.recv_timeout(Duration::from_secs(10)).unwrap();
            assert_eq!(actual.len(), body.len());
            assert_eq!(actual, body, "intruder={intruder}");
            assert_eq!(
                head.to_ascii_lowercase().contains("content-encoding: gzip"),
                encoding.is_some()
            );
            assert!(head.contains(content_type));
            task.join().unwrap();
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                app.drain_repeater_results();
                app.drain_intruder_results();
                let done = if intruder {
                    !app.intruder.running
                } else {
                    app.repeater_workspace()[0].response.is_some()
                };
                if done {
                    break;
                }
                assert!(Instant::now() < deadline, "replay response did not finish");
                std::thread::sleep(Duration::from_millis(2));
            }
            if intruder {
                assert!(matches!(
                    app.intruder.results[0].result,
                    RepeaterResult::Response { status: 200, .. }
                ));
                let saved = serde_json::to_vec(&app.intruder_workspace()).unwrap();
                app.apply_intruder_workspace(&serde_json::from_slice(&saved).unwrap());
                assert_eq!(
                    app.intruder
                        .last_plan
                        .as_ref()
                        .unwrap()
                        .preserved_body
                        .as_deref(),
                    Some(body.as_slice())
                );
            } else {
                assert_eq!(
                    app.repeater_workspace()[0]
                        .response
                        .as_ref()
                        .unwrap()
                        .status,
                    200
                );
            }
        }
    }
}

#[test]
fn incomplete_captures_block_both_send_paths_after_workspace_restore() {
    for (truncated, complete) in [(true, true), (false, false)] {
        let mut capture = item("127.0.0.1:1", b"prefix".to_vec(), None, "text/plain");
        capture.request_body =
            CapturedBody::from_stored_parts(b"prefix".to_vec(), 100, truncated, complete);
        let mut app = PerseidApp::test_app();
        app.send_history_to_repeater(&capture);
        let saved = app.repeater_workspace();
        app.apply_repeater_workspace(&saved, 0);
        app.send_active_repeater();
        assert!(
            app.repeater_workspace()[0]
                .error
                .as_ref()
                .unwrap()
                .contains("Acknowledge")
        );
        assert!(app.repeater.is_none());
        app.send_history_to_intruder(&capture);
        let saved = app.intruder_workspace();
        app.apply_intruder_workspace(&saved);
        app.start_intruder_attack();
        assert!(app.intruder.error.as_ref().unwrap().contains("Acknowledge"));
        assert!(app.intruder_engine.is_none());
    }
}
