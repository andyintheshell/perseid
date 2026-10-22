# Safety and privacy

[Documentation](README.md) · [HTTPS setup](tls.md) ·
[Workbench](workbench.md) · [Security policy](../SECURITY.md)

Perseid is intended for systems you own or are explicitly authorized to test,
including purpose-built training labs. Intercepted requests may contain
passwords, session tokens, cookies, or personal data.

## Authorized use

Do not point Perseid at third-party systems without permission. The
loopback-only default, scope filters, and optional scope enforcement are safety
aids, not permission to test an arbitrary host.

## Defaults

- The proxy binds to `127.0.0.1` by default; keep it on loopback unless you
  have a specific reason and understand the exposure.
- Captured request and response bodies are capped by `--capture-limit`
  (1 MiB per message by default).
- History is bounded by `--history-limit` (2000 exchanges by default) and
  `--history-bytes` (half of physical memory by default, with a 256 MiB
  fallback when the platform does not report installed memory). The oldest
  exchanges are evicted when either bound is reached.
- The proxy accepts at most 128 client connections and 64 CONNECT tunnels at
  once by default. Excess connections are closed; excess CONNECT requests get
  a 503 response. Both limits are library configuration, not browser settings.
- Interception buffers at most `--intercept-limit` body bytes per held message
  (1 MiB by default). Larger or streaming bodies are held for header edits only
  and pass through unchanged.
- Scope filtering is always available as a view filter. **Block out-of-scope
  traffic** is opt-in and off by default; when enabled, HTTP requests and
  `CONNECT` tunnels outside the list receive a `403`. An empty scope allows
  every host. Scope is checked again after interception and rewriting, so an
  edit cannot move a request outside the guardrail unnoticed. For HTTPS, the
  `CONNECT` target is checked before a tunnel opens; with TLS interception
  enabled, decrypted requests are checked as well. Without interception, only
  the tunnel target is visible.
- **Repeater sends directly to the target and bypasses the proxy guardrails.**
  Scope filtering and blocking, interception, and match/replace rules do not
  apply to Repeater sends, and Repeater exchanges are not added to Proxy
  history. Confirm the target and request before clicking **Send**.
- Repeater separately bounds queued, active, and unread work to 32 operations
  and 64 MiB of retained-data reservations, with at most eight active sends.
  Requests have an 8 MiB retained-buffer limit and heads a 64 KiB limit. Full
  admission returns a retryable error, not an unlimited queue; these budgets
  are not a cap on all editor/tab state or process memory.
- The prototype has no telemetry. Apart from proxied and explicit Repeater
  traffic, the application does not make network requests; opening the project
  link in the About window is an explicit user action.

## Data handling

For now, captured history, held interception messages, Repeater tabs/responses,
and Decoder panels live in memory only and are not restored after exit. Dropping
the proxy handle or closing the application releases held messages and aborts
their exchanges. There is no traffic database, file export, or background
upload. Clipboard copying is an explicit user action. In a normal desktop run,
the app persists the TLS CA store—certificate, private key, and generation/lock
metadata—described below; the developer screenshot feature also writes synthetic PNGs.

In-memory storage is not secure erasure: OS swap, crash reports, clipboard
history, terminal logs, and screenshots can retain copies. Capture only what
you are authorized to handle, on a machine you trust, and redact shared output.
Retention limits are not a hard cap on the whole process's memory use.

When persistent history and exports are added:

- Data stays local unless the user explicitly exports it.
- Stored traffic can be searched and deleted from the application.
- Sanitized exports should be the default; raw evidence remains an explicit
  choice.
- Sensitive captures should be clearly marked with their retention limits.

## TLS interception

HTTPS interception uses a certificate authority generated and stored locally.

- Perseid creates the CA automatically on first launch and reuses it across
  restarts and updates. The private key stays on the user's machine in the CA
  directory (`--ca-dir`, `PERSEID_CA_DIR`, or the platform data directory). The
  directory is created owner-only and the key is written owner-readable only.
- New/replacement CAs are validated and committed as complete generations;
  the selected certificate and private key must match. Existing top-level PEM
  pairs are reused until explicit regeneration. Use the active paths shown in
  the UI rather than assuming the old top-level filenames are current.
- Trust must be installed deliberately by the user in each browser. Perseid
  serves a local certificate download page, but never writes to system or
  browser trust stores and never installs the CA silently.
- Interception is on by default, matching Burp and ZAP. Turning it off, or
  adding a host to the pass-through list, keeps that traffic opaque. The CA and
  its private key remain available unless the user deletes the files.
- Removal is deliberate and documented in [HTTPS interception](tls.md): remove
  the browser/OS trust entries, then use **Delete files** to remove generations,
  the pointer, and legacy PEM files. A key-free coordination lock may remain.
- Pass-through hosts are an exclude list matched like scope (exact host,
  subdomains, and any port). An empty list excludes nothing.

## Reporting a vulnerability

If you believe you found a security issue in Perseid itself, please use a
private GitHub security advisory:
[report a vulnerability](https://github.com/andyintheshell/perseid/security/advisories/new).

See the [security policy](../SECURITY.md) for supported versions, response
expectations, scope, and safe-harbor terms. Please do not open a public issue
containing working exploit details or captured sensitive traffic.
