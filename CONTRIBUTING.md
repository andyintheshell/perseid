# Contributing

Contributions, testing feedback, and design discussions are welcome as the
prototype takes shape. Small, focused changes with tests and documentation are
the easiest to review.

## Before you start

- For larger features, open an issue first so the approach can be discussed
  against the [roadmap](docs/roadmap.md) and [design](docs/design.md).
- Never include captured credentials, tokens, personal data, or other
  sensitive traffic in issues, pull requests, or test fixtures.

## Development setup

Requirements:

- Rust 1.95.0 or newer (edition 2024), matching `rust-toolchain.toml` and CI.
- A Linux desktop session with OpenGL support for the UI.

From the repository root:

```sh
cargo build --locked
cargo run -p perseid --locked
cargo test --workspace --locked
```

The `perseid` crate includes a headless proxy example for manual smoke tests:

```sh
cargo run -p perseid --example headless
```

Perseid creates its TLS certificate authority on first launch and reuses the
same per-user CA for debug and release runs. That is deliberate: a development
run keeps the certificate your browser already trusts. For experiments that
should not touch your trusted CA, point the app at a scratch directory:

```sh
PERSEID_CA_DIR="$(mktemp -d)" cargo run -p perseid
```

## Required checks

Before sending a change, run the same checks as CI:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

Install the tools with `rustup component add rustfmt clippy`. On Kali/Debian
systems using the distribution's Rust packages, install `rustfmt` and
`rust-clippy` through apt instead, keeping them in step with `rustc`.

## Dependencies

`Cargo.lock` is tracked. It is the exact, reproducible resolution used by
local builds, releases, and CI; every Cargo check in CI runs with `--locked`
so a manifest change that needs a lockfile update fails instead of silently
resolving to newer crates.

Workspace manifests deliberately use semver-compatible ranges (for example,
`egui = "0.37"`) rather than `=` pins. The lockfile supplies the pin for
applications and CI, while ranges let downstream consumers of the `perseid`
library share a compatible version. When a dependency is added or
upgraded, run `cargo update` intentionally and include the resulting
`Cargo.lock` change in the same pull request. Do not commit unrelated
lockfiles from other toolchains.

## Screenshots

The README screenshots are generated headlessly, without a desktop session:

```sh
scripts/screenshots.sh
```

This builds `perseid` with the `screenshots` feature, runs it under Xvfb,
seeds a deterministic demo history, captures the egui framebuffer, and writes
the two PNGs to `docs/images/`. It requires `xvfb`, `xauth`, and Python 3 (for
the PNG check). Use `--out DIR` to write elsewhere, or `--no-build` to reuse an
existing `target/debug/perseid`.

The screenshots contain synthetic demo traffic, never captured traffic. The
separate [Screenshots workflow](.github/workflows/screenshots.yml) runs the
same script on UI changes, verifies the PNGs, uploads them as artifacts, and
publishes an inline preview.

## Project layout

| Path | Contents |
| --- | --- |
| `crates/perseid/src/proxy/` | Embedded proxy: listener, forwarding, CONNECT tunnels, capture, events, and the direct Repeater client. |
| `crates/perseid/src/` | egui desktop application: history model, site map, theme, views, keyboard handling. |
| `crates/perseid/tests/` | End-to-end proxy and Repeater tests. |
| `crates/perseid/examples/` | Headless proxy smoke-test example. |
| `docs/` | User and design documentation. |
| `branding/` | Logo and icon source assets. |

Proxy behavior belongs behind the `perseid::proxy` API so it can be tested
without the UI. UI code should not block on network or disk work; events flow
through the bounded channel.

## Pull requests

- Keep changes focused; separate refactors from behavior changes where
  practical.
- Add or update tests for proxy, history, and scope behavior.
- For UI changes, include a screenshot or a short note describing the manual
  check you performed.
- Update the [roadmap](docs/roadmap.md), [parity matrix](docs/burp-parity.md),
  or user docs when a change affects them.

## License

By contributing, you agree that your contributions are licensed under the
project's dual license, MIT OR Apache-2.0.
