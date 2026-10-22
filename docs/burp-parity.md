# Burp Suite Community parity

[Documentation](README.md) · [Workbench](workbench.md) · [Roadmap](roadmap.md)

**Goal:** Perseid aims to be a practical alternative to Burp Suite Community
Edition (CE) for manual web security testing: the same capabilities, adapted
to a Rust-native, memory-bounded, keyboard-first tool.

This is a direction, not a claim. Several capabilities below are still planned.
For instructions on the implemented tools, use the [workbench guide](workbench.md);
this page is a capability reference, not a setup tutorial.

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
**Intruder**, **Repeater**, **Decoder**, and **Comparer**. Familiar navigation is
part of the goal; reproducing every control before its engine exists is not.
Sequencer remains planned and is not presented as a working tool.

Parity here means capability, not artificial restrictions. Burp CE rate-limits
Intruder to encourage upgrades; Perseid does not reproduce that limit. Scope and
concurrency remain explicit, user-controlled safety settings.

## Capability matrix

Status values: **Working** (usable today), **Partial** (foundation exists),
**Planned** (assigned to a milestone), **Deviation** (intentional difference),
**Beyond CE** (a planned Perseid extra), **Non-goal** (explicitly outside parity).

| Capability | Burp CE | Perseid | Status | Milestone |
| --- | --- | --- | --- | --- |
| Loopback HTTP/1.1 and HTTP/2 proxy and forwarding | Yes | Yes | Working | — |
| Intercept, edit, forward, or drop messages | Yes | HTTP/1.1 and HTTP/2 table with multi-selection, head and buffered-body editing, and opt-in response interception | Working | — |
| Scope / target allowlist | Yes | View filter plus opt-in HTTP/CONNECT blocking | Working | — |
| Target site map | Yes | Host/path tree from history | Partial | — |
| Match/replace rules | Yes | Head and buffered UTF-8 body rules, literal or regex | Working | — |
| Compressed body inspection | Yes | Bounded gzip, deflate, and Brotli decoding in Pretty, Raw, and Hex; wire bytes still forwarded | Working | — |
| Session history | Yes | In-memory, bounded | Partial | M3 |
| History filtering | Yes | Text filter plus scope view filter | Partial | M3 |
| Memory usage display | Yes | Allocator memory in use and peak, rolling graph | Working | — |
| HTTPS interception with a local CA | Yes | Automatic local CA, HTTPS interception on by default, local certificate page, pass-through hosts, manual trust setup | Working | — |
| HTTP/2 proxying | Yes | h2 over TLS via ALPN, h2c prior knowledge on the listener | Working | — |
| WebSocket proxy and frame history | Yes | Plain `ws://` and decrypted `wss://` message history with text/binary reassembly, masking removal, filtering, and Pretty/Raw/Hex detail; no WebSocket message interception, replay, or editing | Partial | M4 (landed early) |
| Repeater | Yes | Editable HTTP/HTTPS request tabs seeded from history, target/virtual-host control, direct replay, and Pretty/Raw/Hex responses; no WebSocket replay or saved tabs yet | Partial | M3 (HTTP/HTTPS); WS in M4 |
| Decoder | Yes | Stacked Text/Hex panels; Plain, URL, HTML, Base64, ASCII hex, numeric Hex/Octal/Binary, and Gzip; 31 common hashes; Smart decode; selection-aware transforms | Partial | M3 |
| Comparer | Yes | One shared item list seeded from history, paste, or file; bounded word/byte diffs; paired Text/Hex panes, difference navigation, and synchronized scrolling; in-memory only | Working | M3 |
| Sequencer | Yes | No | Planned | M4 |
| Intruder-style request fuzzing | Yes (rate-limited demo) | Editable `$$` positions, payload sets, Sniper/Battering ram/Pitchfork/Cluster bomb attacks, up to 32 concurrent requests, and Pretty/Raw/Hex results; one attack at a time, no payload processing or saved attacks | Working | M4 (landed early) |
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
store. Hosts on the pass-through list stay opaque. Inside intercepted tunnels
Perseid negotiates HTTP/2 when the client offers it through ALPN and falls back
to HTTP/1.1 otherwise; HTTPS origins are offered h2 the same way. HTTP history
follows Burp's model: it lists HTTP requests and responses, including decrypted
HTTPS requests, but not the `CONNECT` transport tunnels that carry them.
WebSocket traffic has a separate message-level history. Successful handshakes
provide the URL, TLS state, listener port, and WebSocket ID for text and binary
messages; control frames and incomplete fragments are omitted, and client
masking is removed. Compression extensions are stripped from the forwarded
handshake so captured payloads remain readable. See
[HTTPS interception](tls.md) for the trust and removal workflow.
Interception buffers bodies up to `--intercept-limit`; larger or streaming
bodies are held for header edits, and body rewrite rules apply only to buffered
UTF-8 bodies. The message editor uses Burp's Pretty, Raw, and Hex tabs. Pretty
indents JSON and HTML/XML and lays out URL-encoded forms one field per line;
Pretty and Raw add line numbers, syntax highlighting, and Burp-style title-cased
header names. All three views decode gzip, deflate, and Brotli bodies, mirroring
Burp's default unpacking. The request and response panes use Burp's horizontal
side-by-side layout. Like Burp, the proxy removes unsupported `Accept-Encoding`
codings before forwarding. Interception and body rules still operate on the wire
representation, and forwarding keeps the original response encoding. Scope
enforcement is opt-in, and an empty scope allows every host.

Intruder follows Burp's marker-and-payload model. A raw request can be seeded
from history or edited in place, values are wrapped in `$$`, and one payload
list is edited per position where the attack type needs them. Sniper tests each
position in turn while leaving the others at their marked base value; Battering
ram inserts one payload into every position; Pitchfork advances one set per
position; Cluster bomb tests every combination. Up to 32 requests run
concurrently, and one attack runs at a time. Results are listed by sequence with
their payloads, status, length, and duration, and each request/response opens in
Pretty/Raw/Hex. The direct client deliberately bypasses interception,
match/replace, and Proxy history. When scope enforcement is on, starting an
attack checks the effective first target and refuses an out-of-scope host; every
generated request is checked again before sending, because payloads can change
an absolute-form request line. Payload processing rules, saved attacks, and an
automatic scan of findings are not implemented.

Repeater follows the same manual-first shape as Burp: seed an editable request
from history with **Send to Repeater**, keep several request tabs, set the
target scheme and authority, send with the button or `Ctrl`+`Enter`, and inspect
the response in Pretty, Raw, and Hex. The target bar controls the connection
for origin-form request lines, while an absolute-form request line overrides it
and a non-empty `Host` header is preserved for virtual-host testing. Repeater
uses an independent direct HTTP/HTTPS client and runtime, not the local proxy,
so it remains usable when the proxy is stopped. It deliberately bypasses
interception, match/replace rules, and scope enforcement, and its exchanges do
not appear in Proxy history. Responses are bounded by `--capture-limit`.
WebSocket message interception/replay and saved tabs remain part of the M3/M4
work.

Decoder follows Burp's stacked-panel model. Every operation appends an
editable Text/Hex panel, and applying an operation to an earlier panel replaces
the downstream chain. The Decode as and Encode as menus contain Burp's nine
entries: Plain, URL, HTML, Base64, ASCII hex, Hex, Octal, Binary, and Gzip.
Hex, Octal, and Binary intentionally reproduce Burp's numeric digit-run
conversions; ASCII hex is the byte-oriented hexadecimal codec. Transforms apply
to the selected text when there is a selection and preserve the surrounding
prefix and suffix. Smart decode recognizes gzip, URL, HTML, Base64, and ASCII
hex, adds one panel per layer, and stops after 12 layers or a cycle. Hash
support covers 31 common algorithms. Remaining gaps are the long tail of
provider-specific hashes (SKEIN, TupleHash, and OID aliases), a Send to Decoder
action from message editors, and persistence of the panel chain. Hex panels are
read-only and preview at most 64 KiB, while Copy exports all retained bytes;
each transform is capped at 8 MiB of input and 16 MiB of output.

Comparer follows Burp's two-item model. Both selection tables show the same
shared item list, so a message added once can be compared against any other
row. Items can be sent from the Proxy history context menu (request to item 1,
response to item 2), pasted from the clipboard, or loaded from a file. Word mode
tokenizes ASCII whitespace and compares those tokens; byte mode compares the raw
bytes. The result window offers independent Text/Hex views, modified/deleted/
added highlighting, previous/next difference navigation, and optional
synchronized scrolling. Comparisons are bounded: up to 200 in-memory items of
8 MiB each, 2 MiB and 250,000 words per item for word mode, 512 KiB per item for
byte mode, and a 500 ms diff deadline. Items, open results, and view settings
are not persisted.

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
  Intruder. Perseid runs attacks at the concurrency the user configures, bounded
  by scope checks and resource limits rather than an upgrade restriction.
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
| **M3 — Daily workbench** | Persistent searchable history, Repeater, Decoder, and Comparer. *(Repeater, Decoder, and Comparer are complete; persistence remains.)* |
| **M4 — Protocols and testing tools** | WebSocket proxying and history, Sequencer, and unthrottled Intruder-style fuzzing with explicit scope and user-controlled concurrency. HTTP/2, WebSocket capture/history, and Intruder landed ahead of this milestone; WebSocket message editing/replay and Sequencer remain. |
| **M5 — Parity hardening** | Packaging, crash recovery, accessibility, published benchmarks, and a documented parity audit. |

After M5, Perseid can claim CE manual-toolkit parity subject to the
deviations above. Native extensions, passive/guided checks, and other
Perseid-specific ideas continue after parity; see the [Roadmap](roadmap.md).

Progress is tracked by updating this matrix at the end of each milestone.
