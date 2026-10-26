![Perseid — inspect, modify, replay](https://raw.githubusercontent.com/andyintheshell/perseid/main/branding/perseid-readme-banner.png)

# Perseid

**An open-source, Rust-native alternative to Burp Suite Community Edition for manual HTTP(S) security testing.**

Intercept, inspect, modify, replay, and fuzz web traffic in one application.
Perseid is built for security researchers, CTF players, and developers testing
systems they own or are authorized to assess. It needs no Java runtime or bundled
browser.

[![CI](https://github.com/andyintheshell/perseid/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/andyintheshell/perseid/actions/workflows/ci.yml)
[![Rust 1.95+](https://img.shields.io/badge/Rust-1.95%2B-orange?logo=rust)](https://github.com/andyintheshell/perseid/blob/main/rust-toolchain.toml)
[![Public alpha](https://img.shields.io/badge/status-public%20alpha-FF9915)](https://github.com/andyintheshell/perseid/blob/main/docs/roadmap.md)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-112D68)](https://github.com/andyintheshell/perseid/blob/main/LICENSE-MIT)

[Project home](https://github.com/andyintheshell/perseid) ·
[Getting started](https://github.com/andyintheshell/perseid/blob/main/docs/getting-started.md) ·
[Screenshots](https://github.com/andyintheshell/perseid/blob/main/docs/screenshots.md) ·
[API documentation](https://docs.rs/perseid)

![Perseid HTTP history with side-by-side request and response views](https://raw.githubusercontent.com/andyintheshell/perseid/main/docs/images/perseid-main.png)

> **Public alpha:** the core manual-testing workflows work today, but Perseid is
> actively developed and is not a feature-complete or drop-in Burp replacement.
> See the [capability matrix](https://github.com/andyintheshell/perseid/blob/main/docs/burp-parity.md)
> for current coverage and limitations.

> **Release:** `0.2.0` is published on crates.io and includes project
> persistence.

## What this crate contains

The `perseid` package provides both the desktop application and the
`perseid::proxy` Rust API. Its desktop workbench includes:

- An HTTP/1.1 and HTTP/2 intercepting proxy, traffic history, and a host/path site map.
- Temporary and persistent projects using a single local `.perseid` file, with automatic saving of captured traffic and supported workbench inputs.
- HTTPS interception with a local certificate authority, explicit trust setup, and per-host TLS pass-through.
- Interception controls, Repeater, Intruder, Decoder, Comparer, scope rules, and match/replace.
- WebSocket history for `ws://` and decrypted `wss://` traffic.
- Bounded capture/history resources and a live memory indicator.

WebSocket editing/replay and automated scanning are not implemented. Project
persistence is an initial slice: it has no encryption or schema migrations, and
the UI currently opens only a bounded newest-first history snapshot. See the
[workbench guide](https://github.com/andyintheshell/perseid/blob/main/docs/workbench.md#projects-and-persistence)
for exactly what is saved. Temporary projects evict the oldest captured
records when they exceed the configurable `--temporary-retention-limit` and
`--temporary-retention-bytes` limits (which default to the history flags; `0`
disables a dimension). The limits are soft, not a hard RAM ceiling: each
stream's newest record is kept even when it alone exceeds the byte budget,
in-flight HTTP exchanges are kept until they complete, and headers, indexes,
and database overhead are outside the byte budget. Saved project files are not
automatically pruned and can grow until the disk fills. Burp Suite and Burp
Suite Community Edition are
trademarks of PortSwigger Ltd. Perseid is independently developed, contains no
PortSwigger software or logos, and is not affiliated with, sponsored by, or
endorsed by PortSwigger Ltd. References to Burp are for comparison only.

## Install and run the desktop app

Install the latest version published to crates.io with Cargo. `cargo install`
builds Perseid from source; it does not download a prebuilt installer. With Rust
1.95+ and the platform's native build requirements installed, run:

```sh
cargo install perseid --locked
perseid
```

For Linux build dependencies, macOS setup, and graphics requirements, see the
[platform guide](https://github.com/andyintheshell/perseid/blob/main/docs/getting-started.md).
For compiled binaries that do not require a local Rust toolchain, use
[GitHub Releases](https://github.com/andyintheshell/perseid/releases/latest):
Windows MSI or no-install ZIP (x64/ARM64), universal macOS DMG, or Linux
AppImage/DEB (x86-64/ARM64). Windows packages are unsigned; macOS is ad-hoc
signed but not notarized.

## Use the proxy API from Rust

Add `perseid` to your application with `cargo add perseid`. The proxy owns its
runtime and publishes capture events; dropping the handle requests shutdown and
joins its runtime thread. For example:

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

`ProxyConfig::default()` binds to loopback and leaves TLS interception off. It
does not generate or persist a CA. To decrypt HTTPS, supply a
`CertificateAuthority` in `TlsConfig` and enable the relevant `ProxySettings`.
The desktop app manages CA files separately. Direct Repeater and Intruder
requests bypass proxy interception, rewriting, and proxy scope enforcement.
See the [proxy API example](https://github.com/andyintheshell/perseid/blob/main/crates/perseid/examples/headless.rs)
and [workbench limits](https://github.com/andyintheshell/perseid/blob/main/docs/workbench.md#repeater).

## Safety and license

Use Perseid only on targets you own or are explicitly authorized to assess.
Captured traffic and CA keys are sensitive; read
[Safety and privacy](https://github.com/andyintheshell/perseid/blob/main/docs/safety.md)
and the [security policy](https://github.com/andyintheshell/perseid/blob/main/SECURITY.md).

The source code is dual-licensed under
[MIT](https://github.com/andyintheshell/perseid/blob/main/LICENSE-MIT) or
[Apache-2.0](https://github.com/andyintheshell/perseid/blob/main/LICENSE-APACHE),
at your option. `SPDX-License-Identifier: MIT OR Apache-2.0`.

The Perseid name, logos, and visual identity are project branding and are not
covered by those source-code licenses. Forks and modified versions must remove
or replace them and use their own branding. See the
[Perseid brand terms](https://github.com/andyintheshell/perseid/blob/main/branding/LICENSE-brand.txt).
