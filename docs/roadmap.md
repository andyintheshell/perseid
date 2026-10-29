# Roadmap

[Documentation](README.md) · [Capability matrix](burp-parity.md) ·
[Contributing](../CONTRIBUTING.md)

The north star is feature parity with Burp Suite Community Edition's manual
toolkit. See [Burp Community parity](burp-parity.md) for the capability matrix,
definition of done, and deliberate deviations.

**Current focus: Milestone 3, the daily workbench.** Proxy core and HTTPS
milestones are complete. Search, HTTP/HTTPS Intruder, Repeater, Sequencer,
Decoder, Comparer, WebSocket capture/history, and the first local
project-persistence workflow are available. Search scans all stored HTTP and
WebSocket history as well as current Repeater and Intruder results. The remaining
M3 follow-ups include a full-store WebSocket table view and
recent-project and quit prompts. Stored HTTP paging, lazy details/notes,
saved-project logical retention, atomic v1→v2 migration, and a full-store site map
with background indexing, bounded tree/Contents pages, and lazy details are available.
Temporary and opt-in saved retention use combined count/body limits; physical
file size is not capped. HTTP/2,
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
  project-scoped workbench state are written automatically. On open, the live
  history tables load only a bounded newest-first history snapshot; older
   records are reachable through the paged **Browse stored history…** HTTP
   browser with lazy details and notes, full-store Search, and the paged Target
   site map. The site map derives endpoints from all retained HTTP exchanges,
   using a disposable disk index rather than hydrating the complete store.
  Temporary storage evicts the oldest
  captures when it exceeds the configurable aggregate count or retained-body
  limits. New Project and Open Project ask Save/Discard/Cancel when the current
  temporary project has unsaved work; a recent-project list and quit prompt are
   not implemented. Saved projects now support opt-in combined record/body
   retention, defaulting to unlimited, and atomic v1→v2 schema migration.
   Retention is logical: it does not cap or compact physical files. Whole-project
   deletion uses the filesystem after switching away or quitting; files are
   unencrypted and deletion is not secure erasure.
  Search scans every stored HTTP exchange and WebSocket message in the
  background with case, regex, negative-match, in-scope, and live-update filters,
  and also matches current Repeater tabs and retained Intruder results.
  Project-scoped workbench state persists: UI selections/views, Inspector notes
  and sections, Repeater tabs and their last responses, Intruder configuration
  and result rows, Decoder panels, Comparer items and result-window state,
  Search state and structured result rows, Sequencer samples/settings, and TLS
  interception/pass-through hosts. The
  Comparer diff and Sequencer analysis are recomputed on open. Live capture,
  running attacks or sends, held interception messages, and CA material do not
  persist. Live WebSocket Repeater tabs are also session-only. History
  and message selections can be sent to Decoder; Hex selections currently send
  their displayed hexadecimal text rather than binary bytes.*

- [ ] **Milestone 4 — Protocols and testing tools:** WebSocket proxying with
  history, Sequencer, and unthrottled Intruder-style fuzzing with explicit scope
  and user-controlled concurrency.

  *Status: HTTP/2, WebSocket capture/history, Intruder-style fuzzing, and
  Sequencer landed ahead of this milestone. WebSockets support plain `ws://`,
  decrypted `wss://`, complete text/binary message reassembly, masking removal,
  a bounded UI history snapshot, and Pretty/Raw/Hex inspection. WebSocket
  message interception, editing, and live-connection replay are available. Intruder supports `$$`
  positions, editable and file-loaded payload sets, all four attack modes, up
  to 32 concurrent requests, live result inspection, and
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
