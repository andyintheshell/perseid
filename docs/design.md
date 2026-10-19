# Design

Wiremoth is a Rust-native web security tool for people who like to understand
what happens on the wire. It is designed around the everyday workflow of
intercepting a request, inspecting it, changing it, sending it again, and
understanding the response.

## Why Wiremoth?

Web security testing often comes down to the same small set of actions:
capture a request, inspect its parameters and headers, replay it with changes,
compare the result, and follow a hypothesis. Those actions should be fast and
comfortable even on modest hardware.

Wiremoth aims to be:

- **Manual-first.** The request and response are the center of the interface,
  not a dashboard of speculative findings.
- **Lightweight by design.** No embedded browser or web-based desktop UI;
  memory limits, streamed bodies, and bounded history are requirements.
- **Keyboard-friendly.** Fast navigation among requests, replay tabs, search,
  and comparisons.
- **Transparent.** Inspection tools should show exactly what was sent,
  received, and changed.
- **Extensible.** Optional, narrowly scoped checks and extensions rather than
  a mandatory monolithic scanning engine.
- **Local-first.** Captured traffic stays on the user's machine unless the
  user explicitly exports or shares it.

## Architecture

Wiremoth is a set of small Rust components, with the user interface
independent from the proxy and analysis code:

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

Current workspace layout:

| Component | Responsibility |
| --- | --- |
| `crates/wiremoth-proxy` | Loopback listener, HTTP forwarding, CONNECT tunnels, body teeing, bounded capture events. |
| `crates/wiremoth` | egui/eframe desktop app: Burp-style workspace, in-memory history, site map, scope filter, message views, keyboard navigation. |

Planned additions include a TLS interception layer with a local CA, a
disk-backed history store, replay/fuzzing workers, and a native extension
point. Network operations run on their own runtime and communicate with the UI
through a bounded channel so a slow interface cannot grow memory without
limit.

The desktop application does not start or embed a Chromium instance. An
external browser points at the loopback proxy; this keeps the binary small and
lets users keep their normal browser, extensions, and devtools.

## Interface direction

The workspace follows Burp's tool organization so existing users can find
familiar surfaces immediately: **Target**, **Proxy**, **Intruder**, and
**Repeater**, in that order. **Proxy > HTTP history** is the default landing
view because it is the live capture surface.

The visual language is a warm, low-contrast light theme: near-white surfaces,
pale orange selection states, and a burnt-orange accent. The palette lives in
one theme module so future views stay consistent. HiDPI text and controls are
first-class requirements, not an afterthought.

Tools that are not implemented yet are shown as explicit previews with the
milestone that will complete them. Intercept, Intruder, and Repeater do not
present nonfunctional controls as if they already worked. Scope is implemented
as a view filter over the site map and history; traffic enforcement belongs to
the interception milestone.

The interface is keyboard-friendly: tab switches, history navigation, filtering,
and zoom all have shortcuts. A mouse remains fully supported. A compact
memory indicator in the status bar reports allocator memory in use and opens a
details window with the current and peak values and a rolling graph, following
Burp's bottom-bar convention.

## Technology

Initial technology choices, subject to prototyping and measurement:

- **Rust** for the application and networking components.
- **Tokio** for asynchronous networking.
- **hyper** for HTTP/1.1 forwarding; HTTP/2 and WebSocket handling are planned.
- **rustls** and a locally generated CA for HTTPS interception (planned).
- **egui + eframe** for a native desktop interface, using the `glow`
  renderer.
- **SQLite** for searchable, disk-backed history (planned).

## Performance philosophy

A small Rust binary does not automatically mean a small application. Wiremoth
aims to minimize **idle CPU use, total resident memory, startup latency, and
avoidable copies**, and to publish measurements instead of unverified
performance claims.

Important implementation principles:

- Disk-backed history with bounded in-memory windows (planned).
- Hard limits on captured body size, with explicit truncation markers.
- A total retained-body budget for history, independent of the exchange count.
- Bounded worker queues, concurrent connections and tunnels, and a bounded UI
  event channel with drop counters.
- Virtualized request lists and hex dumps, plus bounded text previews.
- Avoid full document layout for multi-megabyte responses; show raw text and
  hex views instead.
- A fixed, small malloc-arena limit so proxy and UI threads do not multiply
  allocator overhead.
- A live memory meter so allocator behavior is visible during use.

The Milestone 0 baseline and measurement method are published in
[Performance and memory](performance.md).

## Non-goals

For the initial parity target, Wiremoth is not trying to:

- Replace a commercial vulnerability scanner or reproduce Burp Suite
  Professional's automated scanning, crawler, Collaborator/OAST, or project
  files.
- Build a browser engine, embed Chromium, or ship a full JavaScript crawler.
- Automatically exploit discovered issues.
- Run unattended scans against arbitrary hosts.
- Provide binary compatibility with Java Burp extensions (BApps). A native
  extension point is planned after core parity.

See [Burp Community parity](burp-parity.md) for the capability target and
[Roadmap](roadmap.md) for sequencing.
