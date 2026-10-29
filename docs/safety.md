# Safety and privacy

[Documentation](README.md) · [HTTPS setup](tls.md) ·
[Workbench](workbench.md) · [Security policy](../SECURITY.md)

Perseid is intended for systems you own or are explicitly authorized to test,
including purpose-built training labs. Intercepted requests may contain
passwords, session tokens, cookies, or personal data.

## Authorized use

Do not point Perseid at third-party systems without permission. The
loopback-only default, scope filters, and optional scope enforcement are safety
aids, not permission to test an arbitrary host.

## Defaults

- The proxy binds to `127.0.0.1` by default; keep it on loopback unless you
  have a specific reason and understand the exposure.
- Captured request and response bodies are capped by `--capture-limit`
  (1 MiB per message by default).
- The HTTP and WebSocket history **UI caches** each accept up to
  `--history-limit` (2000 entries by default) and receive the configured
  `--history-bytes` budget (half of physical memory by default, with a
  256 MiB fallback if installed memory is unavailable). The temporary
  in-memory redb project additionally evicts the oldest captured records when
  it exceeds `--temporary-retention-limit` combined HTTP/WebSocket records or
  `--temporary-retention-bytes` retained body/payload bytes; both default to
  the history settings and can be disabled with `0`. In a saved project,
   retention defaults to unlimited; opt-in logical record/body limits are under
   **Proxy > HTTP history > Project storage…**. They do not cap physical disk use.
- The proxy accepts at most 128 client connections and 64 CONNECT tunnels at
  once by default. Excess connections are closed; excess CONNECT requests get
  a 503 response. Both limits are library configuration, not browser settings.
- Interception buffers at most `--intercept-limit` body bytes per held message
  (1 MiB by default). Larger or streaming bodies are held for header edits only
  and pass through unchanged.
- Scope filtering is always available as a view filter. **Block out-of-scope
  traffic** is opt-in and off by default; when enabled, HTTP requests and
  `CONNECT` tunnels outside the list receive a `403`. An empty scope allows
  every host. Scope is checked again after interception and rewriting, so an
  edit cannot move a request outside the guardrail unnoticed. For HTTPS, the
  `CONNECT` target is checked before a tunnel opens; with TLS interception
  enabled, decrypted requests are checked as well. Without interception, only
  the tunnel target is visible.
- **Repeater sends directly to the target and bypasses the proxy guardrails.**
  Scope filtering and blocking, interception, and match/replace rules do not
  apply to Repeater sends, and Repeater exchanges are not added to Proxy
  history. Confirm the target and request before clicking **Send**.
- **Intruder also sends directly, with its own scope check.** When **Block
  out-of-scope traffic** is enabled, starting an attack checks the effective
  first target (an absolute-form request line overrides the target bar) and
  refuses it if out of scope. Each generated request is checked again before it
  is sent, because a payload can change an absolute-form target; out-of-scope
  requests are recorded as failed results and are not transmitted. Intruder
  does not consult interception, match/replace, or Proxy history, so review the
  template and target before starting.
- Repeater separately bounds queued, active, and unread work to 32 operations
  and 64 MiB of retained-data reservations, with at most eight active sends.
  Requests have an 8 MiB retained-buffer limit and heads a 64 KiB limit. Full
  admission returns a retryable error, not an unlimited queue; these budgets
  are not a cap on all editor/tab state or process memory.
- Intruder bounds simultaneous requests to 32 and runs one attack at a time.
  The engine queues at most 64 unread results or 64 MiB; the UI retains at most
  1,000 result rows and 64 MiB, evicting the oldest. Each request has a
  60-second timeout, and responses use the configured capture limit.
- The prototype has no telemetry. Apart from proxied, explicit Repeater, and
  explicit Intruder traffic, the application does not make network requests;
  opening the project link in the About window is an explicit user action.

## Data handling

Perseid has two project modes:

- **Temporary (default):** captured history and project-scoped workbench state
  stay in an in-memory redb project. The oldest captured records are evicted
  when the temporary store exceeds its aggregate count or retained-body limits
  (`--temporary-retention-limit`/`--temporary-retention-bytes`, defaulting to
  `--history-limit`/`--history-bytes`). Data is discarded on exit unless you
  use **File > Save Project As…**. **New Project** and **Open Project** ask
  Save/Discard/Cancel before replacing a temporary project that has unsaved
  records or workbench edits; Save opens Save As and only continues after a
  successful save. A quit prompt is not implemented.
- **Persistent:** **Save Project As…** or **Open Project…** uses one
  user-selected `.perseid` redb database. Captured HTTP/WebSocket records and
  project-scoped workbench state are committed automatically. **Save Project
  Copy…** creates another file. There is no encryption at rest. Saved projects
  default to unlimited retention; **Project storage…** can opt into oldest-first
  combined HTTP/WebSocket record-count and retained-body-byte limits. Applying
  lower limits removes eligible records immediately and persists the policy.
  In-flight HTTP and each stream's newest record are retained even over budget.

Saved workbench state includes scope, match/replace rules, the selected
tab/section, filters, message views, Inspector notes and sections, Repeater tabs
and their last responses, Intruder configuration and results, Decoder panels,
Comparer items and result-window state, Search state and structured result rows,
Sequencer samples/settings/analysis, and TLS interception and pass-through hosts.
It does not include running attacks or sends, held interception messages, pending
network operations, or proxy lifecycle state. Opening a project loads only a
bounded newest-first history snapshot; older records remain in the file and can
be found by Search; **Browse stored history…** pages through all retained HTTP
records and loads details and notes on demand. The site map remains cache-based. If
a stored HTTP, WebSocket, or workspace snapshot cannot be decoded, the open fails
visibly and the previously open project and running proxy stay active. The project does not
include the local TLS CA certificate or private key.

New project files are created owner-only (`0600`) on Unix; Windows uses the
user's normal filesystem ACLs. This is access control, not encryption. Project
files can contain cookies, bearer tokens, credentials, personal data, and
binary uploads. Do not share them unless you have reviewed and intended to share
their contents. Clipboard copying is explicit. There is no background upload or
telemetry.

Retention counts captured records and body/payload bytes, excluding headers,
indexes, and workbench snapshots. It does not impose a hard physical file-size
cap. Freed redb pages can be reused without shrinking the file; there is no
automatic compaction. Save Copy writes retained data to a fresh file and may
reduce its size, but also copies retained workbench state.

Clear history deletes that stream's captures (and HTTP Inspector notes), not
independent tool snapshots or the project file. To delete a saved project,
switch away or quit, then remove the `.perseid` file through your file manager;
remove copies/backups separately. Unsaved temporary work is discarded on exit.
Retention, Clear history, and filesystem deletion are not secure erasure.

Schema-v1 files are upgraded to v2 atomically on open. The upgrade preserves
captures and notes and leaves saved retention unlimited. Back up the closed
file first if you need compatibility with an older Perseid; newer unsupported
schemas are rejected. See [Projects and persistence](workbench.md#projects-and-persistence).

Only committed transactions are expected to survive a crash. Capture events can
be dropped before storage commits; the project status reports capture loss when
detected. A forced termination cannot resume in-flight network requests. On
normal exit the app flushes the project writer, but temporary project contents
are always at risk of exit.

RAM retention limits are not a hard cap on whole-process memory. OS swap, crash
reports, clipboard history, terminal logs, and screenshots can retain copies.
Capture only data you are authorized to handle, on a machine you trust, and
redact shared output.

## TLS interception

HTTPS interception uses a certificate authority generated and stored locally.

- Perseid creates the CA automatically on first launch and reuses it across
  restarts and updates. The private key stays on the user's machine in the CA
  directory (`--ca-dir`, `PERSEID_CA_DIR`, or the platform data directory). The
  directory is created owner-only and the key is written owner-readable only.
- New/replacement CAs are validated and committed as complete generations;
  the selected certificate and private key must match. Existing top-level PEM
  pairs are reused until explicit regeneration. Use the active paths shown in
  the UI rather than assuming the old top-level filenames are current.
- Trust must be installed deliberately by the user in each browser. Perseid
  serves a local certificate download page, but never writes to system or
  browser trust stores and never installs the CA silently.
- Interception is on by default, matching Burp and ZAP. Turning it off, or
  adding a host to the pass-through list, keeps that traffic opaque. The CA and
  its private key remain available unless the user deletes the files.
- Removal is deliberate and documented in [HTTPS interception](tls.md): remove
  the browser/OS trust entries, then use **Delete files** to remove generations,
  the pointer, and legacy PEM files. A key-free coordination lock may remain.
- Pass-through hosts are an exclude list matched like scope (exact host,
  subdomains, and any port). An empty list excludes nothing.

## Reporting a vulnerability

If you believe you found a security issue in Perseid itself, please use a
private GitHub security advisory:
[report a vulnerability](https://github.com/andyintheshell/perseid/security/advisories/new).

See the [security policy](../SECURITY.md) for supported versions, response
expectations, scope, and safe-harbor terms. Please do not open a public issue
containing working exploit details or captured sensitive traffic.
