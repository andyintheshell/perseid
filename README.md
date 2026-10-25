<p align="center">
  <img src="branding/perseid-readme-banner.png" alt="Perseid — inspect, modify, replay" width="100%">
</p>

# Perseid

**An open-source, Rust-native alternative to Burp Suite Community Edition.**

Intercept, inspect, modify, replay, and fuzz HTTP traffic in a lightweight desktop
workbench. Built for security researchers, CTF players, and developers testing
applications they own or are authorized to assess. **No Java runtime or bundled
browser required.**

[![CI](https://github.com/andyintheshell/perseid/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/andyintheshell/perseid/actions/workflows/ci.yml)
[![Rust 1.95+](https://img.shields.io/badge/Rust-1.95%2B-orange?logo=rust)](rust-toolchain.toml)
[![Public alpha](https://img.shields.io/badge/status-public%20alpha-FF9915)](docs/roadmap.md)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-112D68)](https://github.com/andyintheshell/perseid/blob/main/LICENSE-MIT)
[![OpenSSF Best Practices](https://www.bestpractices.dev/projects/15832/badge)](https://www.bestpractices.dev/projects/15832)

**[Get started](docs/getting-started.md)** · [Screenshots](docs/screenshots.md) ·
[Feature comparison](docs/burp-parity.md) · [User guide](docs/workbench.md) ·
[Contribute](CONTRIBUTING.md)

[![Perseid HTTP history with side-by-side request and response views](docs/images/perseid-main.png)](docs/screenshots.md)

> [!IMPORTANT]
> **Public alpha: the core manual-testing workflows work today.** Perseid is
> actively developed, not yet a feature-complete or drop-in Burp replacement.
> Expect rough edges, and see the [capability matrix](docs/burp-parity.md) for
> an honest account of what is implemented and what is still planned.

## Why Perseid?

- **A familiar manual-testing workflow.** Go from a captured request to
  interception, replay, payload fuzzing, decoding, and comparison in one app.
- **Native and cross-platform.** Built in Rust with egui; CI tests Linux,
  Apple Silicon macOS, and Windows. No JVM or embedded browser.
- **Explicit resource controls.** Configurable per-message capture and
  in-memory UI-cache limits, bounded Intruder concurrency, and a live
  heap-memory indicator. Saved project history remains on disk until cleared.
- **Open development.** Source available under MIT or Apache-2.0; feedback,
  bug reports, and focused contributions are welcome.

## What works today

| Tool | Available today |
| --- | --- |
| **Proxy & HTTP history** | HTTP/1.1 and HTTP/2 forwarding; filtered history; Burp-style target site map with a host/folder tree, display filters, and sortable Contents table; Pretty, Raw, and Hex message views; Copy as curl for complete captures; gzip, deflate, and Brotli decoding. |
| **Projects** | Temporary projects by default; save/open/copy single-file `.perseid` projects with automatic persistence for captured history and project-scoped workbench state. Older saved history is stored but not yet pageable in the UI. |
| **Search** | Search the currently loaded HTTP-history snapshot's request and response heads and decoded bodies, with case, regex, negative, in-scope, and dynamic-update options; highlighted Pretty/Raw/Hex results. Older saved rows are not searched yet. |
| **HTTPS inspection** | Local CA generation, certificate download page, per-host TLS pass-through, and manual trust installation/removal. |
| **Intercept** | Hold, edit, forward, or drop requests and optionally responses, including buffered text bodies. |
| **WebSocket history** | Capture and inspect text and binary frames for `ws://` and decrypted `wss://` connections; no message editing or replay yet. |
| **Repeater** | Editable HTTP/HTTPS requests, multiple tabs, history seeding, target and virtual-host control, and direct replay independent of the proxy. |
| **Intruder** | `$$` payload positions, editable payload sets, Sniper, Battering ram, Pitchfork, and Cluster bomb modes, bounded concurrency, and response inspection. |
| **Decoder** | Stacked encoding/decoding panels, selection-aware transforms, Smart decode, and 31 common hashes. |
| **Comparer** | Text and hex comparison, word- and byte-level diffs, synchronized scrolling, and inputs from history, files, or the clipboard. |
| **Sequencer** | Manual and live token sampling with character- and bit-level randomness tests, effective-entropy estimates, and per-analysis settings. |
| **Scope & rewriting** | Opt-in proxy scope enforcement, literal/regex match-and-replace rules, and capture/history limits. |

The interface follows **Target → Proxy → Intruder → Repeater → Sequencer →
Decoder → Comparer**. See the [workbench guide](docs/workbench.md) for behavior
and limitations, or browse the [screenshot gallery](docs/screenshots.md).

## Quick start

Install the latest version published to crates.io with Cargo (it compiles from
source), or choose a prebuilt binary from
[GitHub Releases](https://github.com/andyintheshell/perseid/releases/latest).
The release includes Windows x64/ARM64 MSI and no-install ZIPs, a universal
macOS DMG, and Linux x86-64/ARM64 AppImage and DEB packages. Windows packages
are unsigned; the macOS app is ad-hoc signed but not notarized. See the
[platform requirements](docs/getting-started.md#requirements) before installing.
The latest published `0.1.0` crate and GitHub release predate project
persistence. The current source/workspace version is `0.2.0` and includes the
first persistence support. Until the `0.2.0` release is published, build from a
source checkout to use projects.

### Install from crates.io

```sh
cargo install perseid --locked
perseid
```

This builds the latest published crate on your machine and needs Rust 1.95+
plus the platform's native build dependencies.

### Build from a source checkout

```sh
git clone https://github.com/andyintheshell/perseid.git
cd perseid
cargo run -p perseid --locked
```

1. Perseid opens on **Proxy > HTTP history**, listening on `127.0.0.1:8080`
   by default.
2. Follow the [local HTTP walkthrough](docs/getting-started.md#try-it-with-a-local-target)
   to capture your first request without installing a certificate.
3. For HTTPS, open `http://127.0.0.1:8080/` directly and follow the
   [TLS setup guide](docs/tls.md). Perseid creates a local CA on first launch,
   but **never installs trust automatically**.
4. Configure a dedicated testing browser to use `127.0.0.1:8080` for both
   HTTP and HTTPS. Turn on **Proxy > Intercept** when you want to hold and edit
   requests; it is off by default.

> [!WARNING]
> Test only systems you own or have explicit permission to assess. Use a
> dedicated browser profile. Proxy scope rules **do not apply to direct
> Repeater sends**. Intruder also sends directly, but checks each generated
> request when scope enforcement is enabled. Read
> [Safety and privacy](docs/safety.md) before capturing real traffic.

## Public alpha: current limitations

Perseid is already usable for hands-on HTTP security testing, but there are
important boundaries:

- **Project persistence is an initial slice.** `.perseid` files save captured
  HTTP/WebSocket history and the full workbench state, but the UI only loads a
  bounded newest-first history snapshot; older stored rows are not pageable or
  searchable yet. Files are not encrypted, migrations and recent-projects are
  not implemented, and temporary-project contents are lost on exit unless saved.
  Temporary projects bound captured records and retained body bytes with the
  configurable `--temporary-retention-*` limits (defaulting to `--history-*`)
  and evict the oldest records first. These are soft limits: each stream's
  newest record is kept even when it alone exceeds the byte budget, in-flight
  HTTP exchanges are kept until they complete, `0` disables a dimension, and
  headers, indexes, and database overhead are outside the byte budget. Saved
  projects are not automatically pruned and can grow until the disk fills. New
  Project and Open Project ask Save/Discard/Cancel before replacing a temporary
  project with unsaved work.
- **Transient activity is not persisted.** Running attacks and sends, held
  interception decisions, pending network operations, and proxy lifecycle state
  are not restored. The local TLS CA certificate and private key are stored
  separately from project files.
- **No WebSocket editing or replay yet.** WebSocket frames can be captured and
  inspected, but not intercepted, modified, or resent.
- **Not an automated scanner.** There is no crawler, automated vulnerability
  scanner, or bundled browser. Intruder sends requests using user-supplied
  payloads; it does not discover vulnerabilities automatically.
- **Sequencer results are indicative.** Token capture is expression-based and
  the statistical tests are bounded approximations, not a proof that a token
  is predictable or safe.
- **Some message bodies cannot be edited.** Large, streaming, and binary
  intercepted bodies are header-editable only. Previews and direct-send seeds
  can be truncated or lossy.
- **Performance varies by environment.** Resource limits are configurable,
  but there are no universal memory or speed guarantees. See the
  [measured baseline](docs/performance.md).

Burp Suite Community manual-toolkit parity is a
[direction](docs/burp-parity.md), **not a claim of current feature parity**.
Burp Suite and Burp Suite Community Edition are trademarks of PortSwigger Ltd.
Perseid is independently developed, contains no PortSwigger software or logos,
and is not affiliated with, sponsored by, or endorsed by PortSwigger Ltd.
References to Burp are for comparison only.

## Help build Perseid

Want an open-source Burp-style workbench to improve? **Contributions are
welcome**, whether you're interested in Rust networking, egui, usability,
platform support, tests, or documentation.

- **Try it and report issues:** [Open an issue](https://github.com/andyintheshell/perseid/issues)
  with a minimal reproduction, your OS, and what you expected to happen.
- **Contribute code or documentation:** Read [CONTRIBUTING.md](CONTRIBUTING.md)
  and pick a focused change. HTTP correctness, accessibility, low-memory
  behavior, and macOS/Windows testing are especially useful.
- **Discuss future work:** Read the [roadmap](docs/roadmap.md) and
  [capability matrix](docs/burp-parity.md) before proposing major features.
- **Report vulnerabilities privately:** Follow [SECURITY.md](SECURITY.md).
  Never post live credentials, private keys, or sensitive traffic in issues.

## Documentation

| I want to… | Start here |
| --- | --- |
| Install, build, or configure Perseid | [Getting started](docs/getting-started.md) |
| Learn Proxy, Intruder, Repeater, Sequencer, Decoder, and Comparer | [Workbench guide](docs/workbench.md) |
| Set up HTTPS interception and certificate trust | [TLS guide](docs/tls.md) |
| Troubleshoot setup, traffic, or proxy problems | [Troubleshooting](docs/troubleshooting.md) |
| Compare implemented and planned capabilities | [Burp Community capability matrix](docs/burp-parity.md) |
| Understand the architecture | [Design](docs/design.md) |
| Find all guides and references | [Documentation index](docs/README.md) |

## License

The source code is dual-licensed under [MIT](LICENSE-MIT) **or**
[Apache-2.0](LICENSE-APACHE), at your option. Third-party dependencies, fonts,
and assets retain their own licenses; the Perseid Marks have separate terms
below.

`SPDX-License-Identifier: MIT OR Apache-2.0`

### Trademarks and Perseid branding

The Perseid name, logos, and visual identity identify this project and are not
covered by the source-code licenses. Forks and modified versions must remove or
replace them and use their own branding. See the
[Perseid brand terms](branding/LICENSE-brand.txt).
