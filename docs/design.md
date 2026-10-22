# Design

[Documentation](README.md) · [Workbench](workbench.md) ·
[Contributing](../CONTRIBUTING.md) · [Roadmap](roadmap.md)

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

Perseid currently ships as one Cargo package. The proxy and byte-transform
engines have module boundaries so their behavior can be tested without opening
the UI; they are not separate, GUI-free build targets. The following diagram
shows current responsibilities, with planned work labelled explicitly:

```text
External browser
      |
      v
Local HTTP/1.1 and HTTP/2 proxy (TLS interception on by default)
      |
      +----> Scope and match/replace policy
      |
      +----> Interception queue <----> Hold, edit, forward, drop
      |
      +----> Bounded capture and in-memory history
      |                |
      |                +----> Metadata filtering (search/diff planned)
      |
      +----> Optional passive and guided checks (planned)
                       |
                       v
               Native egui interface
                       |
                       +----> Intruder (payload attacks over the direct client)
                       |
                       +----> Repeater (direct HTTP/HTTPS client, own runtime)
                       |
                       +----> Decoder (local byte transforms and hashes)
```

Current crate layout:

| Component | Responsibility |
| --- | --- |
| `crates/perseid/src/proxy/` | Embedded proxy: loopback listener, HTTP forwarding, interception policy, scope guard, match/replace rules, CONNECT tunnels, TLS interception with a local CA, body teeing, bounded capture events, the direct Repeater client, and the Intruder attack engine. Exposed as `perseid::proxy`. |
| `crates/perseid/src/` | egui/eframe desktop app: Burp-style workspace, interception editor, in-memory history, Intruder and Repeater editors, Decoder panels and transform engine, site map, scope and rewrite controls, TLS/CA controls, message views, keyboard navigation. |

The proxy stays behind its own module boundary even though it is compiled into
the application crate. The UI talks to it through the `perseid::proxy` API, so
proxy behavior remains testable without the UI.

HTTPS interception lives in the proxy module: a locally generated ECDSA CA mints
per-host certificates, rustls terminates the browser connection inside a
`CONNECT` tunnel, and decrypted HTTP/1.1 or HTTP/2 requests flow through the
same forwarding, interception, scope, and rewrite pipeline as plain HTTP. The
`CONNECT` tunnel is transport and is not inserted into HTTP history; the table
shows the decrypted request/response exchanges, as Burp does. Upstream
connections verify origin certificates against the platform trust store. The
CA is created automatically on first launch and loaded by the app from a stable
per-user data directory; trust is installed and removed by the user in the
browser, never by Perseid. Interception is on by default, and pass-through hosts
remain opaque. The proxy also serves the certificate as a local download page,
so installing trust does not require finding the file on disk.

The app persists each new CA as a complete certificate/key generation and
atomically replaces a pointer to select it. Store operations are serialized
across processes, and loaded pairs are checked for matching keys. Existing
top-level PEM pairs remain usable without migration until explicit regeneration.
See [CA storage and lifecycle](tls.md#where-the-ca-lives).

Repeater deliberately sits beside the proxy rather than behind it: the
`perseid::proxy` module exposes a direct HTTP/HTTPS client with its own runtime
and connection pool, and the UI sends edited messages to that engine. Replay
therefore works when the proxy is stopped and avoids re-entering interception,
match/replace, or scope policy. Responses are captured with the same byte limit
as proxy traffic and delivered to the UI over a channel.

Intruder sits on that same direct client. Its engine parses `$$` positions once,
renders each attack sequence lazily, and keeps at most the configured number of
requests in flight. Sniper, Battering ram, Pitchfork, and Cluster bomb differ
only in how payload indices map to positions; rendering then substitutes those
values into the frozen template and sends through the direct client. Completed
results carry their sequence and payloads and enter a bounded channel, so a fast
target cannot grow an unbounded queue. The desktop UI drains the channel every
frame and retains a separately bounded result table. The engine does not use the
proxy's live policy, but the workspace copies enforced scope into the plan; the
engine checks every generated request's effective target before sending and the
workspace preflights the first one.

Decoder is a local utility and never touches the network. Its byte-level engine
lives in `crates/perseid/src/decoder.rs`, independent of egui so the transforms
can be unit-tested directly. The panel chain, selection handling, coloring, and
widgets live beside it in `crates/perseid/src/app/decoder.rs`. A transform
accepts at most 8 MiB and may produce at most 16 MiB, including preserved
prefix and suffix bytes; gzip decompression is bounded by the same output cap.
Results carry byte ranges tagged with the operation that produced them, and the
Text and Hex views use those ranges for their operation colors. Applying an
operation truncates the panels after its source and appends the result, so the
chain cannot describe transformations that no longer produced the visible
bytes.

Planned additions include a disk-backed history store, WebSocket handling, and
a native extension point. Proxy, Repeater, and Intruder network operations run
off the UI thread. Proxy capture events use a bounded, lossy channel with a drop
counter; Repeater jobs and Intruder events use separate bounded queues. Repeater
admission reserves both an operation slot and bytes through result consumption,
not just through network completion. Held messages use a separate bounded
interception channel with a one-shot decision per message, so capture-event
overflow does not discard the user's interception decision channel.

Capture/history limits constrain retained body data, not total resident memory.
Repeater additionally has an aggregate pending-work budget, and Pretty body
formatting has independent input/output and depth limits. Editor state,
connection pools, other caches, and graphics resources also consume memory;
these controls are not a shared process-wide allocation or RSS ceiling.

The desktop application does not start or embed a Chromium instance. An
external browser points at the loopback proxy; this keeps the binary small and
lets users keep their normal browser, extensions, and devtools.

## Interface direction

The workspace follows Burp's tool organization so existing users can find
familiar surfaces immediately: **Target**, **Proxy**, **Intruder**,
**Repeater**, **Decoder**, and **Comparer**, in that order. **Proxy > HTTP
history** is the default landing view because it is the live capture surface.

The visual language is a warm, low-contrast light theme: near-white surfaces,
pale orange selection states, and a burnt-orange accent. The palette lives in
one theme module so future views stay consistent. HiDPI text and controls are
first-class requirements, not an afterthought.

Planned tools such as Sequencer are not presented as working controls before
their engines exist. **Comparer** is a real two-item diff workspace: one shared
item collection, word/byte comparison, and paired Text/Hex result panes.
**Intruder** is a real fuzzing workspace:
an editable raw request with `$$` position highlighting, Add/Clear/Auto marker
actions, one payload set per relevant position, all four Burp attack types, a
target bar with optional `Host` synchronization, bounded concurrency, and a
result table with request/response inspection. **Repeater** is a real editor
with request tabs seeded from HTTP history, a target bar, an editable raw
request, direct Send (`Ctrl`+`Enter`), and Pretty/Raw/Hex responses. **Decoder**
is a stack of real transform panels: Text and read-only
Hex views, Decode as and Encode as menus, a hash menu, Smart decode, and
selection-aware transforms that replace the downstream chain. **Proxy >
Intercept** is a real editor: held messages show their raw
head, editable buffered text body, and read-only binary preview, with Burp's
single Intercept on/off control plus explicit Forward and Drop actions. HTTP/2
messages use a reconstructed head: the request line is origin-form,
`:authority` is represented by `Host`, and
response status lines omit the reason phrase. Pseudo-headers are regenerated
when the edited message is forwarded. Scope serves both as a view filter and, when
enabled, as a traffic guardrail checked before interception and again after
edits. **Proxy > Proxy settings** shows the automatically created certificate authority,
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
- **hyper** for HTTP/1.1 and HTTP/2 forwarding plus direct Repeater and Intruder
  traffic; WebSocket handling is planned.
- **regex** for match/replace rules, with rules compiled once per policy update.
- **rustls** and an automatically generated CA for HTTPS interception. The CA
  persists across runs, interception is on by default, and the user installs
  and removes browser trust manually.
- **egui + eframe** for a native desktop interface, using the `glow`
  renderer.
- **base64**, **flate2**, **html-escape**, **num-bigint**, and the RustCrypto
  digest crates for Decoder. The transforms are implemented in-process and
  bounded; Decoder does not embed a scripting engine.
- **SQLite** for searchable, disk-backed history (planned).

## Performance philosophy

A small Rust binary does not automatically mean a small application. Perseid
aims to minimize **idle CPU use, total resident memory, startup latency, and
avoidable copies**, and to publish measurements instead of unverified
performance claims.

Important implementation principles:

- Disk-backed history with bounded in-memory windows (planned).
- Hard limits on captured body size, with bounded text and hex previews.
- A total retained-body budget for history, independent of the exchange count
  (half of physical memory by default, with a 256 MiB fallback).
- Concurrent proxy connection/tunnel limits and a bounded capture-event channel
  with drop counters. Loss recovery applies queued events before resolving
  in-flight rows; late or duplicate completions do not decrement unrelated
  exchanges' in-flight counts.
- A bounded interception queue: held bodies are capped by `--intercept-limit`,
  larger or streaming bodies stay header-only, and unresolved messages apply
  backpressure rather than being silently discarded.
- Repeater admission covers queued jobs, active sends, and unread results: at
  most 32 operations and 64 MiB of retained-data reservations, with at most eight
  active sends. Response capacity is reserved before work starts; backpressure
  is reported without stopping already admitted sends. Requests have an 8 MiB
  retained-buffer limit and heads have a 64 KiB limit.
- Intruder caps concurrency at 32, allows one active attack, and queues at most
  64 unread events or 64 MiB before the worker waits. The UI keeps at most 1,000
  result rows and 64 MiB, evicting the oldest; generated requests share the
  8 MiB request and 64 KiB head limits.
- Virtualized history tables and Hex rows, plus bounded body previews. Pretty
  and Raw use a multiline text editor with syntax highlighting and a painted
  line-number gutter; unlike Hex, their text layout is not row-virtualized.
  History/Repeater views decode gzip, deflate, and Brotli bodies with a decoded
  preview cap.
- Pretty body formatting independently caps input and output at 256 KiB and
  nesting at 128 levels. A document that would exceed a budget falls back to
  bounded Raw text instead of expanding indentation without limit. Editor
  caches remain a separate memory cost from captured/decoded input.
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
