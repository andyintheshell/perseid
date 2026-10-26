# Installed release smoke tests

The release workflow runs `installer-smoke.yml` after packaging, on pushes to
`release` and manual release-workflow dispatches. Manual dispatch on another
branch tests packages without changing the draft release. Each case downloads
the artifacts from that same workflow run; it never rebuilds the application.
Draft publication requires every smoke test to succeed.

Fresh native runners test Windows x64/ARM64 MSI installation and portable ZIP
extraction, Linux x64/ARM64 DEB installation and normal AppImage execution
(with FUSE, not extraction), and the universal macOS DMG on both Intel and ARM.
The DMG is mounted and its app copied into `/Applications`.

Each case launches the packaged executable, waits up to 90 seconds for a
visible process-owned window, waits another five seconds, verifies the process
and window still exist, and captures one PNG cropped to that window. Blank
captures fail. Screenshots and logs are uploaded as workflow artifacts even
on failure; successful screenshots are also attached to the draft GitHub
Release as `installer-*.png`. Linux uses Xvfb and a window manager.

Windows hosted runners lack suitable hardware OpenGL. After installing/extracting
the package, CI downloads Mesa llvmpipe 26.2.5 from
[`mmozeiko/build-mesa`](https://github.com/mmozeiko/build-mesa/releases/tag/26.2.5),
verifies its pinned SHA256 and DLL architecture, and places `opengl32.dll` beside
the installed executable. The executable's PE header selects x64 or ARM64;
neither case uses an emulated x64 app to stand in for ARM64. The driver is
statically linked and software rendering is explicitly selected. Provisioning
fails rather than overwriting an existing packaged OpenGL DLL.

This is a CI environment dependency only: the original release packages and
checksums are unchanged, and no Mesa DLL is published in release assets. It
does not solve OpenGL availability for end users or test their GPU drivers.
When updating Mesa, update both architecture-specific digests in
`scripts/windows_smoke_opengl.py` and validate both Windows architectures.

This is a packaging/startup smoke test, not a functional or visual regression
test. Inspect the screenshots when reviewing a release. It does not exercise
installer wizard interactions, shortcuts, upgrades, uninstall, all supported OS
versions, or proxy functionality. macOS launches the installed executable
directly: this does **not** validate Finder/LaunchServices or Gatekeeper's
download-quarantine/notarization path. Current macOS packages are ad-hoc signed.
Hosted runner desktop/capture restrictions can also fail a test; diagnostics
should distinguish those failures from application failures.

Run the platform-independent harness tests locally:

```sh
python3 -m unittest discover -s scripts -p test_installer_smoke.py
```
