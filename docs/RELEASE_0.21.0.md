# OpenTabletDriver Rust 0.21.0

## Faster Windows input scheduling and native report path

This release overhauls Windows daemon scheduling, HID read pipelining, and the
native report-processing hot path to eliminate avoidable latency and per-report
overhead:

- **Windows process scheduling parity:** The daemon and foreground driver
  process now raise themselves to `HIGH_PRIORITY_CLASS` and opt the process out
  of Windows 11 EcoQoS and background timer-resolution throttling
  (`PROCESS_POWER_THROTTLING_EXECUTION_SPEED | PROCESS_POWER_THROTTLING_IGNORE_TIMER_RESOLUTION`),
  matching OpenTabletDriver's `DriverDaemon` so a windowless background daemon
  is never power-throttled or timer-coalesced. The report thread keeps
  `THREAD_PRIORITY_TIME_CRITICAL` and thread-level EcoQoS opt-out.
- **Double-buffered overlapped HID read pipelining:** Completed Windows HID and
  WinUSB reads copy into a preallocated delivery buffer and immediately re-arm
  the next overlapped `ReadFile` IRP *before* running the filter pipeline and
  `SendInput`, keeping an active read IRP in `hidclass.sys` while output runs.
  Foreground sessions also flush stale queued HID input after device
  initialization.
- **Per-report syscall and clock-read elimination:**
  - Replaced the unconditional per-report `WaitForSingleObject(output.event, 0)`
    syscall with a Dekker-ordered atomic fast-path flag on `OutputGate`.
  - Eliminated redundant `Instant::now()` (QPC) calls in the Windows wait loop
    and skipped clock reads when no timed scroll or managed binding is active.
- **Zero-copy native absolute pen fast path:** Pure native absolute mouse
  profiles (such as an imported Wacom PTH-660 + built-in Radial Follow +
  left-click tip profile) now execute directly without constructing or copying
  the full ~1 KB multi-capability `ReportValues` envelope on every report, and
  precompute `Mapper` virtual-desktop normalization scales. The general report
  path also avoids copying `ReportValues` when pressure projection is inactive.
  Radial Follow math stays bit-exact with the original C# curve.

### Offline benchmark results

Measured on Ryzen 7 5800X3D (Windows 11, synthetic 20,000-report osu! PTH-660
trace, built-in Radial Follow, discard sink, 3 rounds):

| Measurement | v0.20.0 | v0.21.0 | Change |
| --- | --- | --- | --- |
| `absolute+radial_follow` p50 / p99 / p99.9 | 220.4 / 340.6 / 410.8 ns | 130.2 / 150.3 / 170.3 ns | -41% / -56% / -59% |
| `session/absolute+radial_follow` p50 / p99 / p99.9 | 310.6 / 480.9 / 561.1 ns | 190.4 / 250.5 / 270.5 ns | -39% / -48% / -52% |
| Paced 200 Hz `pipeline_ns` p50 / p99 / max | 1.74 / 2.26 / 28.8 µs | 0.99 / 1.32 / 8.6 µs | -43% / -42% / -70% |
| Rust heap allocations per report | 0 | 0 | unchanged |

These are offline software benchmarks without a physical tablet, USB transfer,
`SendInput`, or display; physical pen-to-display latency still needs hardware
measurement.

## Upstream OpenTabletDriver 0.6.x fixes

Adopted fixes through upstream `0.6.x` commit
`3345dc73a2d607d013a4762e0fa2672118ef132e` (see
[later upstream fixes](parity/UPSTREAM_AFTER_0.6.8.1.md)):

- **Bosto BT-12HD auxiliary buttons and pen bit check** (upstream #5308): added
  8-button auxiliary specs to `Bosto/BT-12HD.json`, decoded `0x04` auxiliary
  packets through `UCLogicAuxReport`, and gated `0x01` pen reports on bit 4 of
  byte 1.
- **Bamboo 8-byte report length check** (upstream #5311): 8-byte Bamboo tablet
  and auxiliary reports now emit a null wheel reading instead of failing as a
  short packet.
- **Linux `hid_digimend_uclogic` driver detection** (upstream #5279): updated
  `packaging/linux/install.sh` status/restore checks alongside `wacom` and
  `hid_uclogic`.

## Code quality and CI hygiene

- Cleaned up Rust 1.100 deprecations (`Atomic*::fetch_update` -> `try_update`)
  and Clippy lints across `otd-core`, `otd-macos`, `examples/bench`, and the
  Windows driver/UI crates.

## Checks and limits

CI (format, Clippy, tests on Windows, Linux and macOS, plus parity evidence
validation) passed on the merged commit, and all four release packages were
built by the Release build workflow and verified with
`scripts/release.py verify` (archive contents, architecture, version, source
revision, and SHA-256). New unit tests cover `ProcessPriority` raising and
restoring priority/power-throttling state, Bosto auxiliary/pen parsing, and
8-byte Bamboo reports.

No physical tablet, live driver, panel, or game was started for this release.
Physical aim feel, end-to-end pen-to-display latency, hotplug and sleep/wake,
Windows Ink drawing, and native Linux/macOS runtime execution still require
manual testing on real hardware. Managed features require the CPU-matching
.NET 10 runtime; Linux targets glibc 2.31+ and its frontend needs GTK3; native
macOS targets 11.0 while packaged .NET apphosts declare 12.0 and remain
unsigned/not notarized.
