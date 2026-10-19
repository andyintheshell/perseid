# Getting started

Wiremoth builds as a small Cargo workspace: `wiremoth-proxy` (the proxy library)
and `wiremoth` (the desktop application).

## Requirements

- Rust 1.95.0, as pinned in `rust-toolchain.toml` (edition 2024).
- A Linux desktop session with OpenGL support (X11 or Wayland).
- A browser or HTTP client that can be configured to use a proxy.

## Build and run

From the workspace root:

```sh
cargo run -p wiremoth
```

This starts the proxy on `127.0.0.1:8080` (loopback only) and opens the
Wiremoth window on **Proxy > HTTP history**. The toolbar shows whether the
proxy is running and lets you retry if the address is already in use.

### Options

```text
--listen <ADDR>           address for the proxy to listen on [default: 127.0.0.1:8080]
--capture-limit <BYTES>   maximum body bytes kept per request/response [default: 1048576]
--intercept-limit <BYTES> maximum body bytes buffered for interception or rewriting [default: 1048576]
--history-limit <N>       maximum number of exchanges kept in memory [default: 2000]
--history-bytes <BYTES>   maximum total body bytes retained by history [default: 268435456]
--no-proxy                start the UI without the proxy
--ui-scale <FACTOR>       UI scale, e.g. 1.5 or 2 [default: system / desktop scale]
```

Run `cargo run -p wiremoth -- --help` for the authoritative list.

## Workspace

The top-level tabs follow Burp's tool order. Switch with `Ctrl`+`Shift`+`T`,
`P`, `I`, or `R` (use `Cmd` on macOS).

| Tab | Available today |
| --- | --- |
| **Target** | A host/path site map built from captured HTTP, and an editable scope list. Scope can filter the site map and history, and can optionally block out-of-scope traffic before it is forwarded. |
| **Proxy** | **HTTP history** is the live capture view and the default landing page. **Intercept** holds plain-HTTP requests and responses for editing and forwarding. **Match/replace** rewrites heads and buffered bodies. |
| **Intruder** | A planned-workspace overview. Fuzzing and payload configuration arrive in Milestone 4. |
| **Repeater** | A read-only preview of the selected request and response. Editing and resending arrive in Milestone 3. |

The scope input accepts a bare host, a `host:port` authority, or a full URL;
matching also covers subdomains and ignores ports. Toggle **Filter views to the
scope** in **Target > Scope** to hide out-of-scope hosts from the workspace.
Enable **Block out-of-scope traffic** to return `403` for HTTP requests and
`CONNECT` tunnels outside the list. Enforcement is off by default, and an empty
scope allows every host. The HTTP history filter matches method, host, path,
URI, and status.

The detail pane shows the request and response side by side, each with its own
Raw, Headers, and Hex tabs. This follows Burp's default horizontal message-editor
layout. Drag the divider between the panes to give either side more room. Click
a history row or use the keyboard to select it.

The status bar shows a live **Memory** figure: the bytes currently held by
Wiremoth's allocator, the closest equivalent to the memory figure in Burp
Suite's status bar. Click it for the current and peak values and a rolling
three-minute graph. See [Performance and memory](performance.md) for the M0
baseline, resident-memory context, and method.

## Intercepting traffic

Open **Proxy > Intercept** and click the **Intercept is off** button to turn
interception on. As in Burp, this is the single on/off control: plain-HTTP
requests and responses are held before they are forwarded, and the button
changes to **Intercept is on**. Held messages appear in a queue; the status bar
shows how many are waiting. Edit the raw head, edit the body when it is
buffered UTF-8 text, then choose **Forward** or **Drop** in the toolbar.
**Forward** applies match/replace rules and re-checks the scope before sending.
Holding a message does not stop unrelated connections from progressing. Turning
interception off stops new messages from being held; anything already waiting
still needs **Forward** or **Drop**.

Only bodies up to `--intercept-limit` (1 MiB by default) are buffered for
editing. Larger or streaming bodies are still held for header edits, but their
body bytes pass through unchanged and body editing is disabled. In the
interception editor, binary bodies are shown as a read-only hex preview.

In history, **Raw** always shows text so it never mixes in a hex dump. Bodies
with `Content-Encoding: gzip`, `deflate`, or `br` (Brotli) are decoded for all
three inspection views, including **Hex**, which makes ordinary compressed HTML
readable. The displayed headers have `Content-Encoding` removed;
`Content-Length` is updated when the complete decoded length is known. A note in
Raw and Hex records the original wire size.

This matches Burp's model: Burp unpacks compressed bodies for the message editor
by default and does not recompress them for display. Wiremoth's proxy still
forwards the encoded bytes unchanged, so the browser and origin server see the
wire representation. As Burp does by default, Wiremoth also removes content
codings it cannot decode (`zstd`, for example) from the outgoing
`Accept-Encoding` header, so servers normally choose gzip, deflate, Brotli, or
identity. If a server still returns an unsupported coding, the wire bytes are
shown with a note instead of an empty panel. Binary bodies are shown lossily in
Raw, with control bytes masked as `·`.

Responses such as `304 Not Modified` and `204 No Content` have no body by
definition. Raw labels them explicitly, so a force-refreshed page that reuses
its cached copy does not look like a failed capture.

HTTPS is still a `CONNECT` passthrough. Scope enforcement can block a tunnel
before it opens, but Wiremoth cannot edit requests inside it until TLS
interception lands.

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

## Pointing a browser at Wiremoth

Configure your browser — or an extension such as FoxyProxy — to use an HTTP
proxy at `127.0.0.1:8080` for HTTP and HTTPS.

- Plain HTTP requests are forwarded and captured: method, headers, bounded
  bodies, status, size, duration, and capture truncation are visible in the
  history table and detail pane. They can also be held and edited when
  interception is enabled.
- HTTPS requests arrive as `CONNECT` tunnels. Wiremoth relays the bytes in
  both directions and records the exchange with byte counts, but does not
  decrypt TLS yet (Milestone 2). Browsers therefore keep seeing the site's
  real certificate; no CA needs to be trusted for now.

History is keyboard-friendly: `j`/`k` or the arrow keys move the selection,
`Home`/`End` jump to the ends, `Ctrl`+`F` focuses the filter, and `Escape`
leaves the filter. Clicking a row also selects it.

## Current limitations

- HTTP/1.1 only; HTTP/2 and WebSocket-specific handling are still planned.
- HTTPS is TLS passthrough; there is no interception, editing, or replay of
  HTTPS contents yet.
- Interception is available for plain HTTP. Only buffered UTF-8 bodies can be
  edited; larger, streaming, and binary bodies are header-only.
- Scope enforcement can block HTTP and `CONNECT`, but it is off by default and
  an empty scope allows every host.
- Match/replace body rules apply only to buffered UTF-8 bodies. Rules are
  runtime-only and are not persisted between launches.
- Compressed-body unpacking is always on for the inspection views and has no
  UI toggle yet. It affects display only; forwarding keeps the wire encoding.
- Repeater cannot edit or send requests yet; Intruder has no engine yet.
- History is in memory only. It is bounded by `--history-limit` and
  `--history-bytes`; captured bodies are capped per message by
  `--capture-limit`, and interception buffers up to `--intercept-limit` per
  message. The oldest exchanges are evicted when either history bound is
  reached.
- There is no disk persistence, project file, or settings persistence yet.

## HiDPI displays

The UI follows the scale factor reported by the windowing system. On X11
desktops that do not report one (for example when `Xft.dpi` is unset),
Wiremoth falls back to `GDK_SCALE` or `QT_SCALE_FACTOR`, so it matches the
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
- **HTTPS sites work but show no decrypted traffic:** expected for now;
  `CONNECT` tunnels are relayed without interception. The site map deliberately
  lists HTTP endpoints only.
