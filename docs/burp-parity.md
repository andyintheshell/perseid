# Capability matrix

**Burp Community scoped parity: 82.1%** · Historical estimate: **2026-11-03**, `0.3.1` development build ([#166](https://github.com/andyintheshell/perseid/pull/166)). Not overall product parity or a fresh audit.

[Documentation](README.md) · [Workbench](workbench.md) · [Roadmap](roadmap.md) · [Screenshots](screenshots.md)

## High-level comparison

✅ Available workflow · 🟡 Partial / limited · 🛠 Planned, not shipped · ❌ Not included · ↔ Perseid deliberate difference · 🧩 Add-on / plugin · ? Not verified

| Feature | Perseid | Burp Community | Burp Pro | OWASP ZAP | Caido |
| --- | --- | --- | --- | --- | --- |
| HTTP(S) proxy | ✅ | ✅ | ✅ | ✅ | ✅ |
| Intercept and edit | ✅ | ✅ | ✅ | ✅ | ✅ |
| Message inspection | ✅ | ✅ | ✅ | ✅ | ✅ |
| Scope and rewrite rules | ✅ | ✅ | ✅ | ✅ | ✅ |
| [Target site map](#target-site-map) | ✅ Captured endpoints | ✅ | ✅ | ✅ | ✅ |
| [HTTP history](#http-history) | 🟡 | ✅ | ✅ | ✅ | ✅ |
| Search | ✅ | 🟡 Local search | ✅ | ✅ | ✅ HTTPQL |
| [WebSocket history](#websocket-history) | 🟡 | ✅ | ✅ | 🧩 | ✅ |
| [WebSocket editing and replay](#websocket-editing-and-replay) | ✅ Live connection | ✅ | ✅ | 🧩 | ✅ Replay |
| [Repeater / manual replay](#repeater) | 🟡 | ✅ | ✅ | 🧩 Requester | ✅ Replay |
| Decoder / transforms | ✅ | ✅ | ✅ | 🧩 | ✅ Convert workflows |
| Comparer / diff | ✅ | ✅ | ✅ | 🧩 | 🧩 Compare |
| [Intruder / fuzzing](#intruder) | ✅ No processing | 🟡 Throttled | ✅ | 🧩 | ✅ Automate |
| [Sequencer / token analysis](#sequencer) | 🟡 | ✅ | ✅ | 🧩 | 🧩 Sequencer |
| Memory display | ✅ Heap only | ✅ | ✅ | ✅ | ? UI undocumented |
| [Project persistence](#project-persistence) | 🟡 | ❌ Project files | ✅ | ✅ Sessions | ✅ Plan limits |
| [Collaborator / OAST](#collaborator--oast) | 🛠 P0 | ❌ | ✅ | 🧩 | 🧩 |
| [Crawling and discovery](#crawling-and-discovery) | 🛠 | ❌ Automated crawler | ✅ | ✅ | 🧩 |
| [Scanning and findings](#scanning-and-findings) | 🛠 | ❌ Scanner | ✅ | ✅ | 🧩 Scanner; ✅ Findings |
| [Session / auth automation](#session--auth-automation) | 🟡 | ✅ Rules / macros | ✅ | ✅ | 🧩 / workflows |
| [Evidence export and reports](#evidence-export-and-reports) | 🟡 | 🟡 No scan reports | ✅ | 🧩 | 🟡 JSON / CSV; external reports |
| [Browser integration](#browser-integration) | ↔ External browser | ✅ Bundled | ✅ Bundled | ✅ Launch external | ✅ Browser setup |
| [Extensions](#extensions) | ↔ No BApps | ✅ BApps¹ | ✅ BApps | ✅ ZAP add-ons | ✅ Caido plugins |

✅ means a usable workflow, **not every competitor option**. Detail tables cover Perseid gaps and qualified comparisons. Competitor availability is documentation-based, not hands-on validation; 🧩 does not imply equivalent scope or quality. Caido includes its plugin ecosystem; Basic has project/workflow/plugin limits. ¹ Some BApps require Pro.

## Target site map

| Workflow | Perseid | Burp Community | Burp Pro | OWASP ZAP | Caido |
| --- | --- | --- | --- | --- | --- |
| Browse captured endpoints | ✅ All retained endpoints; continuous Contents | ✅ | ✅ | ✅ Sites tree | ✅ Sitemap |
| Filter / inspect endpoints | ✅ Scope, search, sorting | ✅ | ✅ | ✅ | ✅ |
| Tree navigation / expansion | ✅ Expandable URL tree; continuous scrolling | ✅ Branch navigation | ✅ Branch navigation | ✅ Sites tree | ✅ Shift-click expands subtree [C1] |
| Discover uncaptured URLs | ❌ No inference or parameter discovery | 🟡 Manual mapping | ✅ Crawl / discovery | ✅ Spiders / add-ons | 🧩 |

Contents and tree windows load automatically in the background; 500 is an internal
window bound, not a page or endpoint limit. Multiple tree selection and expand-all
remain unsupported. See [continuous site-map browsing](site-map-pagination.md)
for implementation and validation details. The historical parity percentage above
has not been recalculated.

## HTTP history

| Workflow | Perseid | Burp Community | Burp Pro | OWASP ZAP | Caido |
| --- | --- | --- | --- | --- | --- |
| Browse / filter captures | ✅ All retained rows; lazy details | ✅ | ✅ | ✅ | ✅ HTTPQL |
| Search captured bodies | ✅ Background search | 🟡 Local search | ✅ Project search | ✅ Search | ✅ HTTPQL |
| Delete a capture | 🟡 Finished exchanges only | ✅ | ✅ | ✅ | ❌ No supported UI / CLI [C2] |
| Full Burp filtering / table options | 🟡 Not full equivalence | ✅ Baseline | ✅ | Different UI | Different UI |

## WebSocket history

| Workflow | Perseid | Burp Community | Burp Pro | OWASP ZAP | Caido |
| --- | --- | --- | --- | --- | --- |
| Capture text / binary traffic | ✅ `ws://` and decrypted `wss://` | ✅ | ✅ | 🧩 WebSockets | ✅ |
| Browse retained messages | ✅ Virtual table; lazy payloads | ✅ | ✅ | 🧩 | ✅ |
| Full Burp history options | 🟡 Not full equivalence | ✅ Baseline | ✅ | Add-on UI | Different UI |

## WebSocket editing and replay

| Workflow | Perseid | Burp Community | Burp Pro | OWASP ZAP | Caido |
| --- | --- | --- | --- | --- | --- |
| Intercept / edit messages | ✅ Both directions; text / binary | ✅ | ✅ | 🧩 | ✅ Interception |
| Send messages on live connection | ✅ Repeated sends | ✅ | ✅ | 🧩 | ✅ Replay session [C3] |
| Reconnect / new handshake | ❌ | ✅ | ✅ | 🧩 Reopen [Z1] | ✅ New session / URL [C3] |
| WebSocket fuzzing | ❌ | 🧩 WS Turbo Intruder [B1] | 🧩 WS Turbo Intruder [B1] | 🧩 | ? Automated fuzzing undocumented |

## Repeater

| Workflow | Perseid | Burp Community | Burp Pro | OWASP ZAP | Caido |
| --- | --- | --- | --- | --- | --- |
| Edit / resend HTTP(S) | ✅ | ✅ Repeater | ✅ Repeater | 🧩 Requester | ✅ Replay |
| Send / protocol / history options | 🟡 Not full Burp equivalence | ✅ Baseline | ✅ | Different options | Different options |
| Persist HTTP tabs | ✅ Saved project | ❌ Project files | ✅ | 🧩 Session-dependent | ✅ Projects |
| WebSocket tabs | 🟡 Session-only; live connection | ✅ | ✅ | 🧩 Separate UI | ✅ Replay; free tier: 1 session [C3] |
| Proxy rules on direct sends | ↔ Bypasses scope, intercept, rewrites | Tool-specific | Tool-specific | Tool-specific | Tool-specific |

## Intruder

| Workflow | Perseid | Burp Community | Burp Pro | OWASP ZAP | Caido |
| --- | --- | --- | --- | --- | --- |
| Payload attacks | ✅ Four Burp-style attack modes | 🟡 Rate-limited | ✅ | 🧩 Fuzzer | ✅ Automate |
| Payload input | ✅ Editable / file-loaded | ✅ | ✅ | 🧩 | ✅ |
| Payload processing | ❌ | ✅ | ✅ | 🧩 Processors | ✅ Workflows |
| Concurrency | ✅ Up to 32; scope checks | 🟡 Throttled | ✅ Resource pools | 🧩 Configurable | ✅ Configurable |

## Sequencer

| Workflow | Perseid | Burp Community | Burp Pro | OWASP ZAP | Caido |
| --- | --- | --- | --- | --- | --- |
| Load / collect tokens | ✅ Manual / HTTP-history regex | ✅ | ✅ | 🧩 Token Generation and Analysis [Z2] | 🧩 Replay / manual / live [C4] |
| Structured token selection | ❌ Expression-based only | ✅ | ✅ | 🧩 Token selector [Z2] | 🧩 Cookie / header / JSONPath [C4] |
| Randomness analysis | 🟡 Bounded statistical approximations | ✅ Sequencer tests | ✅ Sequencer tests | 🧩 Different analysis | 🧩 10 tests [C4] |
| Entropy output | ✅ Effective-entropy chart | ✅ | ✅ | 🧩 Analysis dialog | 🧩 Shannon entropy results [C4] |

## Project persistence

| Workflow | Perseid | Burp Community | Burp Pro | OWASP ZAP | Caido |
| --- | --- | --- | --- | --- | --- |
| Save / reopen work | ✅ Local `.perseid`; automatic writes | ❌ Project files | ✅ | ✅ Sessions | ✅ Projects |
| Tool-state coverage | 🟡 HTTP tabs / tool state; WS tabs excluded | — | ✅ | 🟡 Add-on-dependent | ✅ Project context |
| Retention | 🟡 Opt-in count / body limits; no disk-size cap | Session-only | ✅ Disk projects | ✅ Disk sessions | 🟡 Basic: 2 projects |
| Delete project | 🟡 Close, then delete file externally | — | ✅ File management | ✅ Session management | ✅ Project management |
| Sensitive data | ⚠️ Unencrypted; no CA material or transient activity | ⚠️ Captures may contain secrets | ⚠️ Captures may contain secrets | ⚠️ Captures may contain secrets | ⚠️ Captures may contain secrets |

## Collaborator / OAST

| Workflow | Perseid | Burp Community | Burp Pro | OWASP ZAP | Caido |
| --- | --- | --- | --- | --- | --- |
| Integrated payloads / callbacks | 🛠 Not shipped | ❌ | ✅ Collaborator | 🧩 OAST Support | 🧩 OmniOAST |
| Direction / provider | P0: Interactsh-first [#191](https://github.com/andyintheshell/perseid/issues/191) | External tools | Public / private Collaborator | Interactsh / BOAST / callbacks | Plugin-dependent |

[Perseid adoption gates](collaborator.md)

## Crawling and discovery

| Workflow | Perseid | Burp Community | Burp Pro | OWASP ZAP | Caido |
| --- | --- | --- | --- | --- | --- |
| Automated crawling | 🛠 Longer-term | ❌ | ✅ | ✅ Traditional / AJAX spiders | 🧩 Crawler |
| Hidden-content discovery | ❌ | ❌ Built-in automated discovery | ✅ | 🧩 Forced Browse / Parameter Digger | 🧩 |

## Scanning and findings

| Workflow | Perseid | Burp Community | Burp Pro | OWASP ZAP | Caido |
| --- | --- | --- | --- | --- | --- |
| Passive / active scanning | 🛠 Longer-term | ❌ Built-in scanner | ✅ | ✅ / add-on rules | 🧩 |
| Findings triage | 🛠 | ❌ Scanner findings | ✅ Issues | ✅ Alerts | ✅ Findings |

## Session / auth automation

| Workflow | Perseid | Burp Community | Burp Pro | OWASP ZAP | Caido |
| --- | --- | --- | --- | --- | --- |
| Cookie handling | 🟡 Optional in-memory jar | ✅ | ✅ | ✅ Sessions | ✅ / workflows |
| Login macros / re-authentication | ❌ | ✅ Session rules / macros | ✅ Plus scan logins | ✅ Authentication / scripts | 🧩 / workflows |

## Evidence export and reports

| Workflow | Perseid | Burp Community | Burp Pro | OWASP ZAP | Caido |
| --- | --- | --- | --- | --- | --- |
| Copy request as cURL | ✅ Complete captures | ✅ | ✅ | ✅ | ✅ |
| HAR export | ❌ | 🧩 Logger++ [B2] | 🧩 Logger++ [B2] | 🧩 Import/Export [Z3] | ❌ Documented formats: JSON / CSV [C5] |
| Findings reports | 🛠 No report export | ❌ Scan reports | ✅ | 🧩 Report Generation | External reporting tools |

## Browser integration

| Workflow | Perseid | Burp Community | Burp Pro | OWASP ZAP | Caido |
| --- | --- | --- | --- | --- | --- |
| Browser choice | ↔ Bring your own | ✅ Bundled Chromium | ✅ Bundled Chromium | ✅ Launch installed browser | ✅ Browser setup |
| Perseid trust policy | Manual proxy / CA setup; never installs trust automatically | — | — | — | — |

## Extensions

| Workflow | Perseid | Burp Community | Burp Pro | OWASP ZAP | Caido |
| --- | --- | --- | --- | --- | --- |
| Native ecosystem | 🛠 Possible after core parity | ✅ BApps / API | ✅ BApps / API | ✅ Add-ons / scripts | ✅ Plugins / workflows |
| Burp BApp compatibility | ↔ Non-goal | ✅ Edition-dependent | ✅ | ❌ Own ecosystem | ❌ Own ecosystem |

## Parity accounting

| Rule | Value |
| --- | --- |
| Baseline | 14 equally weighted CE manual-workflow rows; not the entire comparison above |
| ✅ Working: 9 × 1 | Proxy; intercept; inspection; scope / rewrites; WS editing / replay; Decoder; Comparer; Intruder; memory display |
| 🟡 Partial: 5 × 0.5 | Site map; HTTP history; WS history; Repeater; Sequencer |
| Calculation | **(9 + 5 × 0.5) / 14 × 100 = 82.1%** |
| Excluded | Search; project files; browser; extensions; crawling; scanning; OAST; auth automation; reports |
| Interpretation | Equal weights and half credit are judgment calls, not implementation percentages |
| Last status change | Decoder promoted after automated regression tests and general native macOS smoke confirmation, 2026-11-03; not an itemized manual audit |
| Promotion bar | Usable workflow, practical automated tests, manual check, documented safety behavior |
| Next audit | Versioned [workflow audit #68](https://github.com/andyintheshell/perseid/issues/68); recalculate when scoped statuses change |

## Sources and operating limits

Verification: public documentation and plugin READMEs, **not hands-on testing**. Remaining `?` cells are unresolved after checking Caido's tool, Replay, Automate, UI, and logging docs; they do not mean unsupported. Caido's new-session workflow does not establish reuse of a captured proxy connection. Token-analysis results do not imply identical statistics or charts.

| Evidence | Confirmed comparison |
| --- | --- |
| [C1: Caido sitemap navigation](https://docs.caido.io/app/guides/sitemap_viewing) | Shift-click fully expands a sitemap element; no large-project performance claim |
| [C2: Caido data deletion](https://docs.caido.io/app/guides/data_deleting) | No supported capture-deletion UI / CLI; manual SQLite edits are explicitly discouraged |
| [C3: Caido WebSocket Replay](https://docs.caido.io/app/guides/replay_websocket) · [0.57 release](https://caido.io/blog/2026-06-05-release-v0-57-0/) | New WS session, URL, handshake, message sends; release notes limit free tier to one WS session |
| [C4: Caido tool mapping](https://docs.caido.io/burp-suite/core/tools) · [Compare README](https://github.com/caido-community/Compare#readme) · [Sequencer README](https://github.com/caido-community/sequencer#readme) | Plugin availability, collection / extraction methods, statistical tests and entropy results |
| [C5: Caido export formats](https://docs.caido.io/app/guides/exports_requests) | JSON / CSV; exporting current rows requires Individual / Team; generic JSON is not HAR |
| [B1: WebSocket Turbo Intruder](https://portswigger.net/bappstore/ba292c5982ea426c95c9d7325d9a1066) | WebSocket fuzzing extension listed for Professional and Community |
| [B2: Logger++ listing](https://portswigger.net/bappstore/470b7057b86f41c396a97903377f3d81) · [Maintainer HAR fix](https://github.com/nccgroup/LoggerPlusPlus/issues/138) | Both editions supported; HAR export evidenced by maintainer-confirmed fix, not native Burp export |
| [Z1: ZAP WebSocket history](https://www.zaproxy.org/docs/desktop/addons/websockets/about/) | Add-on v18 added reopening connections to resend messages |
| [Z2: ZAP token generation](https://www.zaproxy.org/docs/desktop/addons/token-generator/) | Select a token, generate samples, inspect the analysis dialog |
| [Z3: ZAP Import/Export](https://www.zaproxy.org/docs/desktop/addons/import-export/) | Save selected messages as HAR; HAR export API |
| [Unresolved: Caido UI](https://docs.caido.io/app/guides/ui) · [Logs](https://docs.caido.io/app/guides/logs_viewing) · [Automate](https://docs.caido.io/app/quickstart/automate) | No confirmation of application memory display or automated WebSocket fuzzing |

| Reference | Purpose |
| --- | --- |
| [Burp editions](https://portswigger.net/burp/communitydownload) · [Tools](https://portswigger.net/burp/documentation/desktop/tools) | Public feature / edition comparison |
| [ZAP features](https://www.zaproxy.org/docs/desktop/start/features/) · [Add-ons](https://www.zaproxy.org/docs/desktop/addons/) | Core and add-on workflows |
| [Caido features](https://caido.io/features/) · [Docs](https://docs.caido.io/quickstart/) · [Plans](https://caido.io/pricing/) | Core tools, ecosystem and plan limits |
| [Caido plugins](https://caido.io/plugins/) · [Reporting](https://docs.caido.io/burp-suite/core/reporting) | Plugin availability; no native formatted report generation |
| Comparison checked | 2026-11-11; ? = unverified, not a claim of absence |
| [Workbench](workbench.md) | Shipped behavior, paging, body / operation / UI-cache limits; ✅ does not remove these limits |
| [HTTPS](tls.md) · [Safety and privacy](safety.md) | CA trust, unencrypted evidence, retention; test only authorized systems |
| [Roadmap](roadmap.md) · [Priority backlog #73](https://github.com/andyintheshell/perseid/issues/73) | Planned work, not shipped capability |
