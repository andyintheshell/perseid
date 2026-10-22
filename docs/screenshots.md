# Screenshots

[Documentation](README.md) · [Workbench guide](workbench.md) ·
[Getting started](getting-started.md)

These full-window scenes are generated deterministically by
`scripts/screenshots.sh`. The same images are uploaded by CI and posted to
same-repository pull requests so every UI tab is reviewed together; forks can
use workflow artifacts. Traffic and credentials in these scenes are synthetic.
The main scenes are captured at 2× and saved at native resolution (2560×1640)
for HiDPI displays; the memory panel is a smaller close-up. Screenshots
illustrate the UI, not platform support or performance guarantees.

[History](#proxy--http-history) · [Intercept](#proxy--intercept) ·
[Intruder](#intruder) · [Repeater](#repeater) · [Decoder](#decoder) ·
[Comparer](#comparer) · [Comparer result](#comparer-result) ·
[Memory](#memory-usage)

## Proxy > HTTP history

![Proxy HTTP history with request and response detail](images/perseid-main.png)

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
