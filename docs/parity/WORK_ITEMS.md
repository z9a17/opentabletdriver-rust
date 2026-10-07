# Parity work items

This is the canonical implementation backlog for the [full parity plan](../FULL_PARITY_PLAN.md). The original 65 tasks started **open and unclaimed**; consult the [current source audit](PARITY_AUDIT_2026-10-07.md) and archived [GitHub workstream history](GITHUB_TRACKING.md) for current scope and progress. Existing code is a starting point, not completion evidence for these broader tasks. The [2026-10-07 audit](PARITY_AUDIT_2026-10-07.md) reconciles current implementations with remaining gaps; source additions and unmerged PRs do not close acceptance criteria.

Dependencies are hard prerequisites for merging the described behavior; investigation and fixture collection may start earlier. `None` denotes a task ready to claim. Source groups refer to [CAPABILITY_MATRIX.md](CAPABILITY_MATRIX.md). Every task also inherits the [definition of done](AGENT_HANDOFF.md) and [validation rules](VALIDATION.md). File names under `Start` exist unless explicitly described as new. F02 moved the portable modules into `crates/otd-core`; `src/` keeps the Windows adapters, plugin loading and UI.

## F: Foundations and existing behavior

Sources: UP-DEVICE, UP-REPORT, UP-PIPELINE, UP-DAEMON. Shared edits: core module extraction and report ownership contracts need one integration owner.

<a id="f01"></a>
### F01 - Freeze observable behavior and compatibility contracts

**Depends on:** None. **Start:** `crates/otd-core/src/protocol.rs`, `crates/otd-core/src/state.rs`, `crates/otd-core/src/mapping.rs`, `crates/otd-core/src/relative.rs`, `crates/otd-core/src/output.rs`, `crates/otd-core/src/config.rs`.

Record input/output units, float tolerances, event order, pressure threshold boundaries, timestamps, proximity/reset behavior, duplicate suppression and failures. Compare baseline upstream methods to current Rust behavior. Create small golden traces for existing absolute/relative/filter/contact paths and list intentional deviations separately. Specify report ownership, stage ordering and cancellation before changing architecture.

**Accept:** Existing tests and new behavioral fixtures pass; every discovered difference has a task or a justified compatibility record. Include disabled-filter import and non-positional reports. No live input is needed for this task.

<a id="f02"></a>
### F02 - Extract a portable, testable driver core

**Depends on:** F01. **Start:** `src/main.rs`, `src/ui_main.rs`, `src/session.rs`, core modules and `Cargo.toml`.

Move shared behavior into library modules with narrow device, output, clock and control interfaces. Separate Windows FFI from parsing/mapping/state. Preserve CLI and panel behavior during extraction; avoid building a generic framework for unimplemented features. Keep platform adapters thin and expose fake endpoints/output for tests.

**Accept:** Golden output remains equivalent; both binaries build; portable unit tests compile on a non-Windows runner; no extra report queue or steady-state allocation is introduced. The extraction PR should not also implement new device families.

<a id="f03"></a>
### F03 - Establish the evidence ledger and baseline update process

**Depends on:** None. **Start:** `docs/parity/upstream-inventory.json`, `scripts/parity-inventory.ps1`, capability matrix.

Add a separate machine-readable ledger keyed by CAP ID, configuration path/parser type, and plugin identity/version/class. Store implementation state, target OS/architecture, fixture/hash, test command, hardware evidence, source revision, blocker, owner and PR/release. Add validation for missing IDs, stale revisions and unsupported completion claims. Produce a new/changed/removed report when a source snapshot changes.

**Accept:** Every inventory record can be traced to evidence or an explicit open state; regeneration leaves evidence intact; importing configuration files cannot label hardware supported. Preserve historical results when the baseline advances.

<a id="f04"></a>
### F04 - Build repeatable performance measurements

**Depends on:** F02. **Start:** `crates/otd-core/src/test_alloc.rs`, existing replay/benchmark tests, new benchmark harness.

Benchmark parsing, mapping, bindings, filter chains, native ABI calls, managed boundary calls, and OS output separately. Record sample rates, CPU/wall time, p50/p95/p99/max, allocation counts, memory, wakeups and environment. Add idle, one-device, multi-device, UI-open and managed-plugin modes. Reproduce current 0.4.0 and upstream results under the same workload before choosing a regression threshold.

**Accept:** A checked-in command generates structured results; repeated runs characterize noise; native successful paths have zero Rust allocations. Claims distinguish synthetic CPU work from actual pen-to-display latency.

<a id="f05"></a>
### F05 - Add differential, property and malformed-input harnesses

**Depends on:** F02. **Start:** pure core modules, new fixtures/reference-runner tooling.

Use the pinned upstream implementation to generate expected report, transform, binding and settings behavior. Record licenses and fixture provenance. Add fuzz/property entry points for byte parsing, configuration values, stage ordering and lifecycle sequences. Stub clocks/output so tests are deterministic and cannot move the cursor.

**Accept:** Truncated/oversized reports, NaN/infinity, unknown IDs, overflow and invalid settings do not panic or create stuck actions. A deliberately changed mapping/transition makes the differential suite fail. Any numeric tolerance is explained.

<a id="f06"></a>
### F06 - Close the existing Windows hardware validation gap

**Depends on:** None. **Start:** [issue #1](https://github.com/z9a17/opentabletdriver-rust/issues/1), `docs/HARDWARE_VALIDATION.md`.

Run a current release on the actual PTH-660: resumed movement/click after replug, held-contact unplug, idle and contact shutdown, repeated reconnect, sleep/wake, monitor removal, relative mode and native/managed filtering. Obtain a real 0x1e trace if the hardware emits one; otherwise preserve that qualification. Keep original-driver coexistence state documented.

**Accept:** Record build hash, OS/firmware/transport, steps and observed output. Reopening a handle does not prove input recovery. Without hardware participation, leave the task awaiting validation and continue independent work.

## D: Device configuration, reports and transports

Sources: UP-DEVICE, UP-CONFIG, UP-REPORT. Split parser work by exact type names in the inventory; manufacturer names alone do not define parser ownership.

<a id="d01"></a>
### D01 - Load the complete upstream device configuration schema

Release 0.14.5 adds a separately pinned [current device catalog](device-catalog.json): 357 configurations and 53 referenced parser names. [Source audit and compatibility limits](../TABLET_CATALOG_0.14.5.md). The stable baseline criteria below and historical evidence are unchanged; the current catalog's hardware evidence remains unverified.

**Depends on:** F02, F03. **Start:** `crates/otd-core/src/config.rs`, `crates/otd-core/src/protocol.rs`; new device-specification module distinct from user profiles.

Represent digitizer/pen/button/wheel/analog specifications, all identifiers, attributes, device-string predicates, optional lengths and initialization declarations. Preserve documented legacy field aliases and custom overrides. Load/index the pinned database at startup, retain provenance, validate unsupported or contradictory declarations and keep regex work out of report processing.

**Accept:** All 339 files deserialize with an explicit validation result; 52 referenced parser types resolve to implemented or clearly missing entries. Unknown fields are preserved or diagnosed, and unsupported parsers never silently become the PTH-660 parser.

<a id="d02"></a>
### D02 - Match, pair and initialize device endpoints

**Depends on:** D01. **Start:** `src/hid.rs`, `src/session.rs`, `src/main.rs` selection logic.

Match VID/PID plus optional report lengths, strings and attributes using upstream precedence. Pair auxiliary endpoints by physical device identity. Implement declared feature/output writes and string initialization in the upstream sequence, with appropriate permissions, length checks, failure reporting and cancellation. Keep diagnostic inspection separate from an active initialization session.

**Accept:** Fake devices cover ambiguous matches, two identical models, wrong collections, denied access and partial initialization. A failed sequence closes resources and does not continue output. Actual writes have a source/configuration reference; no guessed initialization reports.

<a id="d03"></a>
### D03 - Introduce the full report and capability model

**Depends on:** F02, D01. **Start:** `crates/otd-core/src/protocol.rs`, `crates/otd-core/src/state.rs`, managed report adapter contract.

Represent pen, absolute position, proximity/distance, pressure, tilt, eraser, tool identity, auxiliary buttons, mouse/puck, absolute/relative analog, wheels and touch where upstream provides them. Preserve raw report length/data and distinguish absent fields from zero. Define device/session identity, monotonic receipt time and sequence metadata separately from plugin-visible values. Allocate capacities at device setup.

**Accept:** Pure tests cover each report category and maximum capacities; one category cannot clear unrelated held state. Borrowed/native and owned/retained report lifetimes are explicit. PTH-660 behavior and zero-allocation native dispatch remain intact.

<a id="d04"></a>
### D04 - Process PTH-660 auxiliary input

**Depends on:** D02, D03, B01. **Start:** `src/hid.rs`, `crates/otd-core/src/protocol.rs`, `crates/otd-core/src/session.rs`, `src/session.rs`.

Open the 44-byte auxiliary collection and implement the IntuosV2 auxiliary reports used by the device, including express keys and ring/mode data supported by upstream. Merge pen and auxiliary activity into the tablet's state without blocking pen reads. Define ordering and cleanup if only one endpoint disconnects.

**Accept:** Captured or upstream-derived fixtures decode button/wheel transitions; loss of the auxiliary stream releases only its actions. Live express-key/ring tests are recorded separately from parser tests. Touch report recognition must not fabricate gesture output.

<a id="d05"></a>
### D05 - Support independent tablets and resilient session lifecycle

**Depends on:** D02, D03, B01. **Start:** supervisor in `src/main.rs`, `src/session.rs`, `src/original_driver.rs`.

Manage more than one tablet, stable identity across reconnect, independent profiles/pipelines and endpoint arrival/removal. Handle cancellation, read failure, sleep/wake, partial device loss and output cleanup. Publish device state changes through the control interface when available. Preserve explicit behavior for original-driver coexistence; do not expand process suspension to unrelated applications.

**Accept:** Two simulated tablets interleave without state leakage; disconnecting either leaves the other operational; repeated connect/reconfigure/stop does not leak handles or timers. Physical same-model and mixed-model tests remain visible qualifications.

<a id="d06"></a>
### D06 - Port all referenced Wacom parser variants

**Depends on:** D03, F05. **Start:** new parser modules, inventory entries under Wacom.

Split into child tasks by parser type: Intuos generations, Wacom-driver variants, Bamboo/BambooPad, Cintiq, Graphire, PL/PTU, auxiliary and extended reports. Include wireless/offset/tool/mouse/touch variants actually dispatched by these parsers. Share checked field readers only where layouts truly match.

**Accept:** Every referenced Wacom parser and dispatched report variant has a fixture or explicit evidence blocker, short-read coverage and a ledger entry. Compare decoded values to upstream. Do not infer all 91 configuration files are hardware-tested from one IntuosV2 test.

<a id="d07"></a>
### D07 - Port UC-Logic, Huion and XP-Pen parser families

**Depends on:** D03, F05. **Start:** new parser modules; UP-CONFIG UCLogic/Huion/XP_Pen.

Implement the referenced base, tilt, generation, offset-pressure and auxiliary variants. Account for OEM configurations using these parsers, including Gaomon and other brands. Assign child tasks from the exact 52-type list, not duplicated manufacturer backlogs.

**Accept:** Endianness, pressure ranges, tip/proximity flags, tilt, buttons, wheel data and initialization-dependent layouts match reference fixtures. Each applicable configuration maps to a parser test and initialization status. Include malformed and boundary reports.

<a id="d08"></a>
### D08 - Complete remaining parser and configuration coverage

**Depends on:** D03, F05. **Start:** inventory parser entries not owned by D06/D07.

Cover generic Plugin tablet/aux parsers, SkipByte, Veikk, XenceLabs, XENX, Genius, ViewSonic, Acepen, Bosto, FlooGoo, Lifetec, RobotPen and any remaining referenced types. Reconcile all manufacturer/configuration records after parser tasks merge. Device-specific quirks need a source reference and a scoped fixture.

**Accept:** No orphan configuration or unassigned referenced parser remains. The ledger clearly separates parser implementation, initialization replay, captured reports and physical verification. Missing captures remain open validation work.

<a id="d09"></a>
### D09 - Add supported alternate endpoints and wireless transports

**Depends on:** D02, D03, D05. **Start:** device backend interface; UP-DEVICE WinUSB and relevant configuration attributes.

Implement WinUSB where used by upstream, supported Bluetooth report variants and wireless receiver relationships. Preserve endpoint capabilities, reads/writes, string queries, initialization and disconnect semantics. Coordinate with managed device-hub and Wireless Kit plugin work; do not equate a PID entry with functioning transport support.

**Accept:** Backend tests include cancellation, permission failures and wrong endpoint rejection; native hardware evidence covers each transport claimed. Describe external-driver requirements without silently replacing system drivers. Keep USB PTH-660 working independently.

## B: Bindings and action ownership

Sources: UP-BINDING, UP-SETTINGS, UP-DAEMON. New binding state belongs in the core; OS injection belongs in platform output adapters.

Status: pen, auxiliary, tablet mouse, wheel and scroll action/editor source is delivered, alongside managed binding endpoints and shared ownership. See [pen buttons](../PEN_BUTTONS.md), [Windows workflows](../WINDOWS_COMPATIBILITY_0.17.0.md) and the current audit. Current regression/hardware evidence and full B04 toggle/preset semantics remain open; the original acceptance criteria below are unchanged.

<a id="b01"></a>
### B01 - Implement shared key/button ownership and cleanup

**Depends on:** F02. **Start:** `crates/otd-core/src/state.rs`, `crates/otd-core/src/output.rs`; new action state module.

Track desired and successfully emitted actions by device/binding owner. A release removes only that owner's hold; output failures do not falsely advance emitted state. Handle contact, eraser changes, proximity loss, reload, endpoint failure and normal shutdown. Define recovery after an injection API partially succeeds.

**Accept:** Two bindings holding the same key/button produce one press and one final release; failure/retry and disconnect/reconfigure traces cannot leave logical ownership behind. Keyboard modifiers and mouse buttons follow the same tested lifetime model without per-report allocation.

<a id="b02"></a>
### B02 - Implement pen, auxiliary, mouse and keyboard actions

**Depends on:** B01, D03. **Start:** binding engine, `crates/otd-core/src/output.rs`, `src/output.rs`, `crates/otd-core/src/config.rs`.

Port adaptive pen actions, mouse buttons, key bindings and multi-key chords. Support pen side buttons, auxiliary buttons and tablet mouse buttons. Preserve tip/eraser thresholds, press/release order, disabled settings and per-device button counts. Expose descriptors to configuration/UI without coupling them to Windows key codes.

**Accept:** Differential traces cover every built-in binding class, overlapping chords, eraser inversion, repeated reports and unsupported platform actions. Windows injection tests verify left/right modifiers and extended keys where applicable. Hardware button tests are separately recorded.

<a id="b03"></a>
### B03 - Implement wheels, rings, strips and scroll bindings

**Depends on:** B02, D03. **Start:** new analog/wheel state; UP-BINDING WheelBindings/DeltaThresholdBindingState.

Support absolute/relative wheel reports, mode buttons, step counts, threshold accumulation, wraparound and directional actions. Route tablet mouse scroll through its configured bindings. Keep absolute analog absence distinct from a zero position; use upstream device specifications to interpret units.

**Accept:** Replay both directions, wraparound, inactive ring, mode change, sub-threshold accumulation and reconnect. Ensure slow movement accumulates and button transitions are never coalesced away. Include a representative physical ring/wheel when available.

<a id="b04"></a>
### B04 - Finish binding policies and preset actions

0.17.1 source increment: native held-action toggles and guarded per-device TOML
presets, plus the unchanged managed whole-collection JSON PresetBinding provider.
See [the delivery contract](B04_BINDINGS_0.17.1.md). Acceptance remains unrun.

**Depends on:** B02, B03, C01. **Start:** binding settings/engine; UP-BINDING PresetBinding and threshold states.

Implement drag-only behavior, pressure/tilt disable settings, preset-selection actions and correct state changes when an action replaces its own profile. Integrate managed `IBinding`/`IStateBinding` instances through P06 without duplicate input ownership.

**Accept:** Threshold equality/zero/max and changed thresholds behave as specified; preset switching during held input produces cleanup and a coherent new state. Native policies can complete before P06, but managed-binding acceptance remains open until P06 passes.

## O: Mapping and output

Sources: UP-PIPELINE, UP-OUTPUT, UP-PLATFORM. OS-specific prerequisites must be reported, not treated as successful output.

<a id="o01"></a>
### O01 - Match absolute and relative transform semantics

**Depends on:** F05, D01. **Start:** `crates/otd-core/src/mapping.rs`, `crates/otd-core/src/relative.rs`, `crates/otd-core/src/radial_follow.rs`.

Generalize device dimensions and ranges; match upstream transform order, clipping versus limiting, rotation, area coordinates, sensitivity and reset behavior. Include reports suppressed by limiting and duplicate reports around range loss. Record the current extra relative resets/fractional accumulation as compatibility decisions where they differ.

**Accept:** Differential fixtures cover non-square tablets, rotated/cropped areas, negative display origins, clipping off, limiting edges and reset boundaries. Fixes preserve event and binding semantics and have defined floating-point tolerance.

<a id="o02"></a>
### O02 - Complete Windows mouse, keyboard and display output

**Depends on:** B02, O01. **Start:** `src/output.rs`, `src/display.rs`.

Complete relative/absolute mouse, wheel, keyboard and display capabilities used by built-in bindings. Handle virtual-screen coordinates, DPI scaling, monitor hotplug, missing selected displays and API failures. Keep prepared event batches fixed-size where possible and refresh topology off the input path.

**Accept:** Fake and real API checks establish ordering and partial-failure handling; Windows tests cover multiple monitors, negative origin, fractional DPI and display removal. Document OS acceleration effects on relative mouse output.

<a id="o03"></a>
### O03 - Deliver Windows pen and drawing application support

**Depends on:** D03, B02, P06. **Start:** output backend interfaces, managed output-mode bridge.

Run unchanged compatible Windows Ink and Windows Pen Pointer modes; assess VMulti with its external driver prerequisites. Add native pen output if it provides equivalent behavior and useful measured savings, without replacing the original-plugin compatibility test. Implement pressure, tilt, eraser, hover, barrel/tip transitions and contact lifecycle supported by each backend.

**Accept:** Record pressure ramps, tilt/eraser changes, hover/contact and disconnect cleanup in representative drawing applications. Compare the same plugin/settings against upstream. Distinguish Windows pointer injection, Ink and WinTab expectations; do not promise one API covers every application.

<a id="o04"></a>
### O04 - Match Linux Artist Mode and virtual pad semantics

**Depends on:** D03, B02, X02. **Start:** new Linux output mode; UP-OUTPUT LinuxArtistMode, EvdevVirtualTablet/Pad.

Define tablet/pad capabilities, pressure/tilt/proximity, stylus buttons, synchronous frames, eraser and auxiliary pad events. Coordinate with X03's Linux output/display implementation and managed pointer services. Preserve pressure/tilt disable settings and device-specific ranges.

**Accept:** Recorded evdev events match expected capability descriptors and order; supported applications receive tablet pressure and pad actions. Permission-denied, disconnect and virtual-device destruction release resources. No Artist Mode claim from mouse movement alone.

<a id="o05"></a>
### O05 - Route all supported report and pointer capabilities

**Depends on:** D03, P04, B03. **Start:** pipeline/output capability interfaces.

Route non-positional reports, tool/mouse, raw/analog/touch events, range loss and pointer flush/reset correctly. Preserve pressure/tilt/eraser mutations made by plugins. Touch reports must reach compatible plugins; core gesture synthesis is required only where baseline core actually supplies it. Define precise unsupported-capability errors per backend.

**Accept:** A capability matrix test proves no report is accidentally forced into a pen-position packet. Mixed event traces preserve ordering and flush/reset semantics. Touch/gesture plugin qualification is tracked through P09/V03.

## C: Settings, migration and profiles

Sources: UP-SETTINGS, UP-PLUGIN, UP-DAEMON. Treat device specification data and user profile data as separate schemas.

<a id="c01"></a>
### C01 - Version the Rust profile schema and preserve semantics

**Depends on:** F01, F02. **Start:** `crates/otd-core/src/config.rs`, example profiles.

Represent full profile collections, settings revision, tools, binding stores and enabled/disabled ordered plugin settings. Preserve unknown data for future migration and detect unsupported active features. Make the legacy forced-Radial-Follow behavior explicit and opt-in in migrated profiles; new OTD imports must honor Enable. Prevent duplicate native/managed execution of the same migrated filter.

**Accept:** Old Rust profiles still load through a documented migration; OTD import/save does not lose disabled entries or constructor defaults; unknown enabled behavior cannot be silently represented as equivalent. Tests cover missing versus null values and stable serialization.

<a id="c02"></a>
### C02 - Import complete OTD profiles and plugin settings

**Depends on:** C01, D01, P01. **Start:** OTD JSON import and plugin type resolution.

Import every profile, output mode, binding, wheel settings, filters, tools, locks and plugin properties. Resolve assembly/class identities using the installed plugin metadata, or preserve unresolved entries with actionable diagnostics. Provide a preview of migrated, unsupported and intentionally changed settings. Keep original OTD files intact.

**Accept:** Round-trip fixtures cover multi-tablet profiles, all built-in settings, missing plugins, stale versions and relative DLL paths. An unchanged working profile can be compared against upstream after relevant features exist; importing unsupported features does not count as executing them.

<a id="c03"></a>
### C03 - Apply configuration changes transactionally

**Depends on:** C01, D05, P04, B01. **Start:** runtime config preparation and session ownership.

Validate and initialize a replacement pipeline off the report path, then switch at a defined device boundary. Stop old timers/callbacks, release affected actions and retain a known-good configuration on failure. Detect concurrent edits with a revision/generation. Explicitly define whether each change preserves or resets filter/relative state.

**Accept:** Failure during validation, plugin construction or activation leaves one coherent configuration; repeated apply/undo under input has no stale emissions or leaked state. Async retirement acceptance depends on P05 before async profiles can hot-reload.

<a id="c04"></a>
### C04 - Add presets, import/export and durable storage

**Depends on:** C01, C03. **Start:** profile persistence, new preset store.

Add named presets and upstream-compatible settings export where representable. Support per-platform user paths and portable mode, atomic save, backup/recovery, read-only destinations and conflict detection. Keep plugin paths relocatable with explicit base-directory resolution. Preset application uses the same transaction path as the UI/API.

**Accept:** Interrupted or failed writes preserve a valid previous profile; moving a portable directory preserves relative references; export reports nonrepresentable Rust extensions. Preset switch tests include held inputs and unavailable plugins.

<a id="c05"></a>
### C05 - Provide area editing and conversion services

**Depends on:** C01, O01. **Start:** configuration/mapping library; UP-SETTINGS area converters.

Implement aspect ratio and usable-area locks, display mapping defaults, full/centered area operations and upstream conversion modes such as percentage, Gaomon and XP-Pen conventions. Keep conversion functions shared by UI and CLI. Return units and validation errors suitable for graphical editors.

**Accept:** Known upstream examples and property tests cover conversions, round trips, rotation and invalid dimensions. Locks affect editing as specified without silently rewriting an unrelated profile.

## P: Unchanged .NET plugins and native extension API

Sources: UP-PLUGIN, UP-PIPELINE, UP-CATALOG, UP-REPORT, UP-DAEMON. Compatibility is a required deliverable. A Rust port of a plugin does not close its unchanged-DLL test.

<a id="p01"></a>
### P01 - Inventory plugin binaries, APIs and prerequisites

**Depends on:** None. **Start:** source inventory, `compat/OtdCompat`, `src/dotnet.rs`.

For the 50 eligible catalog identities, select compatible releases, verify archive hashes and record exported classes/categories, referenced assemblies, platform requirements, external drivers and public/private API dependencies. Include baseline-compatible manually installed examples. Inspect metadata/source without executing code merely to classify it. Keep downloaded binaries out of Git unless redistribution is justified.

**Accept:** Every identity has a versioned corpus entry, checksum/source/license, category and result or blocker. Include synchronous/pixel filters, async interpolators, output modes, tools, bindings, report parsers/providers, device access and UI-dependent cases. Name missing APIs precisely.

<a id="p02"></a>
### P02 - Match managed discovery, settings and dependency initialization

**Depends on:** P01, D01, C01. **Start:** `compat/OtdCompat/EntryPoints.cs`, `src/dotnet.rs`, `src/plugins.rs`.

Honor PluginName/Ignore/SupportedPlatform and property metadata: defaults, nullability, enums, units, tooltips, sliders, booleans, validation and actions. Implement required property/field service injection and OnDependencyLoad order. Inject the actual tablet specification. Preserve CLR assembly/type identity and per-plugin dependency resolution; report missing services and runtime/version mismatches clearly.

**Accept:** Differential fixtures cover constructors with defaults, inherited fields/properties, failed initialization and overlapping dependency versions. Discovery and normal profile parsing have explicit execution boundaries. Metadata drives typed UI controls without report-path reflection.

<a id="p03"></a>
### P03 - Preserve report data, identity and managed lifetimes

**Depends on:** P01, D03. **Start:** managed `Report`/`Instance` adapter and native ABI bridge.

Supply the relevant baseline report interfaces, raw bytes, buttons, tilt, proximity, analog/touch and tool data. Preserve report mutations and replacement objects. Determine which plugins require concrete report types. Borrow/reuse only where proven safe; use owned snapshots or compatible objects for retaining/async plugins. Document ownership and array lifetimes at the native boundary.

**Accept:** Unchanged plugins reading each category see the same values as upstream; pressure/button changes survive the return path; a plugin retaining a report never observes a later report overwrite it. Measure compatibility-path allocation separately.

<a id="p04"></a>
### P04 - Match pipeline stages and emission semantics

**Depends on:** P02, P03, O01. **Start:** filter chain, mapping/output stages.

Support PreTransform and PostTransform, stable order within stages, binding placement and the output mode transform. Match synchronous zero/one/multiple emissions and non-positional events. Keep the native built-in filter's settings/order compatible with imported stores. Reject unsupported stage declarations explicitly.

**Accept:** Test tablet-space and screen-space RadialFollow unchanged, a suppressing filter, multiple-output filter and mixed native/managed chains. Coordinate units and binding order match upstream traces. No discarded report may leave a held action silently orphaned.

<a id="p05"></a>
### P05 - Support async filters and timer-driven output

**Depends on:** P03, P04. **Start:** managed lifecycle/timer services and device-owned scheduler.

Implement AsyncPositionedPipelineElement/ITimer behavior and plugin-owned callback handling. Define time units, thread affinity, serialized output, bounded storage, overflow/backpressure, generations and disposal quiescence. Preserve source timestamps separately from emission time. Never wait on a UI thread or pretend timing out a callback safely stops plugin code.

**Accept:** Test unchanged interpolation/resampling plugins plus fixtures retaining reports and emitting after consume. Stop/reload/disconnect prevents stale output, runaway timers and use-after-free. Queue overflow is observable and cannot discard key/button releases. Measure jitter and memory in a sustained run.

<a id="p06"></a>
### P06 - Host managed output modes, bindings and tools

**Depends on:** P02, P04, B02. **Start:** managed bridge exports, output service adapters, tool lifecycle.

Support IOutputMode and pointer providers, IBinding/IStateBinding and ITool with relevant driver, display, keyboard, mouse, pressure/tilt/eraser and timer services. Preserve initialization, update, action and dispose semantics. Expose capability-specific errors and route managed binding output through ownership rules. Extend service availability as S02/P07 land.

**Accept:** Unchanged representative output, binding and tool binaries work with real settings and cleanup. Windows Ink/pen, preset/scroll actions and a lifecycle tool are required cases; async-dependent cases also require P05. Validate failed construction/disposal and missing external prerequisites.

<a id="p07"></a>
### P07 - Complete driver, parser, provider and device-service compatibility

0.17.1 source increment: actual original helper assemblies, driver/config/parser/
device metadata and asynchronous daemon providers with scoped ownership. Concrete
readers/shared stream I/O and managed DeviceReport remain open. See
[the provider delivery contract](P07_SERVICES_0.17.1.md); acceptance remains unrun.

**Depends on:** P02, D02, D03, S02. **Start:** adapter assemblies and UP-PLUGIN/UP-DEVICE component interfaces.

Add required report parser/configuration provider/device hub/device endpoint/driver/daemon services. Audit references to Desktop, Configurations, Native and core assemblies, static globals, events and concrete report types. Keep request lifetimes and callbacks consistent with Rust ownership. Document any required upstream helper assembly and license.

**Accept:** Plugin-supplied parser/configuration and device-access/tool examples function unchanged; enumeration, strings, reads/writes, notifications and shutdown have tests. No adapter silently returns empty devices or default values as a substitute for a required service.

<a id="p08"></a>
### P08 - Implement plugin catalog and package lifecycle

**Depends on:** P01, P02, C01. **Start:** plugin manager control service, UP-CATALOG.

Implement local DLL/archive installation, catalog browsing/filtering, download, update, uninstall, version/platform checks, dependency placement and offline cache. Verify metadata hashes; stage and validate archives before atomic replacement. Reject traversal, absolute paths and archive bombs. Handle files locked by active plugins and schedule restart when unloading is impossible. Preserve settings through upgrades and rollback.

**Accept:** Tests cover corrupt/hash-mismatched archives, failed download, duplicate identities, incompatible versions, missing hashes with explicit policy, rollback and active-plugin removal. The UI makes execution/install scope clear. Catalog eligibility is not shown as tested compatibility.

<a id="p09"></a>
### P09 - Qualify the unchanged plugin ecosystem

**Depends on:** P05, P06, P07, P08, O05, S03. **Start:** P01 corpus and compatibility harness.

Run each eligible identity's selected binary against both the Rust bridge and the pinned OTD host on applicable platforms. Cover all exported compatible classes, settings, lifecycle, errors and dependencies. Exercise touch/gesture, remote UI/IPC, calibration, device-access and script/tool cases in addition to filters. Resolve API gaps through child issues; do not hide them behind a generic unsupported label.

**Accept:** Every corpus row has reproducible evidence or an explicit blocker; no baseline-compatible category is left unimplemented at G2/G6. Where dependencies require hardware or external software, retain a qualified result and a pending end-to-end test. Publish limitations with intermediate releases.

<a id="p10"></a>
### P10 - Evolve the native plugin API without breaking version 1

**Depends on:** D03, P04, P05. **Start:** `crates/otd-plugin-api`, `plugins/ema`, native loader.

Specify a versioned extension for report capabilities, stage metadata, settings descriptors and optional scheduling needed by native plugins. Preserve v1 loading and position-only behavior. Define struct sizes/alignment, capability negotiation, pointer ownership, panic/exception boundaries and lifetime. Keep portable examples and authoring documentation.

**Accept:** Old EMA DLL still loads; cross-version rejection is clear; supported architectures agree on layouts; integration tests cover reset/dispose/failure and no report-path allocations. Native API work must not postpone the required managed compatibility cases.

## S: Daemon, IPC, command line and diagnostics

Sources: UP-DAEMON, UP-CLI, UP-PLATFORM. Control interfaces must not place serialization or client backpressure on device threads.

<a id="s01"></a>
### S01 - Separate daemon lifetime from UI lifetime

**Depends on:** F02, B01. **Start:** `src/main.rs`, `src/ui.rs`, instance guard and stop signaling.

Create a per-user daemon owning devices and pipelines. At the owner's request, GUI close shuts down the daemon and waits for process exit after cleanup, starting in 0.16.1; minimizing and headless CLI operation retain input. This intentionally differs from upstream detach-on-close. Explicit Stop/Shutdown remain distinct. Preserve a foreground diagnostic mode. Handle single-instance discovery, stale endpoints, normal shutdown, failed startup and original-driver coexistence. Avoid elevation for normal supported operations.

**Accept:** GUI close waits for daemon cleanup and process exit; reopening starts a fresh daemon, while minimizing and headless operation preserve input. Daemon exit releases cooperative output and resources; two clients cannot create duplicate injectors. Tests cover startup races and client crash. Any crash recovery limitations are documented.

<a id="s02"></a>
### S02 - Add a versioned local control protocol

**Depends on:** S01, C01. **Start:** new IPC request/response/event types and control service.

Expose device/status, configuration/preset, plugin, start/stop, logs and diagnostic operations with request IDs, errors, cancellation and compatibility negotiation. Use per-user local endpoints and bounded subscribers; prepare configuration changes through C03 once available. Keep event snapshots/revisions consistent across clients.

**Accept:** Integration tests cover unknown versions/methods, invalid/large payloads, slow/disconnected clients and concurrent edits. Unauthorized local users cannot control the session. Report output continues when a client blocks; closed subscribers are reclaimed.

<a id="s03"></a>
### S03 - Support the upstream daemon client contract

**Depends on:** S02, C03, P06, P08, S05. **Start:** UP-DAEMON IDriverDaemon and StreamJsonRpc transport.

Implement or adapt the upstream RPC method/event/data contract so original console/UX and compatible remote-control plugins can use supported functions. Audit actual framing, serialization, pipe naming, events and reconnection with a real baseline client. Keep compatibility endpoints distinguishable to prevent accidental connection to the original daemon.

**Accept:** Original client exercises settings, device detection, plugin lifecycle, logs/debug streams and resynchronization; unsupported intermediate methods return clear errors. The complete IDriverDaemon surface is accounted for before parity. Stubbed successful responses do not pass.

<a id="s04"></a>
### S04 - Reach command-line and scripting parity

**Depends on:** S02, S05, C04, C05, P08, R02. **Start:** `src/main.rs` commands and UP-CLI declarations.

Add upstream-equivalent load/save/defaults/preset, detect, install/uninstall, output/filter/tool/binding setters, area/sensitivity/reset/lock operations, getters/lists, device strings, diagnostics, updates, stdio and editor workflows. Preserve current Rust commands and document aliases/differences. Return stable exit codes and machine-readable output where appropriate. The current inventory is the [command matrix](CLI_COMMAND_MATRIX.md).

**Accept:** A command matrix maps every upstream command to a working Rust command or open task; scripted end-to-end tests use a fake daemon and real IPC. Invalid commands never inject input. Updating delegates to R02 rather than bypassing its validation.

<a id="s05"></a>
### S05 - Add bounded diagnostics and tablet debugging

**Depends on:** S02, D03. **Start:** current capture/logging, new diagnostic stream and export.

Provide raw/decoded report debugger, input/output statistics, device strings, current logs, environment/backend details and diagnostic bundles. Redact device paths, serials and personal filesystem paths by default. Allocate/clone debug data only while subscribed and sample/bound streams independently of pen processing.

**Accept:** A slow debugger cannot stall output; disconnect stops its work; sustained logging stays bounded. Exports contain versions and relevant settings without secrets or raw identifiers by default. User-requested raw capture is explicit and clearly labeled.

## U: Desktop user interface

Sources: UP-UI, UP-SETTINGS, UP-CATALOG. Deliver workflows accessible without editing TOML/JSON. Preserve unsupported saved properties without requiring a raw-settings editor.

<a id="u01"></a>
### U01 - Specify workflows and choose the frontend strategy

**Depends on:** None. **Start:** `src/ui.rs`, upstream controls/windows and platform UX projects.

Map upstream user tasks and error states: output/areas, bindings, filters/tools, tablet switcher, plugin manager, presets, logs/debugger, tray/startup/update and settings recovery. Prototype area/property controls and the daemon connection boundary using the current native UI and one justified alternative if necessary. Record accessibility, distribution, maintenance and idle CPU implications in an architecture decision.

**Accept:** A workflow checklist and reviewed UI/module boundary exist; the prototype demonstrates keyboard navigation and DPI scaling. A screenshot alone is not implementation parity. Keep framework selection separate from moving all backend code.

<a id="u02"></a>
### U02 - Build graphical area and output editors

**Depends on:** U01, C05, S02. **Start:** UI area/display module.

Add monitor and tablet diagrams, numeric and drag editing, size/center/rotation, aspect and usable-area locks, clipping/limiting, relative sensitivity/reset, display selection and conversions. Display units and effective values. Handle disconnected tablets/displays without discarding profiles and route apply through the validated control service.

**Accept:** UI automation/manual scenarios map to the same configuration as equivalent CLI operations; keyboard editing works; drag rounding does not corrupt settings. Test small windows, high DPI, multiple monitors and invalid values without requiring input injection.

<a id="u03"></a>
### U03 - Build device, binding and tool editors

**Depends on:** U01, B04, D05, P06, S02. **Start:** UI profile/tablet/binding modules.

Add tablet selection, output-mode selection, pen/eraser thresholds, side/aux/mouse/wheel bindings, keyboard chords, drag-only and pressure/tilt controls, and tool enable/configuration. Show controls from real device/output capabilities. Reuse descriptors for native and managed types; preserve unsupported saved settings for recovery.

**Accept:** Users configure all built-in input categories without raw text, switch tablets without cross-editing, and receive errors for unavailable output capabilities. Apply/save/cancel states and unsaved changes remain coherent.

<a id="u04"></a>
### U04 - Complete plugin configuration and manager workflows

**Depends on:** U01, P02, P08, S02. **Start:** current plugin list and property editor.

Add typed property controls, defaults, units/tooltips/actions/validation, stage information, enable/disable/reset, catalog search/details, local install, update/uninstall and compatibility diagnostics. Show package version and restart requirements. The owner requested removal of raw JSON/per-property reset UI in 0.14.2 and filter reorder controls in 0.14.3. Retain selected-filter Defaults, saved execution order and unknown settings; do not reintroduce the removed controls as parity work.

**Accept:** Install/configure/update/remove a representative unchanged .NET plugin through the UI with settings preserved. Unsupported platform/version and failed download are intelligible; editing a disabled entry does not run it. Multi-class packages are represented correctly.

<a id="u05"></a>
### U05 - Complete tray, startup, presets and updater behavior

**Depends on:** U01, S01, C04, R02. **Start:** application lifecycle and new tray/startup integration.

Add tray/status, open/close behavior, explicit daemon exit, preset management, startup registration controls, daemon reconnect/watchdog, update availability and recovery. Keep startup opt-in and scoped to this application. Present clear status for no tablet, disconnected daemon and output failure.

**Accept:** Closing a window preserves configured daemon behavior; uninstall/disable removes only this application's registration; failed updates preserve a launchable previous install. UI restart does not create a second injector or lose unsaved profile changes.

<a id="u06"></a>
### U06 - Finish diagnostics, onboarding and accessibility

**Depends on:** U01, S05, U02, U03, U04. **Start:** UI diagnostics/help modules and workflow checklist.

Provide tablet debugger, logs/export, device information/string reader, startup help and links to platform setup. Complete accessible names, tab order, keyboard-only operation, focus/error handling, text scaling and platform conventions. Keep logs/status usable when no device/runtime/plugin is present.

**Accept:** All U01 workflows have executed test evidence on supported UI platforms, including failure states; high DPI/small windows do not hide essential controls. Debugger subscriptions stop on close and idle CPU remains within measured budgets.

## X: Platform support

Sources: UP-PLATFORM, UP-DEVICE, UP-OUTPUT, UP-RELEASE. A cross-compile alone does not close native runtime or hardware checks.

<a id="x01"></a>
### X01 - Define portable OS services and managed hosting

**Depends on:** F02, D03. **Start:** device/output/display/clock/storage abstractions and `src/dotnet.rs`.

Specify supported OS/architecture targets, dynamic library naming/loading, .NET host discovery, platform capability reporting, paths, monotonic time and cancellation. Factor Windows-specific calls behind implementations. Keep native-only profiles usable without .NET on every platform.

**Accept:** Portable core builds without Windows APIs; mocked services run core tests; missing .NET and wrong-architecture plugins give precise diagnostics. Plugin support uses actual baseline platform attributes instead of a guessed universal compatibility flag.

<a id="x02"></a>
### X02 - Implement Linux tablet access and lifecycle

**Depends on:** X01, D02, D05. **Start:** new Linux HID backend and packaging permissions.

Implement enumeration, capabilities, report I/O, feature/output/string operations, hotplug and suspend/resume with appropriate hidraw/backend behavior. Provide udev/setup instructions and conflict/permission diagnostics matching supported upstream workflows. Keep driver access scoped to needed devices.

**Accept:** Native Linux tests and hardware captures cover access, initialization, cancellation, replug and denied permissions. No root-only success is claimed as normal user operation. Test USB and each additional claimed Linux transport.

<a id="x03"></a>
### X03 - Implement Linux output, displays and timing

**Depends on:** X02, O01, B02. **Start:** uinput/evdev output, X11/Wayland display and timer services.

Support mouse/keyboard/tablet/pad output and integrate O04. Match applicable X11 and Wayland monitor enumeration/mapping behavior, permissions, virtual-device capabilities, clock/timer semantics and cleanup. Document compositor-specific limits observed in the baseline and test environment.

**Accept:** Real Linux sessions validate cursor, keys, pressure/pad and topology changes on the claimed display systems. Event traces and application evidence cover Artist Mode. A headless runner's successful build cannot substitute for display/session tests.

<a id="x04"></a>
### X04 - Implement macOS devices, output, displays and timing

**Depends on:** X01, D02, D05, O01, B02. **Start:** new IOKit/HID and application-services backends.

Implement baseline macOS device access, mouse/keyboard output, display mapping, timing and lifecycle with its permission model. Test Apple Silicon and Intel where supported. Preserve pressure/tilt only where the selected upstream-equivalent output contract provides them; do not invent cross-platform Ink behavior.

**Accept:** Native macOS tests cover denied/granted permissions, disconnect/sleep, modifiers, display scaling and plugin hosting. Physical tablet and GUI evidence are required for supported claims; signing/entitlement blockers remain explicit.

<a id="x05"></a>
### X05 - Package and validate each platform frontend/runtime

**Depends on:** X03, X04, U01, R01, R02. **Start:** platform UI adapters, package scripts and CI.

Deliver the chosen GUI strategy on Linux/macOS plus Windows, native daemon/CLI packages, required setup files and architecture-correct managed hosting. Cover upstream-supported distribution formats through prioritized child tasks, signing/notarization where needed, upgrades and uninstall. Share frontend behavior while respecting native platform integration.

**Accept:** Clean-machine install/launch/update/remove tests run on each claimed target; no build-tree absolute path or undeclared SDK dependency is needed. Record which packages include a runtime and which require installation. U06/V04 close remaining interactive evidence.

## V: Validation and final audit

Sources: all capability source groups. See [VALIDATION.md](VALIDATION.md) for common procedures and evidence formats.

<a id="v01"></a>
### V01 - Build the device and report evidence corpus

**Depends on:** F03, F05, D03. **Start:** fixtures and coverage ledger.

Maintain sanitized captures and differential fixtures indexed by configuration/parser/report variant, firmware and transport. Recruit hardware evidence through documented reproducible steps; avoid claiming a generated fixture was captured. Prioritize shared parser families while accounting for device-specific initialization and quirks.

**Accept:** All 339 configurations and 52 parser references have traceable implementation/validation states; each supported parser/transport family has representative hardware evidence before G6. Per-model untested qualifiers remain visible. Fixtures include source/license and omit serials/paths.

<a id="v02"></a>
### V02 - Test failures, races and sustained operation

**Depends on:** F05, B01, C03, P05, D05. **Start:** lifecycle/fuzz/fault harnesses.

Exercise short reads, disappearing endpoints, failed injection, broken plugins, invalid settings, stalled clients, callback races, queue overload, cancellation and reload during held actions. Run reconnect/reload loops and long soaks with handle/thread/timer/memory monitoring. Separate cooperative error recovery from a host-process crash.

**Accept:** No unexplained stuck action, stale generation output, unbounded queue or resource growth in deterministic tests and soaks. Minimized failing seeds are preserved. Crash/restart behavior and any OS-held-input limitation are documented and tested where possible.

<a id="v03"></a>
### V03 - Automate the plugin compatibility matrix

**Depends on:** P01, F05, P09. **Start:** unchanged-plugin integration tests and corpus manifest.

Build a repeatable runner comparing class/settings/input/output/lifecycle behavior with baseline OTD. Test managed runtime versions and declared architectures, dependencies, exception paths, version mismatches and plugin installation/update. Run small deterministic cases in CI and slower/plugin-specific cases on suitable runners.

**Accept:** Results identify exact archive/DLL hashes, API adapters, OS/runtime, command and class coverage. Passing one class cannot mark an entire multi-class package compatible. Blocked external/hardware tests are excluded from pass totals and tracked visibly.

<a id="v04"></a>
### V04 - Validate actual device, desktop and application workflows

**Depends on:** F06, O02, O03, U06, X05. **Start:** hardware guides and per-platform result files.

Test representative tablets/transports, multi-tablet operation, relative mode, drawing pressure/tilt/eraser, held-button disconnect, suspend/resume, multiple monitors/DPI and current GUI/CLI workflows. Compare upstream and Rust with the same plugin/configuration/application settings. Include clean-install and upgrade runs.

**Accept:** Record exact software/hardware context and observations, including every failure. The application matrix has mouse/relative, Windows drawing, Linux Artist Mode and applicable macOS cases. Device reopen messages and replay tests are never presented as live cursor evidence.

<a id="v05"></a>
### V05 - Enforce native and managed performance budgets

**Depends on:** F04, P05, D05, S05. **Start:** benchmark and soak results.

Compare release-mode native and managed paths against frozen Rust/upstream baselines with UI/debugging on/off and single/multiple devices. Profile regressions before changing scheduling, allocation, batching or filter math. Measure third-party allocation/GC separately and test that async buffers stay bounded.

**Accept:** Publish reproducible percentiles, CPU/memory/wakeup and allocation evidence with noise/context. No unreviewed native regression exceeds the adopted budget; any accepted compatibility tradeoff is explicit. End-to-end latency claims require actual timestamped hardware measurement.

<a id="v06"></a>
### V06 - Reconcile every parity claim before completion

**Depends on:** V01, V02, V03, V04, V05. **Start:** capability matrix, ledger, GitHub tracker and pinned source inventory.

Audit every CAP row, work item, parser/configuration record, plugin identity and platform workflow. Recheck upstream release/catalog drift, but retain a clearly named fixed baseline unless a reviewed update is adopted. Remove stale README/release claims and document approved behavior differences with tests.

**Accept:** No missing capability is disguised as not applicable; all gating blockers are resolved or the parity release remains blocked. Publish a readable coverage report distinguishing implemented, automated, hardware-verified and qualified support.

## R: CI, packaging, licensing and release

Sources: UP-RELEASE plus each dependency's actual license. Every release must describe its real scope.

<a id="r01"></a>
### R01 - Expand build, CI and artifact provenance

**Depends on:** F02. **Start:** `.github/workflows/ci.yml`, `scripts/package.ps1`, `scripts/build-compat.ps1`.

Add native platform runners as backends land, compiler/runtime declarations, locked dependencies, portable core and platform tests, DLL/API integration and artifact smoke checks. Produce checksums, dependency inventories and source/build provenance tied to tags. Keep external binary downloads pinned and cached without hiding checksum failures.

**Accept:** Clean runners build the supported matrix; packaged binaries load required plugins outside the checkout; checks fail on wrong versions/architectures or missing assets. Performance and hardware jobs are labeled separately from deterministic CI checks.

<a id="r02"></a>
### R02 - Deliver installation, update, rollback and uninstall

**Depends on:** S01, C04, R01. **Start:** package/setup tooling and update service.

Implement portable and installed workflows, per-user startup/daemon registration, platform setup/permissions, version checks, staged update verification, rollback and clean uninstall. Preserve profiles/plugins across upgrades. Windows/macOS updater behavior and Linux package-manager behavior should match the relevant upstream workflow, with platform-specific child tasks.

**Accept:** Fresh install, migration from 0.4.x, failed/interrupted update, rollback and uninstall leave predictable state. Never remove another driver's files/registration or overwrite OTD settings. Missing signing credentials are a recorded distribution blocker, not fabricated signing.

<a id="r03"></a>
### R03 - Maintain licenses, source provenance and operator documentation

**Depends on:** F03, P01. **Start:** `LICENSE`, `LICENSE.LGPL-3.0`, `NOTICE.md`, managed notices and docs.

Track copied/ported source, database material, managed contracts, native dependencies and plugin binary redistribution. Retain appropriate license texts and corresponding-source/build information. Document device/OS setup, configuration migration, .NET/runtime/plugin limitations, diagnostics and support-report templates. Keep current status separate from the roadmap.

**Accept:** Each shipped component has provenance and applicable notices; source/build instructions match the release; no third-party binary is redistributed solely because it was downloaded for a test. License changes receive explicit review.

<a id="r04"></a>
### R04 - Publish the full-parity release only after all gates pass

**Depends on:** V06, R02, R03, U05, S04, P10.

Freeze the release candidate, complete source/compatibility/performance/hardware audits, package every claimed platform, verify downloads/checksums and publish the final coverage report with limitations. Assign the release version based on actual stability and compatibility; do not pre-label an incomplete Windows milestone as 1.0/full parity.

**Accept:** G0-G6 are closed with evidence, repository and tag match artifacts, source/licenses are available and rollback works. If a blocking gap remains, publish an accurately scoped intermediate release and keep this task open.
