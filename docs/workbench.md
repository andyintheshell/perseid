# Workbench guide

[Documentation](README.md) · [Getting started](getting-started.md) ·
[HTTPS setup](tls.md) · [Troubleshooting](troubleshooting.md)

The everyday loop is **capture → inspect → edit → replay**. Use an external
browser for normal interaction and Perseid to understand and change the HTTP
messages. Only test local labs or systems you are explicitly authorized to test.

## Workspace at a glance

| Tab | Use it for |
| --- | --- |
| **Target** | A site map derived from history and a shared scope list. |
| **Proxy** | HTTP history, held interception messages, match/replace rules, and proxy/TLS settings. |
| **Intruder** | Editable requests with `$$` payload positions, payload sets, and direct attacks. |
| **Repeater** | Editable HTTP/HTTPS requests sent directly to their targets. |
| **Decoder** | Local encoding, decoding, hashing, and layered transformations. |
| **Comparer** | Word- or byte-level diffs between any two requests, responses, or loaded messages. |

See the [screenshot gallery](screenshots.md) for each tab. The app opens on
**Proxy > HTTP history**.

## Target and scope

**Target > Site map** groups captured requests by host and path, showing the
latest exchange for each path. It is built from retained history, not from a
crawler. Evicted or cleared history therefore changes the map.

In **Target > Scope**, add a bare host, a `host:port` authority, or a full URL.
Matching includes subdomains and ignores ports; URL schemes and paths do not
make scope a path-level or port-level allowlist.

Two independent controls use that list:

- **Filter views to the scope** hides out-of-scope history/site-map entries.
  It does not stop network traffic.
- **Block out-of-scope traffic** rejects out-of-scope proxy requests and
  `CONNECT` tunnels with `403`. This is opt-in and off by default. After
  interception or rewriting, edited request targets are checked again.

**An empty scope allows every host**, even when blocking is selected.
Opaque HTTPS tunnels can be checked only by their `CONNECT` target; decrypted
requests can be checked individually.

> [!WARNING]
> Scope enforcement applies to the proxy, **not Repeater**. Intruder also checks
> enforced scope for every generated request, but does not use the proxy's
> interception or match/replace pipeline. Always review the target before a
> direct send. Scope is a guardrail, not authorization to test.

## Capture and history

Configure a browser or client to proxy through `127.0.0.1:8080` for HTTP and
HTTPS. The [local walkthrough](getting-started.md#try-it-with-a-local-target)
is a quick way to verify the path without a browser or CA trust.

- **HTTP history** has one row per HTTP exchange, including decrypted HTTPS.
  `CONNECT` transport tunnels do not get their own rows. Right-click a row to
  send its request or response to Repeater, Intruder, or Comparer.
- The text filter matches method, host, path, URI, and status—not response-body
  contents. Scope filtering can narrow the view further.
- **Capture** pauses recording of new exchanges, not forwarding or interception.
- History is bounded by both exchange count and retained-body bytes. The oldest
  entries are evicted when a bound is exceeded.
- The status bar shows in-flight requests, held messages, dropped capture events,
  rejected connections, and evictions. A dropped-event warning means the history
  may be incomplete even if the client received its response.

Click the **Memory** indicator for the current and peak allocator heap and a
rolling graph. This is not process RSS or a hard memory ceiling; see
[Performance and memory](performance.md).

## Inspect messages

Select a history row to open the request and response side by side. Drag the
divider to resize the panes.

| View | Representation |
| --- | --- |
| **Pretty** | JSON indentation, HTML/XML layout, and one field per URL-encoded form entry; otherwise readable text. |
| **Raw** | Reconstructed message text with line numbers and syntax highlighting. Binary/control bytes may be displayed lossily. |
| **Hex** | A bounded byte-offset dump. In history and Repeater, this uses the same decoded body representation as the text views. |

History and Repeater views decode gzip, deflate, and Brotli bodies for
inspection. Displayed `Content-Encoding` is removed after successful decoding;
`Content-Length` is adjusted only when the complete decoded length is known.
Unsupported or invalid encodings fall back to captured wire bytes. The proxy
removes unsupported outgoing `Accept-Encoding` codings such as `zstd`.

These views are **not byte-for-byte wire dumps**: headers are reconstructed,
header names are title-cased, HTTP/2 uses a synthetic textual head, compressed
bodies are decoded, and previews may be truncated. Copying a view copies its
displayed representation. Viewing a message does not itself rewrite the body
being forwarded.

Pretty body formatting has independent **256 KiB input and output limits** and
a **128-level nesting limit**. If formatting would exceed a budget, the view
falls back to bounded Raw text rather than expanding indentation without limit.
These body-formatting limits are separate from capture and decoded-preview
limits and do not make the whole editor or process a fixed-size allocation.

## Intercept and edit

1. Open **Proxy > Intercept** and click **Intercept is off** to arm request
   holding. Tick **Responses** if you also want to hold server responses.
2. Send a request from your testing browser or client. A held message appears
   in the table with its time, direction, method, URL, status, and length.
3. Select a row and switch to **Raw** to edit its head or eligible text body.
   **Pretty** and **Hex** are read-only previews in this editor.
4. Choose **Forward** to send the selected message, or **Drop** to reject it.

Use `Shift` to select a range, or `Ctrl`/`Cmd` to toggle rows. Each message keeps
its draft while you switch selections. **Forward all** and **Drop all** act on
the entire held list, not just the active row.

Only bodies with a known size within `--intercept-limit` (1 MiB by default)
are buffered. Large, streaming, and binary bodies remain header-editable only;
their body bytes pass through unchanged. HTTP/2 requests use an origin-form
request line and a synthetic `Host` header for `:authority`; HTTP/2 responses
have no reason phrase.

Forwarding applies match/replace rules and rechecks scope. Dropped exchanges
remain in history without a response status. Turning interception off **does
not release already held messages**: forward or drop them explicitly. Closing
the app aborts unresolved exchanges.

The interception editor works on the held body bytes, not a decompressed copy.
Do not assume a compressed or binary body is safely editable just because its
history preview was readable.

## Match/replace rules

Open **Proxy > Match/replace**. A rule chooses request or response, head/body/all,
a search pattern, a replacement, and literal or Rust-regex matching. Regex
replacements support capture references such as `$1`.

Rules run in list order, after interception and before forwarding. Body rules
apply only to buffered UTF-8 bodies, not streaming, oversized, or binary data.
Empty patterns and disabled rules are ignored. Invalid regexes stay visible
with an error and are excluded from the live policy; valid rules and toggles
remain active. Rules are not saved between launches.

## Intruder

Intruder automates variations of one request. It uses the same direct
HTTP/HTTPS client as Repeater, so an attack works with the proxy stopped or in
`--no-proxy` mode. It does **not** pass through interception, match/replace, or
Proxy history.

1. Right-click a history exchange and choose **Send to Intruder**, or open
   Intruder and paste or edit a raw request. Review the **Target** scheme and
   host.
2. Wrap each value to fuzz in `$$`, for example
   `GET /product?id=$$1$$ HTTP/1.1`. Select text and click **Add $$** to wrap it,
   or place the cursor and use **Add $$** for an empty position (`$$$$`).
   **Clear $$** removes every marker. **Auto $$** marks query values,
   `application/x-www-form-urlencoded` body values, and cookie values.
3. Edit the payload list on the right, one value per line. Blank lines are
   ignored. **Deduplicate** keeps the first occurrence. Sniper and Battering ram
   use one shared set; Pitchfork and Cluster bomb use one set per position.
4. Choose an attack and click **Start attack** or press `Ctrl`+`Enter`:

   | Attack | Positions per request | Request count |
   | --- | --- | --- |
   | **Sniper** | Each position in turn; other positions keep their base values | positions × payloads |
   | **Battering ram** | Every position receives the same payload | payloads |
   | **Pitchfork** | One payload from each set, advancing together | shortest set |
   | **Cluster bomb** | Every combination of one payload per position | product of set sizes |

5. Watch the result table. Select a row to inspect the generated request and
   response in Pretty/Raw/Hex. The header shows received/total counts and, for
   a finished attack, elapsed time.

Markers are literal delimiters, not part of the request: they are removed when
the request is generated. An unclosed `$$` is an error. Sniper uses the text
inside each marker as that position's base value. The other attack types replace
every marked position.

`$$` can appear anywhere in the request, including headers and bodies. Keep the
**Update Host header** option on when the target bar changes so the request stays
consistent; turning it off allows virtual-host testing.

**Scope:** when **Block out-of-scope traffic** is enabled, **Start attack** first
checks the effective target (an absolute-form request line overrides the target
bar) against the scope. Every generated request is checked again before it is
sent, because a payload can change an absolute-form target. Out-of-scope
requests produce failed result rows and do not reach the target. Intruder still
does not use proxy interception or match/replace rules, so confirm the target
and template before starting.

**Limits:** concurrency is 1–32 and one attack runs at a time. **Stop attack**
stops scheduling new requests; requests already in flight finish. The UI retains
up to 1,000 result rows and 64 MiB of result data, evicting the oldest rows. The
engine queues at most 64 unread results (and 64 MiB) before applying
backpressure. Generated requests are limited to 8 MiB with 64 KiB heads, each
request has a 60-second timeout, and response bodies use the same capture limit
as the rest of the app. Results are not saved between launches.

## Repeater

1. Right-click a history exchange and choose **Send to Repeater**, or open
   Repeater and use `+` for a blank tab.
2. Review the **Target** scheme/host, raw request head, and body.
3. Change one thing and click **Send**, or press `Ctrl`+`Enter` (`Cmd`+`Enter`
   on macOS).
4. Inspect the status, timing, size, and Pretty/Raw/Hex response. Keep separate
   tabs for variants you want to compare manually.

For an origin-form line such as `GET /admin HTTP/1.1`, the target bar selects
the connection destination. An absolute-form line such as
`GET https://lab.test/admin HTTP/1.1` overrides it. A non-empty edited `Host`
header is preserved, allowing virtual-host testing against a different
connection destination; a missing or blank header is filled from the target.

Repeater has its own direct HTTP/HTTPS client. It works with the proxy stopped
or in `--no-proxy` mode and negotiates HTTP/2 over TLS when the origin offers it.
It verifies origin certificates against platform trust anchors, falling back
to bundled public roots if none are available.

**Repeater bypasses interception, match/replace, and proxy scope enforcement.**
Its exchanges do not appear in Proxy history. It also has no browser cookie jar
or automatic session refresh; review copied cookies and tokens before sending.

Captured history seeds use the decoded, bounded message representation, so
compressed, binary, or large bodies can be lossy. Review seed warnings before
sending. Responses retain up to `--capture-limit` body bytes and report
truncation or incomplete transfers. Requests have a 60-second end-to-end timeout,
with up to eight concurrent sends. Saved tabs and WebSocket replay are absent.

Admission is bounded across **queued requests, active sends, and unread results
together**: at most **32 pending operations** and **64 MiB of retained-data
reservations**. A send reserves space for its response before network work
starts; the reservation is released when the UI consumes the result. These are
fixed engine limits, not additional CLI flags, and do not cap all retained
editor or tab state.

Requests are limited to **8 MiB of retained request buffers**, including header
and string storage, with **64 KiB request and response heads**. A larger
`--capture-limit` does not raise those request/head limits. Repeater rejects a
configured response capture limit at startup if it cannot fit a maximum-sized
request and its reserved response within the pending-byte budget.

When admission is full, **Send reports a retryable budget error without stopping
other sends**. The UI normally drains responses automatically; retry after
responses have been processed. Library consumers must drain `Repeater::try_recv`
to release admission for unread results. See
[budget troubleshooting](troubleshooting.md#repeater-reports-a-budget-or-size-error).

## Decoder

Paste or type into the first panel, then choose **Decode as**, **Encode as**,
**Hash**, or **Smart decode**. Every operation adds a panel; transforming an
earlier panel replaces the chain below it. Select text to transform only that
part, preserving the surrounding prefix and suffix. With no selection, the
whole panel is used.

| Menu entry | Behavior |
| --- | --- |
| **Plain** | Copy the input or selection without changing it. |
| **URL** | Decode `%xx` escapes, or percent-encode every byte using lowercase hex. |
| **HTML** | Decode named/numeric entities, or encode every character as a numeric entity. |
| **Base64** | Decode standard or URL-safe alphabets, with or without padding; encode as standard padded Base64. |
| **ASCII hex** | Byte-oriented hexadecimal; decoding accepts whitespace and requires an even digit count. |
| **Hex / Octal / Binary** | Convert numeric digit runs, not arbitrary bytes. For example, numeric Hex encoding changes decimal `1234` to `4d2`. |
| **Gzip** | Compress or decompress, with a decompressed-output limit. |

Use **ASCII hex**, not numeric Hex, when you mean a byte encoding. HTML encoding
of non-UTF-8 input is a textual per-byte representation; high bytes do not
round-trip byte-for-byte through Unicode entities.

**Smart decode** detects gzip, URL, HTML, Base64, and ASCII hex layers. It adds
one panel per layer and stops after 12 layers, a cycle, or no recognizable
encoding. Numeric conversions are excluded so plain numbers are not
reinterpreted automatically. No match produces a notice rather than an empty
result panel.

<details>
<summary>Supported hashes (31)</summary>

BLAKE2B-160/256/384/512, BLAKE2S-128/160/224/256, BLAKE3,
Keccak-224/256/384/512, MD2, MD4, MD5, RIPEMD-160, SHA-1,
SHA-224/256/384/512, SHA-512/224, SHA-512/256, SHA3-224/256/384/512,
SM3, Tiger, and Whirlpool. Output is lowercase hexadecimal. SKEIN, TupleHash,
and provider-specific OID aliases are not implemented.

</details>

Text panels are editable; Hex panels are read-only and preview at most 64 KiB.
Binary results open in Hex. Switching to Text gives a lossy preview, and editing
it replaces the bytes with UTF-8 text. **Copy** and **Copy output** export all
retained data as text or hex, not just the visible hex preview.

Manual encode/decode/hash transforms accept at most 8 MiB of input and produce
at most 16 MiB of output, including preserved selection prefixes/suffixes. There
is no **Send to Decoder** action from message editors yet, and chains are not saved.

## Comparer

Comparer finds small changes between two messages. It uses one shared item
collection: both **Select item 1** and **Select item 2** show the same rows, so
select a different row in each table to choose the pair.

- Right-click a history row and choose **Send request to Comparer** to add the
  request as item 1, or **Send response to Comparer** to add the response as
  item 2. The item is appended to the shared collection and selected in that
  table.
- **Paste** reads text from the clipboard; **Load** opens an in-app path dialog
  that also accepts a dropped file. **Remove** deletes the item selected in the
  table you last clicked, and **Clear** empties the collection.
- Choose **Words** or **Bytes** under **Compare selected**. Word mode tokenizes
  on ASCII whitespace and highlights changed tokens. Byte mode compares raw
  bytes; it is useful when whitespace or encoding differences matter.

The result window has a Text/Hex choice for each pane, Previous/Next difference
buttons, length labels, and a color key for modified, deleted, and added data.
Tick **Sync views** to mirror vertical scrolling and wheel input, or use the
**Sync** button in either pane header. Text is a display-safe rendering of the
bytes: control and invalid bytes appear as `·`, CRLF is shown as LF, and the
panes are virtualized by line rather than laid out as one large document.

Comparer keeps up to **200 items** of **8 MiB each** in memory. A word compare
accepts up to **2 MiB and 250,000 words per item**; a byte compare accepts up to
**512 KiB per item** because Myers' algorithm is more expensive on bytes.
Oversized inputs are rejected with a message, and both modes give the diff
algorithm a 500 ms deadline. Items and results are not saved between launches.

## Keyboard shortcuts

Use `Cmd` in place of `Ctrl` on macOS unless noted.

| Action | Shortcut |
| --- | --- |
| Target / Proxy / Intruder / Repeater / Decoder / Comparer | `Ctrl`+`Shift`+`T` / `P` / `I` / `R` / `D` / `C` |
| Focus the history filter | `Ctrl`+`F` |
| Leave the filter | `Esc` |
| Move through history, when not editing text | `j` / `k` or up/down arrows |
| First / last visible history row | `Home` / `End` |
| Start or stop an Intruder attack | `Ctrl`+`Enter` |
| Send the active Repeater request | `Ctrl`+`Enter` |
| Zoom in / out / reset | `Ctrl`+`+` / `Ctrl`+`-` / `Ctrl`+`0` |

## Limits and persistence

| Setting or surface | Current behavior |
| --- | --- |
| Captured bodies | 1 MiB retained per request/response by default; forwarding can stream larger bodies. |
| Interception buffering | Known-size bodies up to 1 MiB by default; other bodies are header-only. |
| History | 2,000 exchanges and half of physical RAM in retained body bytes by default; 256 MiB fallback, with a 64 MiB floor when RAM is reported. |
| Inspection previews | Up to 256 KiB text/decoded body and 64 KiB Hex input; these are preview limits, not whole-process memory bounds. |
| Pretty body formatting | Independent 256 KiB input/output and 128-level nesting budgets; falls back to bounded Raw text on excess. |
| Repeater work | Eight active sends; at most 32 pending operations and 64 MiB of reservations across queued, active, and unread work. |
| Repeater request/head size | 8 MiB of retained request buffers; 64 KiB heads. The response capture limit must also fit the pending-work budget. |
| Intruder work | Up to 32 simultaneous requests and one attack at a time; the engine queues up to 64 unread results or 64 MiB. |
| Intruder results | At most 1,000 rows and 64 MiB retained in the UI; oldest rows are evicted. Generated requests are limited to 8 MiB with 64 KiB heads and a 60-second timeout. |
| Comparer work | 200 items of up to 8 MiB each; word diffs accept 2 MiB and 250,000 words per item; byte diffs accept 512 KiB per item. Both diff modes use a 500 ms deadline. |
| Saved state | No persisted history, Intruder results, Repeater tabs, Decoder chains, Comparer items, scope, or rewrite rules. The local CA is persisted. |
| Missing tools/protocols | No Sequencer, WebSocket tooling, or HTTP/3/QUIC support. |

See [CLI options](getting-started.md#options) to change retention limits and
[Safety and privacy](safety.md) for the data-handling implications. Larger
budgets allow more sensitive data to remain in memory; they do not improve
protocol support or make an unsupported editing operation safe.
One exchange can remain even if it alone exceeds the history-body byte budget.
