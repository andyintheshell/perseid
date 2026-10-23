# Perseid

[Project home](https://github.com/andyintheshell/perseid) ·
[Documentation](https://github.com/andyintheshell/perseid/tree/main/docs) ·
[Contributing](https://github.com/andyintheshell/perseid/blob/main/CONTRIBUTING.md)

The single `perseid` package contains the native egui desktop workbench and
its embedded HTTP/1.1/HTTP/2 interception proxy with WebSocket capture. Rust
callers use `perseid::proxy`; the application binary adds HTTP and WebSocket
history, message viewers, interception controls, Intruder and Repeater
workspaces, Decoder, and local CA storage.

## Run the desktop app

From the repository root:

```sh
cargo run -p perseid --locked
```

See [Getting started](https://github.com/andyintheshell/perseid/blob/main/docs/getting-started.md)
for platform dependencies, Homebrew setup, CLI flags, and a local-target walkthrough.

## Use the proxy from Rust

The proxy owns its runtime and publishes capture events. Dropping the handle
requests shutdown and joins its runtime thread. For example:

```rust,no_run
use std::sync::mpsc::RecvTimeoutError;
use std::time::Duration;
use perseid::proxy::{Proxy, ProxyConfig};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let proxy = Proxy::start(ProxyConfig::default())?;
    println!("listening on {}", proxy.local_addr());

    loop {
        match proxy.recv_timeout(Duration::from_secs(1)) {
            Ok(event) => println!("{event:?}"),
            Err(RecvTimeoutError::Timeout) if proxy.is_running() => continue,
            Err(_) => break,
        }
    }
    Ok(())
}
```

**Defaults are not the same as the desktop app's startup behavior.**
`ProxyConfig::default()` binds loopback and leaves TLS interception off. It
does not generate or persist a CA. To decrypt HTTPS, a caller supplies a
`CertificateAuthority` in `TlsConfig` and enables the relevant `ProxySettings`.
The desktop app makes those decisions and manages the filesystem separately.

The API also exposes editable interception decisions, scope and match/replace
policy, bounded body capture, capture-drop counters, complete WebSocket
messages through `ProxyEvent::WebSocketMessage`, and separate direct
HTTP/HTTPS `Repeater` and `Intruder` clients. Both bypass proxy interception,
rewriting, and proxy scope enforcement. The desktop Intruder workspace supplies
the enforced scope with each plan, and the engine checks every generated
request.

Repeater admission includes queued requests, active work, and unread results.
`send` can return `RepeaterError::Busy` without stopping other requests; drain
`try_recv` to release reservations before retrying. Intruder attacks use `$$`
positions and four attack types, run up to 32 requests concurrently, and deliver
bounded result events. Oversized requests and capture configurations are
rejected separately. See the
[workbench limits](https://github.com/andyintheshell/perseid/blob/main/docs/workbench.md#repeater)
for the count/byte budgets.

Loaded CA certificate/key pairs are checked for a match. The desktop app owns
the [generation-based CA store](https://github.com/andyintheshell/perseid/blob/main/docs/tls.md#where-the-ca-lives);
the proxy API itself still has no filesystem policy or automatic trust installation.

For a runnable event-consumer example, see
[headless.rs](https://github.com/andyintheshell/perseid/blob/main/crates/perseid/examples/headless.rs):

```sh
cargo run -p perseid --example headless --locked
cargo doc -p perseid --no-deps --locked --open
```

Headless use does not open a window, but still builds the same crate and GUI
dependencies. This prototype API can change; consult the source/API docs for
the checkout you are using.

## Safety and license

Use only on explicitly authorized targets. Captured traffic and CA keys are
sensitive; read
[Safety and privacy](https://github.com/andyintheshell/perseid/blob/main/docs/safety.md)
and the [security policy](https://github.com/andyintheshell/perseid/blob/main/SECURITY.md).

Dual-licensed under MIT or Apache-2.0, at your option.

`SPDX-License-Identifier: MIT OR Apache-2.0`
