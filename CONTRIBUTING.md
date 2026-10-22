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
- A graphical desktop with OpenGL support to run the UI. CI tests do not open
  the app window; the screenshot workflow is a separate Linux/Xvfb check.

The required CI workflow runs format, Clippy, tests, and packaging in three
parallel jobs: Linux, macOS on Apple Silicon, and Windows. Linux installs its
native UI build dependencies; the other runners use their hosted platform
toolchains. This is build/test coverage, not a claim that release packaging or
every desktop environment is supported equally on all platforms.

From the repository root:

```sh
cargo build --locked
cargo run -p perseid --locked
cargo test --workspace --locked
```

The `dev` and `release` profiles set `debug = 0` so build artifacts and linker
memory stay small on constrained machines. `test` inherits `dev`, and `bench`
inherits `release`. This removes compiler debug info only; it does not change
runtime behavior or disable `debug_assertions`. When you need source-level
debugging, enable it for a single build without editing the manifest:

```sh
CARGO_PROFILE_DEV_DEBUG=2 cargo build
CARGO_PROFILE_TEST_DEBUG=2 cargo test --workspace --locked
```

Use `line-tables-only` instead of `2` for a smaller middle ground that still
supports line-level stepping.

Cargo is limited to two parallel build jobs by the repository's
`.cargo/config.toml`. This keeps peak rustc and linker memory bounded on small
VMs. Override it for a faster build on a machine with more headroom, or reduce
it further if the machine still struggles:

```sh
CARGO_BUILD_JOBS=4 cargo build
CARGO_BUILD_JOBS=1 cargo test --workspace --locked
```

CI sets `CARGO_BUILD_JOBS=4` on all three runners to keep the same check shape;
reduce the local job count if your machine has less memory.

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
seeds a deterministic demo history, captures every documented UI tab from the
egui framebuffer, and writes the PNGs to `docs/images/`. It requires `xvfb`,
`xauth`, and Python 3 (for the PNG check). Use `--out DIR` to write elsewhere,
or `--no-build` to reuse an existing `target/debug/perseid`.

The screenshots contain synthetic demo traffic, never captured traffic. The
separate [Screenshots workflow](.github/workflows/screenshots.yml) runs the
same script on UI changes, verifies the PNGs, uploads them as artifacts, and
posts or updates a pull-request comment containing the full-window preview of
every tab. The pull-request workflow always starts and uses a small
change-detection job to skip the screenshot build when no relevant paths
changed. That keeps a skipped run from leaving a required check pending.

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
- For UI changes, include a screenshot. Every UI tab must be represented; the
  Screenshots workflow posts or updates a comment with all tab previews for
  same-repository pull requests. If that automatic comment is unavailable
  (for example, on a fork), link the workflow artifact or attach the previews
  to the pull request yourself.
- Update the [roadmap](docs/roadmap.md), [parity matrix](docs/burp-parity.md),
  or user docs when a change affects them.

## License

By contributing, you agree that your contributions are licensed under the
project's dual license, MIT OR Apache-2.0.
