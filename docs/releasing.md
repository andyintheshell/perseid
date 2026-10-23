# Releasing Perseid

Perseid follows PhotoCraft's release flow: native platform builds run from a
protected `release` branch, the workflow creates or refreshes an unpublished
GitHub Release, and a maintainer tests the downloads and publishes it manually.
The workflow is deliberately separate from the normal `main`/pull-request CI.

## Current release formats

The release matrix builds these nine application downloads:

| Platform | Release assets |
| --- | --- |
| Windows x64 and ARM64 | `.msi` installer and no-install `-portable.zip` for each architecture |
| Linux x86-64 and ARM64 | `.AppImage` and `.deb` for each architecture |
| macOS Intel and Apple Silicon | Universal `.dmg` containing `Perseid.app` |

Every draft also includes `SHA256SUMS.txt`. Windows packages are unsigned. The
macOS bundle is ad-hoc signed but not notarized, so Gatekeeper may prevent it
from opening normally on another Mac. The Linux formats and native ARM builds
also need real-device smoke testing before we treat them as supported downloads.

## Preparing a release

1. Bump `[workspace.package] version` in the root `Cargo.toml` on `main`. Run
   `cargo check --workspace` to update `Cargo.lock`; review and commit both files
   through the normal pull-request flow. The version produces tag `v<version>`
   and determines all asset names.
2. Merge the reviewed `main` branch into the protected `release` branch and push
   it. For the first release, create `release` from the updated `main` after this
   setup pull request has landed. The release workflow also supports manual
   dispatch, but its build jobs only run when the selected branch is `release`.
3. Wait for all five native build jobs and the draft job to finish. The draft is
   not public until someone publishes it. On a failed build, fix the issue on
   `main`, merge the fix into `release`, and push again.
4. Download and smoke-test the draft assets (check the list below), edit its
   notes if needed, then use **Publish release** in GitHub. Treat an untested
   draft as a release candidate, not evidence every target works.

The version in `Cargo.toml` is the only release-version input. Stable versions
create ordinary releases; a SemVer pre-release version (for example
`0.2.0-rc.1`) is marked as a GitHub pre-release. MSI and macOS bundle versions
use the numeric `X.Y.Z` portion because those package formats do not represent
SemVer pre-release labels consistently.

Pushing to `release` again while a draft exists rebuilds the same version and
replaces all its assets (including stale ones). The initial generated release
notes are preserved on a rebuild, so review or edit them after the final build.
The workflow refuses to change an already-published release. To publish another
version, bump `Cargo.toml` first. Keep write access to `release` restricted to
maintainers.

## GitHub setup

- Allow GitHub Actions to create releases (`Settings → Actions → General →
  Workflow permissions`, with read/write access or an equivalent repository
  policy). The workflow only grants `contents: write` to its final draft job.
- Protect the `release` branch and restrict who can push to it. Continue normal
  development and review on `main`.
- No signing secrets are required by this initial workflow. This means MSI and
  Windows executables are unsigned, and the macOS DMG is not notarized.

## First-draft smoke test

- **Windows:** Install the MSI on x64 and ARM64 Windows, confirm it creates a
  Start Menu shortcut, launch it, uninstall it, and test an upgrade. Extract the
  portable ZIP and run `perseid.exe` without Rust installed. User settings and
  the local CA remain in the normal per-user location.
- **macOS:** Mount the DMG, drag `Perseid.app` into Applications, check
  `lipo -info Perseid.app/Contents/MacOS/perseid` reports both `arm64` and
  `x86_64`, and verify the ad-hoc signature. Test launch behavior on both CPU
  families; notarization/Gatekeeper approval is not expected yet.
- **Linux:** Run each AppImage on x86-64 and ARM64 with a graphical desktop;
  install and uninstall each DEB on a compatible Debian-family system. Check
  Wayland and X11/OpenGL startup and confirm the proxy and local CA work.
- **All platforms:** Verify the downloaded files against `SHA256SUMS.txt`,
  inspect generated notes, and check that upgrading does not remove existing
  settings or a generated CA/private key.

## Local packaging helpers

Each platform helper is intended to run on its native OS after installing Rust
1.95.0. The Linux helper also needs `cargo-packager` 0.11.8; the Windows helper
needs WiX v5.0.2 and its UI extension. Outputs go to ignored `dist/` and the
generated `packager.json` is ignored as well.

```sh
# Linux
cargo build --release --locked -p perseid
python scripts/generate-packager-config.py --os linux
cargo packager --release --config packager.json
python scripts/collect-release.py --os linux --arch x64  # or arm64

# macOS (builds both Rust targets and creates a universal DMG)
bash scripts/package-macos.sh
```

```powershell
# Windows (native x64 or ARM64)
cargo build --release --locked -p perseid
dotnet tool install --global wix --version 5.0.2
wix extension add -g WixToolset.UI.wixext/5.0.2
pwsh scripts/package-windows.ps1 -Arch x64 # or arm64
```

The first hosted release run is the first full validation of these package
formats. In particular, validate the native ARM64 Windows runner, ARM64
AppImage tooling, MSI install/upgrade behavior, macOS Gatekeeper experience, and
Linux runtime dependencies before publishing.
