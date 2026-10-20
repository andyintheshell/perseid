# Burp Suite Community parity

**Goal:** Perseid aims to be a practical alternative to Burp Suite Community
Edition (CE) for manual web security testing: the same capabilities, adapted
to a Rust-native, memory-bounded, keyboard-first tool.

This is a direction, not a claim. Several capabilities below are still planned.

## What parity means

Parity means that for every task supported by Burp CE's built-in manual
toolkit, Perseid offers an equivalent, documented workflow. It does not mean
identical UI, identical protocols or internals, or byte-for-byte compatibility
with Burp extensions.

The reference feature set is PortSwigger's published CE toolkit:

- HTTP(S) and WebSocket proxy with history
- Repeater
- Decoder
- Sequencer
- Comparer
- Intruder

Source: [Burp Suite Community Edition](https://portswigger.net/burp/communitydownload)
(checked October 2026).

The workspace deliberately follows Burp's tool order: **Target**, **Proxy**,
**Intruder**, and **Repeater**. Tools that are not implemented yet are labelled
as previews rather than presented as working features. Familiar navigation is
part of the goal; reproducing every control before its engine exists is not.

Parity here means capability, not artificial restrictions. Burp CE rate-limits
Intruder to encourage upgrades; Perseid's fuzzer will not reproduce that
limit. Scope and concurrency remain explicit, user-controlled safety settings.

## Capability matrix

Status values: **Working** (usable today), **Partial** (foundation exists),
**Planned** (assigned to a milestone), **Deviation** (intentional difference),
**Non-goal** (explicitly outside parity).

| Capability | Burp CE | Perseid | Status | Milestone |
| --- | --- | --- | --- | --- |
| Loopback HTTP/1.1 proxy and forwarding | Yes | Yes | Working | — |
| Intercept, edit, forward, or drop messages | Yes | Plain HTTP, head and buffered-body editing | Working | — |
| Scope / target allowlist | Yes | View filter plus opt-in HTTP/CONNECT blocking | Working | — |
| Target site map | Yes | Host/path tree from history | Partial | — |
| Match/replace rules | Yes | Head and buffered UTF-8 body rules, literal or regex | Working | — |
| Compressed body inspection | Yes | Bounded gzip, deflate, and Brotli decoding in Raw, Headers, and Hex; wire bytes still forwarded | Working | — |
| Session history | Yes | In-memory, bounded | Partial | M3 |
| History filtering | Yes | Text filter plus scope view filter | Partial | M3 |
| Memory usage display | Yes | Allocator memory in use and peak, rolling graph | Working | — |
| HTTPS interception with a local CA | Yes | Automatic local CA, HTTPS interception on by default, local certificate page, pass-through hosts, manual trust setup | Working | — |
| HTTP/2 proxying | Yes | No (HTTP/1.1 only) | Planned | M4 |
| WebSocket proxy and frame history | Yes | No | Planned | M4 |
| Repeater | Yes | Read-only preview | Partial | M3 |
| Decoder | Yes | No | Planned | M3 |
| Comparer | Yes | No | Planned | M3 |
| Sequencer | Yes | No | Planned | M4 |
| Intruder-style request fuzzing | Yes (rate-limited demo) | No | Planned (unthrottled) | M4 |
| Extension support | BApps | No | Deviation | Post-parity |
| Bundled Chromium browser | Yes | External browser | Deviation | — |
| Project files | No (Pro feature) | SQLite persistence planned | Beyond CE | M3 |
| Automated scanner, crawler, Collaborator/OAST | No (Pro features) | No | Non-goal | — |

Interception, scope enforcement, and match/replace cover plain HTTP and
decrypted HTTPS. HTTPS `CONNECT` targets can be blocked by scope. TLS
interception is on by default, as it is in Burp: Perseid creates a local CA on
first launch, serves its certificate from a local download page, and reuses the
CA across restarts and updates. The user still installs trust manually in the
browser; the CA private key stays local, and Perseid never writes to a trust
store. Hosts on the pass-through list stay opaque, and HTTP/1.1 is the only
protocol negotiated inside the tunnel. See [HTTPS interception](tls.md) for the
trust and removal workflow.
Interception buffers bodies up to `--intercept-limit`; larger or streaming
bodies are held for header edits, and body rewrite rules apply only to buffered
UTF-8 bodies. The message editor decodes gzip, deflate, and Brotli bodies in
Raw, Headers, and Hex, mirroring Burp's default unpacking, and the request and
response panes use Burp's horizontal side-by-side layout. Like Burp, the proxy
removes unsupported `Accept-Encoding` codings before forwarding. Interception
and body rules still operate on the wire representation, and forwarding keeps
the original response encoding. Scope enforcement is opt-in, and an empty scope
allows every host.

Burp Suite's status bar exposes a memory figure. Perseid's equivalent is
allocator heap in use, with the peak and a rolling graph in the memory window;
it is available on every platform. RSS and its anonymous/file-backed split are
documented in [Performance and memory](performance.md) for system-level
comparison; they include mapped libraries and graphics-driver code.

## Deliberate deviations

- **External browser instead of bundled Chromium.** Perseid is used with any
  browser or HTTP client configured to proxy through it. This keeps the binary
  small, avoids owning a browser engine, and lets users keep their normal
  browser profile and devtools. Browser-side tooling such as DOM Invader is
  not part of the parity target.
- **No binary BApp compatibility.** Burp extensions are Java-based. Perseid
  acknowledges extension capability but plans a native extension point after
  core parity rather than reproducing Burp's extension ABI. Features that are
  only reachable through BApps are not part of the parity claim.
- **No artificial Intruder throttling.** Burp CE deliberately rate-limits
  Intruder. Perseid will run fuzzing at the rate the user configures, bounded
  by scope and resource settings rather than an upgrade restriction.
- **Keyboard-first ergonomics.** Parity is about capabilities and outcomes,
  not reproducing Burp's mouse-driven workflow.
- **Persistence is a Perseid extra.** Burp CE cannot save project files;
  Perseid plans local, bounded SQLite history as part of its daily workflow.

## Definition of done

A capability counts toward parity only when:

1. It is usable for the task without developer tools or manual file editing.
2. It has automated tests where practical, plus a manual workflow check.
3. Its safety behavior is documented: scope, data retention, and CA trust.
4. Its status in the matrix and the user documentation are up to date.

Verification uses local practice targets such as OWASP Juice Shop, DVWA, and
WebGoat, and PortSwigger Web Security Academy labs. Only test systems you own
or are explicitly authorized to test.

## Parity milestones

| Milestone | Parity outcome |
| --- | --- |
| **M1 — Proxy core** | Intercepting HTTP/1.1 proxy with editable hold/forward/drop, scope controls, match/replace rules, and in-memory session history. *(Complete.)* |
| **M2 — HTTPS** | Local CA lifecycle, TLS interception, explicit browser trust setup, and pass-through controls. *(Complete.)* |
| **M3 — Daily workbench** | Persistent searchable history, Repeater, Decoder, and Comparer. |
| **M4 — Protocols and testing tools** | HTTP/2, WebSocket proxying and history, Sequencer, and unthrottled Intruder-style fuzzing with explicit scope and user-controlled concurrency. |
| **M5 — Parity hardening** | Packaging, crash recovery, accessibility, published benchmarks, and a documented parity audit. |

After M5, Perseid can claim CE manual-toolkit parity subject to the
deviations above. Native extensions, passive/guided checks, and other
Perseid-specific ideas continue after parity; see the [Roadmap](roadmap.md).

Progress is tracked by updating this matrix at the end of each milestone.
