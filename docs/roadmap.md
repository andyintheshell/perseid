# Roadmap

[Documentation](README.md) · [Capability matrix](burp-parity.md) ·
[Contributing](../CONTRIBUTING.md)

The north star is feature parity with Burp Suite Community Edition's manual
toolkit. See [Burp Community parity](burp-parity.md) for the capability matrix,
definition of done, and deliberate deviations.

**Current focus: Milestone 3, the daily workbench.** Proxy core and HTTPS
milestones are complete. Search, HTTP/HTTPS Intruder, Repeater, Sequencer,
Decoder, Comparer, WebSocket capture/history, and the first local
project-persistence workflow are available. The remaining M3 work is
paging/search over all saved history, project workflow polish, and project-wide
retention controls. The temporary in-memory project currently has no aggregate
capture-retention cap, and saved files have no automatic disk cap. HTTP/2,
WebSocket history, Intruder, and Sequencer arrived ahead of the protocol
milestone. This is a sequence of capabilities, not a schedule or release
promise.

## Milestones

- [x] **Milestone 0 — Proof of concept:** benchmark idle memory, history
  rendering, and large-response viewing on modest Linux hardware.

  *Complete: on a 4 vCPU / 5.3 GiB Kali arm64 VM at 200% scaling, a release
  build used 92.1 MiB RSS idle, 98.9 MiB with 2,000 exchanges, and at most
  108.6 MiB peak while viewing 8 MiB text and binary responses capped at
  1 MiB. The status bar now reports allocator heap in use (5.8 MiB with 2,000
  exchanges) with a peak and rolling graph. Method, breakdown, and full
  results: [Performance and memory](performance.md).*

- [x] **Milestone 1 — Proxy core:** reliable HTTP/1.1 capture and forwarding,
  request/response interception with edit, forward, and drop, explicit scope
  configuration, and match/replace rules.

  *Complete: plain-HTTP requests and responses can be held, edited, forwarded,
  or dropped through a bounded, backpressured interception queue. Scope rules
  can filter the workspace and optionally block out-of-scope HTTP requests and
  `CONNECT` tunnels with a `403`. Match/replace rules run after interception on
  heads and buffered UTF-8 bodies. Requests and responses larger than the
  interception limit stay header-only. HTTPS interception arrived in Milestone 2.*

- [x] **Milestone 2 — HTTPS:** local CA generation, TLS interception,
  documented browser trust setup, and pass-through controls.

  *Complete: a locally generated ECDSA certificate authority is created on
  first launch, persisted in a stable per-user directory, and reused across
  updates. Decryption is on by default and can be turned off for a run.
  The proxy serves the certificate from a local download page; trust is still
  installed and removed manually in the browser, and Perseid never touches a
  system or browser trust store. Pass-through hosts stay opaque, scope can still
  block tunnels, and decrypted exchanges flow through the same interception,
  match/replace, and bounded-capture pipeline as plain HTTP. See
  [HTTPS interception](tls.md).*

- [ ] **Milestone 3 — Daily workbench:** redb-backed persistent project history
  and supported workspace state, Search, Repeater, Decoder, and Comparer.

  *Status: projects start in temporary memory by default. Save As, Open, and Save
  Copy use a single redb `.perseid` file; captured HTTP/WebSocket records and
  supported workspace inputs are written automatically. On open, the UI loads
  only a bounded newest-first history snapshot; older records remain on disk but
  are not yet pageable or searchable. Temporary storage has no aggregate capture
  eviction cap. There are no recent-projects or
  save/discard-on-quit prompts, and schema migrations are not implemented.
  Search covers the loaded HTTP-history heads and decoded bodies with case,
  regex, negative-match, in-scope, and live-update filters. Repeater persists
  request tabs/drafts but not response history; Intruder persists its request
  and payload configuration but not results. Decoder panel contents and
  Comparer inputs persist, while Inspector notes and computed results do not.
  WebSocket message interception/replay remains unimplemented. Decoder still
  lacks SKEIN/TupleHash/OID aliases and Send to Decoder.*

- [ ] **Milestone 4 — Protocols and testing tools:** WebSocket proxying with
  history, Sequencer, and unthrottled Intruder-style fuzzing with explicit scope
  and user-controlled concurrency.

  *Status: HTTP/2, WebSocket capture/history, Intruder-style fuzzing, and
  Sequencer landed ahead of this milestone. WebSockets support plain `ws://`,
  decrypted `wss://`, complete text/binary message reassembly, masking removal,
  a bounded UI history snapshot, and Pretty/Raw/Hex inspection. WebSocket
  message interception, editing, and replay remain. Intruder supports `$$`
  positions, editable payload sets, Sniper/Battering ram/Pitchfork/Cluster bomb
  attacks, up to 32 concurrent requests, live result inspection, and
  enforced-scope checks for every generated request. Payload processing rules
  and saved attacks remain. Sequencer supports manual and live regex sampling,
  character- and bit-level tests, effective-entropy results, and per-analysis
  settings, with bounded in-memory samples.*

- [ ] **Milestone 5 — Parity hardening:** packaging, crash recovery, safe
  handling of sensitive traffic, accessibility, performance benchmarks, and a
  documented parity audit.

  *Status: tests, rustfmt, clippy, CI, and the M0 memory baseline are in
  place, and cross-platform release packages are published. Clean-install
  validation, crash recovery, accessibility, and broader parity benchmarks
  remain.*

- [ ] **After parity:** native extension point, passive and guided checks,
  import/export, integrations, and additional protocol coverage.

Priorities may change based on real-world use and measurements. Safety and
scope defaults are not deferred: each milestone lands with the relevant
protections documented.
