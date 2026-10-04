# Offline input audit, 4 October 2026

The report path is already fast enough that its usual processing time does not explain a perceptible slowdown when streaming. This audit found a small session optimization and a gap in our measurements: paced replay covered only the native filter, although the saved driver profile uses the unchanged .NET RadialFollow DLL. The live osu!lazer/OBS slowdown remains unverified.

The session optimization in 0.16.8 shares one monotonic-clock reading between report timing and the display-refresh check. No filter math, report ordering, bindings, saved settings or output ownership changed. Native full-session processing fell from a median 270.5 ns to 250.5 ns per report, about 7%. That saves 0.02 µs per report; it is not a perceptible-latency claim.

## Scope and method

Windows 11 Pro build 26300, Ryzen 7 5800X3D, 16 logical CPUs, FSOS AMD Gaming power plan. The baseline is 0.16.7 at `ca2c5d9c7baf0fd6cb5af136ed97a54c5e1ae166`. Upstream is the project's pinned OpenTabletDriver 0.6.7 at [`736003e`](https://github.com/OpenTabletDriver/OpenTabletDriver/tree/736003ed72c8bbb28033b039d5a0bb76c344145c), not the current upstream release.

Both harnesses used the same 20,000-report synthetic PTH-660 trace and an 85 × 47.8125 mm area. The chosen replay rate was 500 Hz; the physical tablet's current rate was not measured. The unchanged tablet-space RadialFollow DLL has SHA256 `830d61f29c07b109398f1e001df3c7fbd359b1ad2491a6d12ccc243dacf16383` and uses the saved profile's 0.7039 / 0.302 mm radii, smoothing 0.302, soft knee 0.603 and leak 0.201.

There were three alternating Rust/upstream baseline runs, three native/managed replay runs at each CPU load level, and three alternating before/after session runs. Baseline cases used three timed rounds and 250 ms warm-up; the clock comparison used five rounds and 500 ms warm-up, plus the same tiered-compilation pause as pipeline cases. Numbers below are medians of each run's median. See the [raw results and provenance](perf/2026-10-04/summary.md).

The agent opened no driver, game, OBS window or tablet. Output went to a discard sink; `SendInput` was not called. CPU load consisted of 16 normal-priority workers scoped to the Rust replay, not a simulation of OBS or GPU load. No global priority, affinity, power plan, registry or configuration setting was changed.

The game and drivers were absent at the start. A user-started osu! process appeared at 14:15, during the third clock-comparison pair; the original .NET driver started at 14:17, after those comparisons. Neither was controlled or measured. The baseline and CPU-stress runs had finished by 14:07. Background activity was not otherwise controlled, and the final script smoke ran while those user processes were open.

## Results

Bulk pipeline processing before the optimization, including harness overhead:

| Path | Rust p50 | Pinned OTD p50 | Rust p99 | Pinned OTD p99 |
| --- | --- | --- | --- | --- |
| Absolute, no filter | 120 ns | 110 ns | 150 ns | 471 ns |
| Native Radial Follow / upstream DLL | 170 ns | 210 ns | 261 ns | 561 ns |
| Same unchanged RadialFollow DLL | 391 ns | 210 ns | 862 ns | 561 ns |

The managed bridge is slower than upstream's direct managed filter path in this dataset. Its median difference is about 0.18 µs per report, much smaller than the 2 ms replay interval. Native Rust cases allocated nothing on the measured Rust heap. That counter does not include .NET allocations: the bridge creates owned report snapshots so plugins can retain reports safely. Those snapshots were preserved.

Paced Rust replay, from event signal to pipeline return, including the discard sink:

| Path and synthetic load | p50 | p99 | Largest observed report |
| --- | --- | --- | --- |
| Native, no workers | 5.98 µs | 8.82 µs | 96.7 µs |
| Managed DLL, no workers | 8.48 µs | 13.20 µs | 1.46 ms |
| Native, 16 workers | 4.76 µs | 61.61 µs | 237.3 µs |
| Managed DLL, 16 workers | 7.41 µs | 46.50 µs | 2.05 ms |

CPU contention worsened the tail even though the median fell. A lower stressed median does not establish better responsiveness. Managed replays also contain occasional millisecond outliers. These samples include initial reports, and no GC/JIT or kernel trace was collected; their cause is not established. Upstream was not measured under the same synthetic load, so this table cannot rank the two drivers under streaming load.

The event model coalesces missed signals rather than preserving a HID queue. All 24,000 signals per Rust filter path were processed, with zero coalesced signals in these runs. The model excludes USB, real HID completion/backlog, `SendInput`, lazer's input/update/draw threads, OBS and display presentation. It does not measure pen-to-screen latency.

The native session clock comparison was stable across runs: before p50 values were 270.5 / 270.5 / 280.5 ns; after were 250.5 / 250.5 / 250.5 ns. Median p99 changed from 360.7 to 340.6 ns. The managed session was noisier, so no separate percentage gain is claimed for it.

## Source and saved-state findings

- The Windows reader already raises its own priority and opts out of EcoQoS. Increasing the entire process priority or assigning CPU affinity is not an evidence-based fix from these measurements.
- Inactive debug capture uses an atomic gate; it does not take the capture mutex for each report. Output ownership remains synchronized so companion devices and failed releases cannot leave buttons held.
- The saved driver profile enables the tablet-space .NET RadialFollow filter. The new benchmark measures that path in both the session loop and paced replay, with the complete raw packet. Replay errors stop the measurement rather than being timed as successful processing.
- The current lazer data folder, resolved through `storage.ini`, has its built-in tablet handler disabled, mouse relative mode disabled, multithreaded execution, `FrameSync=Unlimited` and the OpenGL renderer. These were saved on 3 October; they are configuration evidence, not runtime measurements.
- OBS's 3 October streaming log recorded two rendering stalls and two encoding skips over about 697,000 frames. That does not establish lazer's frame timing or exclude an input problem.

The Unlimited/OpenGL combination is a useful condition to inspect in a later live reproduction. The [official osu! explanation](https://github.com/ppy/osu/wiki/Latency-and-unlimited-frame-rates) describes high-frame-rate GPU pipeline stalls, including effects from other applications. This is a hypothesis for the reported slowdown, not a diagnosis. Settings were left alone.

## Measurement changes and remaining evidence

The harness now supports the managed filter in paced replay, a managed session case, selectable report rate/warm-up in the comparison script, and scoped CPU contention in the standalone Rust harness. Both replay harnesses include output time in their total and mark the timing endpoint. Older totals subtracted output time; historical tables retain their original definition. The summary separates native/managed paths, rates, sinks and load levels, and identifies the Rust allocation counter's managed-heap limit.

The offline script ran through both harnesses and produced the native/managed summary. Synthetic sessions decoded every report, reported no output failures, and matched the 20,000-report output counts before and after the clock change. The changed PowerShell script passed ScriptAnalyzer. The ordinary fmt/Clippy/test suite was not run, following the repository release policy.

A later live run needs the same map and filter settings with OBS off/on, timing from the real driver's report completion through output, and lazer's separate input/update/draw timings. The [official frame-statistics overlay](https://github.com/ppy/osu-framework/wiki/Debug-Overlays:-Frame-Statistics-Overlay) exposes those thread timings. A managed-runtime trace is needed before attributing the outliers to GC or JIT. This audit does not claim the streaming slowdown is fixed.
