# Safety and privacy

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
  edit cannot move a request outside the guardrail unnoticed. For HTTPS, the
  `CONNECT` target is checked before a tunnel opens; with TLS interception
  enabled, decrypted requests are checked as well. Without interception, only
  the tunnel target is visible.
- The prototype has no telemetry. Apart from proxied traffic, the application
  does not make network requests; opening the project link in the About window
  is an explicit user action.

## Data handling

For now, captured history and held interception messages live in memory only
and disappear when the process exits. Dropping the proxy handle or closing the
application releases held messages and aborts their exchanges. There is no disk
persistence, export, or background upload. The only files Perseid writes are
the TLS certificate authority and its private key described below.

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
- Trust must be installed deliberately by the user in each browser. Perseid
  serves a local certificate download page, but never writes to system or
  browser trust stores and never installs the CA silently.
- Interception is on by default, matching Burp and ZAP. Turning it off, or
  adding a host to the pass-through list, keeps that traffic opaque. The CA and
  its private key remain available unless the user deletes the files.
- Removal is deliberate and documented in [HTTPS interception](tls.md): remove
  the browser trust entry, then delete the CA files.
- Pass-through hosts are an exclude list matched like scope (exact host,
  subdomains, and any port). An empty list excludes nothing.

## Reporting a vulnerability

If you believe you found a security issue in Perseid itself, please use a
private GitHub security advisory:
[report a vulnerability](https://github.com/andyintheshell/perseid/security/advisories/new).

Please do not open a public issue containing working exploit details or
captured sensitive traffic.
