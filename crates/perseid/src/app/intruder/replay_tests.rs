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
fn captured_head_markers_are_rejected_without_replacing_the_draft() {
    for literal in ["§", "§value§"] {
        for in_header in [false, true] {
            let mut capture = item("example.com", b"body".to_vec(), None, "text/plain");
            if in_header {
                capture
                    .request
                    .headers
                    .push(("X-Literal".into(), literal.into()));
            } else {
                capture.request.path = format!("/?q={literal}");
                capture.request.uri = format!("http://example.com/?q={literal}");
            }
            let mut app = PerseidApp::test_app();
            let before = serde_json::to_value(app.intruder_workspace()).unwrap();
            app.send_history_to_intruder(&capture);
            let error = app.intruder.error.as_deref().unwrap();
            assert!(error.contains("captured request line or headers contain literal §"));
            assert!(error.contains("Use Repeater"));
            assert!(matches!(app.main_tab, MainTab::Intruder));
            assert!(app.intruder_engine.is_none());
            let mut after = serde_json::to_value(app.intruder_workspace()).unwrap();
            after["error"] = before["error"].clone();
            assert_eq!(after, before);
            assert!(crate::view::replay_request_head(&capture).contains(literal));
            // Repeater does not interpret payload markers and remains usable.
            app.send_history_to_repeater(&capture);
            assert!(app.repeater_workspace()[0].request.contains(literal));
        }
    }
}

#[test]
fn captured_bodies_round_trip_both_tools_and_workspace_json() {
    let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    gzip.write_all(b"a=1\r\n").unwrap();
    let cases = vec![
        (b"a=1".to_vec(), None, "application/x-www-form-urlencoded"),
        (b"a=1\r\n".to_vec(), None, "text/plain"),
        (b"literal $$value$$".to_vec(), None, "text/plain"),
        ("literal §value§ and §".as_bytes().to_vec(), None, "text/plain"),
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
                app.intruder.request = app.intruder.request.replace("q=base", "q=§base§");
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
                        .body_bytes(
                            &app.intruder
                                .last_plan
                                .as_ref()
                                .unwrap()
                                .render_sequence(1)
                                .unwrap()
                                .raw
                        )
                        .unwrap(),
                    body
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
fn legacy_preserved_text_workspaces_become_inline_without_changing_captures() {
    let capture = item(
        "example.com",
        "a=中文\r\n".as_bytes().to_vec(),
        None,
        "text/plain",
    );
    let mut app = PerseidApp::test_app();
    app.send_history_to_repeater(&capture);
    let mut tabs = app.repeater_workspace();
    tabs[0].request = crate::view::replay_request_head(&capture);
    tabs[0].preserved_body = Some(crate::project::BodyWorkspace::from_body(
        &capture.request_body,
    ));
    app.apply_repeater_workspace(&tabs, 0);
    let restored = app.repeater_workspace();
    assert!(restored[0].preserved_body.is_none());
    assert_eq!(
        crate::proxy::split_raw_request(&restored[0].request).1,
        "a=中文\r\n"
    );

    app.send_history_to_intruder(&capture);
    let mut saved = app.intruder_workspace();
    saved.request = tabs[0].request.clone();
    saved.preserved_body = tabs[0].preserved_body.clone();
    app.apply_intruder_workspace(&saved);
    assert!(!app.intruder.body_escaped);
    assert_eq!(
        crate::proxy::split_raw_request(&app.intruder.request).1,
        "a=中文\r\n"
    );
    assert_eq!(capture.request_body.data(), "a=中文\r\n".as_bytes());
}

#[test]
fn legacy_binary_and_conflicting_drafts_restore_without_losing_user_bytes() {
    let capture = item(
        "example.com",
        b"\xff$$\\x00".to_vec(),
        None,
        "application/octet-stream",
    );
    let captured = crate::project::BodyWorkspace::from_body(&capture.request_body);
    for conflict in [false, true] {
        let mut app = PerseidApp::test_app();
        let request = format!(
            "{}{}",
            crate::view::replay_request_head(&capture),
            if conflict { "user edit" } else { "" }
        );
        let tabs = vec![crate::project::RepeaterWorkspace {
            host: "example.com".into(),
            request: request.clone(),
            preserved_body: Some(captured.clone()),
            ..Default::default()
        }];
        app.apply_repeater_workspace(&tabs, 0);
        let restored = app.repeater_workspace();
        assert_eq!(restored[0].body_incomplete, conflict);
        if conflict {
            assert_eq!(restored[0].request, request);
            assert_eq!(restored[0].preserved_body, Some(captured.clone()));
        } else {
            assert!(restored[0].preserved_body.is_none());
            assert_eq!(
                crate::app::replay_body::bytes(
                    &restored[0].request,
                    restored[0].body_escaped,
                    false
                )
                .unwrap(),
                capture.request_body.data()
            );
        }
        app.apply_intruder_workspace(&crate::project::IntruderWorkspace {
            host: "example.com".into(),
            request: request.clone(),
            preserved_body: Some(captured.clone()),
            ..Default::default()
        });
        let restored = app.intruder_workspace();
        assert_eq!(restored.body_incomplete, conflict);
        if conflict {
            assert_eq!(restored.request, request);
            assert_eq!(restored.preserved_body, Some(captured.clone()));
        } else {
            assert!(restored.preserved_body.is_none());
            assert_eq!(
                crate::app::replay_body::bytes(&restored.request, restored.body_escaped, false)
                    .unwrap(),
                capture.request_body.data()
            );
            assert!(
                crate::proxy::marker_ranges(&restored.request)
                    .unwrap()
                    .is_empty()
            );
        }
    }
}

#[test]
fn captured_literal_signs_survive_adjacent_positions_and_workspace_restore() {
    for legacy in [false, true] {
        for (body, expected) in [
            ("price=$5", "price=$9"),
            ("price=5$", "price=9$"),
            ("price=$5$", "price=$9$"),
            ("price=§5", "price=§9"),
            ("price=5§", "price=9§"),
            ("price=§5§", "price=§9§"),
        ] {
            let (host, received, task) = receiver();
            let capture = item(&host, body.as_bytes().to_vec(), None, "text/plain");
            let mut app = PerseidApp::test_app();
            app.scope_enforce = false;
            app.send_history_to_intruder(&capture);
            if legacy {
                let mut saved = app.intruder_workspace();
                saved.request = crate::view::replay_request_head(&capture);
                saved.preserved_body = Some(crate::project::BodyWorkspace::from_body(
                    &capture.request_body,
                ));
                saved.body_escaped = false;
                app.apply_intruder_workspace(&saved);
            }
            assert_eq!(app.intruder.body_escaped, body.contains('§'));
            assert_eq!(
                crate::app::replay_body::bytes(
                    &app.intruder.request,
                    app.intruder.body_escaped,
                    false,
                )
                .unwrap(),
                body.as_bytes()
            );
            let start = app.intruder.request.rfind('5').unwrap();
            let (request, _) = crate::proxy::toggle_marker(&app.intruder.request, start..start + 1);
            app.intruder.request = request;
            app.intruder.payload_sets[0].text = "9".into();
            let saved = serde_json::to_vec(&app.intruder_workspace()).unwrap();
            app.apply_intruder_workspace(&serde_json::from_slice(&saved).unwrap());
            let plan = app.intruder.build_plan();
            let rendered = plan.render_sequence(1).unwrap();
            assert_eq!(plan.body_bytes(&rendered.raw).unwrap(), expected.as_bytes());
            app.start_intruder_attack();
            assert!(app.intruder.error.is_none(), "{:?}", app.intruder.error);
            let (_, actual) = received.recv_timeout(Duration::from_secs(10)).unwrap();
            assert_eq!(actual, expected.as_bytes());
            task.join().unwrap();
        }
    }
}

#[test]
fn binary_body_edits_and_payloads_survive_workspace_restore_and_real_sends() {
    for intruder in [false, true] {
        let (host, received, task) = receiver();
        let capture = item(
            &host,
            b"before\xff$$\\x00after\r\n".to_vec(),
            None,
            "application/octet-stream",
        );
        let mut app = PerseidApp::test_app();
        app.scope_enforce = false;
        let expected = if intruder {
            app.send_history_to_intruder(&capture);
            assert!(app.intruder.body_escaped);
            assert_eq!(
                crate::proxy::marker_ranges(&app.intruder.request)
                    .unwrap()
                    .len(),
                0
            );
            app.intruder.request = app.intruder.request.replace("\\xFF", "§\\xFF§");
            app.intruder.payload_sets[0].text = "é\\x00$$".into();
            let saved = serde_json::to_vec(&app.intruder_workspace()).unwrap();
            app.apply_intruder_workspace(&serde_json::from_slice(&saved).unwrap());
            app.start_intruder_attack();
            assert!(app.intruder.error.is_none(), "{:?}", app.intruder.error);
            "beforeé\\x00$$$$\\x00after\r\n".as_bytes().to_vec()
        } else {
            app.send_history_to_repeater(&capture);
            let mut saved = app.repeater_workspace();
            assert!(saved[0].body_escaped);
            saved[0].request = saved[0].request.replace("\\xFF", "\\xFE");
            let saved = serde_json::to_vec(&saved).unwrap();
            app.apply_repeater_workspace(
                &serde_json::from_slice::<Vec<crate::project::RepeaterWorkspace>>(&saved).unwrap(),
                0,
            );
            app.send_active_repeater();
            assert!(app.repeater_workspace()[0].error.is_none());
            b"before\xfe$$\\x00after\r\n".to_vec()
        };
        let (_, actual) = received.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!(actual, expected, "intruder={intruder}");
        task.join().unwrap();
    }
}

#[test]
fn captured_text_can_be_edited_and_sent_in_both_tools() {
    for intruder in [false, true] {
        let (host, received, task) = receiver();
        let capture = item(&host, b"a=original\r\n".to_vec(), None, "text/plain");
        let mut app = PerseidApp::test_app();
        app.scope_enforce = false;
        if intruder {
            app.send_history_to_intruder(&capture);
            assert!(!app.intruder.body_escaped);
            app.intruder.request = app.intruder.request.replace("a=original", "a=§original§");
            app.intruder.payload_sets[0].text = "edited".into();
            let saved = app.intruder_workspace();
            app.apply_intruder_workspace(&saved);
            app.start_intruder_attack();
            assert!(app.intruder.error.is_none(), "{:?}", app.intruder.error);
        } else {
            app.send_history_to_repeater(&capture);
            let mut saved = app.repeater_workspace();
            assert!(saved[0].preserved_body.is_none());
            saved[0].request = saved[0].request.replace("a=original", "a=edited");
            app.apply_repeater_workspace(&saved, 0);
            app.send_active_repeater();
            assert!(app.repeater_workspace()[0].error.is_none());
        }
        let (_, actual) = received.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!(actual, b"a=edited\r\n");
        task.join().unwrap();
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
        let mut saved = app.repeater_workspace();
        saved[0].response = Some(crate::project::RepeaterResponseWorkspace {
            status: 200,
            ..Default::default()
        });
        app.apply_repeater_workspace(&saved, 0);
        app.send_active_repeater();
        assert!(
            app.repeater_workspace()[0]
                .error
                .as_ref()
                .unwrap()
                .contains("incomplete")
        );
        assert!(app.repeater.is_none());
        assert!(app.repeater_workspace()[0].response.is_none());
        app.send_history_to_intruder(&capture);
        let saved = app.intruder_workspace();
        app.apply_intruder_workspace(&saved);
        app.start_intruder_attack();
        assert!(app.intruder.error.as_ref().unwrap().contains("incomplete"));
        assert!(app.intruder_engine.is_none());
    }
}
