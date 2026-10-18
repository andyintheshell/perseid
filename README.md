<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="branding/wiremoth-logo-dark.png">
    <img src="branding/wiremoth-logo-light.png" alt="Wiremoth — inspect the wire" width="520">
  </picture>
</p>

# Wiremoth

**A lightweight, keyboard-first workbench for inspecting, replaying, and testing HTTP traffic.**

Wiremoth is a Rust-native web security tool for understanding what happens on
the wire: capture a request, inspect it, change it, send it again, and compare
the responses. It is built for web security learning, CTFs, and authorized
application testing — with no embedded browser and memory bounded by design.

> [!IMPORTANT]
> **Status: early prototype.** A loopback-only HTTP/1.1 proxy and a Burp-style
> Target/Proxy workspace work today. HTTPS is tunneled as TLS passthrough;
> local-CA interception is next. See the [roadmap](docs/roadmap.md).

![Wiremoth HTTP history with request and response detail](docs/images/wiremoth-main.png)

## What works today

- Loopback HTTP/1.1 proxy with configurable listen address, capture limit,
  and history bound.
- Burp-style workspace with **Target**, **Proxy**, **Intruder**, and
  **Repeater** tabs in the familiar order, in a warm light theme.
- Bounded in-memory history with filtering, a view-only scope filter, and
  keyboard navigation (`j`/`k`, arrows, `Home`/`End`, `Ctrl`+`F`).
- Request/response tabs with Raw, Headers, and Hex views, truncation markers,
  size, and timing.
- Target site map grouped by host and path, plus an editable scope list that
  filters the site map and HTTP history.
- `CONNECT` tunneling for HTTPS with byte counts — no TLS decryption yet.
- Read-only Repeater preview and clearly labelled Intercept/Intruder previews;
  no fake controls are presented as working features.

## Goal: Burp Suite Community parity

Wiremoth's north star is **feature parity with Burp Suite Community Edition's
manual toolkit**: proxy and history, HTTPS interception, Repeater, Decoder,
Comparer, Sequencer, WebSockets, and Intruder-style fuzzing without artificial
rate limits — adapted to an external browser and a memory-bounded Rust core.

This is a direction, not a current claim. The definition, capability matrix,
and deliberate deviations live in
[Burp Community parity](docs/burp-parity.md).

## Quick start

```sh
cargo run -p wiremoth
```

The proxy listens on `127.0.0.1:8080` and the Wiremoth window opens. Point your
browser — or an extension such as FoxyProxy — at that address for HTTP and
HTTPS.

> Only test systems you own or have explicit permission to test. See
> [Safety and privacy](docs/safety.md).

Detailed setup, CLI flags, HiDPI behavior, and current limitations:
[Getting started](docs/getting-started.md).

## Documentation

| Document | Contents |
| --- | --- |
| [Getting started](docs/getting-started.md) | Build, run, proxy setup, options, limitations |
| [Design](docs/design.md) | Principles, architecture, technology, performance |
| [Roadmap](docs/roadmap.md) | Milestones from prototype to Community parity |
| [Burp Community parity](docs/burp-parity.md) | Definition of parity, capability matrix, deviations |
| [Safety and privacy](docs/safety.md) | Authorized use, data handling, CA trust |
| [Contributing](CONTRIBUTING.md) | Development setup, checks, project layout |

## Contributing

Contributions, testing feedback, and design discussions are welcome. Start with
[CONTRIBUTING.md](CONTRIBUTING.md). Particularly useful early feedback is
low-memory Linux behavior, HTTP correctness, accessibility, and Repeater
workflow design.

## License

Wiremoth is dual-licensed under either of:

- [MIT License](LICENSE-MIT)
- [Apache License, Version 2.0](LICENSE-APACHE)

`SPDX-License-Identifier: MIT OR Apache-2.0`

Third-party dependencies and assets remain subject to their own licenses.
