# Offline optimization evidence, 15 October 2026

Baseline: `274427a4e9809f5c698a4eb82a87a871dc439846` (PR #91).
Implementation: the `perf/managed-overhead` revision containing this record.
Compiler and fixture hashes are in [environment.json](environment.json).
No driver, UI, hardware, real input/output or active settings were used.

## Managed graph

[managed-graph.json](managed-graph.json) records three alternating fresh
processes per version using the existing `compat/GraphBench` benchmark. Each
process warms up, then takes 15 batches of 200,000 synthetic fused pen reports
through `SettingsFixture.DefaultsFilter` and discard callbacks. Process medians:

| Version | ns/report, process 1 / 2 / 3 | Median | Bytes/report |
| --- | --- | --- | --- |
| Base | 525.4 / 545.1 / 542.7 | 542.7 | 410.2 |
| Optimized | 215.7 / 198.8 / 215.8 | 215.7 | 384.0 |

This is one ordinary managed filter graph, not a native-only path or a hardware
latency comparison. Internal/output graphs retain admission tracking.

Build the fixture against its unchanged NuGet SDK dependency, then the harness:

```powershell
dotnet build compat/SettingsFixture/SettingsFixture.csproj -c Release -p:RestoreLockedMode=true -o target/managed-overhead-probe/legacy
dotnet build compat/GraphBench/GraphBench.csproj -c Release -p:RestoreLockedMode=true -o target/managed-overhead-probe/graphbench
dotnet target/managed-overhead-probe/graphbench/GraphBench.dll (Resolve-Path target/managed-overhead-probe/legacy/SettingsFixture.dll).Path (Resolve-Path crates/otd-core/tablets/Wacom/PTH-660.json).Path
```

For the original measurements, the legacy fixture was reused from the prior
RC validation build; its exact DLL hash is recorded. Compile the same harness
from a clean base worktree to repeat the comparison. No third-party downloaded
plugin needs to execute.

## Debugger reads

[Baseline](debug-stream-baseline.csv) and
[implemented](debug-stream-implemented.csv) CSV files record the corpus digest,
median elapsed time and allocation counts. Five batches of 10,000 caught-up
reads are measured for retained depths 0, 64, 512 and 4096. Current reads allocate
nothing at every depth; prior reads allocate one 3584-byte packet vector and
scan retained packets. Both revisions produce digest `8de207a46c7a85e8` over
eviction, wrap, independent cursors, packet/byte bounds and dropped reports.
Timings came from separate runs and are descriptive, not a controlled physical
latency experiment.

The small probe links only the production in-memory stream module:

```powershell
rustc --edition=2024 -O -Awarnings bench/debug-stream.rs -o target/debug-stream-probe.exe
.\target\debug-stream-probe.exe
```

## Focused regression checks

```powershell
cargo test --locked --bin opentabletdriver-rust snapshot_tests -j4
cargo test --locked --bin opentabletdriver-rust managed_wake -j4
cargo test --locked --bin opentabletdriver-rust endpoint_idle_sentinel -j4
cargo test --locked --bin opentabletdriver-rust input_fairness_tests -j4
cargo test --locked -p otd-core --lib debug::stream::tests -j4
```

The recorded Rust total is 29 tests; `managed_wake` was run before the fairness
tests were added and selected the event test alone. Running the final source
with that filter also selects the new fairness tests. The named full fairness
module passed all 12 tests after the final source change.

Run GraphBench with the same fixture/tablet arguments and each mode
`admission`, `background`, `endpoints`, `upstream068`, `wake`. `snapshots` also
runs standalone as `dotnet <GraphBench.dll> snapshots`. All six modes passed.
The implementation record explains the inherited endpoint fixture correction.

The Unix shared library passed this targeted type check with explicit installed
rustup `cargo.exe` and `rustc.exe` paths (compiler selection was process-local):

```text
cargo check --locked -p otd-platform --lib --target x86_64-unknown-linux-gnu -j4
```

No whole-workspace suite, native Unix execution, live driver or hardware check
is claimed. The Windows package is built separately by the test-folder updater.
