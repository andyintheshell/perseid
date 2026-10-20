# Design

Perseid is a Rust-native web security tool for people who like to understand
what happens on the wire. It is designed around the everyday workflow of
intercepting a request, inspecting it, changing it, sending it again, and
understanding the response.

## Why Perseid?

Web security testing often comes down to the same small set of actions:
capture a request, inspect its parameters and headers, replay it with changes,
compare the result, and follow a hypothesis. Those actions should be fast and
comfortable even on modest hardware.

The name comes from the Perseid meteor shower: many faint signals that become
clear when you can watch the whole sky.

Perseid aims to be:

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

Perseid is a set of small Rust components, with the user interface
independent from the proxy and analysis code:

```text
External browser
      |
      v
Local HTTP/1.1 proxy (TLS interception on by default)
      |
      +----> Scope and match/replace policy
      |
      +----> Interception queue <----> Hold, edit, forward, drop
      |
      +----> Bounded capture and in-memory history
      |                |
      |                +----> Search, filtering, comparison
      |
      +----> Repeater / request mutation (planned)
      |
      +----> Optional passive and guided checks (planned)
                       |
                       v
               Native egui interface
```

Current workspace layout:

| Component | Responsibility |
| --- | --- |
| `crates/perseid-proxy` | Loopback listener, HTTP forwarding, interception policy, scope guard, match/replace rules, CONNECT tunnels, TLS interception with a local CA, body teeing, bounded capture events. |
| `crates/perseid` | egui/eframe desktop app: Burp-style workspace, interception editor, in-memory history, site map, scope and rewrite controls, TLS/CA controls, message views, keyboard navigation. |

HTTPS interception lives in the proxy crate: a locally generated ECDSA CA mints
per-host certificates, rustls terminates the browser connection inside a
`CONNECT` tunnel, and decrypted HTTP/1.1 requests flow through the same
forwarding, interception, scope, and rewrite pipeline as plain HTTP. The
`CONNECT` tunnel is transport and is not inserted into HTTP history; the table
shows the decrypted request/response exchanges, as Burp does. Upstream
connections verify origin certificates against the platform trust store. The
CA is created automatically on first launch and loaded by the app from a stable
per-user data directory; trust is installed and removed by the user in the
browser, never by Perseid. Interception is on by default, and pass-through hosts
remain opaque. The proxy also serves the certificate as a local download page,
so installing trust does not require finding the file on disk.

Planned additions include a disk-backed history store, replay/fuzzing workers,
and a native extension point. Network operations run on their own runtime and
communicate with the UI through a bounded channel so a slow interface cannot
grow memory without limit. Held messages use a separate bounded interception
channel with a one-shot decision per message, so capture-event overflow cannot
strand an exchange that is waiting for the user.

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
milestone that will complete them. Intruder and Repeater do not present
nonfunctional controls as if they already worked. **Proxy > Intercept** is a
real editor: held messages show their raw head, editable buffered text body,
and read-only binary preview, with Burp's single Intercept on/off control plus
explicit Forward and Drop actions. Scope serves both as a view filter and, when
enabled, as a traffic guardrail checked before interception and again after
edits. **Proxy > TLS** shows the automatically created certificate authority,
opens the local certificate download page, and manages the HTTPS decryption
toggle, regeneration, removal, and the pass-through host list.

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
- **regex** for match/replace rules, with rules compiled once per policy update.
- **rustls** and an automatically generated CA for HTTPS interception. The CA
  persists across runs, interception is on by default, and the user installs
  and removes browser trust manually.
- **egui + eframe** for a native desktop interface, using the `glow`
  renderer.
- **SQLite** for searchable, disk-backed history (planned).

## Performance philosophy

A small Rust binary does not automatically mean a small application. Perseid
aims to minimize **idle CPU use, total resident memory, startup latency, and
avoidable copies**, and to publish measurements instead of unverified
performance claims.

Important implementation principles:

- Disk-backed history with bounded in-memory windows (planned).
- Hard limits on captured body size, with bounded text and hex previews.
- A total retained-body budget for history, independent of the exchange count.
- Bounded worker queues, concurrent connections and tunnels, and a bounded UI
  event channel with drop counters.
- A bounded interception queue: held bodies are capped by `--intercept-limit`,
  larger or streaming bodies stay header-only, and unresolved messages apply
  backpressure rather than being silently discarded.
- Virtualized request lists and message rows, plus bounded text previews. Pretty
  formats JSON, HTML/XML, and URL-encoded forms; Pretty and Raw are rendered
  line-by-line with numbers and a small purpose-built highlighter, so only the
  visible page is laid out. All views decode gzip, deflate, and Brotli bodies
  with a hard output cap, and Hex previews are bounded byte-offset dumps.
- Avoid full document layout for multi-megabyte responses; show raw text and
  hex views instead.
- A fixed, small malloc-arena limit so proxy and UI threads do not multiply
  allocator overhead.
- A live memory meter so allocator behavior is visible during use.

The Milestone 0 baseline and measurement method are published in
[Performance and memory](performance.md).

## Non-goals

For the initial parity target, Perseid is not trying to:

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
