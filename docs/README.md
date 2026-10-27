# Perseid documentation

[Project home](../README.md) · [Get started](getting-started.md) ·
[Workbench](workbench.md) · [Troubleshooting](troubleshooting.md)

Perseid is an **early prototype** for manual, authorized web security testing.
These guides describe the code on the branch you are reading. A planned feature
in the roadmap is not necessarily available in the application.

The latest published release, Perseid `0.2.1`, includes the local
project-persistence support introduced in `0.2.0`. The `main` branch is preparing `0.3.0-dev`; install
the published crate or a prebuilt release, or build from source for current
development.

## New to Perseid?

1. [Install the prerequisites and capture a local request](getting-started.md).
2. [Configure HTTPS trust in a testing browser](tls.md).
3. [Learn the capture → intercept → replay → decode → compare workflow](workbench.md).
4. [Check the safety and data-retention defaults](safety.md).

You do not need to trust a CA to try plain HTTP. Keep the proxy on loopback and
use a local lab or another target you are explicitly authorized to test.

## Guides

| Guide | What it answers |
| --- | --- |
| [Getting started](getting-started.md) | Which platforms and dependencies? How do I build, run, and configure the app? |
| [Workbench](workbench.md) | How do Target, Proxy, Intruder, Repeater, Sequencer, Decoder, and Comparer work? What are the shortcuts and limits? |
| [HTTPS interception](tls.md) | Where is the CA? How do I install or remove trust and configure pass-through hosts? |
| [Troubleshooting](troubleshooting.md) | Why is traffic missing, held, rejected, or failing TLS? What should go in a bug report? |
| [Safety and privacy](safety.md) | What is retained? What does scope enforce? What happens to credentials and CA keys? |
| [Screenshots](screenshots.md) | What does each tool look like? |
| [Demo GIF storyboard](demo-gif-script.md) | What does the first-time-user demo show, and in what order? |
| [Releasing](releasing.md) | How are platform packages built, reviewed, and published? |

## Project and technical references

| Reference | What it covers |
| --- | --- |
| [Roadmap](roadmap.md) | Completed milestones, current work, and planned capabilities—not delivery dates. |
| [Burp Community parity](burp-parity.md) | Capability status, deliberate differences, and the definition of parity. |
| [Design](design.md) | Architecture, module boundaries, technology choices, and design goals. |
| [Project storage design](storage-design.md) | Original storage proposal, implementation status, deviations, and remaining work. |
| [Performance and memory](performance.md) | Historical M0 measurements, their environment, and a reproduction method. |
| [Contributing](../CONTRIBUTING.md) | Toolchains, checks, headless development, screenshots, and documentation changes. |
| [Proxy library](../crates/perseid/README.md) | The `perseid::proxy` API, its defaults, and the headless example. |
| [Security policy](../SECURITY.md) | Private vulnerability reporting, supported versions, and safe-harbor terms. |
| [Burp and ZAP workflow research](burp-zap-workflows.md) | Dated research informing priorities; not a user manual or usage survey. |
| [Brand notes](../branding/BRAND-NOTES.md) | Visual identity, assets, and font licensing. |

## Platform status

**Linux is the native-dependency and screenshot reference platform.** The
required CI workflow runs formatting, Clippy, tests, and package checks in
parallel on Linux, GitHub-hosted Apple Silicon macOS, and Windows with MSVC.
The Linux screenshot workflow remains a separate Xvfb job. Cross-platform CI
does not install-test release packages or validate every desktop/graphics setup.

GitHub Releases now include Windows, macOS, and Linux packages. Windows
packages are unsigned, and macOS is ad-hoc signed but not notarized. Release
builds do not validate every desktop or graphics setup; see the
[release guide](releasing.md) and [platform requirements](getting-started.md#requirements).
