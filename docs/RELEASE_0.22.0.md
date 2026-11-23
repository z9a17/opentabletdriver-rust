# OpenTabletDriver Rust 0.22.0

## Comprehensive cross-stack performance overhaul

This release optimizes every layer of the driver — the portable report pipeline,
Windows real-time I/O and scheduling, the .NET managed plugin bridge and IPC,
the Windows desktop panel, and the Linux/macOS runtimes — while preserving
allocation-free native report processing and unchanged-plugin compatibility.

### Core report pipeline (`otd-core`)

- **Compact zero-copy hot paths across modes and vendors:** Added
  `CompactPen` fast decoding for 17 stateless non-IntuosV2 parsers (Generic,
  XP-Pen, Huion, UC-Logic, Veikk, and IntuosV3) plus a single-pass IntuosV2
  decoder using fixed-size chunk slicing. Extended `ReportPipeline` fast paths
  to Relative and Pen modes (`FastMode::{Absolute, Relative, Pen, Full}`),
  eliminated per-report 128-entry managed pen action scans, and gated inactive
  output/binding work.
- **Faster math and zero CRT rounding calls:** Replaced per-report `f64::round`
  / `f32::round` calls on hot paths with branchless inline clamping/rounding,
  precomputed `Mapper` reciprocals and `OtdTransform` clip bounds, and
  accelerated built-in `RadialFollow` with a 64-byte-aligned piecewise
  degree-7 Chebyshev table for `ln(tanh(exp(a)))` (max error `1.15e-10`) plus a
  dead-zone early-out.
- **Bitmask state and zero-overhead idle debug taps:** Switched `OutputOwners`,
  `ButtonOutput`, and `ActionState` to compact `u64`/`u128` bitmasks with
  inline fast paths; gated all debug capture taps behind a single relaxed
  atomic mask (`ACTIVE_MASK`) so idle sessions touch no thread-local state;
  and moved `FullCapture::read` allocations outside the capture mutex.
- **Catalog and startup caching:** Cached the built-in tablet database,
  directory-signature-keyed override databases, and compiled endpoint regexes
  (bounded cache), skipped unconnected VID/PID entries during selection, and
  trimmed `regex` features in `otd-core`.

### Windows real-time I/O, scheduling, and output

- **2-IRP FIFO overlapped ring and user-mode completion checks:** Replaced
  copy-on-complete HID/WinUSB reads with a 2-slot FIFO overlapped ring
  (`ReadSlot`) using `std::mem::swap`, enabled `FILE_SKIP_SET_EVENT_ON_HANDLE`,
  raised the HID input buffer to 128 reports (64 fallback), enabled WinUSB
  `RAW_IO`, and checked `HasOverlappedIoCompleted` with an atomic stop mirror
  to avoid zero-timeout kernel polls when reports are queued.
- **Clean thread isolation and scoped timers:** Explicitly demoted cold daemon
  threads (control loop, device supervisor, upstream RPC listeners, managed
  service lanes, and display/foreground watchers) to
  `THREAD_PRIORITY_BELOW_NORMAL` + thread EcoQoS so they never inherit
  `HIGH_PRIORITY_CLASS` (13) or compete with games or the report thread.
  Switched experimental `driver_cpus` from process-wide affinity to per-thread
  `SetThreadAffinityMask` with live epoch refresh, raised MMCSS Pro Audio
  priority to `AVRT_PRIORITY_HIGH`, and scoped `timeBeginPeriod` strictly to
  active `<50 ms` timer waits with deadline reuse.
- **Event-driven watchers and faster injection:** Replaced 1 Hz display
  polling with a demoted `WM_DISPLAYCHANGE` hidden window (`DISPLAY_EPOCH`),
  filtered device removal notifications to the tablet's own symbolic link so
  unrelated USB/BT events never wake the reader, cached the foreground `HWND`
  via `SetWinEventHook(EVENT_SYSTEM_FOREGROUND)` for Windows Ink, added a
  lock-free asymmetric single-owner output fast path (`FlushProcessWriteBuffers`
  promotion on multi-owner registration), and batched consecutive non-left
  button/key actions into a single `SendInput` call.

### Managed plugin bridge, IPC, and native plugin dispatch

- **Zero-wakeup idle IPC and buffered `CompatPipe`:** Replaced 80 Hz idle
  polling across the 4 `upstream-rpc` listeners with `StopEvent` +
  `INFINITE` waits, reused overlapped Win32 events (`ReusableOp`) with a 4 KiB
  read buffer to eliminate per-byte handle creation on StreamJsonRpc headers,
  and cached `current_sid()`, plugin registry snapshots, and pre-encoded event
  frames.
- **Lazy managed host and event-driven completions:** Pure-native profiles no
  longer serialize the 363-tablet catalog or spawn the 5 managed service worker
  lanes and state observer thread until CLR actually initializes. Split static
  configurations out of dynamic snapshot refreshes, replaced broadcast
  `notify_all()` with per-lane `Condvar`s, and added a native-to-managed
  completion callback (`ServiceClient.OnTicketCompleted`) so async managed
  service calls wake immediately instead of polling every 10 ms.
- **Faster `OtdCompat` report path and native plugin chain:** Replaced
  reflection-based report cloning in `OwnedReportSnapshot` with typed fast
  paths and compiled reference-field cloners, added `Volatile.Read` empty
  checks and batched command drains in `Graph.cs`, added a host pre-prepare
  shortcut skipping a per-report FFI round-trip when no internal nodes or
  managed output exist, and pre-partitioned pure-native `PluginChain` stages
  with direct validated function pointers.

### Windows panel UI and Linux/macOS runtimes

- **Lower idle overhead and smoother UI painting:** Added 5-second tray-hidden
  polling backoff with instant `Wake` on restore and snapshot-fingerprint
  deduplication; reused persistent DIB backbuffers (`Canvas::begin_paint`) with
  `WM_PAINT` culling, fused SDF rounded rects, scanline-merged polylines, and
  cached rotated fonts; filtered `WM_DEVICECHANGE` events; paused minimized
  debugger polling; and batched debugger capture `sync_data()` flushes.
- **Linux, macOS, and `otd-platform` hot paths:** Added packed holds, dirty
  bitsets, and lock-free button/key mirrors in `input_owner`; atomic fast-path
  checks on `OutputWake`; zero-copy single-endpoint `PairedSource` delegation
  and single-pass daemon discovery with cached configuration JSON. On Linux:
  direct non-blocking `read()` + `ppoll` with nanosecond timeouts, USB string
  descriptor caching, removed redundant per-session virtual keyboard creation,
  skipped empty `EV_SYN` writes, and added `setpriority(-10)` fallback when
  `SCHED_FIFO` is unavailable. On macOS: 256-slot 64-byte-aligned callback ring
  with zero-copy handoff, USB string descriptor caching, event-driven
  `CGDisplayRegisterReconfigurationCallback`, and single `CLOCK_UPTIME_RAW`
  timestamps per pointer event.

### Offline benchmark results

Measured on Ryzen 7 5800X3D (Windows 11, synthetic 20,000-report osu! PTH-660
trace, discard sink, 5 rounds, 400 ms warmup; paced replay: 200 Hz, 3 s x 3
rounds):

| Measurement | v0.21.0 | v0.22.0 | Change |
| --- | --- | --- | --- |
| `parse` mean | 33.5 ns | 29.4 ns (p50 30.1 / p99 40.1 ns) | -12% |
| `absolute` mean | 83.7 ns (9.09M/s) | 73.2 ns (p50 70.1 / p99 90.2 ns, 10.25M/s) | -13% |
| `absolute+radial_follow` mean (p50 / p99) | 131.8 ns (130.2 / 150.3–200.4 ns) | 106.3 ns (100.2 / 140.3 ns, 7.66M/s) | -19% mean (-23% p50) |
| `relative` mean | 147.5 ns (5.79M/s) | 73.8 ns (p50 70.1 / p99 110.2 ns, 9.98M/s) | -50% (2.00x) |
| `absolute+native_ema` mean | 202.7 ns (4.39M/s) | 138.4 ns (p50 120.2 / p99 220.4 ns, 6.13M/s) | -32% (1.46x) |
| `session/absolute+radial_follow` mean (p50 / p99 / p99.9) | 194.3 ns (190.4 / 250.5 / 270.5 ns) | 138.1 ns (130.2 / 190.4 / 230.4 ns, 7.01M/s) | -29% mean (-32% p50) |
| `session/non_intuos_v2` vs generic `non_intuos_v2` (new) | 215.1 ns generic | 102.0 ns compact (p50 100.2 / p99 150.3 ns, 9.23M/s) | -53% (2.11x) |
| `pen` / `buttons` (new) | — | 77.0 ns (9.90M/s) / 72.2 ns (10.52M/s) | new |
| Paced 200 Hz `radial_follow` `pipeline_ns` p50 / p99 | 0.99 / 1.32 µs | 0.73 / 1.32 µs (mean 0.80 µs) | -26% p50 |
| Rust heap allocations per report | 0 | 0 | unchanged |

These are offline software benchmarks without a physical tablet, USB transfer,
`SendInput`, or display; physical pen-to-display feel and end-to-end latency
still require hardware measurement.

## Checks and limits

CI (format, Clippy, tests on Windows, Linux and macOS, native plugin DLL round
trip, plus parity evidence validation) passed on the merged commit, and all
four release packages (`win-x64`, `linux-x64`, `macos-x64`, `macos-arm64`) were
built by the Release build workflow and verified with `scripts/release.py verify`
(archive contents, architecture, version, source revision, and SHA-256).
Managed bridge compilation (`OtdCompat` Release) and offline `GraphBench`
contracts passed, along with new unit tests covering compact multi-vendor
decoders, Chebyshev `RadialFollow` accuracy, `OutputOwners` >64 overflow,
Windows priority/timer/device-removal/output-fast-path helpers, display
watchers, action batching, and Unix input-owner/runtime behavior.

No physical tablet, live driver, panel, or game was started for this release.
Physical aim feel, end-to-end pen-to-display latency, hotplug and sleep/wake,
Windows Ink drawing, and native Linux/macOS runtime execution still require
manual testing on real hardware. Managed features require the CPU-matching
.NET 10 runtime; Linux targets glibc 2.31+ and its frontend needs GTK3; native
macOS targets 11.0 while packaged .NET apphosts declare 12.0 and remain
unsigned/not notarized.
