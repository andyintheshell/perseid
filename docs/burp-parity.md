# Burp Suite Community Edition parity

[Documentation](README.md) · [Workbench](workbench.md) · [Roadmap](roadmap.md)

Perseid aims to cover the core **manual web-testing workflows** in Burp Suite
Community Edition (CE), with a Rust-native desktop app and explicit body,
operation, and UI-cache limits. Temporary project storage does not yet have an
aggregate memory-retention cap. This is a progress snapshot, **not a claim of
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
| HTTP/HTTPS proxy and intercept | Bounded history view and site map | Paging/search across all saved history |
| Scope and match/replace | Repeater, Decoder, and Sequencer | WebSocket edit and replay |
| Comparer and Intruder | WebSocket capture/history; project persistence | Migrations |
| Memory display | — | — |

**➕ Perseid extra:** searchable HTTP history. **↔ Deliberate differences:** no
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
| **Target site map** | 🟡 Partial | Host/path tree derived from the currently loaded history snapshot; it is not a crawler. Cache eviction can hide entries without deleting saved records; clearing history removes the records. |
| **HTTP history** | 🟡 Partial | Captures are stored in a persistent project when one is open; temporary redb storage retains captures in RAM without an aggregate eviction cap. The UI loads a bounded newest-first snapshot and does not yet page through older saved rows. Complete captures can be copied as curl. |
| **Search** | ➕ Beyond CE | Search the currently loaded HTTP-history request/response heads and decoded bodies with case, regex, negative-match, in-scope, and live-update filters; results highlight in Pretty/Raw/Hex. Older saved rows, WebSocket messages, and Repeater traffic are not included. |
| **WebSocket history** | 🟡 Partial | Capture complete text and binary messages for `ws://` and decrypted `wss://`; saved projects retain them, but the UI loads a bounded snapshot. No message interception, editing, or replay. |
| **Repeater** | 🟡 Partial | Editable HTTP/HTTPS request tabs, history seeding, target/virtual-host control, and response views. Persistent projects save request tabs and drafts, not response history. No WebSocket replay. Direct sends bypass proxy scope, interception, and rewrite rules. |
| **Decoder** | 🟡 Partial | Stacked Text/Hex panels, common encode/decode operations, 31 hashes, Smart decode, and selection-aware transforms. Projects save panel contents, but not operation annotations or selections. Provider-specific hash variants and Send to Decoder remain missing. |
| **Comparer** | ✅ Working | Shared item list, word- and byte-level differences, Text/Hex views, difference navigation, and synchronized scrolling. Persistent projects save input items; computed results and view state are rebuilt/reset on open. |
| **Intruder** | ✅ Working | `$$` positions; Sniper, Battering ram, Pitchfork, and Cluster bomb; up to 32 concurrent requests with scope checks. Projects save request/payload configuration, not result rows. No payload processing. |
| **Sequencer** | 🟡 Partial | Manual token loading and live regex capture from HTTP history; character-level count/transition tests; bit-position and whole-stream tests; effective-entropy chart and per-analysis settings. Capture is expression-based rather than parameter-aware, and the statistical results are bounded approximations. |
| **Memory display** | ✅ Working | Current and peak allocator heap with a rolling graph; this is not total process RSS. |
| **Project persistence** | 🟡 Partial · ➕ Beyond CE | Temporary projects by default; Save As/Open/Save Copy use one local redb `.perseid` file with automatic writes. Temporary RAM storage and persistent disk storage have no aggregate retention cap. No encryption or schema migrations; saved history is not yet pageable beyond the bounded UI snapshot. Burp CE itself does not include project files. |

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
  for supported captures and workbench inputs. They are not required to match
  Burp CE; see the matrix for current paging, migration, and saved-state limits.

## Milestones

| Milestone | Status |
| --- | --- |
| **M1 — Proxy core** | ✅ Complete: HTTP/1.1 proxy, interception, scope, and match/replace. |
| **M2 — HTTPS** | ✅ Complete: local CA lifecycle, TLS interception, and pass-through. |
| **M3 — Daily workbench** | 🟡 In progress: Repeater, Decoder, Comparer, Search, and local project files are available; paging/search across the full saved history and project workflow polish remain. |
| **M4 — Protocols and testing tools** | 🟡 In progress: HTTP/2, WebSocket capture/history, Intruder, and Sequencer are available; WebSocket editing/replay remains. |
| **M5 — Parity hardening** | 🟡 In progress: release packages are published; clean-install testing, crash recovery, accessibility, and broader parity audits remain. |

## When does a capability count as parity?

A capability moves to **Working** when it has a usable workflow, practical
automated tests plus a manual check, and documented safety behavior (scope,
retention, and CA trust). Status and user documentation should be updated
together. Testing is limited to local labs or systems you are explicitly
authorized to assess.
