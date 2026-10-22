# Perseid documentation

[Project home](../README.md) · [Get started](getting-started.md) ·
[Workbench](workbench.md) · [Troubleshooting](troubleshooting.md)

Perseid is an **early prototype** for manual, authorized web security testing.
These guides describe the code on the branch you are reading. A planned feature
in the roadmap is not necessarily available in the application.

## New to Perseid?

1. [Install the prerequisites and capture a local request](getting-started.md).
2. [Configure HTTPS trust in a testing browser](tls.md).
3. [Learn the capture → intercept → replay → decode workflow](workbench.md).
4. [Check the safety and data-retention defaults](safety.md).

You do not need to trust a CA to try plain HTTP. Keep the proxy on loopback and
use a local lab or another target you are explicitly authorized to test.

## Guides

| Guide | What it answers |
| --- | --- |
| [Getting started](getting-started.md) | Which platforms and dependencies? How do I build, run, and configure the app? |
| [Workbench](workbench.md) | How do Target, Proxy, Repeater, and Decoder work? What are the shortcuts and limits? |
| [HTTPS interception](tls.md) | Where is the CA? How do I install or remove trust and configure pass-through hosts? |
| [Troubleshooting](troubleshooting.md) | Why is traffic missing, held, rejected, or failing TLS? What should go in a bug report? |
| [Safety and privacy](safety.md) | What is retained? What does scope enforce? What happens to credentials and CA keys? |
| [Screenshots](screenshots.md) | What does each tool look like? |

## Project and technical references

| Reference | What it covers |
| --- | --- |
| [Roadmap](roadmap.md) | Completed milestones, current work, and planned capabilities—not delivery dates. |
| [Burp Community parity](burp-parity.md) | Capability status, deliberate differences, and the definition of parity. |
| [Design](design.md) | Architecture, module boundaries, technology choices, and design goals. |
| [Performance and memory](performance.md) | Historical M0 measurements, their environment, and a reproduction method. |
| [Contributing](../CONTRIBUTING.md) | Toolchains, checks, headless development, screenshots, and documentation changes. |
| [Proxy library](../crates/perseid/README.md) | The `perseid::proxy` API, its defaults, and the headless example. |
| [Security policy](../SECURITY.md) | Private vulnerability reporting, supported versions, and safe-harbor terms. |
| [Burp and ZAP workflow research](burp-zap-workflows.md) | Dated research informing priorities; not a user manual or usage survey. |
| [Brand notes](../branding/BRAND-NOTES.md) | Visual identity, assets, and font licensing. |

## Platform status

**Linux is the CI reference platform.** Apple Silicon macOS has been built and
smoke-tested locally; there is no macOS CI job. Windows-specific code exists,
but Windows builds and setup are not validated by this repository's CI.

See [requirements](getting-started.md#requirements) before assuming that an OS
mentioned in a path or keyboard example has full release support.
