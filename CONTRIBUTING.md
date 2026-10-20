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

From the workspace root:

```sh
cargo build
cargo run -p perseid
cargo test --workspace
```

The `perseid-proxy` crate also has a headless example for manual smoke tests:

```sh
cargo run -p perseid-proxy --example headless
```

## Required checks

Before sending a change, run the same checks as CI:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Install the tools with `rustup component add rustfmt clippy`. On Kali/Debian
systems using the distribution's Rust packages, install `rustfmt` and
`rust-clippy` through apt instead, keeping them in step with `rustc`.

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
| `crates/perseid-proxy/` | Proxy library: listener, forwarding, CONNECT tunnels, capture, events. |
| `crates/perseid/` | egui desktop application: history model, site map, theme, views, keyboard handling. |
| `docs/` | User and design documentation. |
| `branding/` | Logo and icon source assets. |

Proxy behavior belongs behind the `perseid-proxy` API so it can be tested
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
