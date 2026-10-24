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
| **Repeater** | Editable HTTP/HTTPS requests sent directly to their targets. |
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

- **New Project** shows a discard warning, then starts an empty temporary
  project. It does not delete an existing saved file.
- **Open Project…** opens an existing `.perseid` file. Enter its path in the
  dialog; there is no native file picker. If the proxy is running, it stops for
  the switch and restarts afterward. An Open does not prompt to save an unsaved
  temporary project, so use Save As first if you want to keep it.
- **Save Project As…** transfers the current project into a new file and makes
  that file active. The destination must not already exist; Perseid does not
  overwrite projects.
- **Save Project Copy…** writes a consistent copy to a new path and leaves the
  current project active. Its destination must also be new.

Saved projects persist HTTP and WebSocket captures plus the workbench state:
scope and match/replace rules, the selected tab/section, filters, message views,
Inspector notes and expanded sections, Repeater tabs and their last responses,
Intruder configuration and result rows, Decoder panels, Comparer items and
result-window state, Search state, Sequencer samples/analysis/settings, and TLS
interception and pass-through settings. Active network work is not restored:
running attacks or sends, held interception messages, pending operations, and
proxy lifecycle state stay in the current session. The local TLS CA certificate
and private key stay outside the project file.

Project files are not encrypted. Treat a `.perseid` file as sensitive traffic:
it can contain cookies, tokens, credentials, personal data, and binary bodies.
See [Safety and privacy](safety.md#data-handling) before saving or
sharing one.

**History is stored on disk in a saved project, but the current UI is still
bounded:** opening a project loads only the newest snapshot allowed by
`--history-limit` and `--history-bytes`. Older rows remain in the file but are
not yet pageable or searchable from the UI. In a temporary project, those same
limits bound the UI cache only: the temporary in-memory redb project currently
retains accepted captures without an aggregate eviction/size cap, so its RAM use
can grow with traffic until you clear history or exit. `--capture-limit` still
caps each stored body, and clearing history deletes stored records from the
active project.

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
  send its request or response to Repeater, Intruder, or Comparer, or send its
  response to Sequencer.
- The text filter matches method, host, path, URI, and status—not response-body
  contents. Scope filtering can narrow the view further.
- **Capture** pauses recording of new exchanges, not forwarding or interception.
- The in-memory history view is bounded by exchange count and retained-body
  bytes. These limits evict rows from the UI cache; they do not cap the temporary
  redb project's total RAM storage or prune saved-project files. Older saved
  records remain on disk even when evicted from the UI cache.
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
- There is no WebSocket message interception, editing, replay, or send-to-tool
  action yet. WebSocket history has the same count and retained-byte UI-cache
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
| **Hex** | A bounded byte-offset dump. In history and Repeater, this uses the same decoded body representation as the text views. |

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
and press **Go** or `Enter`. Search covers retained **Proxy history**, including
decrypted HTTPS exchanges.

Choose which parts of each exchange to examine:

- **Request headers** searches the request line and displayed headers.
- **Request body** searches the decoded request body.
- **Response headers** searches the status line and displayed headers.
- **Response body** searches the decoded response body.

The request and response body locations are on by default, and the headers are
included too. Supported gzip, deflate, and Brotli bodies are decoded before
matching, so searching for readable response text works even when the captured
wire body was compressed. Unsupported or malformed encodings fall back to the
captured bytes, matching the message editor's representation.

The options follow Burp's search model:

- **Case sensitive** makes literal and regex matching case-sensitive.
- **Regex** treats the query as a regular expression. Invalid expressions show
  an error instead of running.
- **Negative match** returns exchanges that do **not** match in any selected
  location.
- **In-scope only** restricts results to the host rules in **Target > Scope**.
  An empty scope includes every host.
- **Dynamic update** reruns the search as the query, options, or captured
  history change. Leave it off to run only when you press Go or Enter.

The results table shows the source, host, URL, status code, response length, and
request time. Select a row to inspect its request or response below. The detail
pane has **Pretty**, **Raw**, and **Hex** views and the same Inspector/Notes rail
as the history detail. Matches in the selected view are highlighted, and the
detail header reports the match count.

Search is limited to the currently loaded HTTP history snapshot. In a saved
project, older rows remain on disk but are not searched yet. WebSocket messages,
Repeater traffic, and their responses are not included. A persistent project
saves the query, options, result IDs, selection, and views; on open, stored
result IDs are checked against the loaded history snapshot and unknown rows are
dropped.
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
3. Edit the payload list on the right, one value per line. Blank lines are
   ignored. **Deduplicate** keeps the first occurrence. Sniper and Battering ram
   use one shared set; Pitchfork and Cluster bomb use one set per position.
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

**Repeater bypasses interception, match/replace, and proxy scope enforcement.**
Its exchanges do not appear in Proxy history. It also has no browser cookie jar
or automatic session refresh; review copied cookies and tokens before sending.

Captured history seeds use the decoded, bounded message representation, so
compressed, binary, or large bodies can be lossy. Review seed warnings before
sending. Responses retain up to `--capture-limit` body bytes and report
truncation or incomplete transfers. Requests have a 60-second end-to-end timeout,
with up to eight concurrent sends. Request tabs and their most recent responses
are saved with a persistent project; pending sends are not. WebSocket replay is
absent.

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
<summary>Supported hashes (31)</summary>

BLAKE2B-160/256/384/512, BLAKE2S-128/160/224/256, BLAKE3,
Keccak-224/256/384/512, MD2, MD4, MD5, RIPEMD-160, SHA-1,
SHA-224/256/384/512, SHA-512/224, SHA-512/256, SHA3-224/256/384/512,
SM3, Tiger, and Whirlpool. Output is lowercase hexadecimal. SKEIN, TupleHash,
and provider-specific OID aliases are not implemented.

</details>

Text panels are editable; Hex panels are read-only and preview at most 64 KiB.
Binary results open in Hex. Switching to Text gives a lossy preview, and editing
it replaces the bytes with UTF-8 text. **Copy** and **Copy output** export all
retained data as text or hex, not just the visible hex preview.

Manual encode/decode/hash transforms accept at most 8 MiB of input and produce
at most 16 MiB of output, including preserved selection prefixes/suffixes. There
is no **Send to Decoder** action from message editors yet. Persistent projects
restore Decoder panel contents, operation annotations, the last applied
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
| History UI cache | 2,000 HTTP exchanges and 2,000 WebSocket messages; each cache uses the configured retained-body-byte budget (half of physical RAM by default, 256 MiB fallback, with a 64 MiB floor when RAM is reported). These are not an aggregate cap on the temporary redb store or saved project files. |
| Inspection previews | Up to 256 KiB text/decoded body and 64 KiB Hex input; these are preview limits, not whole-process memory bounds. |
| Pretty body formatting | Independent 256 KiB input/output and 128-level nesting budgets; falls back to bounded Raw text on excess. |
| Repeater work | Eight active sends; at most 32 pending operations and 64 MiB of reservations across queued, active, and unread work. |
| Repeater request/head size | 8 MiB of retained request buffers; 64 KiB heads. The response capture limit must also fit the pending-work budget. |
| Intruder work | Up to 32 simultaneous requests and one attack at a time; the engine queues up to 64 unread results or 64 MiB. |
| Intruder results | At most 1,000 rows and 64 MiB retained in the UI; oldest rows are evicted. Generated requests are limited to 8 MiB with 64 KiB heads and a 60-second timeout. |
| Comparer work | 200 items of up to 8 MiB each; word diffs accept 2 MiB and 250,000 words per item; byte diffs accept 512 KiB per item. Both diff modes use a 500 ms deadline. |
| Temporary project | History and workspace state exist only in memory and are lost on exit unless saved as a project. |
| Temporary storage growth | The in-memory redb project retains accepted captures without an aggregate eviction cap. `--history-limit`/`--history-bytes` only bound the UI cache; RAM can grow until history is cleared or the app exits. |
| Persistent project | One local `.perseid` redb file; captured records and project-scoped workbench state auto-save. Files are plaintext, have no automatic disk-retention cap, and are not encrypted. |
| Project history view | Opening loads the newest snapshot up to `--history-limit` and `--history-bytes`. Older stored records are not pageable/searchable in the UI yet. |
| Persisted workspace | Scope, match/replace, selected tab/section, filters, message views, Inspector notes and sections, Repeater tabs/responses, Intruder configuration/results, Decoder panels, Comparer items/result view, Search state, Sequencer samples/settings/analysis, and TLS interception/pass-through hosts. Active attacks/sends, held interception, pending operations, proxy lifecycle, and CA certificate/key are not stored. |
| Project transitions | New Project confirms discard; Open Project has no save/discard prompt for a temporary project. Save As/Copy refuse to overwrite an existing file. |
| Local CA | Persisted separately from projects; its certificate and private key are never included in a `.perseid` file. |
| Sequencer work | At most 10,000 tokens of 16,384 characters; analysis uses 5,000 tokens, 1,024 character positions, and 4,096 bit positions. Capture is limited to the retained history. |
| Missing tools/protocols | No WebSocket message interception/replay or HTTP/3/QUIC support. |

See [CLI options](getting-started.md#options) to change the UI-cache limits and
[Safety and privacy](safety.md) for the data-handling implications. Larger
budgets allow more sensitive data to remain in memory; they do not improve
protocol support or make an unsupported editing operation safe.
One exchange can remain even if it alone exceeds the history-body byte budget.
