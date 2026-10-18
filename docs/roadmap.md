# Roadmap

The north star is feature parity with Burp Suite Community Edition's manual
toolkit. See [Burp Community parity](burp-parity.md) for the capability matrix,
definition of done, and deliberate deviations.

## Milestones

- [ ] **Milestone 0 — Proof of concept:** benchmark idle memory, history
  rendering, and large-response viewing on modest Linux hardware.

  *Status: the prototype runs on Kali/X11 at 200% scaling; formal
  measurements are still pending.*

- [ ] **Milestone 1 — Proxy core:** reliable HTTP/1.1 capture and forwarding,
  request/response interception with edit, forward, and drop, explicit scope
  configuration, and match/replace rules.

  *Status: capture, forwarding, and the history UI work; interception, scope,
  and match/replace are not implemented.*

- [ ] **Milestone 2 — HTTPS:** local CA generation, TLS interception,
  documented browser trust setup, and pass-through controls.

  *Status: `CONNECT` tunneling works as TLS passthrough with byte counts;
  interception is not implemented.*

- [ ] **Milestone 3 — Daily workbench:** SQLite-backed persistent history,
  search, Repeater, Decoder, and response diff (Comparer).

  *Status: in-memory history and basic filtering work.*

- [ ] **Milestone 4 — Protocols and testing tools:** HTTP/2 and WebSocket
  proxying with history, Sequencer, and bounded fuzzing covering Burp CE's
  Intruder demo.

  *Status: HTTP/1.1 only.*

- [ ] **Milestone 5 — Parity hardening:** packaging, crash recovery, safe
  handling of sensitive traffic, accessibility, performance benchmarks, and a
  documented parity audit.

  *Status: tests, rustfmt, clippy, and CI are in place; packaging and
  benchmarks are not.*

- [ ] **After parity:** native extension point, passive and guided checks,
  import/export, integrations, and additional protocol coverage.

Priorities may change based on real-world use and measurements. Safety and
scope defaults are not deferred: each milestone lands with the relevant
protections documented.
