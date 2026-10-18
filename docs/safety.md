# Safety and privacy

Wiremoth is intended for systems you own or are explicitly authorized to test,
including purpose-built training labs. Intercepted requests may contain
passwords, session tokens, cookies, or personal data.

## Authorized use

Do not point Wiremoth at third-party systems without permission. Scope
controls and loopback-only defaults are safety features, not permission to
test an arbitrary host.

## Defaults

- The proxy binds to `127.0.0.1` by default; keep it on loopback unless you
  have a specific reason and understand the exposure.
- Captured request and response bodies are capped by `--capture-limit`
  (1 MiB per message by default) with explicit truncation markers.
- History is bounded by `--history-limit` (2000 exchanges by default).
- The prototype has no telemetry. Apart from proxied traffic, the application
  does not make network requests; opening the project link in the About window
  is an explicit user action.

## Data handling

For now, captured history lives in memory only and disappears when the process
exits. There is no disk persistence, export, or background upload.

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
