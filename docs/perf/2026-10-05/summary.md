# Offline results, 5 October 2026

Raw results for the [5 October performance audit](../../PERFORMANCE_AUDIT_2026-10-05.md). Baseline: `01f78b0` (0.16.8). Every comparison alternated baseline and changed builds in the same session.

## Environment

A shared cloud container, not the Windows development machine:

| Item | Value |
| --- | --- |
| CPU | Intel Xeon Processor @ 2.10 GHz, 4 logical CPUs, shared with other tenants |
| OS | Ubuntu 24.04.4, Linux 6.18.44 |
| Rust | rustc 1.97.0, `x86_64-unknown-linux-gnu`, release profile |
| .NET | SDK 8.0.131, runtime 8.0, default tiered compilation and PGO |
| Plugin | `SettingsFixture.DefaultsFilter` from `compat/SettingsFixture`, not the RadialFollow DLL |

No tablet, HID read, `SendInput`, uinput, game or display was involved. Background load was not controlled, so single results vary by up to 2× between runs. Use the paired files rather than individual lines.

## Managed bridge

[`graphbench-paired.txt`](graphbench-paired.txt): three alternating pairs of `compat/GraphBench`, built once against the baseline bridge sources and once against the changed sources, each followed by its `cold` mode in a fresh process.

    dotnet build compat/SettingsFixture/SettingsFixture.csproj -c Release
    dotnet build compat/GraphBench/GraphBench.csproj -c Release
    dotnet compat/GraphBench/bin/Release/net8.0/GraphBench.dll <SettingsFixture.dll> crates/otd-core/tablets/Wacom/PTH-660.json
    dotnet compat/GraphBench/bin/Release/net8.0/GraphBench.dll <SettingsFixture.dll> crates/otd-core/tablets/Wacom/PTH-660.json cold

To build GraphBench against other bridge sources, change its `Compile Include` path; the baseline used `git show 01f78b0:compat/OtdCompat/<file>`.

| Measurement | Baseline | Changed |
| --- | --- | --- |
| `fused_pen` median of 15 trials | 305.1 / 305.7 / 311.7 ns | 220.8 / 223.5 / 223.5 ns |
| `fused_pen` allocation | 320 B per report | 320 B per report |
| `dispatch` (unfused, 17-byte packet), third trial | 471.0 / 471.2 / 482.0 ns | 298.0 / 328.6 / 424.1 ns |
| Graph creation | 4.0 / 4.8 / 5.9 ms | 20.8 / 22.4 / 22.7 ms |
| First paced report after creation | 7,008 / 7,158 / 7,408 µs | 611 / 680 / 751 µs |
| Reports 2–300, p99 | 36.4 / 41.5 / 52.0 µs | 30.5 / 31.6 / 44.3 µs |
| Reports 301–4000, p50 | 1.30 / 1.38 / 1.40 µs | 0.97 / 1.02 / 1.14 µs |
| Reports 301–4000, p99.9 | 40.1 / 41.1 / 44.5 µs | 39.4 / 40.6 / 44.8 µs |

After these pairs, the change stopped caching the interfaces of COM and `IDynamicInterfaceCastable` reports; that check runs only when a type is not cached. Two confirmation runs of the final sources measured `fused_pen` medians of 197.5 and 224.6 ns and first reports of 657 and 630 µs.

The unfused `dispatch` trials run while tiered compilation is still in progress, so they are noisier than `fused_pen`. `fused_pen` and `cold` use the IntuosV2 pen shape the Rust host sends: a `ProximityReport` with native tip state and a 192-byte packet. Cold mode paces reports at 1000 Hz with a spin wait; the maximum after tier-up was 42–211 µs across both builds and follows other tenants' load.

## Bridge equivalence

[`bridge-diff`](bridge-diff) compiles the bridge sources given by `BridgeDir` and runs 468 dispatches through two graphs. The first has a native node, the [`bridge-diff-plugin`](bridge-diff-plugin) filter, `DefaultsFilter` after the transform and another native node. The second has the plugin filter alone. Each runs unfused and fused. Inputs cover every report shape `Import` accepts, with and without native tip state, OutOfRange, unsupported shapes and an invalid kind. The plugin re-emits its input, then a class report from its own assembly, a boxed struct report and a plain `IDeviceReport` at different intervals. It also emits an invalid tool type and throws, so both failure paths run. The native callback moves positions and suppresses some outputs. Every continuation's complete frame (except the pointer value), raw bytes, result, error and failed node are printed.

    dotnet build docs/perf/2026-10-05/bridge-diff-plugin/DiffPlugin.csproj -c Release -o <plugin>
    dotnet build docs/perf/2026-10-05/bridge-diff/BridgeDiff.csproj -c Release -p:BridgeDir=<bridge sources> -o <out>
    dotnet <out>/BridgeDiff.dll <SettingsFixture.dll> crates/otd-core/tablets/Wacom/PTH-660.json <plugin>/DiffPlugin.dll

Baseline and changed outputs were byte-identical: 1,607 continuation frames, 36 rejected inputs, and 4 dispatches failing with "Unknown report tool type." or the plugin's exception at the same node.

## Report pipeline

[`pipeline-paired.txt`](pipeline-paired.txt): three alternating pairs of [`pipeline_trace_bench.rs`](pipeline_trace_bench.rs), copied into `crates/otd-core/tests/`, on the baseline (a worktree at `01f78b0`) and the changed tree. It runs `ReportPipeline::process_with_raw` with a discard sink over a 20,000-report synthetic circular-motion trace with precomputed 2 ms timestamps: the development machine's osu! area, Radial Follow settings, and relative mode at 10 counts/mm. It reports the minimum and median of 40 passes. This is the native pipeline only, without the session loop or plugins.

| Case | Baseline min | Changed min |
| --- | --- | --- |
| Absolute | 89.5 / 89.8 / 90.3 ns | 87.4 / 87.4 / 87.7 ns |
| Absolute with Radial Follow | 147.4 / 148.6 / 149.1 ns | 147.6 / 148.2 / 148.3 ns |
| Relative | 45.3 / 45.7 / 46.1 ns | 42.3 / 42.5 / 42.7 ns |
| Relative with Radial Follow | 149.1 / 149.3 / 149.3 ns | 147.2 / 147.5 / 148.6 ns |

Only relative mode changed code. The absolute differences are within the noise of this host.
