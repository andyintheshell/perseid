# Burp Suite Community Edition parity

[Documentation](README.md) · [Workbench](workbench.md) · [Roadmap](roadmap.md)

Perseid aims to cover the core **manual web-testing workflows** in Burp Suite
Community Edition (CE), with a Rust-native desktop app and explicit body,
operation, and UI-cache limits. Temporary project storage has configurable
aggregate count and retained-body limits; saved projects offer opt-in logical
limits but no physical disk-size cap. This is a progress snapshot, **not a claim of
full feature or product parity**.

Burp CE's built-in manual toolkit is the comparison point: Proxy, Repeater,
Decoder, Sequencer, Comparer, Intruder, and WebSocket history. See
[PortSwigger's CE feature list](https://portswigger.net/burp/communitydownload).
The goal is an equivalent documented workflow—not an identical UI, internal
implementation, or compatibility with Burp extensions.

**Status key:** ✅ Working · 🟡 Partial · ➕ Beyond CE · 🛠 Planned ·
↔ Deliberate difference · ⛔ Non-goal.

## Status at a glance

| ✅ Working | 🟡 Partial | 🛠 Planned |
| --- | --- | --- |
| HTTP/HTTPS proxy and intercept | Bounded history view and site map | Paging through all saved history |
| Scope and match/replace | Repeater, Decoder, and Sequencer | WebSocket edit and replay |
| Comparer and Intruder | WebSocket capture/history; project persistence | Migrations |
| Memory display | — | — |

**➕ Perseid extra:** search across stored HTTP/WebSocket history and current
Repeater/Intruder results. **↔ Deliberate differences:** no
bundled browser or Burp BApp compatibility. **⛔ Non-goals:** automated
scanning/crawling and Collaborator/OAST. Sequencer provides a bounded,
expression-based sampling and analysis workflow; it is not a byte-for-byte
reimplementation of Burp's test suite.

![Perseid HTTP history with request, response, and Inspector panes](images/perseid-main.png)

Browse the [screenshot gallery](screenshots.md) for Search, Intruder, Repeater,
Decoder, Comparer, and Sequencer.

## Capability matrix

| Capability | Status | What works today / what is missing |
| --- | --- | --- |
| **HTTP(S) proxy** | ✅ Working | HTTP/1.1 and HTTP/2 forwarding; HTTPS interception with a local CA, manual browser trust, and per-host pass-through. Perseid never installs trust automatically. |
| **Intercept and edit** | ✅ Working | Hold, edit, forward, or drop HTTP/1.1 and HTTP/2 requests; response interception is opt-in. Buffered bodies can be edited; streaming or oversized bodies are header-only. |
| **Message inspection** | ✅ Working | Pretty, Raw, and Hex views; bounded gzip, deflate, and Brotli decoding; parsed Inspector and per-exchange Notes. |
| **Scope and rewrite rules** | ✅ Working | Filter views by scope; optionally block out-of-scope proxy requests and `CONNECT` tunnels; recheck edited targets. Match/replace supports literal or regex rules on message heads and buffered UTF-8 bodies. |
| **Target site map** | 🟡 Partial | Burp-style URL view: host/folder/resource tree, per-node Contents table with Host, Method, URL, Params, Status, Length, MIME, Title, Notes, and Time; sorting, search, context-menu sends, and Burp's default display filters (not found, CSS, images, binary, 4xx, empty folders). Pause freezes the derived view. Derived from the loaded history snapshot; not a crawler. Crawl paths, scanner Issues, parameter discovery, and unrequested-URL inference are unavailable. Cache eviction can hide entries without deleting saved records; clearing history removes the records. |
| **HTTP history** | 🟡 Partial | The live table uses a bounded newest-first snapshot; **Browse stored history…** pages through retained HTTP rows and loads details/notes on demand. Search scans the full retained store. Complete captures can be copied as curl. Selective per-item deletion remains unavailable. |
| **Search** | ➕ Beyond CE | Search all stored HTTP and WebSocket history plus current Repeater tabs and retained Intruder results, with case, regex, negative-match, in-scope, and live-update filters; stored scans run in the background with progress and stop when superseded. Results highlight in Pretty/Raw/Hex. Query, options, structured result rows, selection, and views persist with a project; old HTTP-only workspaces drop IDs missing from the loaded snapshot. |
| **WebSocket history** | 🟡 Partial | Capture complete text and binary messages for `ws://` and decrypted `wss://`; saved projects retain them, but the UI loads a bounded snapshot. No message interception, editing, or replay. |
| **Repeater** | 🟡 Partial | Editable HTTP/HTTPS request tabs, history seeding, target/virtual-host control, and response views. Persistent projects save request tabs and their last responses. No WebSocket replay. Direct sends bypass proxy scope, interception, and rewrite rules. |
| **Decoder** | 🟡 Partial | Stacked Text/Hex panels, common encode/decode operations, 46 hashes including SKEIN and TupleHash, standard digest OID aliases, Smart decode, and selection-aware transforms. Projects save panel contents, operation annotations, view, and selection. History can send full captured messages, and message panes can send selections to Decoder. Hex selections currently send displayed hexadecimal text rather than the selected binary bytes. |
| **Comparer** | ✅ Working | Shared item list, word- and byte-level differences, Text/Hex views, difference navigation, and synchronized scrolling. Persistent projects save input items, selections, and result-window state; the diff is recomputed on open. |
| **Intruder** | ✅ Working | `$$` positions; editable or file-loaded payloads; Sniper, Battering ram, Pitchfork, and Cluster bomb; up to 32 concurrent requests with scope checks. Projects save request/payload configuration and result rows. No payload processing. |
| **Sequencer** | 🟡 Partial | Manual token loading and live regex capture from HTTP history; character-level count/transition tests; bit-position and whole-stream tests; effective-entropy chart and per-analysis settings. Capture is expression-based rather than parameter-aware, and the statistical results are bounded approximations. |
| **Memory display** | ✅ Working | Current and peak allocator heap with a rolling graph; this is not total process RSS. |
| **Project persistence** | 🟡 Partial · ➕ Beyond CE | Temporary projects by default; Save As/Open/Save Copy use a local redb `.perseid` file with automatic writes. Saved retention is unlimited by default, with opt-in combined count/body limits; these are not physical file-size caps. Schema v1 migrates atomically to v2. New/Open prompt for unsaved temporary work; failed opens preserve the running proxy. Files are unencrypted. Clear history removes captures; delete a closed project file through the filesystem, not a dedicated UI. Local CA material and transient network activity are not stored. Burp CE itself does not include project files. |

For usage details, limits, and safety behavior, see the
[Workbench guide](workbench.md), [HTTPS guide](tls.md), and
[Safety and privacy](safety.md).

## Deliberate differences

- **External browser:** use a browser or HTTP client configured to proxy through
  Perseid. This avoids bundling Chromium and supports users' existing browser
  profiles and tools.
- **No BApp (Burp App Extension) compatibility:** Burp extensions are Java-based.
  A native extension point may follow core parity; Burp's extension ABI is not a
  target.
- **No artificial Intruder throttle:** Perseid does not copy Burp CE's demo rate
  limit. Concurrency is user-controlled and bounded, and scope checks are
  explicit.
- **Persistence is a Perseid extra:** local `.perseid` projects are implemented
  for captures and project-scoped workbench state, excluding CA material and
  transient network activity. They are not required to match Burp CE; see the
  matrix for current paging, migration, and saved-state limits.

## Milestones

| Milestone | Status |
| --- | --- |
| **M1 — Proxy core** | ✅ Complete: HTTP/1.1 proxy, interception, scope, and match/replace. |
| **M2 — HTTPS** | ✅ Complete: local CA lifecycle, TLS interception, and pass-through. |
| **M3 — Daily workbench** | 🟡 In progress: Repeater, Decoder, Comparer, Search across stored HTTP/WebSocket history plus current Repeater/Intruder results, and local project files are available; paging the history/site-map tables across the full saved history and project workflow polish remain. |
| **M4 — Protocols and testing tools** | 🟡 In progress: HTTP/2, WebSocket capture/history, Intruder, and Sequencer are available; WebSocket editing/replay remains. |
| **M5 — Parity hardening** | 🟡 In progress: release packages are published; clean-install testing, crash recovery, accessibility, and broader parity audits remain. |

## When does a capability count as parity?

A capability moves to **Working** when it has a usable workflow, practical
automated tests plus a manual check, and documented safety behavior (scope,
retention, and CA trust). Status and user documentation should be updated
together. Testing is limited to local labs or systems you are explicitly
authorized to assess.
