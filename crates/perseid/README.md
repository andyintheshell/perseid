# Perseid

[Project home](../../README.md) · [Documentation](../../docs/README.md) ·
[Contributing](../../CONTRIBUTING.md)

The single `perseid` package contains the native egui desktop workbench and
its embedded HTTP/1.1/HTTP/2 interception proxy. Rust callers use
`perseid::proxy`; the application binary adds history, message viewers,
interception controls, Repeater tabs, Decoder, and local CA storage.

## Run the desktop app

From the repository root:

```sh
cargo run -p perseid --locked
```

See [Getting started](../../docs/getting-started.md) for platform dependencies,
Homebrew setup, CLI flags, and a local-target walkthrough.

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
policy, bounded body capture, capture-drop counters, and a separate direct
HTTP/HTTPS `Repeater` client. Repeater bypasses proxy interception, rewriting,
and scope enforcement.

For a runnable event-consumer example, see [headless.rs](examples/headless.rs):

```sh
cargo run -p perseid --example headless --locked
cargo doc -p perseid --no-deps --locked --open
```

Headless use does not open a window, but still builds the same crate and GUI
dependencies. This prototype API can change; consult the source/API docs for
the checkout you are using.

## Safety and license

Use only on explicitly authorized targets. Captured traffic and CA keys are
sensitive; read [Safety and privacy](../../docs/safety.md) and the
[security policy](../../SECURITY.md).

Dual-licensed under MIT or Apache-2.0, at your option.

`SPDX-License-Identifier: MIT OR Apache-2.0`
