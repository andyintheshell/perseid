# Safety and privacy

Wiremoth is intended for systems you own or are explicitly authorized to test,
including purpose-built training labs. Intercepted requests may contain
passwords, session tokens, cookies, or personal data.

## Authorized use

Do not point Wiremoth at third-party systems without permission. The
loopback-only default, scope filters, and optional scope enforcement are safety
aids, not permission to test an arbitrary host.

## Defaults

- The proxy binds to `127.0.0.1` by default; keep it on loopback unless you
  have a specific reason and understand the exposure.
- Captured request and response bodies are capped by `--capture-limit`
  (1 MiB per message by default) with explicit truncation markers.
- History is bounded by `--history-limit` (2000 exchanges by default) and
  `--history-bytes` (256 MiB of retained request/response body bytes by
  default). The oldest exchanges are evicted when either bound is reached.
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
  edit cannot move a request outside the guardrail unnoticed. HTTPS tunnel
  contents are not decrypted, so only the tunnel target can be checked.
- The prototype has no telemetry. Apart from proxied traffic, the application
  does not make network requests; opening the project link in the About window
  is an explicit user action.

## Data handling

For now, captured history and held interception messages live in memory only
and disappear when the process exits. Dropping the proxy handle or closing the
application releases held messages and aborts their exchanges. There is no disk
persistence, export, or background upload.

When persistent history and exports are added:

- Data stays local unless the user explicitly exports it.
- Stored traffic can be searched and deleted from the application.
- Sanitized exports should be the default; raw evidence remains an explicit
  choice.
- Sensitive captures should be clearly marked with their retention limits.

## TLS interception (planned)

HTTPS interception requires a local certificate authority. When it lands:

- The CA is generated locally and its private key stays on the user's machine.
- Trust must be installed deliberately by the user, with documented setup and
  removal steps.
- The CA must never be silently installed system-wide.
- Pass-through rules should make it easy to exclude hosts that must not be
  intercepted.

## Reporting a vulnerability

If you believe you found a security issue in Wiremoth itself, please use a
private GitHub security advisory:
[report a vulnerability](https://github.com/andyintheshell/wiremoth/security/advisories/new).

Please do not open a public issue containing working exploit details or
captured sensitive traffic.
