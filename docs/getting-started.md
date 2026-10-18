# Getting started

Wiremoth builds as a small Cargo workspace: `wiremoth-proxy` (the proxy library)
and `wiremoth` (the desktop application).

## Requirements

- A recent stable Rust toolchain (edition 2024, Rust 1.85 or newer).
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
--listen <ADDR>         address for the proxy to listen on [default: 127.0.0.1:8080]
--capture-limit <BYTES> maximum body bytes kept per request/response [default: 1048576]
--history-limit <N>     maximum number of exchanges kept in memory [default: 2000]
--no-proxy              start the UI without the proxy
--ui-scale <FACTOR>     UI scale, e.g. 1.5 or 2 [default: system / desktop scale]
```

Run `cargo run -p wiremoth -- --help` for the authoritative list.

## Workspace

The top-level tabs follow Burp's tool order. Switch with `Ctrl`+`Shift`+`T`,
`P`, `I`, or `R` (use `Cmd` on macOS).

| Tab | Available today |
| --- | --- |
| **Target** | A host/path site map built from captured HTTP, and an editable scope list. Scope hides out-of-scope hosts from the site map and history. It is currently a view filter only: forwarding and capture are unchanged. |
| **Proxy** | **HTTP history** is the live capture view and the default landing page. **Intercept** documents the planned hold/edit/forward workflow; editable interception is not implemented yet. |
| **Intruder** | A planned-workspace overview. Fuzzing and payload configuration arrive in Milestone 4. |
| **Repeater** | A read-only preview of the selected request and response. Editing and resending arrive in Milestone 3. |

The scope input accepts a bare host, a `host:port` authority, or a full URL;
matching also covers subdomains. Toggle **Filter views to the scope** in
**Target > Scope** to apply it. The HTTP history filter matches method, host,
path, URI, and status.

The request/response pane has request and response tabs, each with Raw,
Headers, and Hex views. Click a row or use the keyboard to select it.

## Pointing a browser at Wiremoth

Configure your browser — or an extension such as FoxyProxy — to use an HTTP
proxy at `127.0.0.1:8080` for HTTP and HTTPS.

- Plain HTTP requests are forwarded and captured: method, headers, bounded
  bodies, status, size, duration, and capture truncation are visible in the
  history table and detail pane.
- HTTPS requests arrive as `CONNECT` tunnels. Wiremoth relays the bytes in
  both directions and records the exchange with byte counts, but does not
  decrypt TLS yet (Milestone 2). Browsers therefore keep seeing the site's
  real certificate; no CA needs to be trusted for now.

History is keyboard-friendly: `j`/`k` or the arrow keys move the selection,
`Home`/`End` jump to the ends, `Ctrl`+`F` focuses the filter, and `Escape`
leaves the filter. Clicking a row also selects it.

## Current limitations

- HTTP/1.1 only; HTTP/2 and WebSocket-specific handling are still planned.
- HTTPS is TLS passthrough; there is no interception, editing, or replay yet.
- Requests and responses are captured and forwarded, not paused for editing.
- Scope filtering is view-only. It does not block, drop, or modify traffic.
- Repeater cannot edit or send requests yet; Intruder has no engine yet.
- History is in memory only. It is bounded by `--history-limit`; captured
  bodies are capped per message by `--capture-limit`.
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
- **HTTPS sites work but show no decrypted traffic:** expected for now;
  `CONNECT` tunnels are relayed without interception. The site map deliberately
  lists HTTP endpoints only.
