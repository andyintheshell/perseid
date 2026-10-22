//! Headless smoke test: run the proxy and print captured events.
//!
//! ```sh
//! cargo run -p perseid --example headless
//! curl -x http://127.0.0.1:8080 http://example.com/
//! ```

use std::time::Duration;

use perseid::proxy::{ExchangeResult, Proxy, ProxyConfig, ProxyEvent};

fn main() -> Result<(), perseid::proxy::StartError> {
    env_logger::builder()
        .filter_level(log::LevelFilter::Info)
        .init();

    let proxy = Proxy::start(ProxyConfig::default())?;
    println!("Perseid proxy listening on http://{}", proxy.local_addr());
    println!(
        "try: curl -x http://{} http://example.com/",
        proxy.local_addr()
    );

    loop {
        match proxy.recv_timeout(Duration::from_secs(1)) {
            Ok(event) => match event {
                ProxyEvent::Started { id, request, .. } => {
                    println!(
                        "{id:>4} -> {} {} {}",
                        request.method, request.host, request.path
                    );
                }
                ProxyEvent::ResponseHead { id, head } => {
                    println!("{id:>4} <- {} {}", head.status, head.reason);
                }
                ProxyEvent::WebSocketMessage {
                    id,
                    connection,
                    direction,
                    opcode,
                    payload,
                    ..
                } => {
                    println!(
                        "{id:>4} ws {} {:?} {:?} {} bytes ({})",
                        connection.id,
                        direction,
                        opcode,
                        payload.total_len(),
                        connection.url
                    );
                }
                ProxyEvent::Finished { id, end } => match end.result {
                    ExchangeResult::Response { body } => println!(
                        "{id:>4} == {} ms, {} bytes captured{}",
                        end.duration.as_millis(),
                        body.total_len(),
                        if body.is_truncated() {
                            " (truncated)"
                        } else {
                            ""
                        }
                    ),
                    ExchangeResult::Tunnel {
                        client_to_server,
                        server_to_client,
                    } => println!(
                        "{id:>4} == tunnel closed after {} ms ({} up, {} down)",
                        end.duration.as_millis(),
                        client_to_server,
                        server_to_client
                    ),
                    ExchangeResult::InterceptedTls { requests } => println!(
                        "{id:>4} == TLS-intercepted tunnel closed after {} ms ({requests} requests)",
                        end.duration.as_millis(),
                    ),
                    ExchangeResult::Dropped => {
                        println!("{id:>4} == dropped in the interceptor")
                    }
                    ExchangeResult::Failed { error } => {
                        println!("{id:>4} !! {error}")
                    }
                },
            },
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                if proxy.dropped_events() > 0 {
                    eprintln!("({} events dropped)", proxy.dropped_events());
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    Ok(())
}
