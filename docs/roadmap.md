# Roadmap

[Documentation](README.md) · [Capability matrix](burp-parity.md) ·
[Contributing](../CONTRIBUTING.md)

The north star is feature parity with Burp Suite Community Edition's manual
toolkit. See [Burp Community parity](burp-parity.md) for the capability matrix,
definition of done, and deliberate deviations.

**Current focus: Milestone 3, the daily workbench.** Proxy core and HTTPS
milestones are complete. HTTP/HTTPS Intruder, Repeater, Decoder, and Comparer
work, while persistent history and richer search remain. HTTP/2 and Intruder
arrived ahead of the protocol milestone. This is a sequence of capabilities, not
a schedule or release promise.

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

- [ ] **Milestone 3 — Daily workbench:** SQLite-backed persistent history,
  search, Repeater, Decoder, and response diff (Comparer).

  *Status: in-memory history, basic filtering, editable HTTP/HTTPS Repeater,
  Decoder, and Comparer work. Repeater has request tabs, history seeding, target
  and virtual host control, direct replay, and Pretty/Raw/Hex responses;
  WebSocket replay and saved tabs are not implemented. Decoder has stacked
  Text/Hex panels, Burp-compatible encode/decode menus, 31 common hashes, Smart
  decode, and selection-aware transforms; SKEIN/TupleHash/OID aliases and Send
  to Decoder are still missing. Comparer has a shared item list, word and byte
  diffs, paired Text/Hex result panes, difference navigation, and synchronized
  scrolling, with bounded in-memory items. Persistence remains.*

- [ ] **Milestone 4 — Protocols and testing tools:** WebSocket proxying with
  history, Sequencer, and unthrottled Intruder-style fuzzing with explicit scope
  and user-controlled concurrency.

  *Status: HTTP/2 and Intruder-style fuzzing landed ahead of this milestone.
  Intruder supports `$$` positions, editable payload sets, Sniper/Battering
  ram/Pitchfork/Cluster bomb attacks, up to 32 concurrent requests, live result
  inspection, and enforced-scope checks for every generated request. Payload
  processing rules and saved attacks remain. WebSocket handling and Sequencer
  are still missing.*

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
