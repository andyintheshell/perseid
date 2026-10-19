# Performance and memory

Wiremoth aims to stay comfortable on modest hardware. This page records the
Milestone 0 proof-of-concept measurements: idle memory, a full 2,000-entry
history, and viewing large responses.

![Wiremoth memory window showing current and peak RSS, captured-body bytes, and a rolling graph](images/wiremoth-memory.png)

These are development measurements from one Linux VM, not universal
guarantees. Treat them as a sanity check and a reproducible baseline.

## Milestone 0 results

All figures are process resident set size (RSS) from Linux
`/proc/<pid>/status` (`VmRSS` and `VmHWM`). The app's status-bar meter samples
the same source once per second. The proxy was running, capture was enabled,
and the default limits were used: 1 MiB per captured body and 2,000 exchanges
in history.

| Scenario | Current RSS | Peak RSS | Notes |
| --- | ---: | ---: | --- |
| Idle, empty history | 100.4 MiB | 103.6 MiB | 30 seconds after startup |
| 2,000 exchanges | 104.7 MiB | 104.7 MiB | 125 KiB in captured bodies; `End` and scrolling checked |
| 8 MiB text response, Raw | 108.7 MiB | 108.7 MiB | 1 MiB captured; 256 KiB text preview |
| 8 MiB text response, Hex | 108.7 MiB | 108.7 MiB | 64 KiB hex dump, virtualized by line |
| 8 MiB binary response, Raw | 115.2 MiB | 115.2 MiB | 1 MiB captured; 8 KiB inline binary preview |
| 8 MiB binary response, Hex | 115.2 MiB | 115.2 MiB | 64 KiB hex dump, virtualized by line |

The 2,000-entry test added about 4.3 MiB over idle. Both large-response tests
stayed under 116 MiB even though the wire body was 8 MiB, because capture stops
at 1 MiB and the detail views render bounded previews.

### Findings

- The request table is virtualized, so a full history adds memory for retained
  bodies and metadata, not for laid-out rows. Jumping to the end and scrolling
  through 2,000 rows remained responsive.
- Capture limits dominate large-body memory. An 8 MiB response costs about the
  same as a 1 MiB response once captured.
- The hex view is line-virtualized. Before that change, laying out a 64 KiB
  dump as one text galley peaked at about 182.8 MiB in the text-response
  scenario; the same view now peaks at 108.7 MiB.
- Binary bodies get a smaller 8 KiB inline preview in Raw to keep the
  non-virtualized text view bounded. The Hex tab still exposes a 64 KiB dump.

RSS includes shared library pages and the OpenGL driver's CPU-side allocations.
It does not include GPU/driver memory that is not resident in the process.
Results will vary with fonts, drivers, desktop settings, and captured content.

## Test environment

| Item | Value |
| --- | --- |
| System | Kali GNU/Linux Rolling, arm64, VMware virtual machine |
| CPU / memory | 4 vCPU / 5.3 GiB RAM |
| Kernel | Linux 7.1.5 |
| Display | X11, 2880x1800 at 200% UI scale |
| Window | 1280x820 logical points (2560x1640 physical pixels) |
| Toolchain | rustc 1.95.0, release build |
| Wiremoth limits | 1 MiB capture limit, 2,000 exchange history |

## Method

Build and start a release binary:

```sh
cargo build --release -p wiremoth
GDK_SCALE=2 QT_SCALE_FACTOR=2 ./target/release/wiremoth \
  --listen 127.0.0.1:8080 \
  --capture-limit 1048576 \
  --history-limit 2000
```

For an external reading, locate the process and read its resident memory:

```sh
PID=$(pgrep -n -f './target/release/wiremoth')
grep -E 'VmRSS|VmHWM' "/proc/$PID/status"
```

Wait 30 seconds after startup for the idle reading. Generate traffic through
the proxy and wait a few seconds for the UI to apply the events. The in-app
**Memory** window shows the same current and peak values, plus the bytes
retained for captured bodies.

History was filled with 2,000 sequential loopback requests:

```sh
curl -sS -x http://127.0.0.1:8080 -o /dev/null \
  'http://127.0.0.1:8090/api.json?i=[1-2000]'
```

Large-response tests used two 8 MiB files served by a loopback Python server:
a repeated-text file for the Raw text view and random bytes for the binary
preview and Hex view. Each was requested through Wiremoth with the default
capture limit, then opened in the response pane.

No external hosts were contacted for these measurements. See
[Getting started](getting-started.md) for the normal workspace workflow and
[Design](design.md) for the memory-bounding principles behind these limits.
