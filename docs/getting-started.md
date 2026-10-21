# Getting started

Perseid builds as a small Cargo workspace: `perseid-proxy` (the proxy library)
and `perseid` (the desktop application).

## Requirements

- Rust 1.95.0, as pinned in `rust-toolchain.toml` (edition 2024).
- A Linux desktop session with OpenGL support (X11 or Wayland).
- A browser or HTTP client that can be configured to use a proxy.

## Build and run

From the workspace root:

```sh
cargo run -p perseid
```

This starts the proxy on `127.0.0.1:8080` (loopback only) and opens the
Perseid window on **Proxy > HTTP history**. The toolbar shows whether the
proxy is running and lets you retry if the address is already in use.

### Options

```text
--listen <ADDR>           address for the proxy to listen on [default: 127.0.0.1:8080]
--capture-limit <BYTES>   maximum body bytes kept per request/response [default: 1048576]
--intercept-limit <BYTES> maximum body bytes buffered for interception or rewriting [default: 1048576]
--history-limit <N>       maximum number of exchanges kept in memory [default: 2000]
--history-bytes <BYTES>   maximum total body bytes retained by history [default: 268435456]
--no-proxy                start the UI without the proxy
--ca-dir <DIR>            TLS interception CA directory [default: $PERSEID_CA_DIR or the platform data directory]
--no-tls-intercept        start without decrypting HTTPS (the local CA is still available)
--tls-passthrough <HOST>  host that is never TLS-intercepted (repeatable)
--ui-scale <FACTOR>       UI scale, e.g. 1.5 or 2 [default: system / desktop scale]
```

Run `cargo run -p perseid -- --help` for the authoritative list.

## Workspace

The top-level tabs follow Burp's tool order. Switch with `Ctrl`+`Shift` plus
`T`, `P`, `I`, `R`, or `D` (use `Cmd` on macOS).

| Tab | Available today |
| --- | --- |
| **Target** | A host/path site map built from captured exchanges, and an editable scope list. Scope can filter the site map and history, and can optionally block out-of-scope traffic before it is forwarded. |
| **Proxy** | **HTTP history** is the live capture view and the default landing page. **Intercept** holds HTTP and decrypted HTTPS messages for editing and forwarding. **Match/replace** rewrites heads and buffered bodies. **TLS** shows the automatically created CA, opens the certificate page, and manages the HTTPS toggle, regeneration, removal, and pass-through hosts. |
| **Intruder** | A planned-workspace overview. Fuzzing and payload configuration arrive in Milestone 4. |
| **Repeater** | Editable request tabs seeded from HTTP history, a target bar, direct Send, and Pretty/Raw/Hex responses. HTTP/HTTPS only; WebSocket replay is not supported yet. |
| **Decoder** | Stacked Text/Hex panels with Decode as, Encode as, Hash, and Smart decode; selection-aware transforms and a replaceable downstream chain. Common hashes only; read-only Hex and no Send to Decoder yet. |

The scope input accepts a bare host, a `host:port` authority, or a full URL;
matching also covers subdomains and ignores ports. Toggle **Filter views to the
scope** in **Target > Scope** to hide out-of-scope hosts from the workspace.
Enable **Block out-of-scope traffic** to return `403` for HTTP requests and
`CONNECT` tunnels outside the list. Enforcement is off by default, and an empty
scope allows every host. The HTTP history filter matches method, host, path,
URI, and status.

The detail pane shows the request and response side by side, each with its own
Pretty, Raw, and Hex tabs, following Burp's default horizontal message-editor
layout. Drag the divider between the panes to give either side more room. Click
a history row or use the keyboard to select it.

The status bar shows a live **Memory** figure: the bytes currently held by
Perseid's allocator, the closest equivalent to the memory figure in Burp
Suite's status bar. Click it for the current and peak values and a rolling
three-minute graph. See [Performance and memory](performance.md) for the M0
baseline, resident-memory context, and method.

## Intercepting traffic

Open **Proxy > Intercept** and click the **Intercept is off** button to turn
interception on. As in Burp, this is the single on/off control: HTTP requests
and responses — including decrypted HTTPS when TLS interception is enabled — are
held before they are forwarded, and the button changes to **Intercept is on**.
Held messages appear in a queue; the status bar shows how many are waiting.
Edit the raw head, edit the body when it is buffered UTF-8 text, then choose
**Forward** or **Drop** in the toolbar. **Forward** applies match/replace rules
and re-checks the scope before sending. Holding a message does not stop
unrelated connections from progressing. Turning interception off stops new
messages from being held; anything already waiting still needs **Forward** or
**Drop**.

Only bodies up to `--intercept-limit` (1 MiB by default) are buffered for
editing. Larger or streaming bodies are still held for header edits, but their
body bytes pass through unchanged and body editing is disabled. In the
interception editor, binary bodies are shown as a read-only hex preview.

HTTP/2 messages use a reconstructed editable head. The request line is
origin-form, `:authority` is represented by a synthetic `Host` header, and
response status lines omit the reason phrase. Editing that head updates the
forwarded request or response; pseudo-headers are derived again when the
message is sent.

**Pretty** formats structured bodies for reading: JSON is indented, HTML and XML
are nested, and URL-encoded forms get one decoded field per line. **Raw** shows
the body as plain text and never mixes in a hex dump; binary bodies are rendered
lossily with control bytes masked as `·`. Pretty and Raw number their lines and
highlight the start line, headers, and structured bodies. Header names are
displayed in the conventional title case used by Burp (`Content-Type`, `ETag`,
and so on) without changing the forwarded bytes. **Hex** shows a bounded
byte-offset dump and uses the same compact message font. The message editor does
not add explanatory notes to any view: when there is no body, the view simply
ends after the headers.

Bodies with `Content-Encoding: gzip`, `deflate`, or `br` (Brotli) are decoded
for all three inspection views, including Hex, which makes ordinary compressed
HTML readable. The displayed headers have `Content-Encoding` removed;
`Content-Length` is updated when the complete decoded length is known.

This matches Burp's model: Burp unpacks compressed bodies for the message editor
by default and does not recompress them for display. Perseid's proxy still
forwards the encoded bytes unchanged, so the browser and origin server see the
wire representation. As Burp does by default, Perseid also removes content
codings it cannot decode (`zstd`, for example) from the outgoing
`Accept-Encoding` header, so servers normally choose gzip, deflate, Brotli, or
identity. If a server still returns an unsupported coding, the wire bytes are
shown instead.

Responses such as `304 Not Modified` and `204 No Content` have no body by
definition, so their message views end after the headers.

## Repeater

Repeater is the manual replay workspace. Select an HTTP exchange in **Proxy >
HTTP history** and choose **Send to Repeater**, or open **Repeater** and use the
`+` button for a blank tab. Each tab keeps its own target, edited request,
response, and Pretty/Raw/Hex selection. The tab strip remains available while
other requests are in flight, and the status bar shows how many Repeater sends
are pending.

The target bar sets the scheme and authority used for an origin-form request
line such as `GET /admin HTTP/1.1`. An absolute-form request line such as
`GET https://other.example/admin HTTP/1.1` overrides the target bar. Perseid
preserves a non-empty `Host` header from the editor so you can connect to one
authority while testing a different virtual host; if the header is absent or
blank, Perseid fills it from the target. Edit the request line, headers, and
body directly, then click **Send** or press `Ctrl`+`Enter` (`Cmd`+`Enter` on
macOS).

Repeater sends directly to the target with its own HTTP client rather than
through the local proxy listener. It therefore works when the proxy is stopped
or Perseid was started with `--no-proxy`. It also means Repeater traffic does
not pass through interception, match/replace rules, or scope enforcement, and
it does not add a row to Proxy history; the edited request and response stay in
the Repeater tab. The client supports HTTP and HTTPS and negotiates HTTP/2 over
TLS when the origin offers it. It verifies origin certificates against the
platform trust store.

Response bodies are retained up to `--capture-limit` (1 MiB by default). The
response header reports status, duration, and total body size; a note appears
when the retained body was truncated or the transfer did not finish. Each
request has a 60-second end-to-end timeout, and up to eight tab sends run
concurrently. Requests seeded from history use the same decoded, bounded
representation as the Proxy detail pane, so compressed or very large captured
bodies can be lossy. A seed warning asks you to review the body before sending.
WebSocket messages, saved Repeater tabs, and project persistence are not
implemented yet.

## Decoder

Decoder is the local transformation workspace. Open it with **Decoder** or
`Ctrl`+`Shift`+`D`, type or paste into the first panel, then choose an
operation. Every operation adds a new editable panel below the source; the
operation's color and name appear as a chip on the result. Applying another
operation to an earlier panel replaces the panels downstream, so the visible
chain always matches the operations that produced it. Paste with `Ctrl`+`V`
(`Cmd`+`V` on macOS) or right-click the text area for **Cut**, **Copy**,
**Paste**, and **Select all**.

Each panel has **Text** and **Hex** views. Text is editable monospace content.
Hex is a read-only byte dump with the same operation colors. When a result is
binary, including Gzip output, the panel opens in Hex automatically. A binary
panel can be switched to Text for a lossy preview, but editing it replaces the
bytes with UTF-8 text. Hex rendering stops at 64 KiB per panel; the panel's
**Copy** button and the toolbar's **Copy output** still export the complete
contents. Use **Clear** to reset the chain.

The **Decode as** and **Encode as** menus contain the same nine entries in
Burp order:

- **Plain** copies the input (or selection) into a new panel without changing
  it. It is useful for branching a chain before trying different operations.
- **URL** decodes `%xx` escapes, or percent-encodes every byte using lowercase
  hexadecimal. Decoding leaves malformed escapes untouched.
- **HTML** decodes named and numeric entities, or encodes every character as
  `&#x...;`, matching Burp's all-character output rather than only escaping
  metacharacters.
- **Base64** accepts standard and URL-safe alphabets, with or without padding,
  and ignores whitespace. Encoding always uses the standard padded alphabet.
- **ASCII hex** is the byte-oriented hexadecimal format: two digits per byte,
  with whitespace ignored. It requires an even number of digits.
- **Hex**, **Octal**, and **Binary** are not byte encodings. They reproduce
  Burp's numeric digit-run behavior: **Encode as > Hex** turns the decimal run
  in `port 1234` into `port 4d2`, while **Decode as > Hex** turns the
  hexadecimal run in `deadbeef` into `3735928559`. Use **ASCII hex** when you
  mean bytes.
- **Gzip** compresses or decompresses gzip data. Decompression is bounded by
  the output limit rather than trusting the gzip header.

**Hash** offers 31 common algorithms: BLAKE2B-160/256/384/512,
BLAKE2S-128/160/224/256, BLAKE3, Keccak-224/256/384/512, MD2, MD4, MD5,
RIPEMD-160, SHA-1, SHA-224/256/384/512, SHA-512/224, SHA-512/256,
SHA3-224/256/384/512, SM3, Tiger, and Whirlpool. Output is lowercase
hexadecimal text. The long tail of provider-specific Burp names (SKEIN,
TupleHash, and OID aliases) is not implemented.

**Smart decode** repeatedly detects and applies one layer at a time, adding a
panel for each. It recognizes gzip, URL, HTML, Base64, and ASCII hex. Numeric
Hex, Octal, and Binary are deliberately excluded so ordinary decimal values
are not reinterpreted; for example, `%34%37` stops at `47`. It stops after 12
layers, when no further layer is detected, or when a layer would repeat.
Panels that produce no match show a notice rather than adding an empty panel.

If text is selected in a Text panel, **Decode as**, **Encode as**, **Hash**,
and **Smart decode** transform only that selection and leave the untransformed
prefix and suffix in place. With no selection they transform the whole panel.
The Hex view has no partial selection for transforms; switch to Text or apply
the operation to all bytes.

A single transform accepts at most 8 MiB of input and produces at most 16 MiB
of output. These limits cover the whole panel, including preserved prefix and
suffix bytes, and are reported in the panel instead of silently truncating.
Decoder is currently a separate local surface: there is no **Send to Decoder**
action from Proxy or Repeater, and the panel chain is not persisted between
launches.

## HTTPS interception

Perseid creates a local certificate authority (CA) automatically on first
launch and stores it outside the proxy, in a per-user data directory. The CA is
reused across restarts, debug and release builds, and application updates;
replacing the binary does not invalidate browser trust. Interception is on by
default; the private key stays on your machine.

On first launch Perseid opens a certificate setup window. Install the
certificate in the browser you use for testing. Open **Proxy > TLS > Open
certificate page**, or browse directly to
`http://127.0.0.1:8080/` while the proxy is running. Download
`perseid-ca.pem` and import it; step-by-step Firefox and Chromium instructions,
pass-through configuration, and removal steps are in
[HTTPS interception](tls.md). Trust is always installed manually, never
system-wide by Perseid. Once the proxy is configured, `http://perseid/` opens
the same certificate page.

Each request inside a decrypted `CONNECT` tunnel appears as its own `https://`
entry in HTTP history and flows through interception, scope, and match/replace
like plain HTTP. The `CONNECT` tunnel itself is transport and is not shown as a
separate row, matching Burp's HTTP history. Hosts on the pass-through list stay
opaque, as does all HTTPS when you turn off **Intercept HTTPS** or start with
`--no-tls-intercept`; because those connections are not decrypted, they add no
HTTP history rows. HTTP/2 is negotiated inside the tunnel when the client
offers it through ALPN; HTTP/1.1 remains the fallback, and HTTPS origins are
offered h2 the same way.

## Match/replace rules

Open **Proxy > Match/replace** to add request or response rules. Each rule
chooses the head, the body, or both, and uses literal matching or a Rust regular
expression. Regex replacements support `$1`-style capture references. Rules run
in list order after interception and before the message is sent, so the history
reflects the edited result.

Body rules apply only to buffered UTF-8 bodies. Binary, streaming, and
over-limit bodies are left untouched. Empty patterns are ignored. An invalid
regular expression stays visible in the editor and is skipped while the rest of
the policy, including the interception control and valid rules, remains active.

## Pointing a browser at Perseid

Configure your browser — or an extension such as FoxyProxy — to use an HTTP
proxy at `127.0.0.1:8080` for HTTP and HTTPS.

- Plain HTTP requests are forwarded and captured: method, headers, bounded
  bodies, status, size, and duration are visible in the history table and
  detail pane. They can also be held and edited when interception is enabled.
- HTTPS requests arrive as `CONNECT` tunnels. Perseid intercepts and decrypts
  them by default, so each request inside the tunnel is captured — and can be
  held and edited — as an ordinary `https://` history entry. The `CONNECT`
  transport itself does not get its own row. If interception is off or the host
  is passed through, Perseid relays the encrypted bytes without adding history
  entries.

History is keyboard-friendly: `j`/`k` or the arrow keys move the selection,
`Home`/`End` jump to the ends, `Ctrl`+`F` focuses the filter, and `Escape`
leaves the filter. Clicking a row also selects it.

## Current limitations

- HTTP/1.1 and HTTP/2 are proxied. HTTP/2 uses TLS ALPN for intercepted clients
  and HTTPS origins, and h2c prior knowledge is accepted on the plain listener.
  Cleartext origins use HTTP/1.1. WebSocket-specific handling and HTTP/3/QUIC
  are still planned.
- HTTPS interception terminates HTTP/1.1 or HTTP/2 inside `CONNECT` tunnels.
  Certificate pinning, non-HTTP TLS protocols, and clients that do not trust
  the local CA need the pass-through list.
- Browser trust must be installed and removed manually. Perseid never edits a
  system or browser trust store.
- Interception covers HTTP and decrypted HTTPS. Only buffered UTF-8 bodies can
  be edited; larger, streaming, and binary bodies are header-only.
- Scope enforcement can block HTTP and `CONNECT`, and it checks decrypted
  requests when TLS interception is on. It is off by default, and an empty scope
  allows every host.
- Match/replace body rules apply only to buffered UTF-8 bodies. Rules are
  runtime-only and are not persisted between launches.
- Compressed-body unpacking is always on for the inspection views and has no
  UI toggle yet. It affects display only; forwarding keeps the wire encoding.
- Repeater supports editable HTTP and HTTPS requests, but not WebSocket
  messages or saved tabs. It sends directly to the target, bypassing the proxy
  pipeline and scope enforcement. Intruder has no engine yet.
- Decoder transforms at most 8 MiB of input and 16 MiB of output per
  operation. Hex panels are read-only and preview at most 64 KiB; Copy exports
  all retained bytes. Hash support omits SKEIN, TupleHash, and OID aliases, and
  there is no Send to Decoder integration yet. The panel chain is runtime-only.
  HTML encoding is text-oriented: binary input is represented as numeric
  entities per byte, so high bytes do not round-trip as raw bytes.
- History is in memory only. It is bounded by `--history-limit` and
  `--history-bytes`; captured bodies are capped per message by
  `--capture-limit`, and interception buffers up to `--intercept-limit` per
  message. The oldest exchanges are evicted when either history bound is
  reached.
- Captured traffic is in memory only; there is no project file or settings
  persistence yet. The TLS certificate authority is the exception: it is
  persisted so browser trust survives restarts and updates.

## HiDPI displays

The UI follows the scale factor reported by the windowing system. On X11
desktops that do not report one (for example when `Xft.dpi` is unset),
Perseid falls back to `GDK_SCALE` or `QT_SCALE_FACTOR`, so it matches the
scale GTK and Qt applications already use.

Override the result with `--ui-scale <FACTOR>`, or adjust it at runtime from
the **View** menu or with `Ctrl`+`+` / `Ctrl`+`-`; `Ctrl`+`0` resets to the
detected scale. The window opens at the same logical content size at every
scale.

## Troubleshooting

- **"proxy not running" / address in use:** another process is using the
  listen address. Stop it, pick another address with `--listen`, or use
  **Retry** after freeing the port.
- **The UI is tiny or huge:** adjust it from the **View** menu or pass
  `--ui-scale`; see [HiDPI displays](#hidpi-displays).
- **History or the site map looks empty:** the filter or scope may be hiding
  rows. Clear the history filter, or turn off **Filter views to the scope** in
  **Target > Scope**.
- **Pages appear to hang with an amber "awaiting intercept" count:** traffic is
  held in **Proxy > Intercept**. Forward or drop the current message, then turn
  interception off if you are finished. Pending messages are aborted when the
  proxy stops.
- **A request or `CONNECT` gets a `403`:** **Block out-of-scope traffic** is
  enabled and the host is not in the scope list. Add the host or turn the
  guardrail off.
- **HTTPS sites show no decrypted traffic:** **Intercept HTTPS** is off, the CA
  could not be loaded, or the host is on the pass-through list. Check the status
  in **Proxy > TLS**. If the browser reports an untrusted certificate, import the
  current `perseid-ca.pem` from the certificate page as described in
  [HTTPS interception](tls.md); regenerating the CA requires importing the new
  certificate.
