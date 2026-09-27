# 0.13.1 audit fixes

This release addresses the 14 actionable findings and five follow-up recommendations from the audit of `ec43ebacc088ce96219d996b1644d65b303bae82` (0.13.0). It also includes fixes found during two independent reviews of the implementation. It does not close a full-parity or hardware-validation gate.

## Changes

| Audit finding | Result |
| --- | --- |
| Overdue timers starve input or stop requests | Each loop polls input after at most one timer pass. Failed output cleanup can recover during idle periods, including with no active timers. Idle failed-release retries wait up to 50 ms; input can wake them sooner. |
| Primary and companion can open the same endpoint | Companion discovery starts with an immutable reserved primary path, after primary selection. |
| Profile changes acknowledge quiescence before companions stop | All companion stop events are signalled before joining workers. Completion and cleanup precede quiescence and original-driver restoration. Tools are disposed before restoration. |
| Companion cleanup failures lose their meaning | Workers retain structured cleanup errors. Cleanup failure stops the generation; an ordinary disconnected device does not prevent an otherwise successful drain. Optional discovery failure does not stop primary input. |
| Tablets disagree about the global mouse button or position | A shared Windows output owner registry combines contact ownership, serializes acknowledged output and deduplicates absolute positions globally. Failed releases remain owned for recovery. |
| Missing monitor causes overdue-timer busy polling | Suspended absolute mappings ignore timer deadlines and continue bounded input/display checks. |
| Timers started by the first report never run | The bridge distinguishes absent timer capability from an existing, stopped timer. Graphs without timers retain their fast path. |
| Plugins receive transport prefixes instead of parser Raw | `DecodedPen` returns the pen, buttons and borrowed canonical payload together. No stored address offsets or second lookup are needed. |
| Valid pressure threshold rejected on tablets above 8191 levels | TOML validation uses the selected tablet specification. |
| A failed display snapshot prevents retries | The new fingerprint is committed only after snapshot success. |
| Update concurrency, incomplete rollback and lost backups | Unique download directories, an installation mutex and a same-volume journal serialize replacement. Incomplete transactions restore old files and remove newly introduced files. Recovery renames mapped images aside, preserves terminal markers until payload cleanup succeeds, and requests a fresh process after restoration. |
| Plugin installation fails across volumes | Verified content is copied into a unique sibling of the destination before rename. Install/remove operations share a lock; a failed restoration is reported. |
| Debugger combines another tablet's packet and label | A registration selects one live session; its label and packet snapshot share one lock and identity. |
| Long-uptime timing summary overflows | Histogram buckets use 64-bit counters. The summary handles inconsistent counts without panicking. |

The five additional recommendations are addressed as follows:

- Activation gates both reports and timer callbacks. Retained timer reports cannot reassert contact after physical range loss, including loss during mapping suspension.
- Healthy companion discovery uses device notifications with a 60-second fallback instead of full enumeration every two seconds.
- Managed graph continuations are allocated once, deadline scans avoid LINQ allocations, and obsolete prepared-report buffers/hooks are removed. Managed report/raw snapshots remain owned because plugins can retain reports.
- Linux rejects enabled external plugins explicitly and makes feature initialization delays interruptible. Linux hardware support remains experimental.
- Plugin inspection, tablet discovery, settings import discovery and device-string reads run off the panel thread. Completions preserve active drafts and ignore stale profile/import results. Folder inspection produces one group result. Device-string dialogs open after releasing panel state. Tablet menus use a background-refreshed cache and show when discovery is pending.

## Automated evidence

Checks run on Windows 11 x64 with Rust stable MSVC and .NET 8, entirely in the background:

- `cargo fmt --all --check` and strict workspace/all-target Clippy.
- Workspace tests: 166 passed, 17 excluded by their existing ignore annotations. Later explicit runs cover the supported native/managed fixture tests below.
- A seeded regression sweep covers 963,328 packet cases across 53 registered parser names, with malformed lengths and all first-byte values. It exercises both parser and production adapter and checks canonical payloads. This is bounded testing, not coverage-guided fuzzing or proof for every device.
- Regression tests cover timer fairness, rejected activation, idle cleanup, stale output, monitor failure/retry, tablet pressure, output ownership/deduplication, zero-allocation native arbitration and 64-bit timing counts.
- Companion tests verify that every stop event is signalled before joining and that cleanup errors survive the drain. Primary selection and daemon quiescence ordering were source-reviewed; there was no live two-tablet/profile-switch test.
- Update tests exercise interruption before commit, new-file rollback, lock contention and repeated recovery. A copied test executable is mapped using `LOAD_LIBRARY_AS_IMAGE_RESOURCE`, without executing its entry point. The test proves deletion is blocked, then verifies rename-based recovery and committed-marker preservation while the image remains mapped.
- Plugin placement passes with its source on C: and destination on E:.
- Nine managed integration tests pass using `SettingsFixture`, `DiscoveryFixture` and checksum-pinned RadialFollow 0.3.0, with in-memory output. Timer coverage includes both initially enabled and first-report-started timers. The manual managed replay test also passes.
- Release workspace build and the built sample native DLL round trip.
- Parity ledger validation and 21 Python tests.

The regular CI also runs the portable core/Linux checks on Ubuntu and core tests on macOS. Release notes and the linked PR record their run results.

## Allocation measurement

The offline `GraphBench` harness compiles the actual bridge source and uses the known `DefaultsFilter` fixture, a 17-byte synthetic pen packet, two pen buttons and a callback that validates every output. It warms up 10,000 dispatches, then runs three trials of 100,000 dispatches and deadline queries. Both versions retain report/raw/button snapshots.

| Measurement | 0.13.0 bridge | 0.13.1 bridge |
| --- | ---: | ---: |
| Managed allocation per dispatch | 496 bytes | 136 bytes |
| Managed allocation per deadline query | 32 bytes | 0 bytes |

These values repeated in all three trials. The 360-byte dispatch reduction is specific to this fixture, not a universal plugin claim. Timing varied during JIT warmup and concurrent background work, so no percentage latency improvement is claimed. Native successful report handling and the owner registry pass allocation-count tests with zero allocations after setup.

To reproduce the current harness without HID, UI or input injection:

```powershell
dotnet restore compat/SettingsFixture/SettingsFixture.csproj --locked-mode
dotnet build compat/SettingsFixture/SettingsFixture.csproj -c Release --no-restore
dotnet restore compat/GraphBench/GraphBench.csproj --locked-mode
dotnet build compat/GraphBench/GraphBench.csproj -c Release --no-restore
dotnet compat/GraphBench/bin/Release/net8.0/GraphBench.dll (Resolve-Path compat/SettingsFixture/bin/Release/net8.0/SettingsFixture.dll).Path (Resolve-Path crates/otd-core/tablets/Wacom/PTH-660.json).Path
```

For the baseline comparison, the same harness and fixture were compiled with `Graph.cs` and `EntryPoints.cs` from the audited commit. This harness measures managed dispatch/allocation only; it does not time USB, OS scheduling, `SendInput`, a game or a display.

## Operational limits

No active driver, daemon, UI, hardware or real input session was started, stopped or modified during this work. The new UI completion paths, two-tablet behavior, physical latency, sleep/secure-desktop behavior and Linux hardware still need live validation. In-process third-party plugins can block or crash the host; the bridge is not a sandbox.

Updates intentionally refuse startup while another transaction owns the installation or while recovery cannot restore a consistent file set. Retry after the update finishes; preserve the journal if recovery reports an error. A successful rollback asks for another launch so the restored generation is used. Loaded committed backups may remain until the older processes exit. Tests model process interruption and Windows file locks, not a physical power cut or every filesystem failure.

These updater improvements apply when running 0.13.1 or later. An older executable performing an upgrade still uses its older updater; extracting the complete release ZIP remains available. Publishing this release does not install it or change a running session.
