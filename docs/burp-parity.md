# Burp Suite Community parity

**Goal:** Wiremoth aims to be a practical alternative to Burp Suite Community
Edition (CE) for manual web security testing: the same capabilities, adapted
to a Rust-native, memory-bounded, keyboard-first tool.

This is a direction, not a claim. Most capabilities below are still planned.

## What parity means

Parity means that for every task supported by Burp CE's built-in manual
toolkit, Wiremoth offers an equivalent, documented workflow. It does not mean
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
Intruder to encourage upgrades; Wiremoth's fuzzer will not reproduce that
limit. Scope and concurrency remain explicit, user-controlled safety settings.

## Capability matrix

Status values: **Working** (usable today), **Partial** (foundation exists),
**Planned** (assigned to a milestone), **Deviation** (intentional difference),
**Non-goal** (explicitly outside parity).

| Capability | Burp CE | Wiremoth | Status | Milestone |
| --- | --- | --- | --- | --- |
| Loopback HTTP/1.1 proxy and forwarding | Yes | Yes | Working | — |
| Intercept, edit, forward, or drop messages | Yes | Capture only | Partial | M1 |
| Scope / target allowlist | Yes | View filter over history and site map | Partial | M1 |
| Target site map | Yes | Host/path tree from history | Partial | M1 |
| Match/replace rules | Yes | No | Planned | M1 |
| Session history | Yes | In-memory, bounded | Partial | M3 |
| History filtering | Yes | Text filter plus scope view filter | Partial | M3 |
| HTTPS interception with a local CA | Yes | CONNECT passthrough | Partial | M2 |
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

Scope rules currently filter the site map and history only. They are not yet a
traffic guardrail: capture and forwarding still apply to every host until the
interception milestone enforces scope.

## Deliberate deviations

- **External browser instead of bundled Chromium.** Wiremoth is used with any
  browser or HTTP client configured to proxy through it. This keeps the binary
  small, avoids owning a browser engine, and lets users keep their normal
  browser profile and devtools. Browser-side tooling such as DOM Invader is
  not part of the parity target.
- **No binary BApp compatibility.** Burp extensions are Java-based. Wiremoth
  acknowledges extension capability but plans a native extension point after
  core parity rather than reproducing Burp's extension ABI. Features that are
  only reachable through BApps are not part of the parity claim.
- **No artificial Intruder throttling.** Burp CE deliberately rate-limits
  Intruder. Wiremoth will run fuzzing at the rate the user configures, bounded
  by scope and resource settings rather than an upgrade restriction.
- **Keyboard-first ergonomics.** Parity is about capabilities and outcomes,
  not reproducing Burp's mouse-driven workflow.
- **Persistence is a Wiremoth extra.** Burp CE cannot save project files;
  Wiremoth plans local, bounded SQLite history as part of its daily workflow.

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
| **M1 — Proxy core** | Intercepting HTTP/1.1 proxy with editable hold/forward/drop, scope controls, match/replace rules, and session history. |
| **M2 — HTTPS** | Local CA lifecycle, TLS interception, explicit browser trust setup, and pass-through controls. |
| **M3 — Daily workbench** | Persistent searchable history, Repeater, Decoder, and Comparer. |
| **M4 — Protocols and testing tools** | HTTP/2, WebSocket proxying and history, Sequencer, and unthrottled Intruder-style fuzzing with explicit scope and user-controlled concurrency. |
| **M5 — Parity hardening** | Packaging, crash recovery, accessibility, published benchmarks, and a documented parity audit. |

After M5, Wiremoth can claim CE manual-toolkit parity subject to the
deviations above. Native extensions, passive/guided checks, and other
Wiremoth-specific ideas continue after parity; see the [Roadmap](roadmap.md).

Progress is tracked by updating this matrix at the end of each milestone.
