# Getting started

[Documentation](README.md) · [Workbench](workbench.md) ·
[HTTPS setup](tls.md) · [Troubleshooting](troubleshooting.md)

Install the latest published Perseid crate with Cargo, download a prebuilt
binary from GitHub Releases, or build from source. Then capture a request from a
local target and connect a testing browser. No certificate installation is
needed for the first plain-HTTP walkthrough.

## Requirements

- **Rust 1.95 or newer**, with Cargo, when installing with `cargo install` or
  building from source. `rust-toolchain.toml` pins 1.95.0 for rustup users; the
  manifest minimum is 1.95 and the crate uses edition 2024. Prebuilt downloads
  do not require Rust.
- A graphical desktop with OpenGL support; X11 or Wayland on Linux.
- A browser or HTTP client that supports an HTTP proxy. Git is needed only for a
  source checkout.
- Python 3 and curl for the optional local-target walkthrough below.

| Platform | Validation status |
| --- | --- |
| Linux | CI-tested on Ubuntu 26.04; x86-64/ARM64 AppImage and DEB downloads are available. Packages are built on Ubuntu 22.04 and need compatible desktop/graphics libraries. The published memory baseline is from a Kali ARM64 VM. |
| macOS | CI-tested on the GitHub-hosted macOS 26 Apple Silicon runner. A universal Intel/Apple Silicon DMG is available; it is ad-hoc signed but not notarized. |
| Windows | CI-tested on the GitHub-hosted Windows 2025 x64 runner. x64/ARM64 MSI and no-install ZIP downloads are available; Windows packages are unsigned. Graphics-driver and clean-install coverage is limited. |

## Install a published release

### Build the latest crates.io version

This compiles Perseid locally and requires Rust plus the platform's build
dependencies:

```sh
cargo install perseid --locked
perseid
```

Perseid `0.2.1` is published on crates.io and includes persistent projects.
Prebuilt packages are available from GitHub Releases. A source checkout of
`main` contains ongoing `0.3.0-dev` development.

### Download a prebuilt binary

Choose a platform package from
[GitHub Releases](https://github.com/andyintheshell/perseid/releases/latest):

| Platform | Downloads |
| --- | --- |
| Windows | x64 or ARM64 `.msi` installer, or `-portable.zip` |
| macOS | Universal `.dmg` containing `Perseid.app` |
| Linux | x86-64 or ARM64 `.AppImage` or `.deb` |

`SHA256SUMS.txt` lists checksums. Windows packages are unsigned, and the macOS
DMG is not notarized, so the operating system may warn or block launch. The
AppImage and DEB still require compatible host desktop and graphics libraries.

### Linux

Install Rust through [rustup](https://www.rust-lang.org/tools/install), or use
distribution packages that meet the minimum version. With rustup, the repository
selects its pinned toolchain automatically.

For `cargo install` or a source build on Debian, Ubuntu, or Kali, install the
native build dependencies:

```sh
sudo apt-get update
sudo apt-get install -y build-essential pkg-config libssl-dev \
  libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev \
  libxkbcommon-dev libwayland-dev
```

Other distributions use different package names. You also need a working
desktop OpenGL driver; building the binary alone does not provide a display
server or graphics stack.

### macOS with Homebrew

Install Apple's Command Line Tools if missing (`xcode-select --install`) and
[Homebrew](https://brew.sh/), then:

```sh
brew install rust
rustc --version
cargo --version
```

Homebrew supplies the current Rust toolchain rather than the exact rustup pin.
That is fine for a local build when it meets the manifest minimum, but it is not
the same compiler/linter version as CI. If an existing rustup installation
shadows Homebrew, select Homebrew's toolchain for the current shell:

```sh
export PATH="$(brew --prefix rust)/bin:$PATH"
```

Package-manager or developer-tools installation may require administrator
approval. **Do not run Cargo or Perseid with sudo.** The default loopback port
and per-user CA directory do not require root.

CI pins its runners to Ubuntu 26.04, macOS 26 (Apple Silicon), and Windows 2025.
It runs formatting, linting, tests, and Cargo package checks on all three
platforms in parallel using Rust 1.95.0. The Linux job installs its native
desktop dependencies; the macOS job verifies that its hosted runner is arm64.
Before caching, CI removes each image's unrelated floating `stable` Rust
toolchain so image updates do not invalidate the cache for the pinned project
toolchain. The test suite runs headlessly. Running the app still requires a
working desktop and graphics stack. CI does not install-test release packages
or promise support for every desktop and graphics configuration.

## Build from the repository

```sh
git clone https://github.com/andyintheshell/perseid.git
cd perseid
cargo run -p perseid --locked
```

The app opens on **Proxy > HTTP history** and starts its proxy on
`127.0.0.1:8080`. On first launch it creates a local certificate authority and
opens a setup window. This does **not** install trust in a browser or OS store.
Existing CAs are reused; updating the app does not itself rotate the CA. See
[CA storage and lifecycle](tls.md#where-the-ca-lives) for the generation layout
and compatibility with older installations.

For an optimized binary:

```sh
cargo build --release -p perseid --locked
./target/release/perseid
```

On macOS this produces a command-line-launched desktop executable, not a
packaged `.app` or installer. Use the GitHub release DMG for a prebuilt app.

To explore the interface without starting the listener:

```sh
cargo run -p perseid --locked -- --no-proxy
```

UI-only mode still prepares the local CA. For a run that must not touch your
usual CA, set `--ca-dir` to a dedicated scratch directory; see
[contributor setup](../CONTRIBUTING.md#development-setup).

## Try it with a local target

Keep Perseid running with **Capture** enabled and **Intercept is off**. In a
second terminal, serve a synthetic page from a fresh directory:

```sh
demo_dir="$(mktemp -d)"
printf 'Hello from a local test target!\n' > "$demo_dir/index.html"
python3 -m http.server 8090 --bind 127.0.0.1 --directory "$demo_dir"
```

In another terminal, send one request through Perseid:

```sh
curl --noproxy "" --proxy http://127.0.0.1:8080 http://127.0.0.1:8090/
```

You should see the greeting in curl and a `GET` row with status `200` in
**Proxy > HTTP history**. Select it to inspect the request and response. The
empty `--noproxy` value prevents a local bypass setting from skipping the proxy.
Stop the demo server with `Ctrl+C` when you are done.

Next, right-click that exchange and try **Send to Repeater** or **Send to
Intruder**, or use [Intercept](workbench.md#intercept-and-edit) to hold the next
request. For an Intruder walkthrough, see the
[workbench guide](workbench.md#intruder).

## Connect a testing browser

1. Use a dedicated browser profile containing no personal sessions.
2. Configure an **HTTP proxy at `127.0.0.1:8080` for both HTTP and HTTPS**.
   An extension such as FoxyProxy can keep this separate from normal browsing.
3. Check the browser's proxy-bypass list. Local targets are often excluded by
   default; the curl walkthrough above explicitly disables that bypass.
4. For HTTPS, download the public CA from `http://127.0.0.1:8080/` directly and
   follow [HTTPS interception](tls.md#install-the-certificate).
5. Browse only a local lab or another explicitly authorized target. Requests
   should appear in HTTP history.

> [!NOTE]
> HTTPS decryption and request holding are different controls. **Intercept
> HTTPS** is on by default; the **Intercept is off/on** button controls whether
> messages wait for a Forward/Drop decision. Response holding is a separate,
> opt-in checkbox.

## Options

Pass application flags after Cargo's `--` separator:

```sh
cargo run -p perseid --locked -- --listen 127.0.0.1:8081 --ui-scale 1.5
```

| Flag | Default | Purpose |
| --- | --- | --- |
| `--listen <ADDR>` | `127.0.0.1:8080` | Proxy listen address; keep it on loopback. |
| `--capture-limit <BYTES>` | `1048576` (1 MiB) | Body bytes retained per request/response. |
| `--intercept-limit <BYTES>` | `1048576` (1 MiB) | Maximum body size eligible for buffering/editing or rewriting. |
| `--history-limit <N>` | `2000` | Maximum number of HTTP exchanges or WebSocket messages held in each UI history cache / loaded project snapshot. Does not cap saved project files. Also the default for `--temporary-retention-limit`. |
| `--history-bytes <BYTES>` | Half of physical RAM; 256 MiB fallback | Per-cache in-memory UI budget for HTTP and WebSocket history. In persistent projects this bounds the opened snapshot, not disk retention. Also the default for `--temporary-retention-bytes`. |
| `--temporary-retention-limit <N>` | `--history-limit` | Maximum combined HTTP + WebSocket records retained by a temporary project. `0` disables the count limit. Oldest eligible records are evicted first; saved projects have separate opt-in settings. |
| `--temporary-retention-bytes <BYTES>` | `--history-bytes` | Maximum retained HTTP request/response body bytes plus WebSocket payload bytes in a temporary project. `0` disables the byte limit. The newest record is always kept. |
| `--no-proxy` | Off | Open the UI without starting the proxy. |
| `--ca-dir <DIR>` | `PERSEID_CA_DIR`, then platform data directory | Where the local CA store lives; use the active certificate/key paths shown in the UI. |
| `--no-tls-intercept` | Off | Keep HTTPS tunnels opaque; the CA is still prepared. |
| `--tls-passthrough <HOST>` | Empty | Never decrypt this host or its subdomains; repeat for more hosts. |
| `--ui-scale <FACTOR>` | System/desktop scale | Override the UI scale, for example `1.5` or `2`. |

Run `cargo run -p perseid --locked -- --help` for the authoritative options.
The old `--tls-intercept` flag is deprecated: HTTPS decryption is already on by
default in the desktop app.

## Workspace

Continue with the [workbench guide](workbench.md) for Target scope, history,
message views, interception, match/replace, Intruder, Repeater, Sequencer,
Decoder, Comparer, and shortcuts.

Pretty body formatting has independent 256 KiB input/output and 128-level
nesting budgets; exceeding one falls back to bounded Raw text. Repeater also
limits queued requests, active sends, and unread results together. These are
separate from the capture/history flags above; see
[workbench limits](workbench.md#limits-and-persistence) and
[Repeater backpressure](troubleshooting.md#repeater-reports-a-budget-or-size-error).

## Current limitations

Perseid `0.2.0` introduced saved projects. The history and site-map tables open
only a bounded newest-first snapshot of stored history; older records stay in the
project file and can be found by Search. **Browse stored history…** in HTTP
history pages through all retained HTTP exchanges and loads details/notes on demand.
Temporary projects evict the
oldest captured records and released body bytes when they exceed the configurable
aggregate retention limits, which default to the history flags. New and Open ask
Save/Discard/Cancel before replacing a temporary project with unsaved work; a
failed open keeps the current project and running proxy. Saved retention defaults
to unlimited; **Project storage…** enables combined record/body limits. These
are logical limits, not a physical file-size cap or secure erasure. Projects
are not encrypted. Schema-v1 projects migrate atomically to v2 on open; older
applications cannot reopen upgraded files. To remove a saved project, switch
away or quit, then delete its `.perseid` file and any copies using your file manager.
WebSocket messages can be intercepted, edited, and replayed on live connections;
HTTP/3/QUIC is absent. Sequencer capture is regex-based and its statistical
results are indicative. See [limits and persistence](workbench.md#limits-and-persistence)
and the [capability matrix](burp-parity.md) before relying on a particular
workflow.

## HiDPI displays

Use **View > Zoom In / Zoom Out / Reset Zoom**, or `Ctrl`+`+`, `Ctrl`+`-`, and
`Ctrl`+`0` (`Cmd` on macOS). Reset returns to the startup scale. You can also
pass `--ui-scale`.

On X11, Perseid uses `GDK_SCALE` or `QT_SCALE_FACTOR` as a fallback when the
desktop does not report a native scale. Native scaling takes precedence.

## Troubleshooting

For missing traffic, held requests, address conflicts, certificate errors,
graphics problems, or toolchain conflicts, see [Troubleshooting](troubleshooting.md).
