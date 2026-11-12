# Continuous site-map browsing

Target presents an expandable URL tree beside a continuous, sortable Contents
table and request/response details. There are no Contents or tree paging buttons.
Scroll either pane normally; use Home/End and arrow keys in Contents to navigate
across background-loading boundaries. Selecting a host or folder includes its
descendants, while selecting a resource includes its method/query variants.

Search, scope, filters, and sorting apply to **all retained endpoints**, not just
the rows currently loaded. Project retention can remove exchanges independently
of the display. The earlier live-history-cache bug was fixed under
[#151](https://github.com/andyintheshell/perseid/issues/151).

## Bounded-memory implementation

- An authoritative project scan builds a disposable endpoint-summary index off
  the UI thread. Message bodies are not stored in the derived index.
- Filter, sort, or branch selection changes build a disposable
  disk-backed view with ordinal-addressable Contents and visible-tree rows.
  Deterministic sort keys match the existing column comparators.
- Each pane renders only its visible rows, with automatic background reads of
  at most 500 summaries/nodes per window. This is an internal working-set bound,
  **not a row limit or user-facing page size**. Scrollbar jumps perform range
  reads, not full index scans.
- Database caches and write batches are bounded. Old exchange details are
  hydrated individually. Temporary directories are owner-only on Unix and
  removed when their indexes/views are dropped.
- Selection survives window replacement and sorting. Query changes and project
  switches reject obsolete worker results. Pause freezes the capture snapshot;
  filters, sorting, and expansion remain usable.

Expansion changes rebuild only the visible-tree ordinal index in the background,
sharing the existing ordered Contents and node indexes. Filter, sort, and branch
selection changes rebuild the presentation view.

## Public Burp comparison

[PortSwigger's documentation](https://portswigger.net/burp/documentation/desktop/tools/target/site-map)
describes an expandable URL tree and a sortable Contents table containing the
selected branches and descendants; it does not document a 500-row cap. Perseid
uses that browsing model, existing tree icons, Contents columns, and the shared
request/response layout. This is not a claim of full Target feature parity:
multiple tree selection, discovered-but-unrequested content, and Scanner issues
are separate capabilities. No Burp internals were inspected.

## Validation

Automated tests cover scrolling both panes beyond 500 rows, keyboard navigation
past 500 and 1,000, direct ordinal jumps, bounded windows, branch collapse and
expansion, global search/sorting, selection preservation, stale queries, project
switches, pause/resume, retention, reopened projects, and old details/tool sends.
The opt-in `large_synthetic_project` test exercises 25,003 retained endpoints
after reopening with a ten-record live-history cache and prints view/window
timings.
