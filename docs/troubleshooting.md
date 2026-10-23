# Troubleshooting

[Documentation](README.md) · [Getting started](getting-started.md) ·
[Workbench](workbench.md) · [HTTPS setup](tls.md)

Start with the [local HTTP walkthrough](getting-started.md#try-it-with-a-local-target).
If that works, the build, listener, forwarding path, and history are working;
focus on browser proxy settings or certificate trust next.

## The app will not build or open

### Rust is missing, too old, or selects an unexpected toolchain

Check `rustc --version` and `cargo --version`. The minimum is Rust 1.95;
rustup uses the repository's 1.95.0 pin. Homebrew supplies its own current
toolchain and does not use that pin.

On macOS, check which executables are selected:

```sh
command -v rustc
command -v cargo
```

If you intend to use Homebrew and an older rustup shim takes precedence, use
`export PATH="$(brew --prefix rust)/bin:$PATH"` for the current shell. Keep one
toolchain's Cargo, rustc, rustfmt, and Clippy together. Newer Clippy releases can
add warnings absent from the pinned CI toolchain; include the version when
reporting a check failure.

### Native libraries or a display are missing

Install the [Linux build dependencies](getting-started.md#linux), or Apple's
Command Line Tools on macOS. A Linux UI run also needs an X11/Wayland session
and an OpenGL-capable graphics stack. An SSH-only shell without a display is
not a graphical session.

To exercise only the proxy from a terminal, use the
[headless example](../CONTRIBUTING.md#headless-proxy-and-library-api). This does
not remove the crate's GUI build dependencies. The screenshot script is a
separate Linux/Xvfb developer workflow, not a macOS launch command.

## The proxy is not running

Another process may own port 8080. Choose a different loopback address:

```sh
cargo run -p perseid --locked -- --listen 127.0.0.1:8081
```

Update the browser/client proxy settings to match. The toolbar's **Retry**
button can retry after the conflict is resolved. Do not use sudo or bind a
public interface just to work around a local port conflict.

## History or the site map is empty

1. Confirm that the toolbar shows the proxy listening and **Capture** is on.
2. Check the client uses that address for both HTTP and HTTPS.
3. Check browser bypass rules and the `NO_PROXY`/`no_proxy` environment variables.
   For curl, `--noproxy "" --proxy http://127.0.0.1:8080` forces the proxy path,
   including for a loopback target.
4. Clear the history filter and disable **Filter views to the scope** temporarily.
5. Check UI-cache eviction and dropped-event counters in the status bar. Cache
   eviction removes a row from the current view; it does not delete that record
   from a saved project. The temporary project store can grow beyond these cache
   limits until history is cleared or the app exits.

The site map uses the currently loaded history snapshot, not active crawling.
HTTPS pass-through tunnels have no visible decrypted requests, and `CONNECT`
transport tunnels do not get their own history rows. Repeater and Intruder
exchanges are also separate from Proxy history by design.

## A project does not open or appears to be missing history

- **Check the path and permissions.** Project dialogs take a path directly and
  do not provide a native file picker. Save As and Save Copy require a new path;
  they will not overwrite an existing file.
- **Check the version.** The published `0.1.0` release predates project files;
  use the current `0.2.0` development build to open `.perseid` projects. Newer or
  unsupported schema versions are rejected rather than silently rewritten.
- **History limits do not delete old saved records.** Opening a project loads a
  bounded newest-first snapshot controlled by `--history-limit` and
  `--history-bytes`. Older records remain in the file but are not pageable or
  searchable in the current UI. **Clear history** is destructive and removes
  stored captures from the active project.
- **Check the project status widget** for the last persistence error or capture
  loss. If a saved write failed, do not assume the newest changes reached disk;
  address the reported path/permission/disk-space issue before closing.

See [Projects and persistence](workbench.md#projects-and-persistence) for the
current workflow and saved-state coverage.

## Pages hang with messages awaiting interception

Open **Proxy > Intercept** and use **Forward**, **Drop**, **Forward all**, or
**Drop all**. Turning interception off does not release messages already held.
If response holding is enabled, a request may have reached the target while
its response is still waiting for your decision.

## A request receives 403

Check **Target > Scope > Block out-of-scope traffic**. Add the explicitly
authorized host or turn blocking off. Scope includes subdomains and ignores
ports; an empty list allows everything. Check whether you explicitly dropped
the request as well. A `403` can also originate from the target—inspect the
response and app status rather than assuming every rejection is local.

## HTTPS fails or stays opaque

- Open **Proxy > Proxy settings** and check that the CA is ready and
  **Intercept HTTPS** is selected.
- Remove the host from the pass-through list only if you intend to decrypt it.
- Install the public CA in the testing browser using the
  [trust instructions](tls.md#install-the-certificate). A different profile may
  have a different trust store.
- If the CA was regenerated, remove the old trust entry and import the new
  certificate. Rebuilding the app alone should not require regenerating the CA.
- Pinned certificates and non-HTTP TLS protocols may require pass-through.
- An upstream private/self-signed certificate is a separate trust problem from
  the browser trusting Perseid's CA. There is no documented desktop "ignore TLS
  errors" switch; do not disable verification as a workaround.

For HTTP/3/QUIC-only behavior, use a lab/client configuration that can fall back
to HTTP/1.1 or HTTP/2; QUIC is not proxied. More CA-specific advice is in
[HTTPS troubleshooting](tls.md#troubleshooting).

### The CA files are missing or loading fails after an update

Newly created/regenerated CAs live in a hidden generation subdirectory, selected
by `.perseid-ca-current`. Check the active paths in **Proxy > Proxy settings**,
or download the public certificate from the listener. Existing top-level PEM
pairs are still reused unchanged; upgrading alone does not rotate them.

An invalid pointer, missing generation, or mismatched certificate/key pair is
reported rather than silently replaced. Do not combine PEM files from different
generations, edit the pointer, or remove the coordination lock. Check the error,
selected `--ca-dir`, and permissions first. An isolated scratch `--ca-dir` can
test a clean store without altering your usual CA.

If you intentionally regenerate the CA, remove the old client trust entry and
import the replacement. To remove CA material, use the app's **Delete files**
action: deleting only the old top-level PEM paths does not remove generation
stores. The empty coordination lock can remain afterward; it is not a key.

## Repeater reports a budget or size error

These limits are independent of proxy history limits:

- **Pending-work budget is full:** queued requests, active sends, and unread
  results share 32 operation slots and 64 MiB of retained-data reservations.
  Wait for responses to be processed, then retry Send. The UI drains results
  automatically; the rejected send does not stop other sends. Library callers
  must consume `Repeater::try_recv` results to release admission.
- **Request exceeds the limit:** reduce the request/header size. Repeater allows
  up to 8 MiB of retained request buffers and 64 KiB heads, not an 8 MiB body
  plus unlimited headers. Increasing `--capture-limit` does not raise these
  limits or the HTTP client's 64 KiB response-head limit.
- **Capture limit exceeds the pending-budget maximum:** reduce the configured
  `--capture-limit` and restart. Repeater reserves response capacity up front;
  a maximum-sized request and its response/head reservation must fit the byte
  budget. The startup error reports the accepted maximum in bytes.

There are no CLI flags to raise the fixed pending-work budgets. Increasing
`--history-bytes` changes the UI history-cache budget, not Repeater admission or
the total project-store size. See
[Repeater](workbench.md#repeater) for the complete behavior.

## Intruder will not start an attack

The workspace reports the first validation error above the request editor:

- **No positions or an unclosed marker:** wrap each fuzzed value in `$$`.
  Select text and click **Add $$**, or use **Auto $$** for common parameters.
- **A payload set is empty or missing:** Pitchfork and Cluster bomb need one
  non-empty set per position. Sniper and Battering ram use the shared set.
- **The request count is invalid:** the number of generated requests overflowed
  a 64-bit count. Shrink a payload set or reduce the number of positions.
- **The target is outside the enforced scope:** add the effective host in
  **Target > Scope** or turn off **Block out-of-scope traffic**. An absolute-form
  request line overrides the target bar. The first request is checked before the
  attack and every generated request is checked again; out-of-scope rows show a
  failure instead of a response.
- **The request is invalid or too large:** generated requests use Repeater's
  direct client, with 8 MiB request and 64 KiB head limits. Check the request
  line, `Host`, and target scheme.
- **An attack is already running:** wait for it to finish or stop it, then start
  the next one. Stopping does not cancel requests already in flight.

Attacks do not appear in Proxy history, and proxy interception, match/replace,
and capture controls do not apply. See [Intruder](workbench.md#intruder) for the
attack types and result limits.

## A body is read-only, truncated, or different from the wire

Interception only buffers known-size bodies within `--intercept-limit`, and
only buffered text is body-editable. Chunked/streaming and binary bodies can
remain header-only even when the visible prefix is small.

History, Repeater, and Intruder previews decode supported compression,
reconstruct headers, and limit display size. Raw and Hex do not promise the exact original wire
representation. Review [message views](workbench.md#inspect-messages) and any
Repeater seed warning before replaying a captured body.

Pretty body formatting also has separate 256 KiB input/output and 128-level
nesting limits. If a document exceeds them, Pretty falls back to bounded Raw
text. Raising the capture limit does not raise the formatter's budgets, and the
fallback does not itself alter the message being forwarded.

## The interface is too small or too large

Use the **View** zoom menu or `--ui-scale 1.5` / `--ui-scale 2`. Reset zoom
returns to the startup scale. See [HiDPI displays](getting-started.md#hidpi-displays)
for native-scale and X11 environment behavior.

## Collect a useful bug report

Include:

- `git rev-parse --short HEAD`, OS/architecture, and Rust/Cargo versions;
- debug or release build, relevant CLI flags, and exact reproduction steps;
- expected versus actual behavior, the selected tool, and any status counters;
- a minimal local target or **synthetic** request/response, where possible;
- a redacted screenshot or terminal error if it adds information.

To collect more detailed application logs:

```sh
RUST_LOG=perseid=debug cargo run -p perseid --locked
```

Logs and screenshots can contain URLs, hostnames, paths, tokens, or other
sensitive data. Review them before sharing. Never publish a CA private key,
real credentials, or someone else's traffic.

[Open a normal bug report](https://github.com/andyintheshell/perseid/issues).
For a vulnerability in Perseid itself, follow the **private**
[security reporting policy](../SECURITY.md#reporting-a-vulnerability) instead.
