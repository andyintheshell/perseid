# Site-map pagination and Burp parity

Target's disk-backed site map uses **500 summaries per Contents page** and
**500 child nodes per tree page**. These are working-set bounds, not capture or
index limits. Select a host, then use **Next contents** / **Previous contents**
or **More nodes** / **First nodes**. Search, filters, and sorting run over the
retained endpoint index before pagination, not just the displayed page.
Project retention can remove exchanges independently of pagination.

The earlier bug where Target depended on the bounded live-history cache was
tracked by [#151](https://github.com/andyintheshell/perseid/issues/151) and is
closed. Tests cover 1,003 endpoints across three pages, backward navigation,
tree paging, reopened projects, and lazy loading of old exchange details.

## Public Burp behavior

[PortSwigger's site-map documentation](https://portswigger.net/burp/documentation/desktop/tools/target/site-map)
describes an expandable URL tree and a sortable Contents table containing the
selected branches and their descendants. It does not document a 500-row cap.
This is a public workflow comparison, not a claim about Burp's internal storage,
rendering, or memory management.

## Remaining parity work

Explicit page buttons are not equivalent to browsing one continuous tree and
table. A parity implementation should:

- Present a virtualized Contents list with automatic background window loading,
  a total matching-row count, and keyboard navigation across window boundaries.
- Support expandable tree branches with lazy child loading instead of replacing
  the tree with each selected level or page.
- Keep decoded summaries and hydrated message bodies bounded; do not replace
  `PAGE` with an unbounded collection or simply increase the limit.
- Preserve global filtering and deterministic sorting, selection, and scroll
  position across loads; reject stale results after query or project changes.
- Avoid a full index scan for every scroll-window request (the current cursor
  query scans the endpoint index); benchmark large retained projects.
- Test boundaries beyond 500 and 1,000 rows, tied sort keys, reverse navigation,
  filter changes, retention, project switches, and access to old details/tools.

The pagination hints improve discoverability only; they do not implement this
continuous-browsing parity work.
