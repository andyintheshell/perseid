//! End-to-end attack tests for the Intruder engine.
//!
//! Every origin is a local `127.0.0.1` server. The tests never contact an
//! external target.

use std::convert::Infallible;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use http::{Request, Response, StatusCode};
use http_body_util::Full;
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use perseid::proxy::{
    AttackType, Intruder, IntruderConfig, IntruderError, IntruderEvent, IntruderPlan,
    IntruderResult, RepeaterResult,
};
use tokio::net::TcpListener;

const TIMEOUT: Duration = Duration::from_secs(5);

#[tokio::test]
async fn cookie_jar_adds_missing_cookies_without_overwriting_payloads() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let captured = seen.clone();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let service = service_fn(move |request: Request<Incoming>| {
            let captured = captured.clone();
            async move {
                captured.lock().unwrap().push(
                    request
                        .headers()
                        .get("cookie")
                        .unwrap()
                        .to_str()
                        .unwrap()
                        .to_owned(),
                );
                Ok::<_, Infallible>(
                    Response::builder()
                        .header("Set-Cookie", "next=rotated; Path=/")
                        .body(Full::new(Bytes::new()))
                        .unwrap(),
                )
            }
        });
        let _ = hyper::server::conn::http1::Builder::new()
            .serve_connection(TokioIo::new(stream), service)
            .await;
    });
    let jar: perseid::proxy::cookies::SharedCookieJar = Default::default();
    {
        let mut jar = jar.lock().unwrap();
        jar.apply = true;
        jar.update = true;
        jar.insert(&format!("http://{addr}/"), "sid=jar; Path=/")
            .unwrap();
        jar.insert(&format!("http://{addr}/"), "other=jar; Path=/")
            .unwrap();
    }
    let mut intruder = Intruder::start(IntruderConfig {
        timeout: TIMEOUT,
        cookie_jar: Some(jar.clone()),
        ..Default::default()
    })
    .unwrap();
    intruder
        .start_attack(plan(
            addr,
            "GET / HTTP/1.1\r\nCookie: sid=$$old$$\r\n\r\n",
            &["payload"],
        ))
        .unwrap();
    let (results, _, _) = collect_attack(&mut intruder).await;
    assert!(matches!(
        results[0].result,
        RepeaterResult::Response { status: 200, .. }
    ));
    let value = seen.lock().unwrap()[0].clone();
    assert!(value.contains("sid=payload"));
    assert!(value.contains("other=jar"));
    assert!(!value.contains("sid=jar"));
    assert!(
        jar.lock()
            .unwrap()
            .entries()
            .iter()
            .any(|entry| entry.2 == "next" && entry.3 == "rotated")
    );
    server.abort();
}

fn plan(addr: std::net::SocketAddr, template: &str, payloads: &[&str]) -> IntruderPlan {
    IntruderPlan {
        attack: AttackType::Sniper,
        scheme: "http".to_string(),
        host: addr.to_string(),
        template: template.to_string(),
        payload_sets: vec![payloads.iter().map(|payload| payload.to_string()).collect()],
        concurrency: 2,
        scope: Vec::new(),
    }
}

async fn collect_attack(intruder: &mut Intruder) -> (Vec<IntruderResult>, u64, bool) {
    let deadline = tokio::time::Instant::now() + TIMEOUT;
    let mut results = Vec::new();
    loop {
        if tokio::time::Instant::now() >= deadline {
            panic!("the attack did not finish before the test timeout");
        }
        match intruder.try_recv() {
            Some(IntruderEvent::Result(result)) => results.push(result),
            Some(IntruderEvent::Finished {
                completed,
                cancelled,
                ..
            }) => return (results, completed, cancelled),
            None => tokio::time::sleep(Duration::from_millis(5)).await,
        }
    }
}

async fn start_origin() -> (std::net::SocketAddr, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let service_seen = seen.clone();

    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            let seen = service_seen.clone();
            tokio::spawn(async move {
                let service = service_fn(move |request: Request<Incoming>| {
                    let seen = seen.clone();
                    async move {
                        let target = request
                            .uri()
                            .path_and_query()
                            .map_or_else(String::new, ToString::to_string);
                        seen.lock().expect("request log").push(target.clone());
                        let body = format!("served {target}");
                        Ok::<_, Infallible>(
                            Response::builder()
                                .status(StatusCode::OK)
                                .header("content-type", "text/plain")
                                .header("content-length", body.len().to_string())
                                .body(Full::new(Bytes::from(body)))
                                .unwrap(),
                        )
                    }
                });
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(TokioIo::new(stream), service)
                    .await;
            });
        }
    });

    (addr, seen)
}

#[tokio::test]
async fn sniper_attack_sends_every_payload_and_reports_results() {
    let (addr, seen) = start_origin().await;

    let mut intruder = Intruder::start(IntruderConfig {
        timeout: TIMEOUT,
        ..IntruderConfig::default()
    })
    .unwrap();
    let attack_plan = plan(
        addr,
        "GET /item?id=$$1$$ HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n",
        &["alpha", "beta", "gamma"],
    );
    let attack_id = intruder.start_attack(attack_plan).unwrap();
    let (mut results, completed, cancelled) = collect_attack(&mut intruder).await;

    assert_eq!(completed, 3);
    assert!(!cancelled);
    results.sort_by_key(|result| result.sequence);
    assert_eq!(results.len(), 3);
    for (index, result) in results.iter().enumerate() {
        assert_eq!(result.attack_id, attack_id);
        assert_eq!(result.sequence, index as u64 + 1);
        assert_eq!(result.payloads, vec![["alpha", "beta", "gamma"][index]]);
        match &result.result {
            RepeaterResult::Response {
                status,
                body,
                body_error,
                ..
            } => {
                assert_eq!(*status, 200);
                assert!(body_error.is_none());
                assert_eq!(
                    body.data(),
                    format!("served /item?id={}", ["alpha", "beta", "gamma"][index]).as_bytes()
                );
            }
            RepeaterResult::Failed { error } => panic!("attack request failed: {error}"),
        }
    }

    let seen = seen.lock().expect("request log").clone();
    for payload in ["alpha", "beta", "gamma"] {
        assert!(
            seen.iter()
                .any(|target| target == &format!("/item?id={payload}")),
            "the origin should receive payload {payload}: {seen:?}"
        );
    }

    // A finished attack releases the engine for the next one.
    let next_plan = plan(
        addr,
        "GET /next?id=$$1$$ HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n",
        &["one"],
    );
    let next_id = intruder.start_attack(next_plan).unwrap();
    let (next_results, next_completed, next_cancelled) = collect_attack(&mut intruder).await;
    assert_eq!(next_completed, 1);
    assert!(!next_cancelled);
    assert_eq!(next_results.len(), 1);
    assert_eq!(next_results[0].attack_id, next_id);
}

#[tokio::test]
async fn payloads_cannot_redirect_an_attack_outside_scope() {
    let (addr, seen) = start_origin().await;
    let mut intruder = Intruder::start(IntruderConfig {
        timeout: TIMEOUT,
        ..IntruderConfig::default()
    })
    .unwrap();

    let in_scope = format!("127.0.0.1:{}", addr.port());
    let out_of_scope = format!("localhost:{}", addr.port());
    let payloads = [in_scope.as_str(), out_of_scope.as_str()];
    let mut attack_plan = plan(
        addr,
        "GET http://$$host$$/blocked HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n",
        &payloads,
    );
    attack_plan.scope = vec!["127.0.0.1".to_string()];
    intruder.start_attack(attack_plan).unwrap();
    let (mut results, completed, cancelled) = collect_attack(&mut intruder).await;

    assert_eq!(completed, 2);
    assert!(!cancelled);
    results.sort_by_key(|result| result.sequence);
    assert_eq!(results.len(), 2);
    assert!(matches!(
        results[0].result,
        RepeaterResult::Response { status: 200, .. }
    ));
    match &results[1].result {
        RepeaterResult::Failed { error } => {
            assert!(error.contains("outside the enforced scope"), "{error}");
        }
        RepeaterResult::Response { .. } => panic!("the out-of-scope request should not be sent"),
    }

    // Only the in-scope request reached the origin.
    let seen = seen.lock().expect("request log").clone();
    assert_eq!(seen, vec!["/blocked".to_string()]);
}

#[test]
fn invalid_plans_are_rejected_before_an_attack_starts() {
    let intruder = Intruder::start(IntruderConfig {
        timeout: TIMEOUT,
        ..IntruderConfig::default()
    })
    .unwrap();
    let base = IntruderPlan {
        attack: AttackType::Sniper,
        scheme: "http".to_string(),
        host: "127.0.0.1:1".to_string(),
        template: "GET / HTTP/1.1\n\n".to_string(),
        payload_sets: vec![vec!["one".to_string()]],
        concurrency: 1,
        scope: Vec::new(),
    };
    assert!(matches!(
        intruder.start_attack(base.clone()),
        Err(IntruderError::NoPositions)
    ));

    let empty = IntruderPlan {
        template: "GET /?q=$$1$$ HTTP/1.1\n\n".to_string(),
        payload_sets: vec![Vec::new()],
        ..base
    };
    assert!(matches!(
        intruder.start_attack(empty),
        Err(IntruderError::EmptyPayloadSet { .. })
    ));
}
