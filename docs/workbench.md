# Workbench guide

[Documentation](README.md) · [Getting started](getting-started.md) ·
[HTTPS setup](tls.md) · [Troubleshooting](troubleshooting.md)

The everyday loop is **capture → inspect → edit → replay**. Use an external
browser for normal interaction and Perseid to understand and change the HTTP
messages. Only test local labs or systems you are explicitly authorized to test.

## Workspace at a glance

| Tab | Use it for |
| --- | --- |
| **Target** | A Burp-style site map derived from history, a shared scope list, and an explicit empty scanner-issues view. |
| **Proxy** | HTTP history, WebSockets history, held interception messages, match/replace rules, and proxy/TLS settings. |
| **Intruder** | Editable requests with `$$` payload positions, payload sets, and direct attacks. |
| **Repeater** | Editable HTTP/HTTPS requests and messages on live WebSocket connections. |
| **Sequencer** | Token sampling and character- or bit-level randomness analysis. |
| **Decoder** | Local encoding, decoding, hashing, and layered transformations. |
| **Comparer** | Word- or byte-level diffs between any two requests, responses, or loaded messages. |

See the [screenshot gallery](screenshots.md) for each tab. The app opens on
**Proxy > HTTP history**. The top-toolbar magnifier opens the cross-history
[search window](#search-captured-traffic) without switching tabs.

## Projects and persistence

Perseid starts each run in **Untitled (temporary)**. Captures and workbench state
are held in memory until you choose **File > Save Project As…**. A saved project
is one local `.perseid` file; changes to captured traffic and supported
workbench inputs are saved automatically, so there is no periodic Save command.
Use **Save Project Copy…** to make a separate snapshot without switching away
from the open project.

- **New Project** starts an empty temporary project. If the current temporary
  project has unsaved records or workbench edits, Perseid first asks
  Save/Discard/Cancel. Nothing is deleted from an existing saved file.
- **Open Project…** opens an existing `.perseid` file. Use the modal folder
  browser to navigate to its directory and select the file, or type a path in
  the **Look in** bar. The toolbar provides up, home, refresh, new-folder,
  hidden-file, and list/grid controls. Perseid validates the destination in the
  background before stopping the proxy for a successful switch. A failed open
  keeps the existing listener running. When the current temporary project has
  unsaved work, Open asks Save/Discard/Cancel first; a cancelled prompt or a
  failed open leaves the current project active.
- **Save Project As…** transfers the current project into a new file and makes
  that file active. Choose a folder in the **Save in** bar, then enter a file
  name. If the destination already exists, Perseid asks before replacing it.
  The replacement is written alongside the old file and only swapped in once
  the new project is complete.
- **Save Project Copy…** writes a consistent copy to a new path and leaves the
  current project active. It also asks before replacing an existing file.

Saved projects persist HTTP and WebSocket captures plus the workbench state:
scope and match/replace rules, the selected tab/section, filters, message views,
Inspector notes and expanded sections, Repeater tabs and their last responses,
Intruder configuration and result rows, Decoder panels, Comparer items and
result-window state, Search state and structured result rows, Sequencer
samples/analysis/settings, and TLS interception and pass-through settings.
Active network work is not restored:
running attacks or sends, held interception messages, pending operations, and
proxy lifecycle state stay in the current session. The local TLS CA certificate
and private key stay outside the project file.

Project files are not encrypted. Treat a `.perseid` file as sensitive traffic:
it can contain cookies, tokens, credentials, personal data, and binary bodies.
See [Safety and privacy](safety.md#data-handling) before saving or
sharing one.

### Browse older history

Opening a project loads a bounded newest-first live snapshot controlled by
`--history-limit` and `--history-bytes`. In **Proxy > HTTP history**, choose
**Browse stored history…** for all retained HTTP exchanges. **Older**, **Newer**,
and **Newest** navigate metadata pages of up to 500 rows, newest first. Select
a row to load request/response details, Pretty/Raw/Hex views, and Inspector
notes on demand; send it directly to Repeater, Intruder, or Comparer. Pages
stay in place during capture; **Newest** refreshes to include new arrivals.
Live-table filters do not apply in this browser. **Search** filters all retained
HTTP/WebSocket records, including decoded bodies. The site map and live
WebSocket table still use the bounded snapshot.

### Retention and file size

**Project storage…** in the HTTP history toolbar configures a saved project's
combined HTTP/WebSocket record count and retained body/payload byte limits.
Both default to **0 (unlimited)**. **Apply retention limits** immediately removes
the oldest eligible records and applies the policy to future captures. Limits
survive reopen, Save As, and Save Copy. Lowering them cannot be undone; save a
copy first if you need the records. The status bar reports eviction counts, and
the live caches refresh after committed evictions.

Temporary projects use `--temporary-retention-limit` and
`--temporary-retention-bytes`, defaulting to the history flags. For both modes,
zero disables a dimension. Limits are soft: in-flight HTTP exchanges and each
stream's newest record are kept, even if they exceed a limit. Byte accounting
excludes headers, indexes, workbench snapshots, and database overhead.
`--capture-limit` separately caps each captured body.

These are **logical retention limits, not physical file-size limits**. redb can
reuse freed pages; removing records need not shrink the `.perseid` file. There
is no automatic compaction command. **Save Project Copy…** writes retained data
to a fresh file, which may be smaller; it also includes all saved workbench
state. Neither operation securely erases old disk bytes.

### Migrations and deletion

This development version writes schema v2 and automatically migrates schema-v1
projects from `0.2.x` under an exclusive file lock. The v1→v2 upgrade is one
atomic transaction, preserves captures and notes, and defaults saved retention
to unlimited. Older Perseid versions cannot open the upgraded file; keep a
filesystem backup before opening if you need to use an older application.
Unknown newer schemas and incomplete/corrupt files are rejected with an error.

**Clear history** removes that history's captures from the active project;
HTTP Clear also removes Inspector notes. Independently retained tool snapshots
are not cleared by history retention or Clear history. To discard temporary
work, leave it unsaved. To delete a whole saved project, switch to **New Project**
or quit Perseid, then delete its `.perseid` file using your file manager. Remove
any copies/backups separately. This does not delete the local CA or securely
erase disk contents. There is no separate whole-project deletion button.

## Target and scope

**Target > Site map** follows Burp's URL view. The left tree groups captured
requests by host, folder, and resource; selecting a node filters the
**Contents** table to that node and everything below it. Query variants share
the same tree leaf while each method and URL remains its own Contents row.
Columns cover Host, Method, URL, Params, Status code, Length, MIME type, Title,
Notes, and Time requested, and every column can be sorted. HTTPS hosts use a
lock icon; a host captured over both HTTP and HTTPS uses a globe with a small
lock badge so the tree never presents mixed traffic as a single scheme.

The filter bar only changes what is displayed. Its defaults match Burp's:
not-found items, CSS, images, general binary content, 4xx responses, and empty
folders are hidden. Relax a filter or choose **Show all items** to bring hidden
entries back; nothing is deleted. **Contents search** narrows the selected
node, the pause button freezes the derived map while capture continues, and the
Inspector's Notes appear in the table. The **Issues (0)** tab is empty because
Perseid does not run a scanner, and the crawl-paths view is unavailable because
Perseid does not crawl or infer unrequested URLs.

The map is built from the currently loaded history snapshot, not from a crawler.
UI-cache eviction can remove entries from the map without deleting their saved
project records; clearing history removes the stored records too.

**Target > Issues** is likewise an honest empty state: Perseid has no
vulnerability scanner and does not import scanner findings.

In **Target > Scope**, add a bare host, a `host:port` authority, or a full URL.
Matching includes subdomains and ignores ports; URL schemes and paths do not
make scope a path-level or port-level allowlist.

Two independent controls use that list:

- **Filter views to the scope** hides out-of-scope history/site-map entries.
  It does not stop network traffic.
- **Block out-of-scope traffic** rejects out-of-scope proxy requests and
  `CONNECT` tunnels with `403`. This is opt-in and off by default. After
  interception or rewriting, edited request targets are checked again.

**An empty scope allows every host**, even when blocking is selected.
Opaque HTTPS tunnels can be checked only by their `CONNECT` target; decrypted
requests can be checked individually.

> [!WARNING]
> Scope enforcement applies to the proxy, **not Repeater**. Intruder also checks
> enforced scope for every generated request, but does not use the proxy's
> interception or match/replace pipeline. Always review the target before a
> direct send. Scope is a guardrail, not authorization to test.

## Capture and history

Configure a browser or client to proxy through `127.0.0.1:8080` for HTTP and
HTTPS. The [local walkthrough](getting-started.md#try-it-with-a-local-target)
is a quick way to verify the path without a browser or CA trust.

- **HTTP history** has one row per HTTP exchange, including decrypted HTTPS.
  `CONNECT` transport tunnels do not get their own rows. Right-click a row to
  send its request or response to Repeater, Intruder, or Comparer, send its
  response to Sequencer, or **Clear history**.
- The text filter matches method, host, path, URI, status, MIME type, and file
  extension—not response-body contents. The filter bar opens a Burp-style
  display-filter dialog with in-scope, missing-response, parameterized-request,
  MIME class, status class, regex or negative search, and file-extension
  controls. Like Burp, the default filter hides CSS and image responses plus
  common static extensions (`.js`, `.gif`, `.jpg`, `.png`, `.ico`, `.css`,
  `.woff`, and `.woff2`). The **Filter on** switch temporarily disables every
  display filter. Filters only hide rows; captured exchanges stay in the
  project. Scope filtering can narrow the view further.
- **Capture** pauses recording of new exchanges, not forwarding or interception.
- The in-memory history view is bounded by exchange count and retained-body
  bytes. These limits evict rows from the UI cache and do not prune saved-project
  files. Temporary redb storage has separate aggregate count and body-byte
  retention limits, which default to the history limits and can be configured
  independently; older records are evicted when those limits are exceeded.
  Older saved records remain on disk even when evicted from the UI cache.
- The status bar shows in-flight requests, held messages, dropped capture events,
  rejected connections, and UI-cache evictions. A dropped-event warning means
  the history may be incomplete even if the client received its response.

### Copy as curl

Right-click an HTTP history row or the request detail pane and choose **Copy as
curl**. Perseid copies a POSIX-shell command to the clipboard; it does not run the
request. The command includes the captured method, target URL, headers, and
explicit HTTP-version option (`--http1.0`, `--http1.1`, or `--http2` where
applicable). It does **not** add a proxy by default. To send it through Perseid,
add `--proxy 'http://127.0.0.1:8080'` yourself and ensure curl trusts the CA for
HTTPS interception.

Non-empty bodies are base64-encoded in the command and decoded by Python 3 into
curl's standard input, preserving binary and non-UTF-8 bytes. Bodyless requests
do not use the decoder. The action is disabled until capture finishes and for a
truncated body, because either case could produce a command that does not
represent the captured request. When a body is present, the command omits
captured `Content-Length` and `Transfer-Encoding` framing headers so curl can
frame the copied bytes; it prevents curl from adding a default content type when
none was captured.

Review the command before running or sharing it: copied headers may contain
cookies, bearer tokens, or other credentials, and a command pasted into a shell
may be retained in that shell's history.

For example, a captured HTTP/2 upload could be copied as:

```sh
python3 -c 'import base64,sys;sys.stdout.buffer.write(base64.b64decode(sys.stdin.buffer.read()))' <<'PERSEID_CURL_BODY' | curl \
  --request 'POST' \
  --url 'https://example.test/api/upload' \
  --http2 \
  --header 'content-type: application/octet-stream' \
  --data-binary '@-'
AQID
PERSEID_CURL_BODY
```

The heredoc contains base64 for the captured bytes; shell quoting protects
apostrophes and newlines in methods, URLs, and header values.

### WebSockets history

**Proxy > WebSockets history** is a separate, message-level history. Plain
`ws://` connections and decrypted `wss://` connections inside intercepted TLS
are supported. An opaque CONNECT tunnel cannot be decoded, so enable HTTPS
interception for the host if you need its WebSocket messages.

- Rows represent complete text or binary messages, not frames. Fragmented
  messages are reassembled, client masking is removed, and ping/pong/close
  control frames are omitted. Incomplete messages are discarded.
- The table shows URL, direction, payload length, TLS, time, listener port, and
  a stable WebSocket ID per connection. The filter matches those fields, the
  host, and a negotiated subprotocol.
- The handshake supplies connection metadata but does not become a WebSocket
  row. A successful WebSocket handshake also does not create an HTTP history
  row. A rejected or non-`101` handshake follows the ordinary HTTP capture path.
- The detail pane offers Pretty, Raw, and Hex views. Text messages are
  JSON/HTML/formatted when possible; binary messages use a hex dump. Payloads
  are bounded by `--capture-limit`, and the detail header reports truncation.
- Message capture is independent of interception. **Capture** still pauses
  recording, but enabling or disabling **Proxy > Intercept** does not.
- `Sec-WebSocket-Extensions` is removed from the forwarded handshake, so the
  origin does not negotiate compression that the history cannot decode.
- Right-click a message and choose **Send to Repeater** to edit and resend it
  on its existing live connection. WebSocket history has the same count and retained-byte UI-cache
  defaults as HTTP history. Saved projects persist WebSocket records, but the
  UI currently loads only a bounded newest-first snapshot.

Click the **Memory** indicator for the current and peak allocator heap and a
rolling graph. This is not process RSS or a hard memory ceiling; see
[Performance and memory](performance.md).

## Inspect messages

Select a history row to open the request and response side by side. Drag the
divider to resize the panes.

The **Inspector** rail on the right is open by default. It parses the selected
exchange into collapsible sections, starting with all sections closed:

- **Request**: attributes, query parameters, URL-encoded body parameters,
  cookies, and headers.
- **Response**: attributes, cookies, and headers.

Click a section header to open it, or use the header icons to expand or collapse
all non-empty sections. Query, form, and percent-encoded cookie values are
decoded for display; the message editor continues to show the representation
being forwarded. The Inspector reflects the captured exchange and does not
rewrite it. Its width is draggable. The **Notes** tab stores a note per exchange
and saves notes, the active rail tab, and expanded sections with a persistent
project. Clearing history also clears them.

| View | Representation |
| --- | --- |
| **Pretty** | JSON indentation, HTML/XML layout, and one field per URL-encoded form entry; otherwise readable text. |
| **Raw** | Reconstructed message text with line numbers and syntax highlighting. Binary/control bytes may be displayed lossily. |
| **Hex** | A bounded byte-offset dump. In history and Repeater, this uses the same decoded body representation as the text views. Drag across the hex or ASCII columns to select bytes; **Copy selection** copies them as space-separated hexadecimal. |

History and Repeater views decode gzip, deflate, and Brotli bodies for
inspection. Displayed `Content-Encoding` is removed after successful decoding;
`Content-Length` is adjusted only when the complete decoded length is known.
Unsupported or invalid encodings fall back to captured wire bytes. The proxy
removes unsupported outgoing `Accept-Encoding` codings such as `zstd`.

These views are **not byte-for-byte wire dumps**: headers are reconstructed,
header names are title-cased, HTTP/2 uses a synthetic textual head, compressed
bodies are decoded, and previews may be truncated. Copying a view copies its
displayed representation. Viewing a message does not itself rewrite the body
being forwarded.

Pretty body formatting has independent **256 KiB input and output limits** and
a **128-level nesting limit**. If formatting would exceed a budget, the view
falls back to bounded Raw text rather than expanding indentation without limit.
These body-formatting limits are separate from capture and decoded-preview
limits and do not make the whole editor or process a fixed-size allocation.

## Search captured traffic

Click the magnifying glass at the right side of the top toolbar, or press
`Ctrl`/`Cmd`+`Shift`+`F`, to open the floating **Search** window. Type a term
and press **Go** or `Enter`. Search covers every HTTP exchange and WebSocket
message stored in the active project, including rows older than the loaded
history snapshot, plus the Repeater tabs and retained Intruder results in the
current workspace.

Choose which tools to search:

- **Proxy history** searches stored HTTP exchanges, including decrypted HTTPS
  traffic and rows that are not in the loaded history view.
- **WebSockets** searches stored WebSocket messages and their connection URL.
- **Repeater** searches each tab's current request editor and retained response.
- **Intruder** searches each retained Intruder result's rendered request and
  response.

Choose which parts of each message to examine:

- **Request headers** searches the request line and displayed headers.
- **Request body** searches the decoded request body.
- **Response headers** searches the status line and displayed headers.
- **Response body** searches the decoded response body.

For WebSocket messages, the connection URL and metadata are treated as headers,
and the payload is treated as a body, so either body option finds it. The
request and response body locations are on by default, and the headers are
included too. Supported gzip, deflate, and Brotli bodies are decoded before
matching, so searching for readable response text works even when the captured
wire body was compressed. Unsupported or malformed encodings fall back to the
captured bytes, matching the message editor's representation.

The options follow Burp's search model:

- **Case sensitive** makes literal and regex matching case-sensitive.
- **Regex** treats the query as a regular expression. Invalid expressions show
  an error instead of running.
- **Negative match** returns messages that do **not** match in any selected
  location.
- **In-scope only** restricts results to the host rules in **Target > Scope**.
  An empty scope includes every host.
- **Dynamic update** reruns the search as the query, options, or captured HTTP
  and WebSocket history change. Leave it off to run only when you press Go or
  Enter; Repeater and Intruder edits are picked up on the next run.

Stored HTTP and WebSocket history is scanned in a background worker, so the
window stays responsive and reports progress while older rows are read. Up to
10,000 results are kept; the status line says when the cap was reached. Results
from the loaded history cache are shown first and deduplicated against the
stored scan. Select a row to inspect its request or response below. The detail
pane has **Pretty**, **Raw**, and **Hex** views and, for HTTP exchanges, the same
Inspector/Notes rail as the history detail. Matches in the selected view are
highlighted, and the detail header reports the match count. WebSocket rows show
one **Message** view.

A persistent project saves the query, options, structured result rows,
selection, and views. Rows from Repeater or Intruder are restored even when
those tools no longer hold the original message; selecting such a row shows
that its request or response is no longer available. Workspaces saved by older
versions contained only HTTP result IDs, so unknown IDs are dropped on open as
before.
Matching uses the decoded body representation; uncompressed retained bodies
are searched in full, while decoded compressed previews are bounded by the
same decoder limits as the message editor.

## Intercept and edit

1. Open **Proxy > Intercept** and click **Intercept is off** to arm request
   holding. Tick **Responses** if you also want to hold server responses.
2. Send a request from your testing browser or client. A held message appears
   in the table with its time, direction, method, URL, status, and length.
3. Select a row and switch to **Raw** to edit its head or eligible text body.
   **Pretty** and **Hex** are read-only previews in this editor.
4. Choose **Forward** to send the selected message, or **Drop** to reject it.

Use `Shift` to select a range, or `Ctrl`/`Cmd` to toggle rows. Each message keeps
its draft while you switch selections. **Forward all** and **Drop all** act on
the entire held list, not just the active row.

Only bodies with a known size within `--intercept-limit` (1 MiB by default)
are buffered. Large, streaming, and binary bodies remain header-editable only;
their body bytes pass through unchanged. HTTP/2 requests use an origin-form
request line and a synthetic `Host` header for `:authority`; HTTP/2 responses
have no reason phrase.

Turning interception off attempts to forward every held request and response
in table order, including edits in each message's draft, matching Burp's
behavior. A message with an invalid edited head remains held with an error; fix
the draft and use **Forward**, or use **Drop**. Forwarding applies match/replace
rules and rechecks scope. Dropped exchanges remain in history without a
response status. Closing the app aborts unresolved exchanges.

The interception editor works on the held body bytes, not a decompressed copy.
Do not assume a compressed or binary body is safely editable just because its
history preview was readable.

With interception on, complete WebSocket text and binary messages in both
directions are held in a separate panel above the HTTP table. Edit text or hex
bytes and choose **Forward message** or **Drop message**. Messages on each
direction wait in order; other connections and the opposite direction can
continue. The panel shows the oldest held WebSocket message. **Forward all**,
**Drop all**, and turning interception off also apply to this queue; invalid
drafts remain held for correction. A closed connection is marked in the panel.
Control frames interleaved with a fragmented message wait for its decision;
unchanged forwards preserve the original frames, while edited messages are
reframed and interleaved controls follow the edited message.

The WebSocket message and each edited payload are limited by `--intercept-limit`
(1 MiB by default). Fragment and control-frame wire overhead is additionally
limited to 64 KiB. An oversized or malformed held message closes the connection
rather than forwarding an incomplete message. The bounded interception channel
applies backpressure. Capture records forwarded messages, not dropped ones.

## Match/replace rules

Open **Proxy > Match/replace**. A rule chooses request or response, head/body/all,
a search pattern, a replacement, and literal or Rust-regex matching. Regex
replacements support capture references such as `$1`.

Rules run in list order, after interception and before forwarding. Body rules
apply only to buffered UTF-8 bodies, not streaming, oversized, or binary data.
Empty patterns and disabled rules are ignored. Invalid regexes stay visible
with an error and are excluded from the live policy; valid rules and toggles
remain active. Scope and rules are saved with a persistent project; temporary
projects keep them only for the current run.

## Intruder

Intruder automates variations of one request. It uses the same direct
HTTP/HTTPS client as Repeater, so an attack works with the proxy stopped or in
`--no-proxy` mode. It does **not** pass through interception, match/replace, or
Proxy history.

1. Right-click a history exchange and choose **Send to Intruder**, or open
   Intruder and paste or edit a raw request. Review the **Target** scheme and
   host.
2. Wrap each value to fuzz in `$$`, for example
   `GET /product?id=$$1$$ HTTP/1.1`. Select text and click **Add $$** to wrap it,
   or place the cursor and use **Add $$** for an empty position (`$$$$`).
   **Clear $$** removes every marker. **Auto $$** marks query values,
   `application/x-www-form-urlencoded` body values, and cookie values.
3. Edit the payload list on the right, one value per line. **Load from file**
   appends a UTF-8 text file (up to 16 MiB) to the selected set. Blank lines are
   ignored, and **Deduplicate** keeps the first occurrence. Sniper and Battering
   ram use one shared set; Pitchfork and Cluster bomb use one set per position.
4. Choose an attack and click **Start attack** or press `Ctrl`+`Enter`:

   | Attack | Positions per request | Request count |
   | --- | --- | --- |
   | **Sniper** | Each position in turn; other positions keep their base values | positions × payloads |
   | **Battering ram** | Every position receives the same payload | payloads |
   | **Pitchfork** | One payload from each set, advancing together | shortest set |
   | **Cluster bomb** | Every combination of one payload per position | product of set sizes |

5. Watch the result table. Select a row to inspect the generated request and
   response in Pretty/Raw/Hex. The header shows received/total counts and, for
   a finished attack, elapsed time.

Markers are literal delimiters, not part of the request: they are removed when
the request is generated. An unclosed `$$` is an error. Sniper uses the text
inside each marker as that position's base value. The other attack types replace
every marked position.

`$$` can appear anywhere in the request, including headers and bodies. Keep the
**Update Host header** option on when the target bar changes so the request stays
consistent; turning it off allows virtual-host testing.

**Scope:** when **Block out-of-scope traffic** is enabled, **Start attack** first
checks the effective target (an absolute-form request line overrides the target
bar) against the scope. Every generated request is checked again before it is
sent, because a payload can change an absolute-form target. Out-of-scope
requests produce failed result rows and do not reach the target. Intruder still
does not use proxy interception or match/replace rules, so confirm the target
and template before starting.

**Limits:** concurrency is 1–32 and one attack runs at a time. **Stop attack**
stops scheduling new requests; requests already in flight finish. The UI retains
up to 1,000 result rows and 64 MiB of result data, evicting the oldest rows. The
engine queues at most 64 unread results (and 64 MiB) before applying
backpressure. Generated requests are limited to 8 MiB with 64 KiB heads, each
request has a 60-second timeout, and response bodies use the same capture limit
as the rest of the app. Persistent projects save the attack configuration,
payload sets, result rows, and result-window state; temporary projects keep
them only in memory.

## Repeater

1. Right-click a history exchange and choose **Send to Repeater**, or open
   Repeater and use `+` for a blank tab.
2. Review the **Target** scheme/host, raw request head, and body.
3. Change one thing and click **Send**, or press `Ctrl`+`Enter` (`Cmd`+`Enter`
   on macOS).
4. Inspect the status, timing, size, and Pretty/Raw/Hex response. Keep separate
   tabs for variants you want to compare manually.

For an origin-form line such as `GET /admin HTTP/1.1`, the target bar selects
the connection destination. An absolute-form line such as
`GET https://lab.test/admin HTTP/1.1` overrides it. A non-empty edited `Host`
header is preserved, allowing virtual-host testing against a different
connection destination; a missing or blank header is filled from the target.

Repeater has its own direct HTTP/HTTPS client. It works with the proxy stopped
or in `--no-proxy` mode and negotiates HTTP/2 over TLS when the origin offers it.
It verifies origin certificates against platform trust anchors, falling back
to bundled public roots if none are available.

**HTTP Repeater bypasses interception, match/replace, and proxy scope enforcement.**
Its HTTP exchanges do not appear in Proxy history. It does not share the browser's
cookie jar or automatically re-authenticate; review copied cookies and tokens.

### Cookie jar

Open **Settings** using the gear beside global Search. The shared cookie jar is
empty initially and stays in application memory until Perseid exits, including
when switching projects. It is not saved or exported with a project. Captured
requests, responses and projects may independently contain cookie values.

Proxy request cookies and response Set-Cookie headers update the jar by default,
even when history capture is paused. Turn this off in Settings if needed. New
cookies seen only in browser requests are host-only with path `/`, because the
request does not include their original domain, path, or expiry attributes.
Cookies in HTTPS tunnels are visible only when TLS interception is enabled.
Applying cookies and updating from Repeater/Intruder responses are off by
default and controlled separately. Add or refresh a cookie manually by entering
its response URL and a `Set-Cookie` value in the cookie editor. Use **Settings →
Open cookie jar** to inspect the table, select a cookie to edit or remove, or
empty the jar. Matching uses the connection URL, not an edited Host header, and
honors domain, host-only, path, expiration and Secure (HTTPS only). SameSite is
stored but not enforced because direct requests have no browser site context.
The jar rejects public-suffix domains, cookies over 4096 bytes, and more than
512 stored cookies or 32 KiB of cookie names and values. It does not observe
browser state outside proxied traffic.

When applying cookies, Repeater replaces request cookie names found in the jar
and preserves unrelated names. Intruder preserves all explicitly supplied cookie
names, including fuzz payloads, and adds only missing names; to refresh an
existing Intruder cookie, remove it from the template. Editors and Intruder
result details display the original request rather than send-time jar changes.
Parallel requests can race when a server rotates a cookie, so reduce concurrency
or disable updates for such applications. The jar is not a login macro and will
not re-authenticate an expired session.

Captured history seeds use the decoded, bounded message representation, so
compressed, binary, or large bodies can be lossy. Review seed warnings before
sending. Responses retain up to `--capture-limit` body bytes and report
truncation or incomplete transfers. Requests have a 60-second end-to-end timeout,
with up to eight concurrent sends. Request tabs and their most recent responses
are saved with a persistent project; pending sends are not.

For WebSockets, use **Send to Repeater** from WebSockets history, edit text or
hex, choose a direction, and send repeatedly. The tab shows the original
connection's open/closed state and captured messages with Pretty/Raw/Hex views.
It uses that existing connection only; it cannot reopen a closed connection.
Review and explicitly confirm an incomplete captured prefix before sending.
Each direction has a queue of 16 messages, each bounded by `--intercept-limit`;
queue acceptance does not guarantee delivery. Replay is captured in WebSockets
history but bypasses interception. WebSocket Repeater tabs are live session
state and are not saved in projects.

Admission is bounded across **queued requests, active sends, and unread results
together**: at most **32 pending operations** and **64 MiB of retained-data
reservations**. A send reserves space for its response before network work
starts; the reservation is released when the UI consumes the result. These are
fixed engine limits, not additional CLI flags, and do not cap all retained
editor or tab state.

Requests are limited to **8 MiB of retained request buffers**, including header
and string storage, with **64 KiB request and response heads**. A larger
`--capture-limit` does not raise those request/head limits. Repeater rejects a
configured response capture limit at startup if it cannot fit a maximum-sized
request and its reserved response within the pending-byte budget.

When admission is full, **Send reports a retryable budget error without stopping
other sends**. The UI normally drains responses automatically; retry after
responses have been processed. Library consumers must drain `Repeater::try_recv`
to release admission for unread results. See
[budget troubleshooting](troubleshooting.md#repeater-reports-a-budget-or-size-error).

## Decoder

Paste or type into the first panel, then choose **Decode as**, **Encode as**,
**Hash**, or **Smart decode**. Every operation adds a panel; transforming an
earlier panel replaces the chain below it. Select text to transform only that
part, preserving the surrounding prefix and suffix. With no selection, the
whole panel is used.

| Menu entry | Behavior |
| --- | --- |
| **Plain** | Copy the input or selection without changing it. |
| **URL** | Decode `%xx` escapes, or percent-encode every byte using lowercase hex. |
| **HTML** | Decode named/numeric entities, or encode every character as a numeric entity. |
| **Base64** | Decode standard or URL-safe alphabets, with or without padding; encode as standard padded Base64. |
| **ASCII hex** | Byte-oriented hexadecimal; decoding accepts whitespace and requires an even digit count. |
| **Hex / Octal / Binary** | Convert numeric digit runs, not arbitrary bytes. For example, numeric Hex encoding changes decimal `1234` to `4d2`. |
| **Gzip** | Compress or decompress, with a decompressed-output limit. |

Use **ASCII hex**, not numeric Hex, when you mean a byte encoding. HTML encoding
of non-UTF-8 input is a textual per-byte representation; high bytes do not
round-trip byte-for-byte through Unicode entities.

**Smart decode** detects gzip, URL, HTML, Base64, and ASCII hex layers. It adds
one panel per layer and stops after 12 layers, a cycle, or no recognizable
encoding. Numeric conversions are excluded so plain numbers are not
reinterpreted automatically. No match produces a notice rather than an empty
result panel.

<details>
<summary>Supported hashes (46)</summary>

BLAKE2B-160/256/384/512, BLAKE2S-128/160/224/256, BLAKE3,
Keccak-224/256/384/512, MD2, MD4, MD5, RIPEMD-160, SHA-1,
SHA-224/256/384/512, SHA-512/224, SHA-512/256, SHA3-224/256/384/512,
SM3, Tiger, Whirlpool, SKEIN-256 (128/160/224/256-bit output), SKEIN-512
(128/160/224/256/384/512-bit output), SKEIN-1024 (384/512/1024-bit output),
TupleHash128-256 and TupleHash256-512. Output is lowercase hexadecimal.
TupleHash treats the selected bytes as one tuple element (including an empty
element), uses an empty customization string, and outputs 256 or 512 bits,
respectively; arbitrary tuples and output lengths are not exposed. The hash menu
also lists MD, SHA, BLAKE2, RIPEMD-160, SM3 and Whirlpool OID aliases alongside their canonical
names; selecting an alias uses and saves the canonical algorithm.

Skein and cSHAKE use RustCrypto's maintained `skein` and `cshake` crates,
licensed MIT OR Apache-2.0. TupleHash framing follows NIST SP 800-185 on top of
cSHAKE; no older standalone TupleHash dependency is required.

</details>

Right-click a text selection in a message pane and choose **Send selection to
Decoder**, then append a panel or start a new chain. Pretty selections contain
the displayed formatted or decoded text; Raw selections also reflect the text
preview, which can normalize or replace binary bytes. Hex preview selections
can be copied as hexadecimal; sending one to Decoder currently sends the
space-separated hexadecimal text, not the selected byte values. From HTTP
history, right-click an exchange to send the full captured request or response,
including its original captured body bytes and a
reconstructed HTTP head. Captures may themselves be truncated. Starting a new
chain replaces the current panels; appending preserves them.

Text panels are editable; Hex panels are read-only and preview at most 64 KiB.
Binary results open in Hex. Switching to Text gives a lossy preview, and editing
it replaces the bytes with UTF-8 text. **Copy** and **Copy output** export all
retained data as text or hex, not just the visible hex preview.

Manual encode/decode/hash transforms accept at most 8 MiB of input and produce
at most 16 MiB of output, including preserved selection prefixes/suffixes.
Persistent projects restore Decoder panel contents, operation annotations, the last applied
operation, Text/Hex view, and selection.

## Comparer

Comparer finds small changes between two messages. It uses one shared item
collection: both **Select item 1** and **Select item 2** show the same rows, so
select a different row in each table to choose the pair.

- Right-click a history row and choose **Send request to Comparer** to add the
  request as item 1, or **Send response to Comparer** to add the response as
  item 2. The item is appended to the shared collection and selected in that
  table.
- **Paste** reads text from the clipboard; **Load** opens an in-app path dialog
  that also accepts a dropped file. **Remove** deletes the item selected in the
  table you last clicked, and **Clear** empties the collection.
- Choose **Words** or **Bytes** under **Compare selected**. Word mode tokenizes
  on ASCII whitespace and highlights changed tokens. Byte mode compares raw
  bytes; it is useful when whitespace or encoding differences matter.

The result window has a Text/Hex choice for each pane, Previous/Next difference
buttons, length labels, and a color key for modified, deleted, and added data.
Tick **Sync views** to mirror vertical scrolling and wheel input, or use the
**Sync** button in either pane header. Text is a display-safe rendering of the
bytes: control and invalid bytes appear as `·`, CRLF is shown as LF, and the
panes are virtualized by line rather than laid out as one large document.

Comparer keeps up to **200 items** of **8 MiB each** in memory. A word compare
accepts up to **2 MiB and 250,000 words per item**; a byte compare accepts up to
**512 KiB per item** because Myers' algorithm is more expensive on bytes.
Oversized inputs are rejected with a message, and both modes give the diff
algorithm a 500 ms deadline. Comparer input items, table selections, and
result-window state are saved with a persistent project. The diff itself is
recomputed when the project opens; a result whose inputs are missing or no
longer fit the comparison limits is closed.

## Sequencer

Sequencer analyzes whether a sample of tokens behaves like random data. It can
help review session identifiers, CSRF tokens, password-reset codes, and other
opaque values, but its output is statistical evidence rather than proof of
predictability or safety.

Use **Manual load** to paste tokens, load a text file, or collect a response
from **Proxy > HTTP history** with **Send response to Sequencer**. Manual input
accepts one token per line; a line without spaces may also be comma- or
semicolon-separated. **Live capture** watches responses that complete after
**Start live capture** and extracts matches with the configured Rust regex.
The default expression is a deliberately broad opaque-token heuristic; narrow
it for the target. Choose whether to scan response headers, the decoded body,
or both, and whether duplicate tokens are ignored. Live capture and manual
load share the same sample.

**Sequencer settings** controls token format and the tests used by the next
analysis. Auto detection tries hexadecimal, then Base64, then text; override it
when the representation is known. Character tests compare character frequencies
and adjacent-character transitions when the sample has enough observations per
symbol. Bit tests decode the tokens and examine each bit position plus
whole-stream monobit, runs, longest-run, poker, byte-frequency, pairwise
correlation, and zlib-compression checks. The zlib result is informational.

**Analyze now** opens a floating result window with four tabs:

- **Summary** shows the overall randomness rating, effective entropy at the
  chosen significance level, the entropy chart across all five levels, the
  sample-size reliability, and detected format.
- **Character-level analysis** shows the alphabet, per-position count and
  transition results, and character-level effective entropy. It is skipped when
  the sample is too small for the alphabet, unless **Always** is selected.
- **Bit-level analysis** shows failing bit positions, effective bit entropy, and
  the whole-stream test table with p-values.
- **Analysis settings** changes the format, significance level, character-test
  mode, and bit tests, then re-runs the analysis on the current sample.

The tab holds up to **10,000 tokens**, each up to **16,384 characters**; one
analysis uses at most **5,000 tokens**, **1,024 character positions**, and
**4,096 bit positions**. Longer input is truncated and the result window says
so. Live capture starts with responses that arrive after it is armed, not with
existing history. Persistent projects save the token sample, settings, last
analysis source, and result-window state; the analysis is recomputed when the
project opens. Live capture is not resumed.

## Keyboard shortcuts

Use `Cmd` in place of `Ctrl` on macOS unless noted.

| Action | Shortcut |
| --- | --- |
| Target / Proxy / Intruder / Repeater / Sequencer / Decoder / Comparer | `Ctrl`+`Shift`+`T` / `P` / `I` / `R` / `S` / `D` / `C` |
| Open Search and focus its query | `Ctrl`+`Shift`+`F` |
| Focus the history filter | `Ctrl`+`F` |
| Leave the filter | `Esc` |
| Move through history, when not editing text | `j` / `k` or up/down arrows |
| First / last visible history row | `Home` / `End` |
| Start or stop an Intruder attack | `Ctrl`+`Enter` |
| Send the active Repeater request | `Ctrl`+`Enter` |
| Zoom in / out / reset | `Ctrl`+`+` / `Ctrl`+`-` / `Ctrl`+`0` |
| Enter / exit full screen on macOS | `Ctrl`+`Cmd`+`F` |

On macOS, the title-bar full-screen control enters a native full-screen Space.
Option-click it to zoom the window to the current screen without entering a
separate Space, matching the system green-button behavior.

## Limits and persistence

| Setting or surface | Current behavior |
| --- | --- |
| Captured bodies | 1 MiB retained per request/response by default; forwarding can stream larger bodies. |
| Interception buffering | Known-size bodies up to 1 MiB by default; other bodies are header-only. |
| History UI cache | 2,000 HTTP exchanges and 2,000 WebSocket messages; each cache uses the configured retained-body-byte budget (half of physical RAM by default, 256 MiB fallback, with a 64 MiB floor when RAM is reported). These are UI-cache limits; the temporary store has separate aggregate retention settings. |
| Temporary retention | Combined HTTP + WebSocket records default to `--history-limit`; retained HTTP body + WebSocket payload bytes default to `--history-bytes`. Override with `--temporary-retention-limit`/`--temporary-retention-bytes`; `0` disables a dimension. The oldest records are evicted first; each stream's newest record and any in-flight exchange are always kept, so the limits can be exceeded temporarily. |
| Inspection previews | Up to 256 KiB text/decoded body and 64 KiB Hex input; these are preview limits, not whole-process memory bounds. |
| Pretty body formatting | Independent 256 KiB input/output and 128-level nesting budgets; falls back to bounded Raw text on excess. |
| Repeater work | Eight active sends; at most 32 pending operations and 64 MiB of reservations across queued, active, and unread work. |
| WebSocket interception/replay | Message payloads and replay sends are bounded by `--intercept-limit`; interception also caps wire overhead at 64 KiB. The intercept channel is bounded by the connection limit; each live replay direction queues at most 16 messages. |
| Repeater request/head size | 8 MiB of retained request buffers; 64 KiB heads. The response capture limit must also fit the pending-work budget. |
| Intruder work | Up to 32 simultaneous requests and one attack at a time; the engine queues up to 64 unread results or 64 MiB. |
| Intruder results | At most 1,000 rows and 64 MiB retained in the UI; oldest rows are evicted. Generated requests are limited to 8 MiB with 64 KiB heads and a 60-second timeout. |
| Comparer work | 200 items of up to 8 MiB each; word diffs accept 2 MiB and 250,000 words per item; byte diffs accept 512 KiB per item. Both diff modes use a 500 ms deadline. |
| Temporary project | History and workspace state exist only in memory and are lost on exit unless saved as a project. |
| Temporary storage growth | Bounded by the aggregate temporary retention count and body-byte limits described above (soft limits: each stream's newest record and in-flight exchanges are kept); the status bar shows how many old records were evicted. Headers, indexes, and redb cache overhead are outside the byte limit. |
| Persistent project | One user-selected `.perseid` redb file; captures and workbench state auto-save. Files are unencrypted; saved retention defaults to unlimited but supports opt-in logical count/body limits. These do not cap physical file size. |
| Project history view | Opening loads a bounded newest snapshot. **Browse stored history…** pages all retained HTTP rows and loads details/notes on demand; Search scans all retained HTTP/WebSocket records. The site map and live WebSocket table remain cache-based. |
| Persisted workspace | Scope, match/replace, selected tab/section, filters, message views, Inspector notes and sections, Repeater tabs/responses, Intruder configuration/results, Decoder panels, Comparer items/result view, Search state, Sequencer samples/settings/analysis, and TLS interception/pass-through hosts. Active attacks/sends, held interception, pending operations, proxy lifecycle, and CA certificate/key are not stored. |
| Project transitions | New/Open ask Save/Discard/Cancel first when the temporary project has unsaved records or workbench edits; Save opens Save As and continues only after a successful save. Save As/Copy ask before replacing an existing file and stage the replacement before publishing it. |
| Local CA | Persisted separately from projects; its certificate and private key are never included in a `.perseid` file. |
| Sequencer work | At most 10,000 tokens of 16,384 characters; analysis uses 5,000 tokens, 1,024 character positions, and 4,096 bit positions. Capture is limited to the retained history. |
| Missing tools/protocols | No WebSocket fuzzing or HTTP/3/QUIC support. |

See [CLI options](getting-started.md#options) to change the UI-cache and
temporary-retention limits and [Safety and privacy](safety.md) for the
data-handling implications. Larger budgets allow more sensitive data to remain
in memory; they do not improve protocol support or make an unsupported editing
operation safe. One record can remain even if it alone exceeds the
history-body or temporary retention byte budget.
