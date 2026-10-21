<p align="center">
  <img src="branding/perseid-readme-banner.png" alt="Perseid — inspect, modify, replay" width="100%">
</p>

# Perseid

**A lightweight web security workbench — inspect, modify, and replay HTTP traffic.**

Perseid is a Rust-native web security tool for understanding what happens on
the wire: capture a request, inspect it, change it, send it again, and compare
the responses. It is built for web security learning, CTFs, and authorized
application testing — with no embedded browser and memory bounded by design.
The desktop application and its interception proxy are both part of the single
`perseid` crate; Rust callers can use the proxy as `perseid::proxy`.

> [!IMPORTANT]
> **Status: early prototype.** A loopback-only HTTP/1.1 and HTTP/2 proxy with
> editable request/response interception, opt-in scope enforcement,
> match/replace rules, automatic local-CA HTTPS interception, an editable
> HTTP/HTTPS Repeater, and a Burp-style Decoder work today. See the
> [roadmap](docs/roadmap.md).

![Perseid HTTP history with request and response detail](docs/images/perseid-main.png)

## What works today

- Loopback HTTP/1.1 and HTTP/2 proxy with configurable listen address, capture
  limit, interception limit, and count/byte history bounds. HTTP/2 is negotiated
  with intercepted clients and HTTPS origins over TLS, while h2c prior knowledge
  is accepted on the plain listener.
- Editable request and response interception behind one Burp-style **Intercept
  is on/off** control: hold HTTP and decrypted HTTPS messages, edit the head and
  buffered body, then forward or drop. Bodies over the intercept limit stay
  header-only instead of being silently modified.
- HTTPS interception on by default with a locally generated certificate
  authority, per-host certificates, a built-in certificate download page, a
  pass-through host list, and documented manual browser trust and removal steps.
  Perseid creates the CA automatically and never installs trust automatically.
- Opt-in scope enforcement for HTTP and `CONNECT`, returning `403` for
  out-of-scope hosts. An empty scope allows every host, and enforcement is off
  by default.
- Match/replace rules for request and response heads and buffered UTF-8 bodies,
  using literal text or Rust regular expressions with capture references.
- Burp-style workspace with **Target**, **Proxy**, **Intruder**, **Repeater**,
  and **Decoder** tabs in the familiar order, in a warm light theme.
- Bounded in-memory history (exchange count and total captured-body bytes)
  with one row per HTTP request, filtering, a scope view filter, and keyboard
  navigation (`j`/`k`, arrows, `Home`/`End`, `Ctrl`+`F`). `CONNECT` transport
  tunnels do not clutter the table; decrypted HTTPS requests appear directly.
- Live **Memory** indicator in the status bar with allocator memory in use;
  click it for the current and peak values and a rolling graph.
- Request/response tabs with Burp's Pretty, Raw, and Hex views, plus size and
  timing. Pretty formats JSON, HTML/XML, and URL-encoded form bodies; Pretty and
  Raw add line numbers, syntax highlighting, and title-cased header names.
  Compressed bodies are decoded in every view, and Hex is a bounded byte-offset
  dump.
- Target site map grouped by host and path, plus an editable scope list shared
  by the view filter and the traffic guardrail.
- `CONNECT` tunneling for HTTPS: decrypted by default with the local CA, with
  pass-through hosts and an off switch when you need opaque tunnels.
- Editable **Repeater** with request tabs, a Burp-style target bar, a raw
  request editor, and Pretty/Raw/Hex responses seeded from HTTP history. Replay
  uses its own direct HTTP/HTTPS client, so it works with the proxy stopped and
  can test a virtual `Host` against a different connection target. WebSocket
  replay is not supported yet.
- Burp-style **Decoder** with stacked, editable panels. Decode and encode as
  Plain, URL, HTML, Base64, ASCII hex, numeric Hex/Octal/Binary, or Gzip; hash
  with 31 common algorithms; or let Smart decode add one panel per detected
  layer. Transforms apply to the selected text when there is a selection, and
  applying an operation to an earlier panel replaces the downstream chain.
  Inputs are capped at 8 MiB and outputs at 16 MiB. The long tail of
  provider-specific hashes (SKEIN, TupleHash, and OID aliases) and a
  **Send to Decoder** action are not implemented yet.
- A clearly labelled Intruder preview; no fake controls are presented as
  working features.

## Goal: Burp Suite Community parity

Perseid's north star is **feature parity with Burp Suite Community Edition's
manual toolkit**: proxy and history, HTTPS interception, Repeater, Decoder,
Comparer, Sequencer, WebSockets, and Intruder-style fuzzing without artificial
rate limits — adapted to an external browser and a memory-bounded Rust core.

This is a direction, not a current claim. The definition, capability matrix,
and deliberate deviations live in
[Burp Community parity](docs/burp-parity.md).

## Quick start

```sh
cargo run -p perseid
```

The proxy listens on `127.0.0.1:8080` and the Perseid window opens. Perseid
creates its local CA on first launch, opens a certificate setup window, and
intercepts HTTPS by default. Open `http://127.0.0.1:8080/` directly, or use
**Proxy > TLS > Open certificate page**, to download and install the certificate
in your browser. Then point the browser — or an extension such as FoxyProxy —
at `127.0.0.1:8080` for HTTP and HTTPS. To hold HTTP (and decrypted HTTPS) traffic, open **Proxy > Intercept**
and click **Intercept is off** once to turn interception on. Full trust and
removal steps are in [HTTPS interception](docs/tls.md).

> Only test systems you own or have explicit permission to test. See
> [Safety and privacy](docs/safety.md).

Detailed setup, CLI flags, HiDPI behavior, and current limitations:
[Getting started](docs/getting-started.md).

## Documentation

| Document | Contents |
| --- | --- |
| [Getting started](docs/getting-started.md) | Build, run, proxy setup, options, limitations |
| [HTTPS interception](docs/tls.md) | Automatic local CA, certificate page, browser trust and removal, pass-through hosts |
| [Design](docs/design.md) | Principles, architecture, technology, performance |
| [Performance and memory](docs/performance.md) | M0 measurements, idle RSS breakdown, and method |
| [Roadmap](docs/roadmap.md) | Milestones from prototype to Community parity |
| [Burp Community parity](docs/burp-parity.md) | Definition of parity, capability matrix, deviations |
| [Safety and privacy](docs/safety.md) | Authorized use, data handling, CA trust |
| [Contributing](CONTRIBUTING.md) | Development setup, checks, project layout |

## Contributing

Contributions, testing feedback, and design discussions are welcome. Start with
[CONTRIBUTING.md](CONTRIBUTING.md). Particularly useful early feedback is
low-memory Linux behavior, HTTP correctness, accessibility, and Repeater and
Decoder workflow design.

## License

Perseid is dual-licensed under either of:

- [MIT License](LICENSE-MIT)
- [Apache License, Version 2.0](LICENSE-APACHE)

`SPDX-License-Identifier: MIT OR Apache-2.0`

Third-party dependencies and assets remain subject to their own licenses.
