# Roadmap

[Documentation](README.md) · [Capability matrix](burp-parity.md) ·
[Contributing](../CONTRIBUTING.md)

**Direction: Collaborator first, a usable daily workbench, Burp Community
manual-workflow parity next, and Burp Professional parity as the longer-term
goal.** Perseid remains independently implemented; equivalent workflows matter
more than copying another product's UI or implementation.

## Priorities, not numbered milestones

The old milestone sequence is retired. Use issue priority labels and focus
labels to select work; closed milestones remain historical records, not release
gates. The [live backlog (#73)](https://github.com/andyintheshell/perseid/issues/73)
is the issue index. This page explains direction, the
[capability matrix](burp-parity.md) describes shipped behavior, and release notes
describe specific versions. None is a delivery-date promise.

| Priority | Meaning |
| --- | --- |
| **P0** | Immediate focus: Collaborator/OAST and critical safety, data-loss, or unusable-core-workflow blockers. |
| **P1** | Usability, reliability, accessibility, release quality, and important Community workflow gaps. |
| **P2** | Planned refinements and remaining parity work after P0/P1. |
| **P3** | Longer-term discovery, Pro parity, extensibility, and opportunistic work unless explicitly promoted. |

Focus labels are `focus: collaborator`, `focus: usability`,
`focus: community parity`, and `focus: pro parity`. They can overlap; a focus is
not a priority. Pro-class OAST is deliberately P0 **before** Community parity.
Critical defects can interrupt feature work. Safety, scope, privacy, and bounded
resource handling are part of every feature, not a final hardening phase.

## P0 — Collaborator / OAST

Deliver an integrated manual out-of-band testing workflow: generate payloads,
use them in Repeater/Intruder, poll asynchronously, correlate callbacks to their
source, inspect evidence, and retain it safely with the project. Support an
explicitly selected public or authenticated self-hosted Interactsh server.

**Not available today.** Interactsh is the recommended first backend, not an
approved Rust dependency yet. The newer `interactsh` crate is a spike candidate;
the older `interactsh-rs` is unmaintained. See the
[P0 implementation issue #191](https://github.com/andyintheshell/perseid/issues/191)
and [client assessment, adoption gates, and safety requirements](collaborator.md).
No PortSwigger service/protocol compatibility is promised.

## Make the daily workbench dependable

Do not defer usability until all features exist. Prioritize reproducible user
friction, responsiveness, clear errors, predictable state, and safe project
boundaries alongside the P0 work:

- Nonblocking project controls and Search details: [#136](https://github.com/andyintheshell/perseid/issues/136), [#138](https://github.com/andyintheshell/perseid/issues/138).
- Project-isolated cookies and Repeater scope guardrails: [#174](https://github.com/andyintheshell/perseid/issues/174), [#176](https://github.com/andyintheshell/perseid/issues/176).
- Effective sent requests, explicit protocol selection, and immutable Repeater
  history: [#171](https://github.com/andyintheshell/perseid/issues/171), [#172](https://github.com/andyintheshell/perseid/issues/172), [#173](https://github.com/andyintheshell/perseid/issues/173).
- Settings, recent projects, filtering, crash recovery, and accessibility:
  [#58](https://github.com/andyintheshell/perseid/issues/58), [#152](https://github.com/andyintheshell/perseid/issues/152), [#57](https://github.com/andyintheshell/perseid/issues/57), [#66](https://github.com/andyintheshell/perseid/issues/66), [#69](https://github.com/andyintheshell/perseid/issues/69).
- Durable Intruder results and bounded long attacks:
  [#177](https://github.com/andyintheshell/perseid/issues/177), [#137](https://github.com/andyintheshell/perseid/issues/137).
- Trusted, clean-machine-tested packages: [#56](https://github.com/andyintheshell/perseid/issues/56).

Normal quit already protects unsaved work and acknowledges saved workspace
writes; that is not crash recovery. Recent projects must not silently reopen
files. See [exit guarantees](project-exit-guarantees.md) and
[Safety and privacy](safety.md).

## Reach Community manual-workflow parity

The near-term baseline is a complete, practical manual loop: Proxy, Target,
HTTP/WebSocket history and interception, Repeater, Intruder, Decoder, Comparer,
and Sequencer. Many foundations are already available; the matrix records exact
limits rather than treating a tool's existence as complete parity.

Finish remaining workflows such as history filters/annotations, Intruder payload
processing and saved attacks, Repeater behavior, and Sequencer collection.
Publish a repeatable, versioned [parity audit (#68)](https://github.com/andyintheshell/perseid/issues/68)
with evidence and linked gaps. The historical 82.1% scoped estimate is not an
overall product-completeness score or a reason to ignore usability bugs.

## Longer-term — Professional parity

After a dependable Community baseline, expand toward Burp Professional's
workflows: crawling/discovery, passive and active checks, findings/triage and
reporting, richer session/authentication automation, and integrations. Search,
project persistence, and unthrottled fuzzing already go beyond Community in
selected areas; Collaborator is being prioritized early rather than waiting.

This is a **longer-term goal, not current support or a full Pro-parity claim**.
Define each capability and its safety model in a scoped design issue before
implementation, starting with the [Pro scope inventory #192](https://github.com/andyintheshell/perseid/issues/192).
Scanner/crawler work is deferred, no longer a permanent
non-goal. Keep the external-browser approach and no BApp binary compatibility
as deliberate differences; native extensibility is tracked by
[#72](https://github.com/andyintheshell/perseid/issues/72).

## Evidence required to call work done

- A usable end-to-end workflow with actionable failure and cancellation states.
- Automated success/error/edge-path tests and a recorded manual check where UI
  or platform behavior matters; performance claims need measurements.
- Documented scope, privacy, persistence, retention, and resource limits.
- Updated guides and capability status, with remaining gaps explicitly tracked.

Test only local labs and systems you own or are explicitly authorized to assess.
Priorities can change with user reports and measured behavior; closing an old
milestone never counts as implementing its open issues.
