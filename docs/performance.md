# Performance and memory

[Documentation](README.md) · [Design](design.md) ·
[Runtime limits](workbench.md#limits-and-persistence)

Perseid aims to stay comfortable on modest hardware. This page records the
Milestone 0 proof-of-concept measurements: idle memory, a full 2,000-entry
history, and viewing large responses.

![Perseid memory window showing current and peak memory with a rolling graph](images/perseid-memory.png)

> [!NOTE]
> **Historical M0 measurements, not a benchmark of the current UI.** Fonts,
> message viewers, and other features have changed since this run. The image
> above shows the current memory window, not the exact interface measured.
> These figures come from one Linux VM; they are not macOS results, a current
> release threshold, or a guarantee for arbitrary traffic. The M0 workload
> predates persistent projects and does not measure redb writes, project-open
> time, or saved-file disk growth.

## Milestone 0 results

All figures in the results table are process resident set size (RSS) from Linux
`/proc/<pid>/status` (`VmRSS` and `VmHWM`). The status bar separately reports
allocator heap in use in its **Memory** figure. The proxy was running, capture
was enabled, and the per-message capture limit and exchange count were the
defaults: 1 MiB per captured body and 2,000 exchanges in history. The M0 runs
predate the half-RAM history budget and used the former 256 MiB total-history
default.

| Scenario | Current RSS | Peak RSS | Notes |
| --- | ---: | ---: | --- |
| Idle, empty history | 92.1 MiB | 93.7 MiB | 15 seconds after startup |
| 2,000 exchanges | 98.9 MiB | 98.9 MiB | 125 KiB in captured bodies; `End` and scrolling checked |
| 8 MiB text response, Raw | 102.6 MiB | 102.6 MiB | 1 MiB captured; 256 KiB text preview |
| 8 MiB text response, Hex | 102.6 MiB | 102.6 MiB | 64 KiB hex dump, virtualized by line |
| 8 MiB binary response, Raw | 104.7 MiB | 108.6 MiB | 1 MiB captured; M0 used an 8 KiB inline hex preview |
| 8 MiB binary response, Hex | 104.7 MiB | 108.6 MiB | 64 KiB hex dump, virtualized by line |

The 2,000-entry test added about 6.5 MiB over idle. Both large-response tests
stayed under 109 MiB peak even though the wire body was 8 MiB, because capture
stops at 1 MiB and the detail views render bounded previews.

Each HTTP and WebSocket history UI cache has a retained-body budget. The default
is half of physical memory, measured once at startup, with a 256 MiB fallback if
the platform does not report installed memory (`--history-bytes` overrides it).
On the 5.3 GiB benchmark VM this resolves to about 2.65 GiB per cache across
request and response bodies, so a worst case of 2,000 HTTP exchanges with two
full 1 MiB bodies each still cannot retain the full 4 GiB in that cache. The
HTTP and WebSocket caches have separate budgets. The temporary redb project has
its own aggregate retention limits: combined record count across HTTP and
WebSocket messages, and retained HTTP body/WebSocket payload bytes. By default
both mirror the history settings, so the same numbers bound the UI cache and
the temporary store; `--temporary-retention-limit` and
`--temporary-retention-bytes` change them independently, and `0` disables a
dimension. Persistent project files retain records on disk until cleared and
have no automatic total-store retention cap. The M0 workload retained only
125 KiB, so the UI-cache budget did not bind in the measurements above.

This UI-cache budget counts retained request/response bodies, not headers,
formatting caches, editor state, queued work, graphics resources, or the entire
process. One exchange may remain even when it alone exceeds the configured
history byte budget, and the temporary store always keeps the newest record.
The aggregate retention limits count retained body/payload bytes and record
counts; they do not account for headers, redb page/cache overhead, formatting
caches, editor state, queued work, graphics resources, or the entire process.
Treat the history and retention controls as bounded-data limits, not a hard RSS
ceiling.

### Heap in use and resident memory

The status bar's figure is **heap in use**: the bytes currently held by
Perseid's allocator. It is the closest equivalent to the memory figure in
Burp Suite's status bar. The memory window adds the peak value and a rolling
three-minute graph; this page keeps RSS for system-level comparison.

With the 2,000-exchange history from the table above, the status bar read
5.8 MiB heap in use (6.4 MiB peak). At the same moment, `/proc` reported
97.9 MiB resident memory: 28.2 MiB anonymous and 69.6 MiB file-backed. The
resident figure is within run-to-run variance of the earlier 98.9 MiB reading.
Heap in use is much smaller than RSS because RSS also counts file-backed
library pages that are shared with other processes and can be reclaimed.

### Findings

- The request table is virtualized, so a full history adds memory for retained
  bodies and metadata, not for laid-out rows. Jumping to the end and scrolling
  through 2,000 rows remained responsive.
- Capture limits dominate large-body memory. An 8 MiB response costs about the
  same as a 1 MiB response once captured.
- The hex view is line-virtualized. Before that change, laying out a 64 KiB
  dump as one text galley peaked at about 182.8 MiB in the text-response
  scenario; with virtualization and allocator tuning, the same view peaks at
  102.6 MiB.
- Binary bodies used to get a smaller 8 KiB inline hex preview in Raw. Raw now
  uses the bounded text preview (up to 256 KiB) for every body, decodes
  gzip/deflate/Brotli bodies for display, and masks control bytes for
  unsupported encodings and binary data. Decoding stops at the same 256 KiB
  preview limit, so it does not add another full-body copy. The Hex tab still
  exposes a bounded 64 KiB byte-offset dump.
- glibc was creating extra malloc arenas for the UI and proxy threads. Limiting
  the process to two arenas with `mallopt(M_ARENA_MAX, 2)` reduced idle RSS by
  about 8.3 MiB, from 100.4 to 92.1 MiB.
- The About lockup is now decoded only when that window opens, instead of
  keeping a 1200x320 texture ready from startup.

### Where idle RSS goes

Process RSS counts mapped file pages as well as the heap. The idle 92.1 MiB on
the reference VM breaks down roughly as follows:

| Mapping | RSS |
| --- | ---: |
| `libLLVM` (Mesa shader compiler) | 44.6 MiB |
| `libgallium` (Mesa graphics driver) | 12.0 MiB |
| Perseid executable | 7.8 MiB |
| `[heap]` mapping | 11.3 MiB |
| Anonymous mappings (stacks, buffers) | 6.0 MiB |
| Other libraries and mappings | 10.4 MiB |
| **Total** | **92.1 MiB** |

`/proc/self/status` also splits the resident total into anonymous and
file-backed bytes. Idle, roughly 23 MiB is anonymous (allocator heap, thread
stacks, and private buffers) and the remaining 69 MiB is file-backed (the
executable, Mesa, and other mapped libraries). The allocator tracker reports
about 3 MiB in use at idle; anonymous RSS is larger because it also includes
allocator slack, stacks, and buffers.

The two Mesa mappings are not Perseid's data; they are the system graphics
driver. On this VM, the VMware `vmwgfx` driver uses Mesa's LLVM shader
compiler. Forcing the llvmpipe software path raised idle RSS to about 186 MiB,
so it is not a lower-memory fallback here. Other GPUs and driver versions will
report different totals.

Proportional set size (PSS) at the same moment was 86.9 MiB. RSS measures the
whole process, including mapped library code and driver pages, not just the
application's live data; that is why the status bar reports heap in use, while
this page keeps RSS for system-level context. Results will vary with fonts,
graphics drivers, desktop settings, and captured content.

## Test environment

| Item | Value |
| --- | --- |
| System | Kali GNU/Linux Rolling, arm64, VMware virtual machine |
| CPU / memory | 4 vCPU / 5.3 GiB RAM |
| Kernel | Linux 7.1.5 |
| Display | X11, 2880x1800 at 200% UI scale |
| Window | 1280x820 logical points (2560x1640 physical pixels) |
| Toolchain | rustc 1.95.0, release build |
| Perseid limits | 1 MiB capture limit, 2,000 exchange history, 256 MiB history budget (M0) |

## Repeat the workload

These Linux commands let you repeat representative scenarios with your own
checkout. Results from current code need not match the historical table. Record
the commit, toolchain, graphics stack, display scale, limits, and payload sizes
alongside every new result.

Build and start a release binary:

```sh
cargo build --release -p perseid --locked
GDK_SCALE=2 QT_SCALE_FACTOR=2 ./target/release/perseid \
  --listen 127.0.0.1:8080 \
  --ca-dir "$(mktemp -d)" \
  --capture-limit 1048576 \
  --history-limit 2000 \
  --history-bytes 268435456
```

For an external reading, locate the process and read its resident memory:

```sh
PID=$(pgrep -n -x perseid)
grep -E 'VmRSS|VmHWM|RssAnon|RssFile|RssShmem' "/proc/$PID/status"
```

Wait 15 seconds after startup for the idle reading. Generate traffic through
the proxy and wait a few seconds for the UI to apply the events. The in-app
**Memory** indicator opens the **Memory usage** window, which shows the
current and peak heap in use and a rolling graph. The resident figures in this
document came from `/proc`, not the window. The mapping breakdown above was
read from `/proc/<pid>/smaps` and `/proc/<pid>/smaps_rollup`.

For a self-contained synthetic target, prepare a scratch directory in a second
terminal and serve it on loopback:

```sh
fixtures="$(mktemp -d)"
python3 - "$fixtures" <<'PY'
import os
import sys
from pathlib import Path

root = Path(sys.argv[1])
(root / "api.json").write_text('{"ok":true,"message":"synthetic test data"}\n')
(root / "large.txt").write_bytes(b"x" * (8 * 1024 * 1024))
(root / "large.bin").write_bytes(os.urandom(8 * 1024 * 1024))
PY
python3 -m http.server 8090 --bind 127.0.0.1 --directory "$fixtures"
```

Fill history with 2,000 sequential loopback requests from another terminal:

```sh
curl --noproxy "" -sS -x http://127.0.0.1:8080 -o /dev/null \
  'http://127.0.0.1:8090/api.json?i=[1-2000]'
```

Request the large text and binary files, then open each response in Raw and Hex:

```sh
curl --noproxy "" -sS -x http://127.0.0.1:8080 -o /dev/null http://127.0.0.1:8090/large.txt
curl --noproxy "" -sS -x http://127.0.0.1:8080 -o /dev/null http://127.0.0.1:8090/large.bin
```

The example fixtures above are synthetic reproduction inputs, not an assertion
that they are byte-identical to the original M0 workload. Keep Capture on and
request/response holding off while loading history. Stop the local server with
`Ctrl+C` afterward.

No external hosts were contacted for these measurements. See
[Getting started](getting-started.md) for the normal workspace workflow and
[Design](design.md) for the memory-bounding principles behind these limits.
