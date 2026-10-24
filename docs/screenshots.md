# Screenshots

[Documentation](README.md) · [Workbench guide](workbench.md) ·
[Getting started](getting-started.md)

These full-window scenes are generated deterministically by
`scripts/screenshots.sh`. CI uploads the images as an artifact. Same-repository
pull requests get one comment linking to a generated Markdown gallery page on
the preview branch; the images are not embedded in the PR thread. Forks can use
workflow artifacts. Traffic and credentials in these scenes are synthetic.
The main scenes are captured at 2× and saved at native resolution (2560×1640)
for HiDPI displays; the memory panel is a smaller close-up. Screenshots
illustrate the UI, not platform support or performance guarantees.

[History](#proxy--http-history) · [Projects](#projects) · [Search](#search) ·
[WebSockets history](#proxy--websockets-history) · [Intercept](#proxy--intercept) ·
[Intruder](#intruder) · [Repeater](#repeater) · [Sequencer](#sequencer) ·
[Sequencer result](#sequencer-result) · [Decoder](#decoder) · [Comparer](#comparer) ·
[Comparer result](#comparer-result) · [Memory](#memory-usage)

## Proxy > HTTP history

The request and response editors open side by side, with the parsed Inspector
on the right and per-exchange Notes behind its rail tab.

![Proxy HTTP history with request and response detail and the Inspector side panel](images/perseid-main.png)

## Projects

Perseid starts with a temporary in-memory project (`Untitled (temporary)` in
the status bar). **File > Save Project As…** moves it into a single `.perseid`
file; **Open Project…** switches to an existing one, and **Save Project
Copy…** snapshots it into a new file. Once saved, captures and workbench state
commit automatically, and the status bar shows the open project.

![File menu with project commands over the HTTP history](images/perseid-project.png)

The path is entered directly in the save dialog; the destination is created
exclusively and only becomes the open project once the copy is complete.

![Save Project As dialog with a destination path](images/perseid-project-dialog.png)

## Search

The top-toolbar magnifier opens a floating search window over the HTTP
history. Search request or response heads and decoded bodies, then inspect the
selected result in Pretty, Raw, or Hex with matches highlighted.

![Search window with a decoded JSON response and highlighted matches](images/perseid-search.png)

## Proxy > WebSockets history

A separate message-level history for plain `ws://` and decrypted `wss://`
traffic, with direction, length, TLS, time, listener port, and WebSocket ID.

![Proxy WebSockets history with a formatted JSON message selected](images/perseid-proxy-websockets.png)

## Target > Site map

![Target site map grouped by host and path](images/perseid-target-site-map.png)

## Target > Scope

![Target scope rules and captured hosts](images/perseid-target-scope.png)

## Proxy > Intercept

Hold messages in a table; select a row to inspect or edit its Raw representation.

![Proxy Intercept with three held requests](images/perseid-proxy-intercept.png)

## Proxy > Match/replace

![Proxy match/replace rules](images/perseid-proxy-match-replace.png)

## Proxy > Proxy settings

![Proxy settings certificate authority and pass-through configuration](images/perseid-proxy-tls.png)

## Intruder

Mark `$$` positions, edit payload sets, and review generated requests and
responses from a finished Sniper attack.

![Intruder with a marked request, payload set, and attack results](images/perseid-intruder.png)

## Repeater

Replay edited requests directly to a target, independently of the proxy listener.

![Repeater request and response](images/perseid-repeater.png)

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
