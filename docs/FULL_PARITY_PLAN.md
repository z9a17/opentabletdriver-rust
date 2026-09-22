# Full OpenTabletDriver parity plan

Status: approved project direction, implementation backlog open. The starting-point audit used Rust [v0.4.0 / 8d4d90e](https://github.com/z9a17/opentabletdriver-rust/tree/8d4d90eb0d7d2faeac248082c7da7a4c97c669e6) on 2026-09-21. For shipped behavior, use [PORTING_STATUS.md](PORTING_STATUS.md); the baseline and acceptance gates below remain in force.

The objective is a Rust implementation of the observable functionality of stable OpenTabletDriver, with efficient native report processing and **unchanged existing .NET plugins**. Windows PTH-660 support is the starting point. Full parity includes the upstream device database, Windows/Linux/macOS behavior, configuration and presets, desktop workflows, command-line and daemon interfaces, and the plugin ecosystem.

## Start here

| Document | Purpose |
| --- | --- |
| [Agent handoff](parity/AGENT_HANDOFF.md) | How to select, claim, implement, validate, and hand off a task |
| [Work items](parity/WORK_ITEMS.md) | 65 scoped tasks with dependencies, ownership boundaries, and acceptance criteria |
| [Capability matrix and sources](parity/CAPABILITY_MATRIX.md) | What exists, what is missing, and the pinned upstream references |
| [Validation and performance](parity/VALIDATION.md) | Evidence levels, test scenarios, budgets, and release gates |
| [Behavior contracts](parity/BEHAVIOR_CONTRACTS.md) | Current behavior of each stage, golden traces, and every known difference from upstream (F01) |
| [Source inventory](parity/upstream-inventory.json) | Every baseline configuration, referenced parser, plugin contract source, and catalog record |
| [Evidence ledger](parity/EVIDENCE_LEDGER.md) | Per-record validation state, evidence rules, and baseline update procedure (F03) |
| [GitHub tracking](parity/GITHUB_TRACKING.md) | Parent issue and workstream issue links |

The original [implementation plan](IMPLEMENTATION_PLAN.md) is historical. Its first-release exclusions do not limit this roadmap. [PORTING_STATUS.md](PORTING_STATUS.md) describes shipped behavior; this roadmap describes intended work. Do not mark a task complete because this plan exists.

## 1. Compatibility baseline and scope

Pin core behavior to upstream [v0.6.7, commit 736003ed72c8bbb28033b039d5a0bb76c344145c](https://github.com/OpenTabletDriver/OpenTabletDriver/tree/736003ed72c8bbb28033b039d5a0bb76c344145c), the latest stable release found during this audit. Its source, public contracts, configuration data, settings format, and working behavior are the reference. The observed development branch was `0.6.x` at `cadb51af69e8a69db1ab8d0a9c960176db1ba65c`; later development changes are a separate, explicit baseline update.

Pin catalog coverage to [Plugin-Repository at 2dfdff1cd77d274eb19c4d359b240167537c4db2](https://github.com/OpenTabletDriver/Plugin-Repository/tree/2dfdff1cd77d274eb19c4d359b240167537c4db2). The inventory contains:

- 339 configuration files across 26 manufacturer directories.
- 52 distinct parser type names referenced by those configurations; 117 C# files in the configuration parser directory. File counts and parser type counts measure different things.
- 98 catalog metadata files. Upstream's version eligibility rule permits 57 records for 0.6.7.0, representing 50 identities grouped by name, owner, and repository URL. Eligibility does not prove that a binary loads or works.

Core parity means matching user-visible behavior and supported API contracts, not copying the C# internal architecture or reproducing known defects. An intentional difference needs an explicit record, a regression test, and a migration explanation. An unimplemented upstream feature cannot be reclassified as an intentional difference to pass the gate.

Plugin parity means a binary that works on the baseline and uses its supported contracts can run without source changes or recompilation. Test the latest compatible version of each eligible catalog identity, retain the full 57-record inventory for older-version checks, and expand the corpus for compatible manually installed plugins. Plugins with operating-system or external-driver prerequisites remain subject to the same prerequisites as upstream. Missing hardware or unavailable downloads are **blocked evidence**, not successful compatibility or automatic exemptions.

Windows Ink, VMulti, gestures, and several other workflows are supplied by plugins rather than all being built into upstream core. Both the core contract and the applicable unchanged plugin must be covered. A native replacement may improve performance but does not satisfy the requirement to run the original .NET DLL.

Excluded from the parity claim: arbitrary future upstream versions, tablets/transports unsupported by the pinned upstream baseline, and compatibility with every undocumented private implementation detail ever used by third-party code. If a real baseline-compatible plugin exposes such a dependency, record it and resolve its compatibility task; do not silently discard it. FreeBSD symbols alone do not establish an upstream supported release target; assess it separately if supported baseline distributions or users establish that requirement.

## 2. Starting-point audit (v0.4.0)

Rust 0.4.0 is a Windows USB PTH-660 driver with absolute/relative mouse output, tip/eraser-to-left-click thresholds, native Radial Follow, native filter DLLs, a limited .NET filter bridge, and a native TOML/JSON editing panel. The console and panel run the driver in their own process; a persistent separate daemon and control protocol do not yet exist.

The managed bridge accepts synchronous PreTransform position filters only, reuses one report object, and applies returned X/Y only. It does not supply complete raw reports, tilt, rotation, pen buttons, auxiliary input, arbitrary services, async scheduling, output modes, tools, or bindings. The unchanged RadialFollow 0.3.0 tablet filter has an integration test; it is one compatibility example, not ecosystem parity.

That audit found 46 passing unit tests and two DLL integration checks. Live movement/click evidence came from an earlier build. Relative/filter behavior, reconnect input, sleep/wake, and several display cases still need current hardware evidence. See [hardware validation](HARDWARE_VALIDATION.md) and [issue #1](https://github.com/z9a17/opentabletdriver-rust/issues/1).

Known semantic debt includes force-enabling an imported disabled Radial Follow entry, reporting/skipping unknown imported filters, fixed PTH-660 specifications, a limited report adapter, and raw settings editors. Track these as explicit migration and compatibility work.

## 3. Delivery gates

These are acceptance gates rather than promised calendar dates or release numbers. Independent work can proceed before an earlier gate closes. Task dependencies in the backlog determine when implementation can start.

| Gate | Outcome | Required evidence |
| --- | --- | --- |
| G0: Contracts and measurement | Reproducible baseline, extracted testable core, evidence ledger, differential and performance harnesses | F01-F05; contracts and benchmark commands checked in; no regression from 0.4.0 |
| G1: Complete Windows PTH-660 input | Pen/eraser/buttons, auxiliary collection, wheel behavior, reliable output ownership, lifecycle, and faithful settings | F06, D01-D04, B01-B04, O01-O02, C01-C02; replay plus current-build hardware evidence |
| G2: Existing .NET ecosystem | Report/stage/async/service compatibility, output modes, tools, bindings, discovery, installation, and migration | P01-P10, O03/O05, V03; unchanged binaries tested by category and catalog identity; dependencies on device/control work resolved |
| G3: Configuration-driven devices | Complete baseline schema, parser coverage, supported endpoints, independent tablet state | D05-D09 and V01; all 339 records and 52 referenced parser types accounted for, fixtures plus representative hardware per family/transport |
| G4: Desktop and automation | Separate daemon, graphical workflow parity, presets, import/export, CLI and upstream control compatibility | C03-C05, S01-S05, U01-U06; UI task tests and original-client integration |
| G5: Supported platforms | Linux and macOS input/output/UI/packaging alongside Windows | X01-X05 and O04; native runner checks, permissions/setup, application and hardware evidence |
| G6: Full parity release | All in-scope capability and plugin rows have evidence; supported packages are reproducible and documented | V01-V06 and R01-R04; earlier gates closed; zero unexplained parity gaps or blocking regressions |

For intermediate releases, state exactly which gates and capabilities remain open. A Windows-only milestone must not be advertised as full OpenTabletDriver parity. Hardware support tables must retain untested model/firmware qualifications even when a parser family is validated.

## 4. Architecture direction

Keep the native synchronous path direct: transport read -> checked report decoding -> configured PreTransform filters -> mapping -> configured PostTransform filters/bindings -> output. Preserve the upstream ordering, report suppression, and non-positional event semantics verified in F01/P04. Do not assume that every report yields one output.

Extract portable core modules before adding platform branches throughout the existing executable. Favor the smallest useful boundaries: report/configuration model, protocol decoders, mapping/bindings/pipeline, platform device/output/display implementations, control service, UI, and managed compatibility bridge. Separate crates only where they enable independent builds, tests, platform isolation, or stable ABI ownership. Avoid a wholesale rewrite before the existing behavior is covered.

The control plane owns configuration, plugin discovery/install, logging subscriptions, and UI/CLI requests. It prepares validated immutable runtime configurations away from report processing. A per-device owner applies changes at a defined boundary, releases held actions when necessary, and retires old pipelines after callbacks stop. A failed update preserves a known-good configuration and reports the failure.

Each tablet has independent parser, timing, filter, binding, and reconnect state. Endpoint pairing uses physical identity and configuration matches, not an incidental enumeration order. Shared OS key/button output has explicit ownership so one input cannot release another input's held action.

Async plugins and multiple endpoints need documented ownership and ordering. Introduce bounded queues only at necessary concurrency boundaries, with explicit timestamp, generation, overflow, and shutdown policies. Never reuse a managed report while a plugin can retain it. Correct .NET semantics take priority over an allocation optimization that changes behavior.

Continue hosting .NET on demand through the compatibility bridge. Reuse official managed contracts/helper assemblies where binary type identity requires them. Add adapters for actual dependencies on Desktop/Configurations/Native/core assemblies; do not assume `OpenTabletDriver.Plugin.dll` alone is enough. Rust remains responsible for native device processing and its native output paths. Do not secretly start the original OTD daemon to claim port completion.

The UI talks to the daemon and may close without stopping input. Keep the current panel working during migration. U01 chooses a maintainable platform-native or cross-platform frontend strategy through a small measured prototype; no UI framework choice is pre-approved by this document. Backend behavior must remain independently testable and usable headlessly.

## 5. Performance and reliability constraints

- Zero steady-state allocations in Rust-owned synchronous decoding/mapping/binding/native-filter dispatch after initialization. Variable device capacity is determined at setup and bounded; unsupported capacities are diagnosed.
- No report-path filesystem access, network calls, reflection, JSON serialization, unbounded logging, UI locks, or busy polling. Disable debug report cloning/streaming when no client subscribes.
- Measure the native path, managed bridge overhead, and each third-party plugin separately. Third-party .NET allocation and GC pauses are not under Rust's control and must be visible in results.
- Check observable event ordering and input releases before optimizing duplicate suppression, coalescing, float precision, or locking.
- Establish CPU, memory, latency, wakeup, and long-run baselines before claiming an improvement. The exact method and initial targets are in [VALIDATION.md](parity/VALIDATION.md).
- Device reads, pipeline updates, disconnects, process exits, and failed output calls need explicit cleanup. A native plugin can crash its host; document the boundary and test recovery rather than claiming exception handling provides isolation.

## 6. How agents should divide the work

F01 established behavior contracts and golden traces; F03 establishes the evidence ledger. P01 and U01 can proceed independently. F06 is ready when a person with the tablet can perform the physical steps. F02 follows F01; F04/F05 follow F02. Then split between device/report/binding work, configuration/control work, and managed compatibility work once their shared contracts exist.

Use the stable task IDs as the unit of ownership. Workstream issues contain checklists, but claiming one task does not claim the whole issue. Follow the [handoff procedure](parity/AGENT_HANDOFF.md), announce the branch and file boundaries, and check existing claims before editing shared modules. Create a small child issue when a task needs multiple PRs; retain the parent task ID and acceptance criteria.

Suggested integration order after G0: D01 + C01 -> D02/D03 + B01 -> D04/B02 + C02 -> P02/P03/P04 + S01/S02 -> async/output/plugin management, graphical UI, and additional parser families. Platform and release work can proceed against the extracted interfaces without waiting for every Windows feature.

## 7. Risks and decisions to resolve

| Risk or open decision | Owner task | Required resolution |
| --- | --- | --- |
| .NET binaries reference more than the Plugin package or inspect concrete report classes | P01/P03/P07 | Per-assembly/type inventory; compatible identity and adapter tests |
| Async plugins retain objects, invoke after disposal, or emit from their own threads | P05 | Explicit ownership, bounded scheduling, cancellation/quiescence, retained-reference tests |
| Existing settings rely on old Rust overrides or unsupported imported entries | C01/C02 | Versioned migration with preview, preserved data, diagnostics and round-trip tests |
| UI framework changes increase idle CPU or packaging/runtime requirements | U01/X05 | Prototype, accessibility/platform checks, idle measurements and recorded decision |
| Windows drawing mode requires an external virtual driver or differs across applications | O03/P06/V04 | Distinguish native pen injection, Windows Ink plugin and VMulti requirements; application evidence |
| Broad device counts hide missing initialization, endpoint, or report variants | D01-D09/V01 | Per-configuration and parser evidence; no support claim from importing JSON alone |
| Missing Linux/macOS runners or physical devices | X02-X05/V04 | Record blockers, arrange native CI/community hardware evidence, retain open gates |
| Upstream or catalog changes during the port | F03/V06 | Explicit reviewed snapshot update and new/changed/removed capability diff |
| Binary distribution changes license obligations | R03 | Retain source provenance and notices; examine upstream contracts, filters and bundled dependencies |

Do not invent delivery dates for hardware availability, signing credentials, private driver details, or upstream compatibility that has not been measured.

## 8. Completion rule

A task closes only with its implementation, applicable tests, evidence links, documentation, and release impact recorded. A task requiring hardware can have merged code while its validation remains open. Full parity closes only after the matrix, catalog corpus, transport/device coverage, UI/automation workflows, platform packages, and performance gates are audited together. Keep the final unresolved-gap list visible until it is empty for the stated baseline.
