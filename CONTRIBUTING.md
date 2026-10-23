# Contributing

[Project home](README.md) · [Documentation](docs/README.md) ·
[Design](docs/design.md) · [Security policy](SECURITY.md)

Contributions, testing feedback, and design discussions are welcome as the
prototype takes shape. Small, focused changes with tests and documentation are
the easiest to review.

## Before you start

- For larger features, open an issue first so the approach can be discussed
  against the [roadmap](docs/roadmap.md) and [design](docs/design.md).
- Never include captured credentials, tokens, personal data, or other
  sensitive traffic in issues, pull requests, or test fixtures.

## Development setup

The manifest minimum is Rust 1.95 (edition 2024). `rust-toolchain.toml` and CI
select **1.95.0**. With rustup, that selection is automatic. Homebrew's `rust`
formula supplies its current toolchain instead; it is suitable for local builds
but may produce different compiler or Clippy diagnostics. Include the toolchain
version when reporting a failure.

Follow [platform setup](docs/getting-started.md#requirements) for native
dependencies. A graphical run needs a desktop session and OpenGL. CI pins Ubuntu
26.04, macOS 26 on Apple Silicon, and Windows 2025, and runs format, Clippy,
tests, and packaging in three parallel jobs. Linux installs its desktop build
dependencies. After installing the pinned toolchain, each job removes the runner
image's unrelated floating `stable` Rust toolchain before caching; this keeps
cache keys stable as hosted images are refreshed. This verifies cross-platform
builds and tests, not release installers or every graphics configuration. The
screenshot workflow remains a separate Ubuntu 26.04/Xvfb check.

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

Perseid creates its TLS certificate authority on first launch and reuses the
same per-user CA for debug and release runs. That is deliberate: a development
run keeps the certificate your browser already trusts. For experiments that
should not touch your trusted CA, point the app at a scratch directory:

```sh
PERSEID_CA_DIR="$(mktemp -d)" cargo run -p perseid --locked
```

Do not run Cargo or Perseid as root. Do not use a developer run to experiment
with a CA your everyday browser trusts.

## Headless proxy and library API

The single `perseid` package contains both the desktop app and the
`perseid::proxy` library. A terminal-only example is available:

```sh
cargo run -p perseid --example headless --locked
```

It listens on `127.0.0.1:8080` and prints capture events without opening a
window. It still builds the same crate and native/GUI dependencies. Use the
[local target](docs/getting-started.md#try-it-with-a-local-target) for traffic;
do not run the desktop listener on the same port at the same time.

**Library defaults differ from the desktop app.** `ProxyConfig::default()` does
not create a CA or enable TLS decryption. Rust callers explicitly supply an
authority and policy when needed; the desktop app owns CA filesystem policy and
enables HTTPS interception at startup. See the
[crate README](crates/perseid/README.md) and
[headless example source](crates/perseid/examples/headless.rs).

Generate local API documentation with:

```sh
cargo doc -p perseid --no-deps --locked --open
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
Homebrew's Rust formula includes the tools used here. For changes touching
screenshot-mode code, also run `cargo test --workspace --all-features --locked`.

[CodeQL](.github/workflows/codeql.yml) runs separately from these local Cargo
checks. Actions and Python scans run on every pull request. Rust analysis runs
on PRs that touch `crates/`, Rust build inputs, or the CodeQL workflow itself;
it also runs on pushes to `main`, the weekly schedule, and manual dispatches.
The path filter includes crate README changes, not just `.rs` files.

## Testing and coverage

Tests follow the standard Rust layout:

- Unit tests live in the same file as the code they cover, in a
  `#[cfg(test)] mod tests` module at the bottom. Most helpers and pure logic
  are private, so same-file tests check them without widening the module API.
- End-to-end tests live in `crates/perseid/tests/` and drive the public
  `perseid::proxy` API over loopback sockets (history capture, interception,
  scope, TLS, HTTP/2, WebSockets, Repeater, Intruder).
- If a test module grows to dwarf the code it covers, it may move to a
  `src/<module>/tests.rs` submodule, but same-file placement is the default.

Extract selection math, formatters, parsers, and other pure helpers out of
large `egui` draw functions so they can be unit-tested; painting itself is
usually not worth testing. When adding behavior, cover the error and empty
paths, not just the happy path.

Coverage is measured by [Codecov](.github/workflows/coverage.yml) with
`cargo llvm-cov` on every pull request and on `main`. The workflow measures
the default feature set; `screenshots`-only code is compiled and exercised by
the Screenshots workflow instead. To run the same measurement locally:

```sh
cargo install cargo-llvm-cov --version 0.9.1 --locked
cargo llvm-cov --workspace --locked --lcov --output-path lcov.info
```

With rustup, add the matching `llvm-tools-preview` component first; with a
distribution Rust package, install the distribution's LLVM tools. Coverage
reports are a guide: new logic should come with tests in the same pull
request, and a drop in coverage for the code you touched should be explained
in the PR description.

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

The README screenshots are generated on Linux/Xvfb, without an interactive
desktop session:

```sh
scripts/screenshots.sh
```

This builds `perseid` with the `screenshots` feature, runs it under Xvfb,
seeds a deterministic demo history, captures every documented UI tab from the
egui framebuffer, and writes the PNGs to `docs/images/`. It requires `xvfb`,
`xauth`, and Python 3 (for the PNG check). Use `--out DIR` to write elsewhere,
or `--no-build` to reuse an existing `target/debug/perseid` built with the
`screenshots` feature. The script also uses the Linux `timeout` utility. It is
not a macOS screenshot command; macOS contributors can use the workflow artifacts.

The screenshots contain synthetic demo traffic, never captured traffic. The
separate [Screenshots workflow](.github/workflows/screenshots.yml) runs the
same script on UI changes, verifies the PNGs, uploads them as artifacts, and
posts or updates a pull-request comment with one link to the generated Markdown
gallery page; it no longer embeds every image in the PR thread. Forks can use
the workflow artifact. The pull-request workflow always starts and uses a small
change-detection job to skip the screenshot build when no relevant paths
changed. That keeps a skipped run from leaving a required check pending.

## Project layout

| Path | Contents |
| --- | --- |
| `crates/perseid/src/proxy/` | Embedded proxy: listener, forwarding, CONNECT tunnels, capture, events, and the direct Repeater client. |
| `crates/perseid/src/project/` | redb project schema, records, writer, queries, and Save As/Copy transfer. |
| `crates/perseid/src/app/` | egui desktop modules: project UI, history views, tools, theme, and keyboard handling. |
| `crates/perseid/tests/` | End-to-end proxy and Repeater tests. |
| `crates/perseid/examples/` | Headless proxy smoke-test example. |
| `docs/` | [User guides and technical references](docs/README.md). |
| `branding/` | Logo and icon source assets. |

Proxy behavior belongs behind the `perseid::proxy` API so it can be tested
without the UI. UI code should not block on network or disk work; events flow
through the bounded channel.

## Documentation changes

- Keep the [README](README.md) a short entry point; put workflow detail in the
  [workbench guide](docs/workbench.md), setup in
  [Getting started](docs/getting-started.md), and fixes in
  [Troubleshooting](docs/troubleshooting.md).
- Add a new guide to the [documentation index](docs/README.md) and link back to
  related pages. Use relative links so navigation stays on the reader's branch.
- Verify local file links, heading anchors, screenshots, and commands from the
  repository root. Check CLI examples against `cargo run -p perseid --locked -- --help`.
- Describe what the current code does. Keep planned capabilities labelled,
  distinguish desktop from library defaults, and do not advertise untested
  platforms or historical benchmarks as current release guarantees.
- Use synthetic examples. Preserve warnings about CA trust, sensitive traffic,
  temporary versus saved project state, plaintext project files, and Repeater
  bypassing proxy scope.
- Keep `Cargo.lock` in use with `--locked` in reproducible build/run examples.
  Do not change dependency versions just to update documentation.

## Pull requests

- Keep changes focused; separate refactors from behavior changes where
  practical.
- Add or update tests for proxy, history, and scope behavior.
- For UI changes, include a screenshot. Every UI tab must be represented; the
  Screenshots workflow posts or updates a comment with one link to the full
  gallery for same-repository pull requests. If that automatic comment is
  unavailable (for example, on a fork), link the workflow artifact or attach
  the previews to the pull request yourself.
- Update the [roadmap](docs/roadmap.md), [parity matrix](docs/burp-parity.md),
  or user docs when a change affects them.

## License

By contributing, you agree that your contributions are licensed under the
project's dual license, MIT OR Apache-2.0.
