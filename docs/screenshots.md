# Screenshots

[Documentation](README.md) · [Workbench guide](workbench.md) ·
[Getting started](getting-started.md)

Take a tour of Perseid: capture traffic, inspect messages, then send them to the
tools below. All traffic, tokens, and credentials shown here are synthetic.
Click any image to view it at full size.

- **Capture:** [HTTP history](#proxy--http-history) · [WebSockets](#proxy--websockets-history) · [Intercept](#proxy--intercept)
- **Explore:** [Site map](#target--site-map) · [Scope](#target--scope) · [Search](#search)
- **Test:** [Intruder](#intruder) · [Repeater](#repeater) · [WebSocket Repeater](#repeater--websockets) · [Sequencer](#sequencer)
- **Transform:** [Decoder](#decoder) · [Comparer](#comparer)
- **Configure:** [Projects](#projects) · [Settings](#settings) · [Match/replace](#proxy--matchreplace) · [HTTPS settings](#proxy--proxy-settings) · [Memory](#memory-usage)

These screenshots reflect development code, which may be newer than the latest
release. See the [workbench guide](workbench.md) for behavior and limitations.

## Proxy > HTTP history

Browse captured requests and inspect the request and response side by side.
Use the Inspector for parsed fields or Notes for your observations.

Filter the history by scope, content type, status, or search terms. Each message
pane also has its own search bar and line-wrapping control.

![Proxy HTTP history with request and response detail and the Inspector side panel](images/perseid-main.png)

Right-click an exchange to send it to another tool, add its host to scope,
open Notes, delete the item, or copy its URL or curl command.

![HTTP history context menu with tool shortcuts and Delete item](images/perseid-http-history-context-menu.png)

## Projects

Perseid starts with a temporary in-memory project (`Untitled (temporary)` in
the status bar). **File > Save Project As…** moves it into a single `.perseid`
file; **Open Project…** switches to an existing one, and **Save Project
Copy…** snapshots it into a new file. Once saved, captures and workbench state
commit automatically, and the status bar shows the open project.

![File menu with project commands over the HTTP history](images/perseid-project.png)

The save dialog is a modal folder browser with a **Save in** location bar,
list and grid views, and file-name and file-type fields. It asks before
replacing an existing file, and the destination only becomes the open project
once the complete copy is ready.

![Save Project As dialog with a location bar, file list, and destination file name](images/perseid-project-dialog.png)

## Search

The top-toolbar magnifier opens a floating search window over stored HTTP and
WebSocket history plus current Repeater and Intruder traffic. Search request or
response heads and decoded bodies, then inspect the selected result in Pretty,
Raw, or Hex with matches highlighted.

![Search window with a decoded JSON response and highlighted matches](images/perseid-search.png)

## Settings

Configure project automatic deletion and choose whether Repeater and Intruder
use cookies from the shared, session-only cookie jar.

![Settings with project automatic deletion, session handling, and cookie jar controls](images/perseid-settings.png)

The separate viewer shows cookie scope, values, security flags, and expiration.
All entries below are synthetic.

![Cookie jar viewer with synthetic entries](images/perseid-cookie-jar.png)

## Proxy > WebSockets history

A separate message-level history for plain `ws://` and decrypted `wss://`
traffic, with direction, length, TLS, time, listener port, and WebSocket ID.

![Proxy WebSockets history with a formatted JSON message selected](images/perseid-proxy-websockets.png)

## Target > Site map

The Burp-style URL-view tree, display-filter bar, sortable Contents table, and
shared request/response detail panes.

![Target site map with a folder tree, Burp-style display filters, and a sortable Contents table](images/perseid-target-site-map.png)

## Target > Scope

Choose which hosts or URL prefixes belong to your test. Filter views to scope
and optionally block out-of-scope proxy traffic. Direct Repeater sends are not
restricted by this proxy guardrail; see [scope behavior](workbench.md#target-and-scope).

![Target scope rules and captured hosts](images/perseid-target-scope.png)

## Proxy > Intercept

Hold requests before they reach the target, edit the selected message, then
forward or drop it. Pretty, Raw, and Hex views help inspect the message;
response interception is opt-in.

![Proxy Intercept with three held requests](images/perseid-proxy-intercept.png)

## Proxy > Match/replace

Apply literal or regex rewriting rules to traffic passing through the proxy.

![Proxy match/replace rules](images/perseid-proxy-match-replace.png)

## Proxy > Proxy settings

Manage the local certificate authority and hosts that should bypass HTTPS
decryption. Certificate trust is always a manual setup step; see the [TLS guide](tls.md).

![Proxy settings certificate authority and pass-through configuration](images/perseid-proxy-tls.png)

## Intruder

Mark `§` positions, edit payload sets, and review generated requests and
responses from a finished Sniper attack.

![Intruder with a marked request, payload set, and attack results](images/perseid-intruder.png)

## Repeater

Replay edited requests directly to a target, independently of the proxy listener.

![Repeater request and response](images/perseid-repeater.png)

## Repeater > WebSockets

Edit a message and inspect the connection history alongside it. Replay uses the
original live connection; this offline synthetic scene shows it as closed.

![WebSocket Repeater with a message editor and connection history](images/perseid-repeater-websockets.png)

## Sequencer

Load an existing token sample or capture tokens from new proxy responses. The
tab shows token count and length, the sample table, and the capture settings.

![Sequencer manual load with a seeded token sample](images/perseid-sequencer.png)

## Sequencer result

The result window summarizes overall randomness, charts effective entropy
across the five significance levels, and reports sample reliability. Character
and bit details, plus analysis settings, are separate tabs.

![Sequencer result summary with effective entropy chart](images/perseid-sequencer-result.png)

## Decoder

Build a local encode/decode/hash chain with one panel per transformation.

![Decoder Base64 and SHA-256 chain](images/perseid-decoder.png)

## Comparer

Both item tables show one shared collection. Select a different row in each
table, then compare the pair word by word or byte by byte.

![Comparer with a request and response selected](images/perseid-comparer.png)

## Comparer result

The result window shows paired Text or Hex views with modified, deleted, and
added data highlighted. Use the arrows to move between differences and
**Sync views** to keep both panes aligned.

![Comparer result with highlighted word differences](images/perseid-comparer-result.png)

## Memory usage

Allocator heap in use and peak, not total process RSS. See the
[measurement guide](performance.md) for the distinction.

![Memory window with current, peak, and rolling history](images/perseid-memory.png)

## Recording the demo animation

The [README demo](images/perseid-demo.webp) follows a captured request through
Repeater, Intruder, Decoder, and Comparer in a real desktop session with a
loopback-only synthetic target. It is a separate recording, not an animation of
the still images above.

<details>
<summary>Maintainer details: record and replace the animation</summary>

Run **Actions > Record demo animation > Run workflow**, selecting the branch to
record. The workflow is manual-only and does not run on pull requests. It
builds Perseid on a Linux runner, starts a loopback-only synthetic target, and
records a real desktop session under Xvfb. The
[storyboard](demo-gif-script.md) is aimed at first-time users: it follows one
captured export through Repeater, Intruder, Decoder, and Comparer, using the
synthetic target's base64 responses. The pointer travels between controls
instead of jumping. Lossless RGB capture and PNG intermediates avoid compression
noise before downscaling to 1280×820 at 12 fps. The animation is encoded with
`img2webp` in lossless mode to prevent patchy backgrounds and ghosting from lossy
delta frames; the workflow rejects lossy WebP frames and keeps it under two
minutes and 2.5 MB so it stays practical for the README. Before publishing, the workflow
verifies that the edited Repeater and Intruder requests reached the local
target.

The current approved recording is checked in as
[`images/perseid-demo.webp`](images/perseid-demo.webp) (1280×820, about 87
seconds, 2.2 MB) and used as the [project README](../README.md) hero image. To
update it, run the workflow manually, download `perseid-demo-webp` from the
run, review it, and replace that file. The artifact expires after 30 days.
The workflow uploads available source recordings and diagnostic logs for seven
days so successful runs can be reviewed too. Failed runs may contain only a
partial recording, or no artifacts if they fail before recording starts.
All traffic and values in this workflow are synthetic; the workflow does not
use local files or data from your machine. Animated WebP is supported by
current Chrome, Firefox, Safari, and Edge, and renders in GitHub Markdown.

</details>

## Updating the screenshots

<details>
<summary>Maintainer details: source, generation, and publishing</summary>

The 22 PNGs above come from the
[Screenshots run for `15f7f8b`](https://github.com/andyintheshell/perseid/actions/runs/42131452202),
captured on 2026-11-05. The
[retained gallery](https://github.com/andyintheshell/perseid/blob/dd94b1582395486e50f9dae5ef5942d6a03ee526/index.md)
records the source commit. The main scenes are captured at 2× (2560×1640);
the memory panel is a smaller close-up. They illustrate the UI, not platform
support or performance guarantees.

To refresh them locally with Rust and a wgpu-compatible graphics backend installed
(for example Metal on macOS or Vulkan/Mesa on Linux):

```sh
scripts/screenshots.sh
python3 scripts/check_screenshots.py docs/images
```

The script builds the app with the `screenshots` feature, seeds synthetic scenes,
and verifies the PNGs before replacing files in `docs/images`. It renders the real
egui application offscreen through `egui_kittest`/wgpu, without X11, Xvfb, or a
proxy listener. The graphics backend is still a system runtime dependency.

Alternatively, run **Actions > Screenshots > Run workflow** for the desired
branch. Download the `perseid-screenshots` artifact, review the images, and copy
the PNGs into `docs/images`. Update the source run and capture date above,
check the captions, and commit the refreshed files together.

**CI does not update this checked-in gallery automatically.** It publishes a
separate retained gallery and uploads artifacts. Same-repository pull requests
receive a comment linking to their exact gallery commit; forks can use the
workflow artifacts. Those previews do not replace the files linked on this page.

</details>
