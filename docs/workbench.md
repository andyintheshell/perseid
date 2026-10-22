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
| **Intruder** | A preview of planned fuzzing work; there is no working engine yet. |
| **Repeater** | Editable HTTP/HTTPS requests sent directly to their targets. |
| **Decoder** | Local encoding, decoding, hashing, and layered transformations. |

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
> Scope enforcement applies to the proxy, **not Repeater**. Always review the
> target before a direct replay. Scope is a guardrail, not authorization to test.

## Capture and history

Configure a browser or client to proxy through `127.0.0.1:8080` for HTTP and
HTTPS. The [local walkthrough](getting-started.md#try-it-with-a-local-target)
is a quick way to verify the path without a browser or CA trust.

- **HTTP history** has one row per HTTP exchange, including decrypted HTTPS.
  `CONNECT` transport tunnels do not get their own rows.
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

## Repeater

1. Select a history exchange and choose **Send to Repeater**, or open Repeater
   and use `+` for a blank tab.
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

## Keyboard shortcuts

Use `Cmd` in place of `Ctrl` on macOS unless noted.

| Action | Shortcut |
| --- | --- |
| Target / Proxy / Intruder / Repeater / Decoder | `Ctrl`+`Shift`+`T` / `P` / `I` / `R` / `D` |
| Focus the history filter | `Ctrl`+`F` |
| Leave the filter | `Esc` |
| Move through history, when not editing text | `j` / `k` or up/down arrows |
| First / last visible history row | `Home` / `End` |
| Send the active Repeater request | `Ctrl`+`Enter` |
| Zoom in / out / reset | `Ctrl`+`+` / `Ctrl`+`-` / `Ctrl`+`0` |

## Limits and persistence

| Setting or surface | Current behavior |
| --- | --- |
| Captured bodies | 1 MiB retained per request/response by default; forwarding can stream larger bodies. |
| Interception buffering | Known-size bodies up to 1 MiB by default; other bodies are header-only. |
| History | 2,000 exchanges and half of physical RAM in retained body bytes by default; 256 MiB fallback, with a 64 MiB floor when RAM is reported. |
| Inspection previews | Up to 256 KiB text/decoded body and 64 KiB Hex input; these are preview limits, not whole-process memory bounds. |
| Saved state | No persisted history, Repeater tabs, Decoder chains, scope, or rewrite rules. The local CA is persisted. |
| Missing tools/protocols | No working Intruder engine, Comparer, Sequencer, WebSocket tooling, or HTTP/3/QUIC support. |

See [CLI options](getting-started.md#options) to change retention limits and
[Safety and privacy](safety.md) for the data-handling implications. Larger
budgets allow more sensitive data to remain in memory; they do not improve
protocol support or make an unsupported editing operation safe.
One exchange can remain even if it alone exceeds the history-body byte budget.
