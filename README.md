<p align="center">
  <img src="branding/perseid-readme-banner.png" alt="Perseid — inspect, modify, replay" width="100%">
</p>

# Perseid

**A lightweight web security workbench. Inspect, modify, replay.**

[![CI](https://github.com/andyintheshell/perseid/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/andyintheshell/perseid/actions/workflows/ci.yml)
[![Rust 1.95+](https://img.shields.io/badge/Rust-1.95%2B-orange?logo=rust)](rust-toolchain.toml)
[![Early prototype](https://img.shields.io/badge/status-early%20prototype-FF9915)](docs/roadmap.md)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-112D68)](#license)

[Get started](docs/getting-started.md) · [User guide](docs/workbench.md) ·
[Screenshots](docs/screenshots.md) · [Documentation](docs/README.md) ·
[Roadmap](docs/roadmap.md)

Perseid is a Rust-native desktop tool for web security learning, CTFs, and
authorized application testing. Capture traffic from an external browser,
inspect a request, change it, and send it again—without an embedded browser or
a Java runtime. Captured bodies and in-memory history have configurable limits.

> [!IMPORTANT]
> **Early prototype, not a finished Burp replacement.** HTTP/HTTPS interception,
> Intruder, Repeater, and Decoder work today. WebSockets, Comparer, Sequencer,
> and persistent projects are not implemented. See the
> [capability matrix](docs/burp-parity.md) for the precise status.

[![Perseid HTTP history with side-by-side request and response views](docs/images/perseid-main.png)](docs/screenshots.md)

## What you can do

| Task | Available today |
| --- | --- |
| **Capture and inspect** | HTTP/1.1 and HTTP/2 proxy, filtered history, a host/path site map, and Pretty/Raw/Hex message views with gzip, deflate, and Brotli decoding. |
| **Intercept and edit** | Hold requests and optionally responses; edit heads and buffered text bodies; forward or drop selected messages or the whole queue. |
| **Inspect HTTPS** | Automatically generated local CA, a certificate download page, and per-host TLS pass-through. You install and remove trust manually. |
| **Replay a request** | Editable HTTP/HTTPS Repeater tabs, history seeding, target and virtual-host control, and direct replay independent of the proxy. |
| **Fuzz a request** | Intruder positions marked with `$$`, editable payload sets, Sniper/Battering ram/Pitchfork/Cluster bomb attacks, bounded concurrency, and request/response results. |
| **Transform payloads** | Stacked Decoder panels, selection-aware encoding/decoding, 31 common hashes, and Smart decode for layered data. |
| **Control the session** | Opt-in proxy scope enforcement, literal/regex match-and-replace rules, capture/history limits, and a live heap-memory indicator. |

The workspace follows the familiar **Target → Proxy → Intruder → Repeater →
Decoder** order. The [user guide](docs/workbench.md) explains each tool and its
limits; the [gallery](docs/screenshots.md) shows every tab.

## Quick start

You need **Rust 1.95 or newer** and a graphical desktop with OpenGL support.
Linux is CI-tested; macOS has been built and smoke-tested locally on Apple
Silicon. See [platform setup](docs/getting-started.md#requirements) for Linux
dependencies, Homebrew instructions, and support caveats.

```sh
git clone https://github.com/andyintheshell/perseid.git
cd perseid
cargo run -p perseid --locked
```

1. The window opens on **Proxy > HTTP history**, with the proxy listening on
   `127.0.0.1:8080` by default.
2. Try the [local HTTP walkthrough](docs/getting-started.md#try-it-with-a-local-target)
   to see your first captured exchange without installing a certificate.
3. For HTTPS, open `http://127.0.0.1:8080/` directly and follow the
   [certificate setup guide](docs/tls.md). The app creates its CA on first
   launch but never installs trust automatically.
4. Point your testing browser at `127.0.0.1:8080` for **both HTTP and HTTPS**.
   Request holding is off initially; enable it in **Proxy > Intercept** when
   you want to edit traffic before forwarding.

> [!WARNING]
> Only test systems you own or have explicit permission to test. Use a dedicated
> browser profile. Proxy scope rules **do not apply to direct Repeater sends**.
> Intruder is also direct, but when scope enforcement is on it checks every
> generated request before sending. Read [Safety and privacy](docs/safety.md)
> before capturing real traffic.

## Know the limits

- History, Intruder and Repeater results, Decoder chains, scope, and rewrite
  settings are **in memory only**. The local TLS CA is persisted across launches.
- Large, streaming, and binary intercepted bodies are **header-editable only**.
  Message previews and direct-send seeds may be truncated or lossy.
- There is **no automated scanner, crawler, bundled browser, or WebSocket
  tooling**. Intruder generates requests from user-supplied payloads; it is not
  an automated scanner.
- Burp Suite Community manual-toolkit parity is a
  [goal](docs/burp-parity.md), not a current claim. There are no universal
  memory or performance guarantees; see the [measured baseline](docs/performance.md).

## Find your way around

| I want to… | Start here |
| --- | --- |
| Build, run, or configure the app | [Getting started](docs/getting-started.md) |
| Capture, intercept, replay, or decode | [Workbench guide](docs/workbench.md) |
| Trust, exclude, or remove the local CA | [HTTPS interception](docs/tls.md) |
| Fix a setup or traffic problem | [Troubleshooting](docs/troubleshooting.md) |
| Understand the architecture or contribute | [Design](docs/design.md) · [Contributing](CONTRIBUTING.md) |
| Explore all guides and references | [Documentation index](docs/README.md) |

## Contributing and feedback

Bug reports, workflow feedback, and focused pull requests are welcome. Start
with [CONTRIBUTING.md](CONTRIBUTING.md); useful areas include HTTP correctness,
accessibility, low-memory Linux behavior, and macOS testing.

- [Report a bug or suggest a feature](https://github.com/andyintheshell/perseid/issues).
- [Report a security vulnerability privately](SECURITY.md)—never attach live
  credentials, private keys, or sensitive captured traffic to a public issue.

## License

Dual-licensed under [MIT](LICENSE-MIT) **or** [Apache-2.0](LICENSE-APACHE), at
your option. Third-party dependencies, fonts, and assets retain their own
licenses.

`SPDX-License-Identifier: MIT OR Apache-2.0`
