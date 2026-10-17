# Wiremoth

**A lightweight, keyboard-first workbench for inspecting, replaying, and testing HTTP traffic.**

Wiremoth is an open-source, Rust-native web security tool for people who like to understand what happens on the wire. It is designed around the everyday workflow of intercepting a request, changing it, sending it again, and understanding the response.

The goal is a practical companion for web security learning, CTFs, and authorized application testing—not a giant all-in-one scanner.

> [!IMPORTANT]
> **Project status: early prototype.** A runnable prototype exists today: a loopback-only HTTP/1.1 proxy with a native capture/history UI (see [Getting started](#getting-started)). HTTPS traffic is tunneled as TLS passthrough for now; interception with a local CA is Milestone 2. Everything else in the roadmap below is proposed work, not a claim of implemented functionality.

## Why Wiremoth?

Web security testing is often about the same small set of actions: capture a request, inspect its parameters and headers, replay it with changes, compare the result, and follow a hypothesis. These tasks should be fast and comfortable even on modest hardware.

Wiremoth aims to be:

- **Manual-first.** The request and response are the center of the interface, not a dashboard of speculative findings.
- **Lightweight by design.** No embedded browser or web-based desktop UI; memory limits, streamed bodies, and bounded history are design requirements.
- **Keyboard-friendly.** Fast navigation among requests, Repeater tabs, search, and comparisons.
- **Transparent.** Inspection tools should show exactly what was sent, received, and changed.
- **Extensible.** Optional, narrowly scoped checks rather than a mandatory monolithic scanning engine.
- **Local-first.** Captured traffic stays on the user's machine unless the user explicitly exports or shares it.

## Planned features

### Core workbench

| Feature | Goal |
| --- | --- |
| **Intercepting proxy** | Capture and edit HTTP traffic from an external browser; support HTTPS interception through a user-installed, locally generated CA. |
| **HTTP history** | Search, filter, and inspect requests and responses without retaining everything in memory. |
| **Repeater** | Edit and resend requests, keep multiple working tabs, and inspect raw and structured responses. |
| **Response diff** | Compare responses side by side to spot meaningful differences. |
| **Decoder** | URL, HTML, Base64, and other everyday text transformations. |
| **Scope controls** | Explicit target allowlists and safe defaults for capture and testing. |

### After the core is stable

- **Request fuzzer:** controlled parameter variations, concurrency limits, and clear result comparison.
- **Passive checks:** inspect observed traffic for potentially unsafe configuration and interesting evidence.
- **Guided checks:** opt-in tests against a selected request or parameter, with findings presented as hypotheses to verify.
- **Import/export:** portable HTTP request collections and sanitized evidence exports.
- **Optional integrations:** compatibility with specialized external testing tools where useful.

### Intentionally out of scope for the first release

- Replacing an entire commercial vulnerability scanner.
- Building a new browser engine or full JavaScript crawler.
- Automatically exploiting discovered issues.
- Running unattended scans against arbitrary hosts.

## Proposed architecture

Wiremoth is planned as a set of small Rust components, with the user interface independent from the proxy and analysis code:

```text
External browser
      |
      v
Local proxy / TLS interception
      |
      +----> Bounded capture and SQLite history
      |                |
      |                +----> Search, filtering, comparison
      |
      +----> Repeater / request mutation
      |
      +----> Optional passive and guided checks
                       |
                       v
               Native egui interface
```

**Initial technology choices (subject to prototyping):**

- **Rust** for the application and networking components.
- **egui + eframe** for a native desktop interface; evaluate the `glow` renderer on older Linux hardware.
- **Tokio** for asynchronous networking.
- **Hyper and rustls** as possible building blocks for HTTP and TLS (a transparent/intercepting proxy requires additional protocol and certificate-management code).
- **SQLite** for searchable, disk-backed history.

The desktop application should not depend on starting a Chromium instance. Network operations will run separately from UI work, and long lists and large message bodies should be loaded or rendered incrementally.

## Roadmap

- [ ] **Milestone 0 — Proof of concept:** benchmark an idle egui window, history table, and large response viewer on low-memory Linux hardware.
- [ ] **Milestone 1 — Proxy:** reliable HTTP/1.1 capture and forwarding; explicit scope configuration. *(Prototype: capture, forwarding, and the history UI work today; scope configuration is not implemented yet.)*
- [ ] **Milestone 2 — HTTPS:** local CA generation, TLS interception, and documented browser trust setup. *(Prototype: `CONNECT` tunneling works as TLS passthrough with byte counts; interception is not implemented.)*
- [ ] **Milestone 3 — Daily workflow:** persistent history, search, Repeater, decoder, and response diff.
- [ ] **Milestone 4 — Testing tools:** bounded request fuzzing and a small set of passive checks.
- [ ] **Milestone 5 — Hardening:** tests, crash recovery, safe handling of sensitive traffic, packaging, and performance benchmarks.
- [ ] **Later:** additional protocol support, guided scanning rules, integrations, and platform polish.

Priorities may change based on real-world use and performance measurements.

## Performance philosophy

A small Rust binary does not automatically mean a small application. Wiremoth will aim to minimize **idle CPU use, total resident memory, startup latency, and avoidable copies**—and publish measurements instead of unverified performance claims.

Important implementation principles include disk-backed history, limits on capture size, bounded worker queues, virtualized request lists, and avoiding full document layout for multi-megabyte responses.

## Getting started

Wiremoth builds as a small Cargo workspace: `wiremoth-proxy` (the proxy library) and `wiremoth` (the desktop application).

### Build and run

Requirements: a recent stable Rust toolchain (edition 2024) and a desktop session with OpenGL support.

```sh
cargo run -p wiremoth
```

This starts the interception proxy on `127.0.0.1:8080` (loopback only) and opens the Wiremoth window. Options:

```text
--listen <ADDR>         address for the proxy to listen on [default: 127.0.0.1:8080]
--capture-limit <BYTES> maximum body bytes kept per request/response [default: 1048576]
--history-limit <N>     maximum number of exchanges kept in memory [default: 2000]
--no-proxy              start the UI without the proxy
```

### Pointing a browser at it

Configure your browser — or an extension such as FoxyProxy — to use an HTTP proxy at `127.0.0.1:8080` for HTTP and HTTPS.

- Plain HTTP requests are forwarded and captured: method, headers, (bounded) bodies, status, size, duration, and any capture truncation are visible in the history table and detail pane.
- HTTPS requests arrive as `CONNECT` tunnels. Wiremoth relays the bytes in both directions and records the exchange with byte counts, but does not decrypt TLS yet (Milestone 2).

The prototype has no disk persistence yet: history lives in memory, bounded by `--history-limit`; captured bodies are capped per message by `--capture-limit`.

## Safety and privacy

Wiremoth is intended for systems you own or are explicitly authorized to test, including purpose-built training labs. Intercepted requests may contain passwords, session tokens, cookies, or personal data.

The intended defaults are **loopback-only proxy binding**, explicit capture/scan scope, no background active scanning, and clear controls for retaining or deleting sensitive traffic. A local interception CA must be deliberately trusted by the user and must never be silently installed system-wide.

## Contributing

Contributions, testing feedback, and design discussions are welcome as the first prototype takes shape. Particularly useful early feedback includes low-memory Linux compatibility, accessibility, HTTP correctness, and ways to keep the Repeater workflow fast.

## License

Wiremoth is dual-licensed under either of the following, at your option:

- [MIT License](LICENSE-MIT)
- [Apache License, Version 2.0](LICENSE-APACHE)

`SPDX-License-Identifier: MIT OR Apache-2.0`

Third-party dependencies and assets remain subject to their own licenses.
