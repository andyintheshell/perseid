# Releasing Perseid

Perseid follows PhotoCraft's release flow: native platform builds run from a
protected `release` branch, the workflow creates or refreshes an unpublished
GitHub Release, and a maintainer tests the downloads and publishes it manually.
The workflow is deliberately separate from the normal `main`/pull-request CI.

**Current version status:** `v0.3.0` is the latest published crate and GitHub
release; project persistence was introduced in `v0.2.0`. The workspace
version is `0.3.0`. M3 is complete
for this release scope; remaining M3 issues moved to M4 (see the [roadmap](roadmap.md)).

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

### 0.3.0 release highlights

- M3 daily-workbench release scope is complete; six remaining issues moved to
  M4, not silently marked implemented. The parity estimate remains 78.6%.
- Full-store HTTP paging/lazy details, full-store paged site map, saved logical
  retention, and atomic schema-v1→v2 migration are available. **Back up v1 files
  before opening: `0.2.x` cannot open upgraded projects.**
- Cookie/session settings provide a bounded in-memory jar; application of cookies
  and response updates for Repeater/Intruder are opt-in. No automatic login.
- Normal quit protects unsaved temporary work and acknowledges saved workspace
  writes, with recovery choices on failure/timeout. This is not crash recovery;
  see [exit guarantees](project-exit-guarantees.md).
- Platform package smoke tests below are still required before publication.

### Release procedure

1. Set `[workspace.package] version` in the root `Cargo.toml` on `main` to the
   version being prepared. Run `cargo check --workspace` to update `Cargo.lock`;
   review and commit both files through the normal pull-request flow. The version
   produces tag `v<version>` and determines all asset names.
2. Merge the reviewed `main` branch into the protected `release` branch and push
   it. Manual dispatch can preflight the builders on another branch without
   publishing; dispatching on `release` will create or update the draft as usual.
3. Wait for all five native build jobs and the draft job to finish. The draft is
   not public until someone publishes it. On a failed build, fix the issue on
   `main`, merge the fix into `release`, and push again.
4. Download and smoke-test the draft assets (check the list below), edit its
   notes if needed, then use **Publish release** in GitHub. Treat an untested
   draft as a release candidate, not evidence every target works.

The version in `Cargo.toml` is the only release-version input. Stable versions
create ordinary releases; a SemVer pre-release version (for example
`0.3.0-rc.1`) is marked as a GitHub pre-release. MSI and macOS bundle versions
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

## Release smoke-test checklist

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
- **Projects:** Reopen a copy of a schema-v1 project and verify the atomic v2
  upgrade preserves HTTP/WebSocket binary bodies and notes. Browse older HTTP
  pages, send an old row to a tool, and search beyond the live cache. Apply saved
  retention and check it survives reopen/Save Copy; a failed open must preserve
  the current project and running proxy. Automated fixtures cover these paths,
  but packaged-app startup smoke tests do not exercise them.

### Project storage notes for releases and packages

Projects remain RAM-only until explicitly saved; `.perseid` files live at the
user-selected path, independently of the installation and local CA directory.
Installers/upgrades/uninstallers must not remove user projects or copies.
Saved projects autosave, default to unlimited retention, and offer opt-in
combined record/body limits under **Project storage…**. Limits exclude
workbench snapshots and database overhead and do not cap physical file size.
There is no automatic compaction; Save Copy writes retained data to a fresh file
and may be smaller.

Release notes for the schema-v2 change must mention the atomic v1 upgrade and
that `0.2.x` cannot open upgraded files: preserve a closed-file backup before
opening if backward compatibility is needed. Files are not encrypted. Clear
history removes captures, not independent tool snapshots or the project file.
Delete a whole project through the filesystem after switching away or quitting,
and remove copies/backups separately. None of these operations securely erases
disk bytes. See [Safety and privacy](safety.md) and [Workbench](workbench.md).

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

The previous release completed all five hosted package builders. That
checks artifact generation, not every installer and runtime on clean machines.
Continue smoke-testing native ARM64 builds, MSI install/upgrade behavior, the
macOS Gatekeeper experience, and Linux runtime dependencies before each release.
