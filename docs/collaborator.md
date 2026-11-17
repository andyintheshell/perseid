# Collaborator / OAST: Interactsh assessment

[Documentation](README.md) · [Roadmap](roadmap.md) · [Capability matrix](burp-parity.md)

**Status: planned, P0; not implemented.** Research checked **2026-11-06**.
Collaborator means an independently implemented out-of-band application security
testing (OAST) workflow, not compatibility with PortSwigger's service or protocol.

## Recommendation

Use **ProjectDiscovery Interactsh as the first backend**, behind a small
Perseid-owned provider interface. It already supplies payload registration and
callback collection, public and self-hosted servers, protected-server tokens,
and DNS, HTTP(S), and SMTP(S) interactions. Other protocols depend on the server's
configuration; do not promise them based on the client alone.

Prefer an in-process Rust integration over shipping a Go CLI sidecar. Evaluate
`interactsh` **0.2.4** first, but **do not approve the dependency yet**. If its
maintenance, licensing, or interoperability gates fail, evaluate a maintained
fork or a small independently implemented Interactsh adapter. Do not invent new
cryptography or tie the UI/project format to one crate's types.

This is a documentation and public-source assessment, not a build, dependency
security audit, or live-server interoperability test. No target probes or public
OAST registrations were performed.

## Rust client comparison

| Candidate | Evidence | Decision |
| --- | --- | --- |
| [`interactsh-rs` 0.3.1](https://github.com/pyroraptor07/interactsh-rs) | Registration, polling, parsed logs, and deregistration. The maintainer's 2025 notice explicitly says it is unmaintained, needs dependency updates/a rewrite, and is not recommended as-is. | Do not adopt as-is. |
| [`interactsh` 0.2.4](https://docs.rs/crate/interactsh/0.2.4) | Async Tokio/reqwest client; `Send + Sync`; nonce/context correlation; encrypted callback decoding; custom HTTP client, server/token configuration, retries/timeouts, poll-response byte limit, and documented secret redaction. Declared MSRV 1.85 is below Perseid's 1.95. | Best candidate for a spike, not a production endorsement. |

The newer crate's published API and source are available on docs.rs, but its
declared GitHub repository (`santhreal/interactsh`) returned **404** through both
GitHub's API and raw README retrieval during this review. That does not establish
why it is unavailable; it does mean maintenance and upstream review cannot be
assumed. Package metadata says `MIT OR Apache-2.0`, while the published README
says MIT. Inspect the shipped license files and provenance before adoption.

### Adoption gates and gaps

- **Wire compatibility:** the Rust crate defaults to a 14-character correlation
  ID and 16-character nonce. Upstream currently documents **20 + 13**, and says
  client/server lengths must match. Configure lengths explicitly for the chosen
  server and prove DNS, HTTP, HTTPS, and SMTP correlation end to end. Registration
  success alone is not proof that callbacks work.
- **Session lifecycle:** the public `InteractshClient` API lists generation,
  polling, and mapping removal, but no session export/import, keepalive, or
  deregistration method. Configuration serialization is **not** session restore:
  resume needs the correlation ID, secret, private key, and payload mappings.
  Confirm server expiry behavior and implement or contribute missing lifecycle
  support. Never imply that saved events mean old payloads can still be polled.
- **Resource limits:** the crate limits poll response bytes (10 MiB by default)
  and exposes `forget`/`tracked_count`; Perseid still needs bounded mappings,
  decoded events, queues, retries, deduplication, and visible truncation/expiry.
  Registration/key generation and polling must stay off the UI thread.
- **Security and packaging:** review the pinned dependency graph, crypto
  advisories, licenses, Rustls/native TLS choices, redirects, proxy routing,
  token/error redaction, and Linux/macOS/Windows builds. A redacted `Debug`
  implementation does not guarantee every transport error is safe to log.
- **Correlation:** unknown or forgotten nonces are filtered by this crate.
  Preserve mappings for the documented payload lifetime, handle duplicate and
  delayed callbacks, and test protocol-specific IDs and malformed encrypted data.

## First usable workflow

Tracked in the [P0 implementation issue #191](https://github.com/andyintheshell/perseid/issues/191).

1. Explicitly enable a server for the active project; explain public-service
   disclosure before registration. Support a custom authenticated server.
2. Generate/copy a unique hostname or HTTP(S) URL; insert a payload into a
   Repeater request or Intruder payload set and retain its source association.
3. Poll manually or in a cancellable background loop. Show connection/expiry
   state, last successful poll, errors, and new-event indicators.
4. Inspect protocol, time, source address, payload/source request, and bounded
   raw interaction details. A callback is evidence to investigate, not an
   automatic vulnerability verdict; absence of a callback proves nothing.
5. Save generated-payload metadata and received evidence with a project, with
   explicit sensitive-data warnings. Decide separately whether credential/session
   persistence is safe enough to support; otherwise label sessions non-resumable.

## Safety requirements

- Off by default: no registration, polling, or public-server fallback on startup
  or project open. Server changes require an explicit user choice.
- Public infrastructure sees callback traffic and source addresses; application
  secrets may leak through tested requests. Encryption in the protocol is not a
  guarantee against the server operator. Recommend self-hosting for sensitive
  engagements; public endpoints may rotate or be unavailable.
- Verify control-plane HTTPS certificates. No silent HTTP downgrade or global
  TLS-verification bypass; configure explicit trust for a private server.
- Treat OAST control traffic separately from target scope and the proxy cookie
  jar. Never send target cookies/credentials to the provider. Payload injection
  does not authorize testing an out-of-scope target.
- Stop workers and reject stale results on project switch, disable, or quit.
  Never mix payloads, credentials, or events across projects. Do not automatically
  browse callback URLs, execute returned content, or render captured HTML.

## Validation before shipping

Use deterministic mock-server tests for registration/authentication, encrypted
DNS/HTTP/SMTP events, empty/null polls, duplicates, delays, unknown IDs, malformed
JSON/base64/crypto, oversized responses, timeouts, cancellation, expiry, and
cross-project isolation. Test credential redaction and save/reopen behavior.
Then run an authorized self-hosted Interactsh lab with matching lengths and
record server/client versions and results. Public-server checks should be manual
and explicitly opted into, never a normal CI dependency.

## Sources

- [Upstream Interactsh README](https://github.com/projectdiscovery/interactsh#readme): protocols, self-hosting, tokens, session files, payload lengths, and public-service caveats. Interactsh is MIT-licensed.
- [Published Rust crate and dependency metadata](https://docs.rs/crate/interactsh/0.2.4).
- [Client API](https://docs.rs/interactsh/0.2.4/interactsh/struct.InteractshClient.html) and [configuration](https://docs.rs/interactsh/0.2.4/interactsh/struct.ClientConfig.html).
- [Published configuration source](https://docs.rs/interactsh/0.2.4/src/interactsh/config.rs.html) and [client source](https://docs.rs/interactsh/0.2.4/src/interactsh/client/core.rs.html): defaults, bounds, registration, and correlation.
- [Older client's maintenance notice](https://github.com/pyroraptor07/interactsh-rs#-2025-maintenance-status-update-).
