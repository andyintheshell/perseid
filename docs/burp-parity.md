# Capability matrix

**Burp Community scoped parity: 82.1%** · Historical estimate: **2026-11-03**, `0.3.1` development build ([#166](https://github.com/andyintheshell/perseid/pull/166)). Not overall product parity or a fresh audit.

[Documentation](README.md) · [Workbench](workbench.md) · [Roadmap](roadmap.md) · [Screenshots](screenshots.md)

## High-level comparison

✅ Available workflow · 🟡 Partial / limited · 🛠 Planned, not shipped · ❌ Not included · ↔ Deliberate difference · 🧩 Add-on / plugin · ? Not verified

| Feature | Perseid | Burp Community | Burp Pro | OWASP ZAP | Caido |
| --- | --- | --- | --- | --- | --- |
| HTTP(S) proxy | ✅ | ✅ | ✅ | ✅ | ✅ |
| Intercept and edit | ✅ | ✅ | ✅ | ✅ | ✅ |
| Message inspection | ✅ | ✅ | ✅ | ✅ | ✅ |
| Scope and rewrite rules | ✅ | ✅ | ✅ | ✅ | ✅ |
| [Target site map](#target-site-map) | 🟡 | ✅ | ✅ | ✅ | ✅ |
| [HTTP history](#http-history) | 🟡 | ✅ | ✅ | ✅ | ✅ |
| Search | ✅ | 🟡 Local search | ✅ | ✅ | ✅ HTTPQL |
| [WebSocket history](#websocket-history) | 🟡 | ✅ | ✅ | 🧩 | ✅ |
| [WebSocket editing and replay](#websocket-editing-and-replay) | ✅ Live connection | ✅ | ✅ | 🧩 | ? Replay |
| [Repeater / manual replay](#repeater) | 🟡 | ✅ | ✅ | 🧩 Requester | ✅ Replay |
| Decoder / transforms | ✅ | ✅ | ✅ | 🧩 | ✅ Convert workflows |
| Comparer / diff | ✅ | ✅ | ✅ | 🧩 | ? |
| [Intruder / fuzzing](#intruder) | ✅ No processing | 🟡 Throttled | ✅ | 🧩 | ✅ Automate |
| [Sequencer / token analysis](#sequencer) | 🟡 | ✅ | ✅ | 🧩 | ? |
| Memory display | ✅ Heap only | ✅ | ✅ | ✅ | ? |
| [Project persistence](#project-persistence) | 🟡 | ❌ Project files | ✅ | ✅ Sessions | ✅ Plan limits |
| [Collaborator / OAST](#collaborator--oast) | 🛠 P0 | ❌ | ✅ | 🧩 | 🧩 |
| [Crawling and discovery](#crawling-and-discovery) | 🛠 | ❌ Automated crawler | ✅ | ✅ | 🧩 |
| [Scanning and findings](#scanning-and-findings) | 🛠 | ❌ Scanner | ✅ | ✅ | 🧩 Scanner; ✅ Findings |
| [Session / auth automation](#session--auth-automation) | 🟡 | ✅ Rules / macros | ✅ | ✅ | 🧩 / workflows |
| [Evidence export and reports](#evidence-export-and-reports) | 🟡 | 🟡 No scan reports | ✅ | 🧩 | 🧩 / exports |
| [Browser integration](#browser-integration) | ↔ External browser | ✅ Bundled | ✅ Bundled | ✅ Launch external | ✅ Browser setup |
| [Extensions](#extensions) | ↔ No BApps | ✅ BApps¹ | ✅ BApps | ✅ ZAP add-ons | ✅ Caido plugins |

✅ means a usable workflow, **not every competitor option**. Detail tables cover Perseid gaps and qualified comparisons. Competitor availability is documentation-based, not hands-on validation; 🧩 does not imply equivalent scope or quality. Caido includes its plugin ecosystem; Basic has project/workflow/plugin limits. ¹ Some BApps require Pro.

## Target site map

| Workflow | Perseid | Burp Community | Burp Pro | OWASP ZAP | Caido |
| --- | --- | --- | --- | --- | --- |
| Browse captured endpoints | ✅ Retained history | ✅ | ✅ | ✅ Sites tree | ✅ Sitemap |
| Filter / inspect endpoints | ✅ Scope, search, sorting | ✅ | ✅ | ✅ | ✅ |
| Large-tree navigation | 🟡 500-row pages; no expand-all | ✅ | ✅ | ✅ | ? |
| Discover uncaptured URLs | ❌ No inference or parameter discovery | 🟡 Manual mapping | ✅ Crawl / discovery | ✅ Spiders / add-ons | 🧩 |

## HTTP history

| Workflow | Perseid | Burp Community | Burp Pro | OWASP ZAP | Caido |
| --- | --- | --- | --- | --- | --- |
| Browse / filter captures | ✅ All retained rows; lazy details | ✅ | ✅ | ✅ | ✅ HTTPQL |
| Search captured bodies | ✅ Background search | 🟡 Local search | ✅ Project search | ✅ Search | ✅ HTTPQL |
| Delete a capture | 🟡 Finished exchanges only | ✅ | ✅ | ✅ | ? |
| Full Burp filtering / table options | 🟡 Not full equivalence | ✅ Baseline | ✅ | ↔ Different UI | ↔ Different UI |

## WebSocket history

| Workflow | Perseid | Burp Community | Burp Pro | OWASP ZAP | Caido |
| --- | --- | --- | --- | --- | --- |
| Capture text / binary traffic | ✅ `ws://` and decrypted `wss://` | ✅ | ✅ | 🧩 WebSockets | ✅ |
| Browse retained messages | ✅ Virtual table; lazy payloads | ✅ | ✅ | 🧩 | ✅ |
| Full Burp history options | 🟡 Not full equivalence | ✅ Baseline | ✅ | ↔ Add-on UI | ↔ Different UI |

## WebSocket editing and replay

| Workflow | Perseid | Burp Community | Burp Pro | OWASP ZAP | Caido |
| --- | --- | --- | --- | --- | --- |
| Intercept / edit messages | ✅ Both directions; text / binary | ✅ | ✅ | 🧩 | ✅ Interception |
| Replay on live connection | ✅ Repeated sends | ✅ | ✅ | 🧩 | ? |
| Reopen closed connection | ❌ | ✅ | ✅ | ? | ? |
| WebSocket fuzzing | ❌ | ? | ? | 🧩 | ? |

## Repeater

| Workflow | Perseid | Burp Community | Burp Pro | OWASP ZAP | Caido |
| --- | --- | --- | --- | --- | --- |
| Edit / resend HTTP(S) | ✅ | ✅ Repeater | ✅ Repeater | 🧩 Requester | ✅ Replay |
| Send / protocol / history options | 🟡 Not full Burp equivalence | ✅ Baseline | ✅ | ↔ Different options | ↔ Different options |
| Persist HTTP tabs | ✅ Saved project | ❌ Project files | ✅ | 🧩 Session-dependent | ✅ Projects |
| WebSocket tabs | 🟡 Session-only; live connection | ✅ | ✅ | 🧩 Separate UI | ? Replay |
| Proxy rules on direct sends | ↔ Bypasses scope, intercept, rewrites | ↔ Tool-specific | ↔ Tool-specific | ↔ Tool-specific | ↔ Tool-specific |

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
| Load / collect tokens | ✅ Manual / HTTP-history regex | ✅ | ✅ | 🧩 Token Generation and Analysis | ? |
| Parameter-aware capture | ❌ Expression-based only | ✅ | ✅ | ? | ? |
| Randomness analysis | 🟡 Bounded statistical approximations | ✅ Sequencer tests | ✅ Sequencer tests | 🧩 Different analysis | ? |
| Entropy visualization | ✅ Effective-entropy chart | ✅ | ✅ | 🧩 | ? |

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
| HAR export | ❌ | ? | ? | 🧩 Import/Export | ✅ Export |
| Findings reports | 🛠 No report export | ❌ Scan reports | ✅ | 🧩 Report Generation | ↔ External reporting tools |

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
