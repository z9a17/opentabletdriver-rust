# Performance audit, 5 October 2026

This audit profiled the report path for avoidable work and changed four things. Report values, plugin-visible behavior and saved settings are unchanged. Measurements ran offline in a Linux container, not on the Windows development machine: they compare the baseline with the change on one host, and do not predict Windows times. See the [raw results](perf/2026-10-05/summary.md).

## Changes

**Managed bridge: report interfaces are tested once per type.** Every report that crossed the .NET bridge was tested against each upstream report interface, by `Import` when the report entered .NET and by `Export` each time it returned to the host: 28 interface type tests on the fused path for every report, more without fusion. The interfaces a concrete report type implements never change, so the graph now records them for the last two runtime types it saw and reuses that record. Interface access is unchecked only where that record, for the report's exact type, says it is implemented. COM objects and `IDynamicInterfaceCastable` reports, whose interfaces can differ per object, are still tested on every call. Reports still get a fresh snapshot, raw array and button arrays; plugins may retain them as before.

**Managed bridge: no locked instruction per filter call.** `ConsumeGraph` and `TickGraph` claimed the filter's reentrancy flag with `Interlocked.Exchange`. Only the graph's owning thread reads or writes that flag (`OnEmit` checks the thread first), so a plain check and volatile write now do the same.

**Managed bridge: the report path is compiled when the graph is created.** The first report through a new graph waited while .NET compiled the bridge's report path and the plugin's `Consume`. Graph creation now prepares the bridge's graph, instance and report-snapshot methods (once per process) and each filter's `Consume` method. The graph is created on the session thread before the tablet is read, so this moves the cost from the first pen report to session start. Preparing a method compiles it without calling it. As at a first call, compiling may run a plugin type's static initializer, which .NET allows at any time before first use for types without an explicit static constructor. If a method cannot be prepared, it compiles on its first call as before.

**Relative mode: the fractional carry no longer calls `fmod`.** `RelativeMapper::quantize` kept each axis's fraction with `% 1.0`, which compiles to a call to the C runtime's `fmod` for each axis on every report. A truncating conversion now computes the same bits, including the sign of a zero carry; a test compares the two over 400,000 values and the special cases.

## Results

On the [container described in the raw results](perf/2026-10-05/summary.md#environment), three alternating pairs each:

| Measurement | Baseline | Changed |
| --- | --- | --- |
| Managed bridge, one IntuosV2 pen report, fused, median | 305–312 ns | 221–224 ns |
| First paced report after the graph is created | 7.0–7.4 ms | 0.61–0.75 ms |
| Paced reports 301–4000, p50 | 1.30–1.40 µs | 0.97–1.14 µs |
| Graph creation, once per session | 4–6 ms | 21–23 ms |
| Native relative pipeline, minimum | 45.3–46.1 ns | 42.3–42.7 ns |

The bridge numbers cover the managed side with the `DefaultsFilter` fixture and a managed stand-in for the Rust callback. They exclude the Rust host's own work, the unchanged RadialFollow DLL and the native-to-managed entry. Allocation stays at 320 bytes per report, so garbage collection frequency is unchanged. Tail percentiles after tier-up (p99.9 about 40 µs) did not change measurably on this host. Native absolute processing and Radial Follow did not change.

## Verification

- A differential harness ran the baseline and changed bridge sources over every report shape the bridge imports, including report types defined by a plugin (class and boxed struct), mixed types in one stream, rejected shapes, and both failure paths. The 1,607 frames passed back to the host, results and errors were byte-identical ([method](perf/2026-10-05/summary.md#bridge-equivalence)).
- All `otd-core` and `otd-linux` tests pass in release builds, including the allocation checks. Clippy reports no new warnings in `otd-core`. The Windows crates and examples type-check for `x86_64-pc-windows-msvc` with no warnings. The changed bridge, `GraphBench` and the fixtures build with warnings as errors and unchanged lock files.
- Not run: the Windows test suite, including the .NET bridge integration tests and `native_plugin_round_trip`, `scripts/bench.ps1`, the unchanged RadialFollow DLL, the HID read, `SendInput`, and any tablet, game or pen-to-screen measurement.

## Rejected after measurement

- Shrinking the 1,040-byte report value set to 256 bytes made no measurable difference to pipeline time.
- Replacing `round` in absolute mapping with an exact branch on the fraction made absolute processing about 10 ns slower: the branch mispredicts on real motion, while the C runtime's `round` is branch-free. Mapping is unchanged.
- Leaving the bridge frame's inline arrays uninitialized was slower than zeroing the whole frame.
- Disabling tiered compilation or dynamic PGO for the bridge left the first-report delay in place and made steady-state reports slower.

## Remaining evidence

Repeat the comparison on the development machine with `scripts/bench.ps1` and the unchanged RadialFollow DLL, which also covers the native-to-managed entry and the Rust callback. Pen-to-screen latency and live use with the saved profile still need hardware.
