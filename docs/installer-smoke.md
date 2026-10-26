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
