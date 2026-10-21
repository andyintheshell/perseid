# Security policy

Perseid is a local web security workbench: it intercepts HTTP traffic and
creates a local certificate authority. Captured traffic and the CA private key
are sensitive by design. This policy covers vulnerabilities in Perseid itself.

## Supported versions

Perseid is an early prototype. Security fixes are applied to `main` and to the
most recent tagged release, once releases are published. Older commits and
tags do not receive fixes.

| Version | Supported |
| --- | --- |
| `main` (development) | Yes |
| Latest tagged release | Yes, once published |
| Older releases and commits | No |

If you are testing a build from a branch, include the commit ID in your report.

## Reporting a vulnerability

**Please do not open a public issue containing exploit details or captured
traffic.** Use GitHub's private vulnerability reporting:

<https://github.com/andyintheshell/perseid/security/advisories/new>

If private reporting is unavailable, open a minimal public issue asking for a
private contact channel and do not include vulnerability details.

Please include, where possible:

- the affected version or commit, operating system, and build features;
- a description of the issue and its security impact;
- reproduction steps or a minimal proof of concept;
- relevant logs or configuration, with credentials, tokens, cookies, and
  personal data removed;
- whether you would like to be credited.

Never include live credentials, private keys, or another person's captured
traffic in a report.

## What to expect

- We aim to acknowledge reports within 3 business days, and will respond
  within 14 days.
- We will confirm the impact, agree on a disclosure timeline, and keep you
  informed of remediation progress.
- We prefer coordinated disclosure. We will credit reporters unless they ask to
  remain anonymous.
- This is a volunteer project without a bug bounty. We cannot offer payment.

## Scope

In scope:

- memory-safety, parsing, and denial-of-service bugs reachable from proxied or
  Repeater traffic;
- scope-enforcement bypasses and request/response rewriting that escapes its
  documented limits;
- flaws in local CA generation, storage permissions, certificate issuance, or
  the certificate download page;
- vulnerabilities in the build, CI, or release process that could affect
  users of published artifacts.

Out of scope:

- vulnerabilities in third-party dependencies; please report those upstream.
  We still want to know so we can update the dependency;
- misconfiguration, including exposing the proxy beyond loopback, installing
  the CA in a browser profile used for sensitive accounts, or disabling scope
  enforcement;
- attacks that require control of the same operating-system account or
  administrator access;
- the use of Perseid to test systems without authorization, or any traffic it
  captures in the course of authorized testing;
- social engineering and physical attacks.

Perseid is not a sandbox or an anonymity tool. It forwards traffic by design,
and traffic you intercept is visible to the application.

## Testing rules and safe harbor

Only test systems you own or have explicit permission to test. Do not access
data that is not yours, degrade services, or use Perseid to attack third
parties. Use synthetic accounts and data where possible.

We will not pursue legal action for good-faith research that follows this
policy and applicable law. If a report affects a third party, we may need to
coordinate with them before public disclosure.

## More information

- [Safety and privacy](docs/safety.md) — authorized use, data handling, and CA
  trust and removal.
- [HTTPS interception](docs/tls.md) — how the local CA works and how to remove
  it.
