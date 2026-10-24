# Perseid storage and project persistence design

**Status:** Original design proposal; the first persistence implementation is
in `main` (`0.2.0` in development). See §0 for current status and deviations.<br>
**Design baseline:** Perseid `v0.1.0` (release commit `b8e51e9`, 2026-10-23)<br>
**Initial storage engine:** `redb` 4.4.x, version pinned through `Cargo.lock`  
**User-visible extension:** `.perseid`  
**Scope:** Local desktop projects; no cloud sync or multi-user editing

> This document began as a proposal against the published `v0.1.0` release. Persistence has since landed in `main`; §0 records what is implemented and what differs from the proposal. The remaining sections preserve the original design and acceptance criteria and are not a claim that every proposed feature shipped. For user-facing behavior, see the [Workbench guide](workbench.md) and [Safety and privacy](safety.md).

## 0. Review and implementation status

**Reviewed:** 2026-10-23, against the `0.2.0` workspace implementation.

**Verdict:** the design was adopted in part, with the deviations and unfinished requirements listed below. In particular, temporary-project aggregate retention is implemented, while persistent-disk retention caps are not. The document was written against the `v0.1.0` release tag, while the implementing checkout is `0.2.0`; §3 is a historical map, not the current source layout. The API snippets in §5 and §10 describe the proposed shape and are not compilable code.

### 0.1 Implementation status

Implemented in `crates/perseid/src/project/` (`schema`, `codec`, `records`, `backend`, `queries`, `transfer`, `service`) plus `crates/perseid/src/app/project_ui.rs`:

- Schema v1 as specified in §6.2, including the Perseid `application_id`, `schema_version`, `record_version` and `state` checks from §6.1/§11.1; a newer schema or an unsupported record version is rejected with a typed error.
- One writer actor with a bounded command queue, `Durability::Immediate` file commits, capture batching (128 events / 50 ms), flush receipts and workspace write coalescing (§5.2–5.4).
- HTTP `Started`/`ResponseHead`/`Finished`, WebSocket messages/connections and workspace state persisted, with secondary indexes updated in the same transaction (§6.3–6.5). Bodies are stored as raw bytes; structured records are versioned JSON DTOs.
- The workspace stores project-scoped workbench state: scope and match/replace rules, UI selections and message views, Inspector notes/sections, Repeater tabs and last responses, Intruder configuration and result rows, Decoder panels, Comparer items/result state, Search state, Sequencer samples/settings/analysis source, and TLS interception/pass-through hosts. CA material and transient network/interception tasks are excluded.
- Temporary startup, `New Project`, `Open Project…`, `Save Project As…`, `Save Project Copy…`, in-flight repair on open, and clear-history, with the proxy stopped and restarted around open/new (§4).
- Save/Discard/Cancel confirmation before New or Open replaces a temporary project that has captured records or workbench edits. Save opens Save As and continues the switch only after a successful save; cancelling or failing the save keeps the current project.
- Project status widget (mode, counts, capture loss, retention evictions, last error) and a final flush in `eframe::App::on_exit`.
- Temporary-project aggregate retention: oldest-first eviction by combined HTTP/WebSocket record count and retained HTTP body/WebSocket payload bytes. Limits default to `--history-limit`/`--history-bytes`, can be set with `--temporary-retention-limit`/`--temporary-retention-bytes`, and `0` disables a dimension. The newest record is always kept; persistent projects never evict automatically.
- **Remaining limitation:** saved project files still have no automatic disk-retention cap. `--history-limit` and `--history-bytes` bound the UI snapshot, not the file.
- A corrupt HTTP, WebSocket, or workspace snapshot now fails the open with a contextual error instead of substituting empty state; the previously open project remains active.

### 0.2 Deviations from the proposal

| Proposal | Implemented | Why |
| --- | --- | --- |
| §9.2 ordered delta replay during Save As | The single writer copies a consistent read snapshot to a new `initializing` file, marks it ready, then switches to it. Capture events queued during the copy are processed by that writer after the switch; queue overflow is reported as capture loss. | No separate delta log is used. Single-writer ordering handles normal handover while retaining the proposed file format and service boundary. |
| Paged UI over `list_http`/`list_ws` (§7.1) | Paged queries and cursors exist and are tested; the first UI wiring hydrates a bounded newest-first snapshot (count and byte budget) into the existing `History`/`WebSocketHistory` caches | Keeps the existing UI behavior in this change; the query API is the migration path. |
| Proxy-drop and storage-drop counters kept separately (§8.3) | The proxy's own `dropped_events` interrupts affected in-flight records; project queue overflow increments `capture_loss_count`, which the status widget reports | The two signals exist; only the storage-side count is durable in project metadata today. |
| §7.2 separately versioned workspace component keys | One `workspace/v1` JSON document holds all project-scoped workbench state, stored inline up to 32 MiB or split into 8 MiB chunks in the same `workspace` table with a manifest. Reads are capped at 512 MiB. | A single atomic snapshot keeps cross-tool state consistent; chunking handles result bodies without a schema change. |
| §7.2 persist logical inputs and regenerate derived results | Intruder results store response heads/bodies and rebuild their detail views; Comparer diffs and Sequencer analysis are recomputed on open. Repeater's last response is stored as its Pretty/Raw/Hex views because the tab does not retain the raw body. | Intruder and Repeater responses are already bounded by the UI; storing views preserves the visible result without adding a second body store. |
| Recent Projects and Save/Discard/Cancel prompts (§4.1, Phase C) | No recent-project list or quit prompt. **New Project** and **Open Project** ask Save/Discard/Cancel when the temporary project has captured records or workbench edits; Save opens Save As and continues only after a successful save. Normal exit flushes saved-project state and drops temporary history. | Recent-project and quit flows remain follow-up work; unsaved switches are covered. |
| Migrations (§11.2) | Older schemas are rejected with a clear “requires migration” error; no migration code yet | No schema-v2 exists to migrate; the check keeps forward compatibility safe. |

### 0.3 Test and follow-up notes

- Storage tests live with the modules they cover (`project::service` has the lifecycle tests; `codec`, `records` and `schema` have round-trip/version tests). They cover: capture round-trip and flush, Save As then reopen, Save Copy independence, exclusive destination refusal, invalid/incomplete/newer-schema files, in-flight repair on open, clear-history persistence, legacy and complete workspace round-trips (including result bodies and view state), app-level workspace wiring, chunked workspace replacement/readback, and ID monotonicity across clears.
- Follow-up work, in priority order: (1) wire history views and Search to the paged persistent queries so users can reach older saved records; (2) add a recent-project list and a quit prompt; (3) add schema migration/integrity handling and more crash, scale, and sidecar tests; (4) define persistent disk-capacity/retention controls.

## 1. Original proposal decision

Perseid has **two project modes**, with identical logical data and behavior:

1. **Temporary (default):** At process startup, Perseid creates an in-memory `redb` project. There is no `.perseid` file and no persistence of project content to disk. On exit, the project is discarded unless the user explicitly saves it.
2. **Persistent:** Once the user chooses **Save Project As…**, or opens an existing `.perseid` file, the active project is backed by **one redb database file**. All subsequent meaningful changes are automatically committed to that file. No explicit Save operation is required.

The file is a **normal redb database**, not a ZIP, JSON bundle, SQL database, or custom-written transaction log. All project data, indexes, and crash-recovery metadata live inside the file; the application must not create permanent `.wal`, `.journal`, `-shm`, blob folders, or other sidecars. A project is portable as one `.perseid` file **after all operations that should be included have committed**.

redb's copy-on-write, crash-recovery design guarantees consistency of committed transactions; it does **not** guarantee that every network event reached the database or that an unfinished HTTP request can resume over the network. This distinction must remain visible in the implementation and UI.

### Decisions at a glance

| Area | Decision |
| --- | --- |
| Storage engine | `redb` (pure Rust, copy-on-write B+trees, ACID) |
| File representation | Single `.perseid` redb file, with versioned Perseid schema |
| Startup behavior | New temporary project in RAM, regardless of recent files |
| First Save As | Create a new file, transfer the temporary project's state, then switch atomically at the **application level** |
| Open | Validate, recover if necessary, migrate when supported, then replace the active project |
| Save semantics | Automatic incremental commits in persistent mode; explicit **Save Project Copy…** for independent snapshots |
| Writer | One serial project-writer actor, off the UI and network threads |
| Readers | Short redb read transactions through project queries; no long-lived UI transactions |
| History | Project owns retained records; UI owns only a bounded working cache |
| Data format | Stable, versioned wire DTOs; raw binary payloads in separate redb tables |
| Search | Indexed metadata first; cancellable bounded scans for content/regex; more indexes later if benchmarks justify them |
| Durability | `Durability::Immediate` for disk commits; batch related changes in one transaction |
| CA private key | Remains in per-user app data, **never** embedded in a project |
| Encryption | Not provided in v1; projects contain sensitive plaintext traffic |
| Multiple processes | One write-open process per project; no collaborative or multiprocess writing |

## 2. Goals and non-goals

### 2.1 Goals

- **Exactly one user-managed project file** while a persistent project is open, including after abnormal termination. Recovery information stays in that file.
- **Two simple modes**: temporary from startup; persistent only after explicit Save As/Open.
- **No large blocking operations** on egui's frame thread or proxy forwarding threads.
- **Crash consistency**: reopening produces a valid last-committed state, not a partially updated record/index combination.
- **Usable after restart**: restore HTTP and WebSocket history, requests/responses that were captured, and supported workbench state.
- **Bounded RAM**: disk-backed histories may grow without being copied wholesale into `PerseidApp`.
- **Explicit capture fidelity**: preserve exactly the bytes Perseid captured; do not pretend a truncated/decoded preview is a full original wire message.
- **Extensibility**: new tools can add versioned records/tables without redesigning the file format.
- **Safe evolution**: version checks, migration tests, forward-version rejection, and independent backups.

### 2.2 Non-goals for the initial release

- Reading or writing `.burp`, ZAP/HSQLDB, or other proprietary project formats.
- Syncing a project through cloud file-sharing software while it is open; concurrent editing by multiple processes.
- Reopening a live TCP connection or continuing an interrupted upstream HTTP transfer after restarting Perseid.
- Persisting editor cursor positions, hover state, dialogs, ephemeral selections, or running interception decisions.
- Unlimited full-body capture. Existing capture truncation and safety limits remain until explicitly redesigned.
- Encryption-at-rest, content deduplication, background full-text indexing, and automatic compaction in the first implementation.

## 3. Historical v0.1.0 integration map

This map describes the code at the design baseline, not the current module layout.
The current implementation paths are summarized in §0.1 and `docs/design.md`.

Relevant source modules to adapt:

| Existing location | Current role / integration action |
| --- | --- |
| `crates/perseid/src/proxy/events.rs` | Defines `ProxyEvent::{Started, ResponseHead, Finished, WebSocketMessage}` and the captured byte/metadata types; use as the source of persisted capture events. Do not serialize these runtime types directly. |
| `crates/perseid/src/proxy/mod.rs` | Owns the bounded capture-event receiver and `dropped_events()` metric; preserve forwarding isolation and interception backpressure. |
| `crates/perseid/src/history.rs` | `History` holds `HashMap<u64, HistoryItem>`, `VecDeque` order, byte/count limits and in-flight state. Migrate its responsibilities into project storage and a bounded presentation cache. |
| `crates/perseid/src/websocket_history.rs` | Analogous bounded WebSocket records; persist messages and compact connection metadata. |
| `crates/perseid/src/app.rs` | `PerseidApp` holds history, WebSocket history, workbench settings, tab states, selection and filters; replace direct ownership of the *complete* history with project handles/queries. |
| `crates/perseid/src/app/search.rs` | Refactor cross-history search to stream/cancel project queries; preserve current filter semantics. |
| `crates/perseid/src/app/{repeater,intruder,decoder,comparer,rules,inspector}.rs` | Add explicit snapshot/restore DTOs; decide which parts are persistent vs transient. |
| `crates/perseid/src/site_map.rs` | The site map is derived from HTTP history. Rebuild from project records or later maintain a rebuildable internal index. |
| `crates/perseid/src/ca_store.rs` | **Leave outside project storage**: local CA and private key remain per-user, machine-specific application data. |

In v0.1.0, `PerseidApp::drain_events` dispatches `WebSocketMessage` to `websocket_history.apply()` and other events to `history.apply()`. The HTTP `HistoryItem` has separate `id` and one-based `number`, plus `Phase::InFlight` / `Phase::Finished` states. The proxy event stream has request-start, response-head and terminal events; request-body information arrives with the terminal `ExchangeEnd`. Storage must preserve that event lifecycle, not assume every exchange arrives as one complete object.

**Integration boundary:** avoid spreading redb calls throughout UI panels. Introduce a cohesive `project` subsystem as the single source of truth. The existing models can be adapters during the migration, but must not permanently duplicate all captured bodies in the UI.

Proposed files (names are recommendations; adjust to code conventions):

```text
crates/perseid/src/
  project/
    mod.rs            # ProjectHandle, mode, public commands/queries
    service.rs        # writer actor, command sequencing, lifecycle
    backend.rs        # open/new memory and file databases
    schema.rs         # redb table definitions, constants, schema checks
    codec.rs          # versioned on-disk DTO encoding/decoding
    records.rs        # persisted HTTP/WS/workspace models
    indexes.rs        # atomic secondary-index maintenance
    queries.rs        # paged history, details, search
    migration.rs      # application schema migrations
    transfer.rs       # temporary -> file Save As; project copy
    maintenance.rs    # integrity checks, cleanup, metrics
    tests.rs          # storage-focused tests
  app/
    project_ui.rs     # New/Open/Save As/Save Copy, error/status UI
```

## 4. Proposed user-facing lifecycle and semantics

The behavior in this section is the original target proposal. Where it differs
from the shipped workflow, §0 and the [Workbench guide](workbench.md) describe
current behavior.

### 4.1 Startup: temporary mode

- Every normal launch starts with **Untitled — Temporary**. Do not automatically reopen the previous project.
- Create `Database::builder().create_with_backend(InMemoryBackend::new())`, initialize the same Perseid schema as a file-backed database, and start the project service.
- No `.perseid` file is created; the only allowable disk writes in this mode are existing **global application** files such as the CA and user preferences (not project content).
- Temporary history follows explicit aggregate entry-count and retained-body limits. The writer evicts the oldest HTTP/WebSocket records after a capture commit, keeps at least the newest record, and reports evictions in the status bar. Limits default to `--history-limit`/`--history-bytes` and can be overridden with `--temporary-retention-limit`/`--temporary-retention-bytes`; `0` disables a dimension.
- The UI should distinguish **Temporary** from **Saved** without introducing a confusing dirty asterisk: unsaved temporary data is *always* at risk of exit.
- On New/Open, show a **Save / Discard / Cancel** prompt if the temporary project has meaningful content. Save uses Save As; a cancelled or failed save leaves the current project open. A quit prompt remains future work.

### 4.2 Save Project As…: temporary to persistent

- Prompt for a new `.perseid` path; reject existing files unless an explicit, separately designed replace workflow is invoked.
- On success, change the project mode to persistent **only after** the target has a complete committed schema and all selected state has been transferred through the agreed handover boundary.
- Preserve original IDs, ordering, relevant selections and workspace state.
- Keep the in-memory project available if the transfer fails. Do **not** move the only copy of user data before the new project is committed.
- From that point on, saved-project mutations automatically write to the file; the Save menu may show **Save Project Copy…** rather than implying that users need to periodically press Save.
- If a crash occurs during first Save As, the original in-memory project is necessarily lost and the destination may be only **partially initialized**. Give the destination an explicit initialization state in metadata; do not open it as a valid project. The app cannot promise recovery of an unsaved RAM session after a process crash.

Section 9 specifies a safe transfer protocol while capture continues, including a fallback if capture traffic outpaces transfer. This is the most delicate lifecycle operation.

### 4.3 Open Project…

1. Prompt Save/Discard/Cancel if a temporary session contains data; in persistent mode, flush or visibly report failures before switching.
2. Quiesce the current project operations. Resolve or drop any held interception **explicitly** (never auto-forward an edited request by virtue of opening a file). Stopping the proxy for project switching is the safest initial policy; Repeater/Intruder ongoing operations should be stopped or deliberately canceled.
3. Open the chosen database using **`Database::open`, not `Database::create`**; reject missing/non-database files.
4. Allow redb to perform built-in crash recovery. Validate Perseid's magic/application identifier, schema version, initialization state, key metadata and table compatibility.
5. If compatible, hydrate lightweight workbench state and history query cursors. Do **not** eager-load entire request/response bodies.
6. Rebuild derived views lazily and mark any previously in-flight exchanges as **Interrupted by previous shutdown** (see §8.4).
7. Publish the new project handle and switch the UI. If validation fails, retain the previous project and present the error. Do not overwrite or repair an incompatible file silently.

A file already write-open elsewhere must show an actionable **Already open** error. The first version does not need a read-only viewer; adding one later should not weaken the single-writer guarantee.

### 4.4 New Project, Close and Quit

- **New Project** returns to a fresh in-memory backend with newly initialized IDs; ask before discarding temporary content.
- For a persistent project, send a flush barrier, wait off the UI thread for a confirmed durable commit, close read transactions/worker resources, and only then release the database handle. An ordinary close must not require a snapshot rewrite.
- If a flush fails (disk full, permission change, I/O fault), show **Some changes were not saved** and offer Retry, Save Copy if possible, or Explicitly Discard. Do not describe the file as up-to-date.
- `Drop` is not a substitute for orderly shutdown; forced process termination can only recover committed transactions.

### 4.5 Save Project Copy…

- Create a new, independent `.perseid` file containing a **point-in-time consistent snapshot**. Keep the original project active and writable while the copy is prepared where practical.
- Copy using a **redb read-transaction snapshot and logical table iteration**, not `std::fs::copy` on a live open database file. Copy all core data and indexes consistently; alternatively rebuild indexes in the destination and validate counts.
- Mark the copy `Initializing` until the final durable commit switches it to `Ready`. If copied to a user-selected target that is later found incomplete, report that it is an interrupted copy, not a valid project.
- Reuse the same transfer engine where possible. **Never silently replace an existing project**. If overwrite support is later added, require staging and an atomic replacement strategy with platform-specific verification; avoid permanent sidecars.

## 5. Architecture and threading

```text
Browser / upstream network
          |
          v
    Proxy runtime (Tokio)
          |
     bounded ProxyEvent channel       Repeater / Intruder results
          |                                  |
          v                                  v
    capture/event adapter  <---------- workbench mutations
          |
          v
  PROJECT SERVICE / SINGLE WRITER ACTOR (dedicated thread)
   - owns active mode and redb Database
   - assigns project-wide stable IDs / sequence numbers
   - serializes mutations into short transactions
   - batches disk commits, creates indexes, emits notifications
   - handles transfer, flush, open/close, migrations
          |
          +----- redb InMemoryBackend (temporary)
          |         OR
          +----- redb file backend (persistent: assessment.perseid)
          |
          v
  project query interface / short-lived read transactions
          |
          v
  egui: paged row metadata, bounded detail cache, panel DTOs
```

### 5.1 Project API contract

The UI uses non-blocking commands and asynchronous replies rather than direct redb access. Illustrative interfaces:

```rust
pub enum ProjectMode {
    Temporary,
    Persistent { path: PathBuf },
}

pub enum ProjectCommand {
    Capture(ProxyEvent),
    SetWorkspace(WorkspaceUpdate),
    ClearHttpHistory,
    ClearWebSocketHistory,
    Flush { reply: ReplySender<Result<CommitReceipt, ProjectError>> },
    SaveAs { path: PathBuf, reply: ReplySender<Result<(), ProjectError>> },
    SaveCopy { path: PathBuf, reply: ReplySender<Result<(), ProjectError>> },
    Open { path: PathBuf, reply: ReplySender<Result<(), ProjectError>> },
    Shutdown { reply: ReplySender<Result<(), ProjectError>> },
}

pub struct HistoryQuery {
    pub filter: HistoryFilter,
    pub after: Option<HistoryCursor>,
    pub limit: usize, // hard capped by the service
}

pub struct ProjectHandle { /* opaque channel and shared database/query handle */ }
```

The types above are **illustrative design contracts**, not drop-in compilable code; use the actual app's channels and error types. The public API must support: `list_http`, `get_http`, `list_ws`, `get_ws`, `search`, `load_workspace`, `set_workspace`, `mutate_history`, `flush`, `mode` and `status`.

### 5.2 Single writer; concurrent readers

- Only the project actor initiates write transactions. This gives deterministic ordering for `Started`, `ResponseHead`, `Finished`, workspace edits, deletes and cleanup.
- Readers can use redb MVCC concurrently with a writer, but they must finish read transactions quickly. **Do not retain a read transaction per egui tab or for the lifetime of a search window**; long-held snapshots pin pages and can increase the database's size.
- Commands and query results carry a **project generation ID**, incremented on Open/New/SaveAs switch. Ignore stale query results from prior generations.
- For each write batch, update primary records **and every relevant secondary index in the same redb transaction**. Emit UI notifications only *after* successful commit, or clearly label optimistic transient state as not yet durable.
- A failed commit must not be acknowledged as durable. On persistent I/O failure, stop normal write acknowledgements, expose the failure and protect uncommitted changes in bounded RAM where feasible.

### 5.3 Channel and overload behavior

The existing proxy uses a bounded `std::sync::mpsc` event queue and exposes dropped-event counters; interception has a separate backpressured channel. Do not combine these channels or let a stalled project writer block the interception decision path.

- Make project command queues bounded and instrument their lengths and overflow counts.
- **Never block proxy forwarding on a filesystem sync**, and never silently lose a capture event. If a capture queue overflows, record a loss counter, warn the user and reconcile affected in-flight records as interrupted/unknown. An overflow indicator is not evidence that traffic was saved.
- Continue to honor `MAX_EVENTS_PER_FRAME`-style UI processing bounds, but remove the frame loop as the only owner of durable history state. Prefer a dedicated consumer/adapter that drains capture events independently of frame rate.
- During a transfer or bulk search, enforce resource budgets and cancellation rather than allowing unbounded event buffers.
- Test both storage failures and high-traffic overload. A product promise of **zero capture loss** would require redesigning the current lossy proxy event path; it is **not** part of this plan.

### 5.4 Batching and durability

Initial *tunable* policy:

- Batch up to **128 events or 50 ms**, whichever occurs first, for ordinary capture updates. These are starting measurement parameters, not strict promises.
- `Flush`, project switching, snapshot barriers, explicit user actions requiring confirmation, and shutdown force completion of a durable transaction.
- Set `Durability::Immediate` on disk write transactions and check `commit()` errors. Never use `Durability::None` while reporting commits as saved.
- Workspace edits may be debounced by **250–500 ms**; close/switch still performs a flush barrier.
- Track `last_accepted_seq`, `last_committed_seq`, outstanding command count and last error. “Saved” means the relevant sequence is **durably committed**, not merely received in a queue.

## 6. Native format and schema v1

### 6.1 Physical format and metadata

`.perseid` is exactly a redb database. Do **not** prepend custom bytes to the redb header. Instead store a recognizable Perseid application marker in the metadata table and check it on open.

All persisted values must use **explicit versioned wire representations**, not serialized egui widget structures or compiler-dependent Rust object layouts.

Recommended record strategy:

- Simple metadata: UTF-8 strings / fixed-width little-endian integers or versioned JSON in `meta`.
- Structured records: named-field `serde` wire DTOs encoded as JSON initially (simplest to inspect/migrate); migrate individual hot paths to a versioned binary format only if benchmarks justify it. Limit lengths and reject invalid structures.
- Large bodies: **raw `&[u8]` values**, stored separately; never stringify binary traffic. Each body metadata record includes capture/truncation and encoding information.
- Each serialized record has its own `record_version`; the project also has an overall `schema_version`. Do not rely on redb's internal file-format version to perform Perseid migrations.

Required project metadata:

```text
application_id          = "perseid-project"
schema_version          = 1
project_id              = UUID (random, generated at project creation)
created_at              = UTC timestamp in milliseconds
last_writer_version     = "0.x.y"
state                   = "initializing" | "ready" | "migrating"
next_record_id          = monotonically increasing u64
next_event_sequence     = monotonically increasing u64
last_committed_sequence = u64
capture_loss_count      = u64
```

Unknown application marker => refuse to open. Newer unsupported `schema_version` => refuse to write; explain the upgrade requirement. Missing required values, unexpected record versions, corrupt values or out-of-range sizes => return a typed format error, not a panic.

### 6.2 redb table plan

The names and key encodings below form **schema v1**; write them down in code and fixtures. Examples use a conceptual `TableDefinition<Key, Value>` signature.

| Table | Key | Value | Purpose |
| --- | --- | --- | --- |
| `meta` | UTF-8 name | encoded scalar/JSON | Project identity, schema, counters, state |
| `http_exchanges` | `u64 exchange_id` | versioned `HttpExchangeV1` | Captured request/response metadata, phase, timing, capture flags |
| `http_bodies` | `(u64 exchange_id, u8 part)` | raw byte sequence | Request=0, response=1; absent for not yet captured |
| `http_by_time` | `(timestamp_ms, exchange_id)` | sentinel | Chronological pagination |
| `http_by_host` | `canonical_host + 0x00 + exchange_id_be` | sentinel | Host/site-map filtering; host must not include NUL |
| `http_by_status` | `(status_code, exchange_id)` | sentinel | Status filter; only if status known |
| `ws_messages` | `u64 message_id` | versioned `WsMessageV1` | Message metadata, connection ID, direction, opcode, capture flags |
| `ws_payloads` | `u64 message_id` | raw bytes | Captured WebSocket payload |
| `ws_by_time` | `(timestamp_ms, message_id)` | sentinel | Chronological WS pagination |
| `ws_by_connection` | `(connection_id, message_id)` | sentinel | Connection-message filtering |
| `ws_connections` | `u64 connection_id` | versioned `WsConnectionV1` | Store handshake-derived shared info only once |
| `workspace` | UTF-8 component key | versioned bytes/JSON | Repeater, Intruder, Decoder, Comparer, scope/rules, notes |
| `annotations` | `(entity_kind, entity_id)` | versioned JSON | Notes, tags, user modifications |
| `transfer_mutations` | `u64 event_sequence` | versioned mutation | **Temporary-only**, used during Save As, cleared afterward |

**Index encoding rules:** Timestamp should have a clear nonnegative Unix-epoch-milliseconds convention (validate out-of-range values). Encode numerical composite keys in a well-defined, sorting-compatible way; redb tuple keys can be used after tests confirm range ordering. For the host index, normalize hostnames consistently (lowercase ASCII/IDNA policy, strip an optional trailing dot, handle IPv6 brackets carefully). Reject embedded NUL before using delimiter-based keys. If schema code uses byte keys instead, write an explicit byte layout and round-trip/range tests. Never use a lossy hash as the sole identity of a hostname.

All indexes are **derived from primary records**. Provide a `rebuild_indexes()` maintenance operation that can reconstruct them atomically; avoid treating the site map as irreplaceable primary content. A clear-history operation must remove records, bodies and secondary-index entries in one bounded series of transactions, with a resumable maintenance marker if it cannot fit in one.

### 6.3 HTTP schema (logical DTO)

```rust
// Design DTO; exact serde attributes and byte codec to be specified in codec.rs.
struct HttpExchangeV1 {
    record_version: u16,
    id: u64,                        // stable, never reused in this project
    display_number: u64,            // one-based UI sequence
    started_at_ms: u64,
    duration_ms: Option<u64>,
    scheme: String,
    host: String,
    port: u16,
    method: Vec<u8>,                // preserve captured bytes where available
    target: Vec<u8>,                // preserve URI/path representation
    http_version: String,
    request_headers: Vec<(Vec<u8>, Vec<u8>)>, // preserve order/duplicates
    response_status: Option<u16>,
    response_headers: Option<Vec<(Vec<u8>, Vec<u8>)>>,
    phase: PersistedPhase,          // InFlight | Completed | Failed | Dropped | Interrupted
    result_detail: Option<String>,
    request_capture: BodyCaptureInfo,
    response_capture: BodyCaptureInfo,
}

struct BodyCaptureInfo {
    observed_len: u64,              // total bytes observed if known
    stored_len: u64,                // bytes actually stored
    truncated: bool,
    original_encoding: Option<String>,
}
```

**Important fidelity caveat:** v0.1.0 `RequestInfo`, `HeaderList` and `CapturedBody` define what the proxy currently observes; not every original HTTP/1.1 wire detail survives HTTP/2 and `hyper` normalization. The storage layer must record available exact captured bytes **without inventing unobserved bytes**. If a field is only a normalized representation, document that explicitly. Preserve header ordering and duplicate headers to the extent the capture model allows. Store the raw compressed response body if that is what capture provides; display-only decompressed data should be generated on demand.

At a minimum persist `CapturedBody`'s actual bytes and total/truncated information without assuming `len() == total_len()`. Response-head and Finished updates modify the existing exchange, preserving its `id` and time index. If `Finished` arrives without `Started` due to an upstream dropped event, mark a loss diagnostic; do not create a fabricated full request.

The **UI display number** is not the proxy event ID: v0.1.0 deliberately separates them. Allocate project record IDs and display numbers under the project service; maintain a mapping `(proxy_instance_generation, proxy_event_id) -> project_exchange_id` for in-flight events. IDs cannot collide after reopening a project or restarting the proxy. Once terminal, the mapping can be retired.

### 6.4 WebSocket schema

Use `WebSocketMessageV1` containing ID, display number, timestamp, connection ID, direction, opcode and `BodyCaptureInfo`. Store actual captured payload in `ws_payloads`; store handshake-derived connection metadata once in `ws_connections` even though v0.1.0 currently repeats it in each `WebSocketMessageItem`.

Capture only messages currently exposed by the proxy (complete text/binary messages). Do not synthesize control frames or unobserved fragments. Keep opaque payload bytes; decode UTF-8 only in views.

### 6.5 Workbench state

Use separately versioned workspace entries, not a serialization of `PerseidApp`:

| Component key | Persist | Do not persist |
| --- | --- | --- |
| `scope/v1` | scope hosts, enabled flag, project-scoped enforcement | transient input-field focus |
| `match_replace/v1` | ordered rules, enabled flags and directions | compiled regex objects |
| `repeater/v1` | tab IDs/order, names, target, editable request bytes, saved results/history subject to retention | in-progress socket handles, runtime engine |
| `intruder/v1` | templates, payload sets, attack mode, settings, saved result summaries/bodies with retention | active attack tasks, channel state |
| `decoder/v1` | user content and transform chain | ephemeral focus/cache |
| `comparer/v1` | user inputs, options, references to project records, user-pinned results | derived diffs/cache |
| `annotations/v1` | notes/tags keyed by stable record ID | selected text range |
| `workspace_layout/v1` (optional) | active tool/tab when useful | window geometry, hover, scroll momentum |

Do not serialize `std::sync` handles, network clients, closures, current `egui::Context`, pending intercept decisions, derived `MessageViews`, computed diffs, or secret CA key material. Persist **logical inputs** and regenerate derived results.

For `Comparer` and Repeater items seeded from a history row, decide whether to save (a) an immutable snapshot of the actual editable bytes, or (b) a reference to the original row. Prefer **snapshot plus optional source ID** for user-edited inputs; otherwise deletion/retention of the original row could invalidate a saved working document. Large Intruder results should be kept in separate records/body tables once implemented; don't put megabytes of results into one workspace JSON value.

### 6.6 Project settings vs global settings

**Project:** scope, match/replace, saved tool tabs, annotations, local history-retention policy, search state only if intentionally saved.  
**Global app:** theme, UI scale, window position/size, most recent file paths, global listener defaults, trust-store state, local CA and private key.  
**Session-only:** held interception decisions, uncommitted GUI edits, open connections, current proxy running state, live Intruder attack tasks.

Opening a project must **not** automatically start the proxy, trust a certificate, replay requests, or launch an Intruder attack. Saved rules can be restored to the UI, but the user controls whether to resume live operation.

## 7. Reads, pagination, search and memory budgets

### 7.1 Query model

Persistent projects must not hydrate every history item on open. `History`'s existing `HashMap`+`VecDeque` logic is appropriate as a bounded **temporary** representation or cache, but not as the permanent full dataset for disk-backed mode.

- `list_http(filter, cursor, limit)` returns **lightweight row summaries**, e.g. 100–250 records. The service enforces a maximum page size.
- Use keyset pagination on `http_by_time` using `(timestamp, id)`; this avoids full rescans and maintains stable ordering with equal timestamps.
- `get_http(id)` loads the single record and associated request/response bodies on demand; cache only a small number of selected details, with a byte ceiling.
- On mutation, invalidate only affected rows/filters; avoid full `rebuild_http_visible()` scans across all historical data every frame.
- When opening a new project, increment generation and invalidate `selected`, cached details, visible-list cursors and site-map caches unless references remain valid within the new project.
- Site map: incrementally build from host/path metadata, or rebuild in cancellable batches. Its stored index is optional and always rebuildable.

### 7.2 Search behavior

The v0.1.0 UI supports text/case/regex/negative/in-scope and history-wide searching. redb does **not** provide SQL or a built-in full-text search index.

Initial implementation:

1. Apply cheap **indexed** filters first: host, status, time, scope.
2. Run a cancellable background scan over candidate record IDs for text/regex/negative predicates; decode only the fields selected by the search options.
3. Bound results, decode sizes, regex search time and CPU effort; stream progress and matching IDs to the UI.
4. If searching decoded bodies, honor existing gzip/deflate/Brotli preview limits and clearly distinguish stored original bytes from display-decoded content.
5. A search must stop promptly on project switch, user cancellation or project shutdown; it must not hold one database read transaction open for the entire scan.

Measure this on 10k/100k/1M-entry fixtures before deciding whether to add an **in-file inverted token index**. Do **not** create a Lucene/Tantivy-style sibling index folder: it would violate the project-file rule.

### 7.3 Memory limits and disk growth

- Temporary mode: retention caps bound logical history and captured payloads, including redb page/cache overhead; expose a distinct **recording stopped / data evicted** status when limits are reached.
- Persistent mode: existing in-memory limits become **UI cache limits**, not disk-data deletion rules. Saved rows remain until the user clears them or an explicitly chosen disk-retention policy prunes them.
- Bound both redb's configured cache and Perseid's own application caches. redb's documented builder default cache is large (1 GiB); explicitly set a measured initial value, e.g. **64 MiB** for disk projects, not the default.
- Disk capacity limits must be checked in batches, with an actionable error if a project fills its filesystem. No uncontrolled automatic deletion of persistent history by default.
- Redb may retain reusable freed pages and the database file may not shrink when entries are removed. `compact()` is an explicit maintenance action, run only with appropriate transaction coordination. Do not advertise “Clear history” as an immediate file-size reduction.
- Compression/deduplication are future optional **in-file** changes. If implemented, store codec identifier, original length, checksum and uncompressed-size limit; avoid decompression bombs.

## 8. Write lifecycle, recovery and correctness

### 8.1 Transaction rules

For each batch:

1. Validate and normalize incoming mutations in bounded memory.
2. Begin one redb write transaction.
3. Update primary records and body tables; remove old secondary-index keys when an indexed field changes; insert new keys.
4. Update schema counters and commit sequence in the **same** transaction.
5. Request `Durability::Immediate` on disk; call `commit()` and check result.
6. Only then report the committed sequence to the UI and advance its durable state.

Do not allow an error halfway through index maintenance to commit an inconsistent partial batch. Use fallible APIs and abort on failure. In-memory projects can use the same transaction semantics; their “committed” status means internally consistent in RAM, **not persisted across process exit**.

### 8.2 Crash matrix

| Failure point | Expected recovery |
| --- | --- |
| Before a proxy event is enqueued | Not captured; cannot be recovered |
| Enqueued but not yet written | May be lost after process termination |
| In the middle of a redb write transaction | On reopen, redb selects a valid committed state; incomplete transaction is not exposed |
| After `Durability::Immediate` commit returns | Transaction is expected durable, subject to real filesystem/device guarantees |
| During project Save As transfer | Destination has `initializing` marker until ready; original RAM-only session cannot survive process crash |
| During Save Copy | Main project unaffected; incomplete destination rejected |
| While response is in-flight | Already committed metadata can reopen marked Interrupted; network activity itself cannot resume |
| Power loss / broken disk / corrupted hardware | Crash consistency helps but is not a backup guarantee; provide backups and actionable integrity diagnostics |

redb has internal commit/recovery machinery in the **single database file**. The application should not attempt to replay redb's internal pages or write its own WAL. See the upstream design reference in §14.

### 8.3 Capture loss and errors

- Record counters for proxy event drops, storage queue drops, decode errors and write errors separately.
- If the existing proxy channel drops a terminal event, its `HistoryItem` can remain in-flight; use the existing `interrupt_in_flight` idea to mark affected records as **capture incomplete** with an error reason.
- If the project is persistent and the storage device fails, surface a **Persistence degraded** state prominently. Do not continue claiming the project is autosaving; keep limited unsaved mutations if safe and expose Retry/Save Copy.
- Logging must not print authorization headers, cookies, full URLs containing secrets, or payload contents.

### 8.4 In-flight recovery

`ProxyEvent::Started` should be persisted as a row with `PersistedPhase::InFlight`. `ResponseHead` can update that row; `Finished` records the final request body/result and transitions it to completed/failed/dropped.

On opening a project, there are no live sockets from its previous session. In a **single durable write transaction**, change all remaining `InFlight` entries to `Interrupted { reason: "application terminated before exchange completed" }` and retain whatever body/head fields actually made it to durable storage. A reasonable optimization is a small `in_flight_ids` index, preventing a full database scan on startup. This **does not** continue downloading bodies.

## 9. Save As handover algorithm (temporary to file)

This is the most important behavior to implement and test deliberately. **Do not** implement Save As by swapping the active `Database` handle after an uncoordinated copy: requests arriving during the copy would disappear from the new file.

### 9.1 Correctness contract

Let `S` be the last ordered project mutation visible to the initial snapshot. A successful Save As must:

- Produce one ready file containing exactly one application of every accepted mutation up to a final handover sequence `H >= S`.
- Install the new file as the active writer **before** acknowledging success.
- Preserve consistent references (exchange IDs, workspace tabs, indexes, notes).
- Never mark an uncommitted destination `ready`.
- Leave the in-memory project active on any transfer error.
- Make overload visible instead of claiming missing capture events were saved.

### 9.2 Recommended implementation: snapshot + ordered delta replay

Implement the transfer as a project-service operation, **not** on the egui thread:

1. **Begin boundary.** The single writer commits pending memory mutations and records `S`. Enable an internal, monotonically sequenced `transfer_mutations` log for all subsequent project mutations. Each committed mutation and its log entry are written in one memory transaction.
2. **Snapshot.** A copy worker opens a short-consistency redb read snapshot of the memory project **at `S`**. Create the destination exclusively as a *new* `.perseid` file and initialize its metadata with `state = initializing`.
3. **Bulk copy.** Copy all project records, payload tables and workspace entries visible at `S` into the new database in bounded batches. Exclude temporary transfer-log records. The writer continues accepting mutations against the original memory project, recording each delta.
4. **Catch up.** Replay `transfer_mutations` for sequences `S+1..T` to the disk database in increasing order; make replay idempotent through `(sequence, entity ID)` checks. Deletes and index changes must be represented explicitly, not guessed from current primary records. The disk database remains `initializing` throughout.
5. **Handover barrier.** Once lag is small, briefly stop *applying* new project commands (the bounded capture adapter may continue accepting them), define final sequence `H`, apply remaining deltas through `H` and validate counts/foreign references.
6. **Publish durable destination.** Commit `state = ready`, the final sequence and all indexes with `Durability::Immediate`. Atomically switch the active project backend/generation to the new redb file in the service; resume queued mutations to the **new** writer. Only now report Save As success.
7. **Cleanup.** Release the memory DB/snapshot and transfer mutation log; refresh reader handles/cache. The destination has no permanent companion files.

**Resource limits:** The open memory snapshot pins pages, and the delta log consumes RAM. Bound transfer memory growth and elapsed time. If the snapshot cannot catch up within its budget or a queue overflows, **abort Save As**, discard/mark the incomplete destination, keep the memory project active, and present an explicit reason. Do not silently discard capture records. The pre-existing proxy channel can still lose events independently; those are reported separately.

**Implementation shortcut for first integration tests:** A deliberately **quiesced** Save As (stop accepting *new capture recording*, drain all commands, copy, then resume with explicit visible recording gap) is simpler and valid as an initial internal milestone, but is **not** the finished seamless Save As UX. It must never silently pause recording or pretend the gap was saved. Build the project service and schema so the delta-based handover can replace this shortcut without changing the file format.

### 9.3 Destination-path and incomplete-file rules

- Reserve the path with exclusive creation (e.g. `OpenOptions::create_new(true)` and redb `Builder::create_file`) to avoid overwriting an existing project. **Do not use `Database::create(path)` as an existence check:** redb may open an existing valid database.
- On a normal failed transfer, close handles and offer removal of the incomplete destination. If the process crashes, a single `initializing` `.perseid` file may remain; opening it must show “Incomplete project creation” and must **not** convert it into a ready project automatically.
- Strict *no sidecars next to project* is maintained during normal operation. Do not create `.perseid-wal`, backup directories or sibling binary-blob files. If future overwrite/rename behavior requires staging, specify the temporary-file policy separately and clearly in the UI.

## 10. Redb implementation specifics

Use official `redb` APIs, target the pinned 4.4.x version and avoid experimental multiprocess features.

### 10.1 Creation/opening example

```rust
use redb::{backends::InMemoryBackend, Database};
use std::path::Path;

fn temporary_db() -> Result<Database, redb::DatabaseError> {
    let mut builder = Database::builder();
    builder.set_cache_size(32 * 1024 * 1024);
    builder.create_with_backend(InMemoryBackend::new())
}

fn open_disk_db(path: &Path) -> Result<Database, redb::DatabaseError> {
    let mut builder = Database::builder();
    builder.set_cache_size(64 * 1024 * 1024);
    builder.open(path) // Must already exist: never implicitly create on Open
}
```

For a **new** disk project, create an exclusive file first and initialize it with `builder.create_file(file)`; see §9.3. Cache sizes are illustrative and must be benchmarked. `Database::builder`, `create_with_backend`, `create_file`, `open`, redb tables and durability are documented in the sources below.

### 10.2 Table setup and write example

```rust
use redb::{Database, Durability, TableDefinition};

const HTTP_EXCHANGES: TableDefinition<u64, &[u8]> =
    TableDefinition::new("http_exchanges");

fn put_http(db: &Database, id: u64, encoded: &[u8]) -> Result<(), redb::Error> {
    let mut tx = db.begin_write()?;
    tx.set_durability(Durability::Immediate)?;
    {
        let mut table = tx.open_table(HTTP_EXCHANGES)?;
        table.insert(id, encoded)?;
    }
    tx.commit()?;
    Ok(())
}
```

This illustrates the basic API only; **real writes must also update indexes, body tables and sequence metadata in the same transaction**. Confirm the exact error-conversion signature against the dependency chosen at implementation time. Keep serialization and byte validation outside the redb table plumbing.

### 10.3 File format / concurrency considerations

- redb can run a single write transaction with simultaneous MVCC readers; the project service should enforce one writer rather than relying on incidental lock contention.
- A file-backed redb DB uses **one physical file** for its normal database and crash-recovery state. This is the primary reason for choosing it over SQLite WAL/rollback-journal mode.
- Default redb builder cache is sizeable; set it intentionally. Collect database size, live cache and process RSS in benchmarks.
- redb project files are **not transparent interchangeable formats** across arbitrary library generations; pin a known version and maintain reopen/backward-compatibility fixtures. Independently migrate Perseid's schema.
- `Database::compact()` and integrity checking are **maintenance operations**, not part of each autosave. Coordinate them with transactions and show UI progress/errors.
- Do not assume an operating system or network filesystem will implement flushes faithfully. Initially document that projects should be placed on local disks rather than unreliable network mounts or concurrently synced folders.

## 11. Migrations and compatibility

### 11.1 Version policy

- Separate `application_id`, `schema_version`, `record_version` and redb's own file-format version.
- Ship schema v1 as soon as persistence lands; do not label v1 a stable cross-version contract unless it has migration and fixtures.
- Newer, unknown Perseid schema: **refuse to write** and show an upgrade message. Older supported schema: migrate under exclusive access.
- Keep a matrix of which application versions can open which schema versions.

### 11.2 Migration protocol

For a small schema change that fits in one transaction: check preconditions, create/transform records and indexes, update `schema_version` **last in the same commit**.

For a large migration: record `state = migrating`, target schema and migration cursor; process deterministic idempotent batches; resume or roll back safely after a crash. Never expose a half-migrated database as `ready`. Strongly recommend asking for a **Save Copy**/backup before destructive migration; a migration must never silently delete user data.

Tests must cover each earlier supported schema fixture, unsupported future schema, absent tables, malformed metadata and interrupted migration.

## 12. Security, privacy and validation

A `.perseid` file will frequently contain **cookies, bearer tokens, credentials, personal information and binary uploads**. It is not encrypted by redb.

- Use restrictive newly-created-file permissions where possible (`0600` on Unix) and document Windows ACL behavior. Respect user-selected permissions without claiming encryption.
- Never store the local TLS CA **private key**, OS trust-store state, API secrets from global app config, or filesystem-local absolute paths that aren't necessary for project reproduction.
- Warn before exporting or sharing a complete project. Add optional redaction/export later, with tests that cookies/Authorization headers are actually removed from every relevant record/body.
- Treat project bytes as **untrusted on open**: set maximum sizes for parsed DTOs, counts and strings, disallow panics from malformed data, constrain decompression, validate key/index relationships and avoid arbitrary path traversal.
- Loading a project must **never execute** script content, auto-start Intruder, automatically replay HTTP requests or change TLS trust.
- A fresh project uses a random project ID. A Save Copy keeps provenance but should gain a new `copy_id` if later required by sync/recent-files behavior; decide explicitly, do not conflate independent clones and a continuously edited original.

## 13. Original implementation plan and acceptance tests

The checklists and acceptance matrix below are preserved from the original
proposal, not maintained as the live completion tracker. See §0.1–§0.3 for the
current implementation and remaining work.

Implement in narrowly scoped PRs; make the smallest vertical slice functional before persisting every tool.

### Phase A — Storage foundation

- [ ] Add pinned `redb`, `serde`, serialization, error and UUID dependencies only as needed.
- [ ] Build `project/` with backend selection, table constants, versioned DTOs, schema init/open, mode/generation and typed errors.
- [ ] Add tests: memory DB, disk DB, invalid file, missing path, cross-restart read/write, future schema, exclusive open and no sidecar files.
- [ ] Benchmark redb cache settings on supported Linux/macOS/Windows targets.

**Done when:** a temporary project can store a test record; a disk project can be reopened in a new process with the same record and **only the `.perseid` file exists**.

### Phase B — HTTP capture and disk-backed history

- [ ] Route `Started`, `ResponseHead`, `Finished` through the project writer with stable ID allocation.
- [ ] Persist capture flags, metadata, bodies and indexes atomically; add in-flight and loss handling.
- [ ] Implement paged HTTP rows and lazy details; replace unbounded full-history iteration in `app.rs`.
- [x] Keep temporary retention limits; separate persistent disk retention from UI RAM cache limits.
- [ ] Add clear-history/delete behavior and index consistency tests.

**Done when:** capture 10k requests, reopen, filter/page/select and inspect bytes with bounded RSS; no misleading reconstructed bytes; duplicate/late/missing event tests pass.

### Phase C — Two-mode UI and project transitions

- [x] Add Untitled Temporary status, New, Open, Save As, Save Copy, and Save/Discard/Cancel prompts for New/Open.
- [ ] Add a recent-project list and a quit prompt.
- [ ] Add project generation IDs, flush receipts, visible write-error and capture-loss indicators.
- [ ] Implement snapshot transfer first in a controlled, explicitly quiesced path, then the ordered delta handover for seamless Save As.
- [ ] Test project switching while the proxy and held interception are active; do not auto-forward held requests.

**Done when:** capture in temporary mode, Save As, keep capturing, abruptly kill the process, reopen and see all **durably committed** records through the handover boundary in one file; failed transfer leaves the memory project intact while the process remains running.

### Phase D — WebSockets and tool workspaces

- [ ] Persist WebSocket messages/connection metadata and lazy-load payloads.
- [ ] Add workspace DTOs and restore flows for Repeater tabs, scope, match/replace, Intruder templates/results, Decoder, Comparer and notes.
- [ ] Add component-specific dirty/debounce updates and explicit flush before shutdown.
- [ ] Test deletion of source history while a Repeater/Comparer snapshot is still saved.

**Done when:** close/reopen preserves supported tool inputs/results and rules, but never starts a network task automatically.

### Phase E — Search, resilience and release hardening

- [ ] Move history search off the frame thread; implement cancellation, pagination, bounded decode and progress.
- [ ] Implement reopen-time in-flight repair, schema migrations, integrity errors, graceful disk-full behavior and interruption diagnostics.
- [ ] Implement Save Copy through a consistent read snapshot; verify the copy opens independently.
- [ ] Add crash injection at every transaction/transfer state and keep persistent format fixtures in the test suite.
- [ ] Document sensitivity of plaintext projects and backup behavior in the user guide.

**Done when:** forced kill / abrupt shutdown tests never expose half-committed multi-table state, copies are independent, searches cannot hang the UI, and release docs clearly state what is actually persisted.

### 13.1 Minimum test matrix

| Scenario | Required assertion |
| --- | --- |
| Fresh launch | Temporary mode; no project file created |
| Temporary project closes | Its data does not reappear after restart |
| Save As once | Exact logical state present in one ready `.perseid` file |
| Save As under traffic | Events accepted through handover boundary neither duplicated nor omitted; any preexisting proxy drop is reported |
| Save As fails halfway | Original temporary session still usable; destination not silently treated as complete |
| Open file with wrong app marker | Clear rejection; file untouched |
| Open corrupt/truncated database | Clean error/recovery attempt; no panic; original project not lost |
| Open newer schema | Refuse write, explain unsupported format |
| Crash during `Started` / `ResponseHead` / `Finished` | Last committed fields valid; in-flight is marked Interrupted on reopen |
| Crash during a write transaction | No torn primary/index update visible |
| Disk full / read-only filesystem | Error visible; no false autosave success |
| Open same file twice | Second write opener rejected |
| Clear HTTP/WS history | Associated payloads/indexes removed without deleting unrelated workspace |
| Delete source of Repeater/Comparer item | Immutable edited snapshot still restores |
| 100k+ saved exchanges | Paged UI stays responsive; memory bounded by configured budgets |
| Captured binary/truncated payload | Bytes and truncation flags round-trip exactly |
| Cross-platform file copy | Copy created on Linux can be read on Windows/macOS (CI fixture) |
| Sidecar audit | No persistent `.wal`, `-shm`, journal or blob files adjacent to project |

### 13.2 Measurements to record

Capture write throughput, p50/p95/p99 transaction/commit time, UI frame-time while saving, time-to-open for 10k/100k/1M entries, peak resident memory, redb cache size, file bytes per exchange, search latency and Save As catch-up time. Use realistic mixes of tiny/large, binary, compressed, streaming and truncated HTTP bodies. Compare against the existing v0.1.0 bounded history baseline to detect regressions.

## 14. References and verification notes

### Perseid v0.1.0 and source

- [Perseid v0.1.0 release](https://github.com/andyintheshell/perseid/releases/tag/v0.1.0) — published 2026-10-23; tag commit `b8e51e9`.
- [Architecture and resource philosophy](https://github.com/andyintheshell/perseid/blob/main/docs/design.md) — current architecture and redb project storage.
- [Proxy events](https://github.com/andyintheshell/perseid/blob/main/crates/perseid/src/proxy/events.rs) — `ProxyEvent`, `CapturedBody`, exchange lifecycle.
- [Proxy channel and runtime](https://github.com/andyintheshell/perseid/blob/main/crates/perseid/src/proxy/mod.rs) — bounded event queue, dropped-event metric, separate interception queue.
- [HTTP history](https://github.com/andyintheshell/perseid/blob/main/crates/perseid/src/history.rs) — `HistoryItem`, `Phase`, memory limits/eviction.
- [WebSocket history](https://github.com/andyintheshell/perseid/blob/main/crates/perseid/src/websocket_history.rs).
- [Workbench app](https://github.com/andyintheshell/perseid/blob/main/crates/perseid/src/app.rs) — current state ownership and capture drain.

The links above point to current `main` (workspace `0.2.0`) for readable source paths. The integration map and schema proposal were written against the pinned `v0.1.0` baseline; use §0 for what has since shipped and verify paths against current source before implementing follow-up work.

### redb

- [redb crate v4.4.0](https://docs.rs/redb/4.4.0/redb/) — transactions, MVCC, file-backed storage.
- [Builder API](https://docs.rs/redb/latest/redb/struct.Builder.html) — `create_file`, `create_with_backend`, `open`, cache size.
- [InMemoryBackend](https://docs.rs/redb/latest/redb/backends/index.html) — temporary in-memory storage.
- [WriteTransaction API](https://docs.rs/redb/latest/redb/struct.WriteTransaction.html) — commit and durability.
- [Durability semantics](https://docs.rs/redb/latest/redb/enum.Durability.html) — `Immediate` vs `None`.
- [redb on-disk format and crash-recovery design](https://github.com/cberner/redb/blob/master/docs/design.md) — copy-on-write B+trees and commit/recovery strategy.

## 15. Original final acceptance definition

This was the target for the first persistence implementation. It is not fully
met: in particular, the current UI does not page or search all saved history.
See §0 for the shipped subset and deviations.

The initial project-storage implementation is ready to release when a user can:

1. Launch Perseid and capture traffic without creating a project file.
2. Choose **Save Project As…**, producing **one** `assessment.perseid` file, and carry on working without manual saves.
3. Reopen the file after a clean close or forced process kill, restoring the **last durably committed** history/workspace state and marking incomplete exchanges appropriately.
4. Navigate and search a large stored history without holding the whole dataset in egui memory.
5. Make a portable **Save Project Copy…** without corrupting the live project.
6. See explicit errors if data could not be written or capture events were dropped.

**Bottom line:** Give users Burp-like temporary and disk project modes, PhotoCraft-like native format ownership, and ZAP-like automatic incremental persistence—but use **redb's one-file transactional storage** rather than SQLite sidecars or a hand-written archive/journal engine.
