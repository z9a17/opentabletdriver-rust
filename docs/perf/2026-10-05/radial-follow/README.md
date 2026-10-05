# Radial Follow comparison harnesses

Sources for [the 5 October Radial Follow comparison](../../../RADIAL_FOLLOW_ORDER_2026-10-05.md). They need the .NET 8 SDK, the `compat/SettingsFixture` build for the bridge's dependencies, and the release DLLs:

- RadialFollow 0.3.0: `https://github.com/AbstractQbit/AbstractOTDPlugins/releases/download/0.3.0/RadialFollow.zip`, `RadialFollow.dll` SHA-256 `830d61f29c07b109398f1e001df3c7fbd359b1ad2491a6d12ccc243dacf16383`.
- Temporal Resampler 1.5.1: `https://github.com/shmkle/TemporalResampler/releases/download/v1.5.1/TemporalResampler.zip`, archive SHA-256 `b9214ab071166fbeb93ab067f7fadea929fe43c600ebe52d7cde50ff43b29f6b`, `TemporalResampler.dll` SHA-256 `f16a1b05702bdb942dae9fce38b6b610404c86da06f27cbec530b8bd865aa9c5`.

## DLL against the built-in filter

`make_trace.py` writes `trace.csv` (SHA-256 `307ed4a8a935f13212ec9f0c48fd86d575f0cc621b40337bbdd2b40242454a71`). Then, from the repository root:

    dotnet build docs/perf/2026-10-05/radial-follow/dll-vs-native/RfDiff.csproj -c Release -p:BridgeDir=$PWD/compat/OtdCompat -o <out>
    dotnet <out>/RfDiff.dll <RadialFollow.dll> crates/otd-core/tablets/Wacom/PTH-660.json <dir>/trace.csv <dir>/dotnet-out.csv

`RfDiff` paces the trace at 500 Hz in real time, so the DLL's own 50 ms reset sees the pauses. Copy `native_trace.rs` and `pipeline_trace.rs` to `crates/otd-core/tests/` and run them as their headers say. The first writes the built-in filter's output for the same trace; compare the two CSV files position by position. The second runs both through the report pipeline and prints how many output packets differ.

## Filter order

    dotnet build docs/perf/2026-10-05/radial-follow/filter-order/OrderDiff.csproj -c Release -p:BridgeDir=$PWD/compat/OtdCompat -o <out>
    dotnet <out>/OrderDiff.dll <RadialFollow.dll> <TemporalResampler.dll> crates/otd-core/tablets/Wacom/PTH-660.json rf-first 200 rf-first.csv
    dotnet <out>/OrderDiff.dll <RadialFollow.dll> <TemporalResampler.dll> crates/otd-core/tablets/Wacom/PTH-660.json tr-first 200 tr-first.csv

Each run paces 12 seconds of synthetic aim at the given report rate, fires the graph's timers when due, and prints each output's distance from the pen after the first second. The CSV files hold every output with its time in milliseconds.

Results, 5 October 2026, Linux container, .NET 8.0:

    rf-first rate=200Hz reports=2400 outputs/s=1200 distance from pen (mm): mean=0.646 p50=0.494 p95=1.491 max=19.300
    tr-first rate=200Hz reports=2400 outputs/s=1200 distance from pen (mm): mean=0.541 p50=0.420 p95=1.183 max=19.396
    rf-first rate=200Hz reports=2400 outputs/s=1199 distance from pen (mm): mean=0.645 p50=0.495 p95=1.499 max=19.351
    tr-first rate=200Hz reports=2400 outputs/s=1200 distance from pen (mm): mean=0.543 p50=0.420 p95=1.183 max=19.397

The maximum is the jump at the start of each 3-second cycle. The per-segment and smoothness figures on the summary page come from the second pair's CSV files.
