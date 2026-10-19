# Roadmap

The north star is feature parity with Burp Suite Community Edition's manual
toolkit. See [Burp Community parity](burp-parity.md) for the capability matrix,
definition of done, and deliberate deviations.

## Milestones

- [x] **Milestone 0 — Proof of concept:** benchmark idle memory, history
  rendering, and large-response viewing on modest Linux hardware.

  *Complete: on a 4 vCPU / 5.3 GiB Kali arm64 VM at 200% scaling, a release
  build used 100.4 MiB RSS idle, 104.7 MiB with 2,000 exchanges, and at most
  115.2 MiB while viewing 8 MiB text and binary responses capped at 1 MiB.
  Method and full results: [Performance and memory](performance.md).*

- [ ] **Milestone 1 — Proxy core:** reliable HTTP/1.1 capture and forwarding,
  request/response interception with edit, forward, and drop, explicit scope
  configuration, and match/replace rules.

  *Status: capture, forwarding, the history UI, the Target site map, and a
  view-only scope filter work; editable interception, scope enforcement, and
  match/replace are not implemented.*

- [ ] **Milestone 2 — HTTPS:** local CA generation, TLS interception,
  documented browser trust setup, and pass-through controls.

  *Status: `CONNECT` tunneling works as TLS passthrough with byte counts;
  interception is not implemented.*

- [ ] **Milestone 3 — Daily workbench:** SQLite-backed persistent history,
  search, Repeater, Decoder, and response diff (Comparer).

  *Status: in-memory history, basic filtering, and a read-only Repeater
  preview work; persistence, editable replay, Decoder, and Comparer are not
  implemented.*

- [ ] **Milestone 4 — Protocols and testing tools:** HTTP/2 and WebSocket
  proxying with history, Sequencer, and unthrottled Intruder-style fuzzing
  with explicit scope and user-controlled concurrency.

  *Status: HTTP/1.1 only; the Intruder tab is a planned-workspace overview.*

- [ ] **Milestone 5 — Parity hardening:** packaging, crash recovery, safe
  handling of sensitive traffic, accessibility, performance benchmarks, and a
  documented parity audit.

  *Status: tests, rustfmt, clippy, CI, and the M0 memory baseline are in
  place; packaging and the broader parity benchmarks are not.*

- [ ] **After parity:** native extension point, passive and guided checks,
  import/export, integrations, and additional protocol coverage.

Priorities may change based on real-world use and measurements. Safety and
scope defaults are not deferred: each milestone lands with the relevant
protections documented.
