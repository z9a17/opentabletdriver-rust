# Parity validation and performance gates

This defines the evidence required by the [roadmap](../FULL_PARITY_PLAN.md) and [work items](WORK_ITEMS.md). It is a verification plan, not a claim that the future checks below already exist or pass.

## 1. Evidence levels

Track implementation and evidence independently in the [evidence ledger](EVIDENCE_LEDGER.md). Evidence kinds are `source-reviewed`, `unit-tested`, `differential-tested`, `integration-tested` and `hardware-verified`; `blocked` is a row state with a reason. More than one evidence kind may apply. A blocked hardware check does not erase a passing parser test. A capability with merged code and missing required evidence remains open.

Every result records task/CAP ID, source and Rust commits, exact command or manual steps, OS/architecture/runtime, device/firmware/transport when relevant, plugin class/version/archive and DLL hashes, configuration/fixture hash, expected/actual behavior, date, tester and links to logs/artifacts. Do not store serial numbers, complete HID paths or personal filesystem paths in public results by default.

Keep test fixture origin explicit:

- `captured`: actual device bytes, with sanitized context and permission to publish.
- `upstream-generated`: output obtained from the pinned reference implementation.
- `synthetic`: deliberately constructed boundaries/failure cases.

A differential pass is behavioral evidence for that input corpus. It is not proof of every device variant or hardware timing characteristic.

## 2. Current executable checks

Run from the repository root on Windows with Rust stable MSVC and the declared .NET SDK/runtime. These commands exist today:

```powershell
cargo fmt --all --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace
cargo build --locked --workspace --release
pwsh -File scripts/build-compat.ps1
$env:OTD_TEST_PLUGIN = (Resolve-Path target/release/otd_ema_filter.dll).Path
cargo test --locked native_plugin_round_trip -- --ignored
python scripts/parity-evidence.py validate
python -m unittest discover -s scripts/tests -p 'test_parity_*.py'
```

Manual benchmarks print their results rather than pass or fail. The first keeps every CPU busy for about ten seconds; see [input latency](../INPUT_LATENCY.md) for recorded results:

```powershell
cargo test --release --locked benchmark_reader_wake_latency_under_load -- --ignored --nocapture --test-threads 1
cargo test --release --locked benchmark_reader_effect_on_game_threads -- --ignored --nocapture
cargo test --release --locked benchmark_display_checks -- --ignored --nocapture
cargo test --release --locked benchmark_relative_pipeline -- --ignored --nocapture
```

`cargo test` also replays the golden traces in `tests/golden`, which freeze the driver's current output; see [behavior contracts](BEHAVIOR_CONTRACTS.md).

For the current managed integration test, download the [original RadialFollow 0.3.0 archive](https://github.com/AbstractQbit/AbstractOTDPlugins/releases/download/0.3.0/RadialFollow.zip), verify SHA-256 `d3f5b0200015e6e90948ee5d7870742ac28d82922cc745e9f1f78527e59f8160`, and extract it into an ignored test directory. The archive's `RadialFollow.dll` must remain unchanged:

```powershell
$env:OTD_COMPAT_DIR = (Resolve-Path target/release/compat).Path
$env:OTD_TEST_DOTNET_PLUGIN = (Resolve-Path target/upstream-plugin/extracted/RadialFollow.dll).Path
cargo test --locked dotnet_plugin_round_trip -- --ignored
pwsh -File scripts/package.ps1
```

The last command builds the package; inspect the extracted package separately. `--version`, `settings`, `check-plugins` with the sample profile and `inspect-plugin` exercise packaging without starting tablet output. Inspection/check commands execute trusted plugin code. The archive path in the test command is an example staging convention, not an automatically downloaded prerequisite.

Check native exit codes immediately in scripts. Run `actionlint` when workflows change; run `Invoke-ScriptAnalyzer -Path scripts -Recurse -Severity Warning,Error` for PowerShell changes. Run `git diff --check`, inspect the staged diff, and run gitleaks for configuration/dependency changes before committing. Do not claim an ignored test passed merely because `cargo test` reports it as ignored.

For documentation-only changes, validate references, inventory reproduction, task IDs/dependencies and examples. A release/version/package change still follows project release checks. Add future harness commands here only after they exist and have been executed.

## 3. Behavioral test matrix

| Layer | Required cases | Evidence |
| --- | --- | --- |
| Device schema | Every baseline file; optional/legacy fields; unknown fields; invalid lengths/specifications | Parse/validation report keyed to all 339 records |
| Matching/init | Duplicate models; ambiguous IDs; strings/attributes; auxiliary pairing; denied access; failed init writes | Fake backend traces and representative real devices |
| Parsers | Every referenced type and dispatched report ID; minimum/short/long buffers; endian/range boundaries | Upstream comparison, synthetic failure cases and captured variants |
| Mapping | Raw/mm/pixel units; rotation; non-square devices; cropping; clipping versus limiting; negative origins | Golden transforms with justified tolerances |
| Relative | Fractional movement; unequal sensitivities; strict reset boundary; duplicate range-entry reports; replug | Deterministic clock replay and actual movement |
| Bindings | Threshold equality; tip/eraser switches; chords; shared keys; drag-only; wheels/wraparound; presets | Exact press/release sequence, failure/retry and hardware input |
| Pipeline | Pre/post order; non-positional events; zero/many outputs; replacement/mutated reports | Rust/upstream trace comparison and unchanged plugins |
| Async | Retained reports; independent timers; late/reentrant callbacks; ordering; overload; disposal | Instrumented fixtures plus original interpolation plugins |
| Settings | All profiles/stores; disabled/unknown entries; enum/default/null; old Rust migration; presets | Round-trip/differential tests and loss report |
| Apply/lifecycle | Reload during input; invalid replacement; endpoint loss; sleep; shutdown; daemon/client crash | Fault injection, handle/thread/timer counts and hardware steps |
| Output | Mouse/key/scroll; pressure/tilt/eraser; flush/reset; partial API failure; application behavior | Fake output assertions, native integration and application records |
| Control/UI | Every method/event/command and user workflow; concurrent edits; slow client; no device/runtime | IPC/original-client tests and platform UI scenario results |
| Distribution | Clean machine; portable relocation; update interruption; rollback; uninstall; permissions | Extracted/installable artifact checks on every claimed platform |

Parser families and transports require representative physical evidence before a broad family claim. Every individual model keeps its own verification level: shared parser evidence supports an implementation claim, not a claim that every model/firmware was physically exercised.

## 4. Managed-plugin qualification

P01's corpus must cover the categories below before P09/V03 can close. Names are candidates from the pinned catalog, not additional claims of current compatibility. Confirm exact classes, metadata and prerequisites during P01.

| Category | Initial examples | Essential assertions |
| --- | --- | --- |
| Tablet/pixel filters | RadialFollow tablet and screen classes, Flip Axes | Units, settings/defaults, stage and output ordering |
| Suppression/report mutation | Hover Distance Limiter, Tilt Calibration, Tablet Debounce | No-report behavior and full report-property preservation |
| Async/interpolation | BezierInterpolator, SpringInterpolator, Temporal Resampler | Time, retained objects, asynchronous output and teardown |
| Drawing/output modes | Windows Ink, Windows Pen Pointer, VMultiMode, Enhanced Output Modes | Pointer services, pressure/contact, prerequisites and app behavior |
| Bindings | Additional Keys, DualActionBinds, Scroll Bindings, Cycle Keybind | State callbacks, injected keyboard/mouse services and release ownership |
| Tools/driver services | Monitor Toggle, Tablet Calibration, Wireless Kit Addon | Driver/display/device access, configuration changes and disposal |
| IPC/UI/script/touch | OTD-IPC, UX Remote, ScriptRunner, Touch Gestures Installer, TouchEmu | Real dependencies and user-visible behavior beyond a filter callback |

Use controlled test processes for intentional hangs/crashes and plugins with broad side effects. This is test isolation, not a claim that ordinary in-process plugins are sandboxed. Preserve binaries unchanged; record hashes before and after tests. Do not redistribute corpus binaries without checking their terms. An archive's metadata hash establishes file identity, not safety or behavioral correctness.

Test the supported CLR runtime range with the same binaries. Validate missing runtime, wrong architecture, missing adjacent managed/native dependency, version conflicts, exceptions and disposal failures. A plugin loading successfully is only the first step; settings, outputs and lifecycle must work.

## 5. Performance method and initial targets

F04 first measures the current release and upstream baseline on the same machine, release build, power mode, device/report rate, display/OS settings and plugin parameters. Keep native-only, native-plugin and managed-plugin workloads separate. Warm up .NET/JIT before steady-state measurements and report startup cost separately. Use fixed traces plus live input where appropriate.

Collect repeated runs and publish p50/p95/p99/max CPU processing duration, wall duration, throughput, allocations/bytes per report, process CPU, working set/private bytes, handle/thread/timer counts, context switches/wakeups and async queue depth. Report measuring-tool overhead and variability. Separate read-wait time, pipeline work, OS injection and physical display latency; do not add unrelated percentile values together as an end-to-end latency result.

Initial engineering targets below are **proposed budgets**, not measured achievements. Adopt or adjust them in F04/V05 with recorded rationale; do not relax them silently to make a graph pass.

| Metric | Target and evaluation |
| --- | --- |
| Native allocations | Zero Rust-owned steady-state allocations for successful synchronous parse/map/binding/native dispatch after warmup. Error/debug/control paths measured separately. |
| Native report CPU | p99 below 10% of the tested device's report interval for the standard built-in chain, excluding device wait and external OS scheduling. Track worst case and missed intervals separately. |
| Regressions | Investigate repeatable >5% CPU or p99 change outside the benchmark's noise interval. Adopt an accepted threshold only after stable repeated baselines; shared CI timing alone must not fail a release unpredictably. |
| Managed overhead | Measure boundary-only overhead and allocation separately from plugin work, GC and timer jitter. No promise of zero allocations for arbitrary unchanged .NET plugins. |
| Idle | No spinning/read polling without a need. Initial target <0.1% of one CPU core averaged over 60 seconds with daemon idle, debugger unsubscribed and UI closed; verify measurement resolution. |
| Control impact | UI open, log streaming and profile inspection must not cause systematic report deadline overruns. Bound diagnostic buffers/subscribers. |
| Memory/resources | No unbounded queue, log, report retention or continuing handle/thread/timer growth. Compare post-warmup/post-quiescence counters across repeated cycles; explain runtime caches. |
| Startup/package | Record native and first-managed-load startup times, binary/package size and runtime requirements; optimize based on measured tradeoffs. |

Use at least a 30-minute automated reconnect/reload/input soak for major lifecycle changes and a 24-hour representative run for the final release candidate. Test realistic report rates plus faster synthetic stress; label synthetic rates honestly. Async overflow policies must preserve control/button/proximity transitions and expose overload rather than silently introducing an ever-growing latency queue.

## 6. Hardware and application records

Extend [HARDWARE_VALIDATION.md](../HARDWARE_VALIDATION.md) with platform/model-specific procedures rather than replacing its historical evidence. A live record includes OS/build, tablet model/firmware/transport, monitor arrangement/DPI, pointer acceleration, plugin/runtime/application versions, profile, exact artifact hash and expected/observed result.

For Windows drawing, test a pressure-capable drawing application with its input API selected explicitly; include an eraser/tilt case and a mouse-only/relative application. For Linux, test mouse and Artist Mode on claimed X11/Wayland environments, including pad behavior. For macOS, test permission changes, mouse/keyboard integration, displays and whichever drawing path upstream/plugins actually support. Record application names/versions rather than asserting all drawing software works.

Reconnect testing must include moving and clicking after recovery, not just an enumeration message. Include held input, several repetitions, sleep/wake and disappearing selected displays. If a physical step cannot be executed, mark it pending and link the required procedure.

## 7. Release checklist

- Each release states the upstream/source/catalog baseline and shipped capabilities; partial releases keep G6 open.
- Run the current build/test/lint checks and applicable native/managed integrations on the tagged source.
- Reproduce inventory and validate task/evidence references when the baseline changes.
- Verify extracted/installable artifacts outside the checkout, plugin dependencies, runtime absence/presence and checksums.
- Review profile migration, stale-input cleanup, setup/upgrade/rollback and user-facing compatibility errors.
- Retain dependency/license/source notices, package hashes and build provenance.
- Publish performance and required hardware evidence with unresolved limitations; do not claim full parity while any blocking capability or required evidence remains open.

For an evidence-only release, validate the ledger, inventory and documents as well as the existing release checks. Keep unfinished implementation tasks open.
