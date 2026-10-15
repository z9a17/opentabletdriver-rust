# OpenTabletDriver 0.6.8-rc1 adoption

The reviewed adoption scopes are implemented in source on
`parity/upstream-068rc1`, based on Rust main
`670486b038203eedd59d723176b2398a5dcfdbf4`. The reference is
[upstream v0.6.8-rc1, commit `126f4828564abb673d39e97a093fd815aab335c2`](https://github.com/OpenTabletDriver/OpenTabletDriver/tree/126f4828564abb673d39e97a093fd815aab335c2).
The owner requested implementation on 2026-10-15. Source completion is separate
from package delivery, unchanged-plugin compatibility and live acceptance.

## Source and runtime composition

| Component | Source and modifications |
| --- | --- |
| Embedded tablet catalog | Exact RC configuration blobs: 359 configurations, 54 referenced native parser types; previous current catalog was 357/53 at a126f7b |
| Plugin SDK | Actual RC source in `compat/UpstreamPlugin`, assembly version 0.6.8; compatibility default interface bodies and documented restored older API members |
| Configurations and Native | Exact NuGet 0.6.8 packages identifying the same RC source commit above |
| Core, Desktop, UX and Console | Original 0.6.7 source baseline at 736003ed, retained native-host adaptations, and selected RC changes; assembly version 0.6.8 and net10.0 |
| GTK and macOS launchers | Retained original frontend launchers retargeted to net10.0/0.6.8, sharing the selectively updated UX; existing artwork overrides remain |

Managed components require .NET 10; the runtime is not bundled. Project and lock
files record the exact dependency closure. The copied older host sources are
not represented as a complete RC source replacement. Copyrights, LGPL notices
and the project's separately licensed artwork/filter attribution remain in
[NOTICE.md](../../NOTICE.md) and each compatibility component's provenance file.

The Plugin SDK's new `IDeviceEndpoint` initialization members have default bodies
that fail explicitly for legacy direct implementors. The new
`IOutputMode.DisableRotation` defaults to false and rejects true when a legacy
mode lacks an implementation. The source host also restores removed legacy
constructor/setter behavior documented in
[SDK provenance](../../compat/UpstreamPlugin/PROVENANCE.md). These changes are
intended to preserve unchanged older binaries; a loading or behavior pass is
not claimed merely from source compatibility.

## Implemented scopes

### UP068-UI: areas, options and diagnostic editors

- Aspect locking derives the tablet size from the current final display ratio,
  clamps the dependent dimension and recomputes its counterpart together
  ([#5101](https://github.com/OpenTabletDriver/OpenTabletDriver/pull/5101)).
  Full/Quarter display changes are treated as compound operations. For example,
  changing a 1920x1080 display with a 200x112.5-mm tablet area to 3840 pixels wide
  fits a 224-mm-wide tablet at 224x63, preserving the display ratio.
- Full/Quarter tablet commands fit the largest rectangle inside the rotated
  physical bounds, including extreme portrait ratios and cardinal angles
  ([#5102](https://github.com/OpenTabletDriver/OpenTabletDriver/pull/5102)).
  Both native Windows and copied Eto handlers use the corrected behavior.
  No selected detected tablet means no preview bounds/context actions; display
  edits preserve the pending tablet mapping while disconnected.
- Native Pen Output exposes `Disable rotation` beside pressure and tilt. Original
  JSON/TOML configuration and managed mode options carry the same setting.
- Native and copied bulk string readers skip VID 10429/index 200, a common DFU
  request, unless the unchecked dangerous-string option is explicitly enabled
  ([#4841](https://github.com/OpenTabletDriver/OpenTabletDriver/pull/4841)).
  Each dump fixes its device IDs and opt-in for the operation. Deliberate
  single-index requests retain their explicit request behavior.
- Copied binding editors isolate edit drafts and cancellation snapshots, and
  restore original property values when switching back to the initial type
  ([#5148](https://github.com/OpenTabletDriver/OpenTabletDriver/pull/5148)).
  Native Windows already used private drafts.
- Unvalidated string placeholder metadata appears as a cue in native managed
  settings and copied Eto editors; it never becomes a saved/default property
  value ([#5132](https://github.com/OpenTabletDriver/OpenTabletDriver/pull/5132)).

The optional rotated-background presentation from
[#5098](https://github.com/OpenTabletDriver/OpenTabletDriver/pull/5098) is
intentionally deferred. The current appearance remains while the sizing and
aspect corrections are adopted. Native Windows retains its coherent signed
desktop coordinate convention; the upstream monitor-offset change is not copied
blindly into that separate coordinate path.

### UP068-DEVICE: catalog, input and rotation

[The device adoption record](../TABLET_CATALOG_0.6.8_RC1.md) details exact catalog
provenance, Parblo/Silvercrest additions, Wacom auxiliary corrections, XP-Pen
status suppression, stationary wheel accumulation, WinUSB fastboot/Quest
exclusions, Linux Digimend naming, and copied parser/detection fixes.

Raw rotation uses the declared `MinRotation`/`MaxRotation` range, with full signed
32-bit values across the managed graph. Native pen packets preserve orientation
and can emit a rotation-only change. Capable backends receive their platform
units: Windows synthetic pen 0..359 degrees, Linux Artist Mode `ABS_Z` at
0..65535, and macOS tablet-pointer rotation in degrees. Ordinary Windows/Linux
mouse APIs do not acquire a rotation channel, and this does not add a macOS
Artist Mode. The position-only native filter DLL ABI remains unchanged.

### UP068-PLUGIN: SDK and report pipeline

[The managed adoption record](UP068_PLUGIN.md) describes synchronous Internal
filters, early pressure admission before ordinary filters can retain reports,
duplicate/retained report identities, output-mode integration, dependency
injection, async frequency bootstrap, rotation and placeholder metadata.

Asynchronous Internal filters are explicitly unsupported. Late or foreign
Internal emissions use the graph fault/held-output cleanup, and late custom-mode
entry through Internal elements requests a mode restart. Ordinary
PreTransform/PostTransform background continuation remains supported with owned
snapshots. Native-only profiles retain their raw contact/pressure behavior.
Implementation of the reviewed scope does not claim arbitrary Internal-filter
or complete unchanged-plugin ecosystem compatibility.

### UP068-INTEGRATION: control, console and packaging

- Windows `getbindings` and copied Unix Console getters include wheel buttons,
  clockwise/counterclockwise assignments and thresholds
  ([#4781](https://github.com/OpenTabletDriver/OpenTabletDriver/pull/4781)).
- Windows panel clients request optional daemon build information separately
  from the strict protocol-v2 status schema. A version/source-commit mismatch,
  or an older daemon unable to confirm its build, produces a warning while
  preserving the connection. The check repeats for a new daemon instance;
  older panels are not forced to decode added status fields
  ([#4834](https://github.com/OpenTabletDriver/OpenTabletDriver/pull/4834)).
- Explicit Windows Detect reloads the complete configuration override set.
  Invalid reloads retain the previous database and existing workers, and return
  an error; valid reloads update discovery without silently restarting input
  ([#4947](https://github.com/OpenTabletDriver/OpenTabletDriver/pull/4947)).
- The managed macOS `Equal` mapping uses the normal equals key, as native macOS
  already did ([#4815](https://github.com/OpenTabletDriver/OpenTabletDriver/pull/4815)).
- Projects, dependency locks, runtime paths, package provenance and notices
  describe the net10.0/0.6.8 composition. Native Windows already exposes the
  presets folder and checks SendInput acceptance. Intel and Apple Silicon Mac
  packages already existed before this adoption; their presence is not new
  runtime or hardware evidence.

## Validation and handoff

Source implementation and cross-review are complete for the scopes above.
The following offline validation passed on 2026-10-15:

- Windows release workspace compilation, plus managed bridge, Gtk, macOS Eto,
  Console, archive helper and native daemon-launcher builds. Existing Rust
  dead-code/unused-import and copied-host nullability warnings remain.
- 22 focused native regressions: aspect sizing (3), pen rotation state (1),
  pressure admission (1), stationary wheel accumulation (1), current parser
  fixtures (9), Windows rotation packets (1), daemon-version warnings (1),
  protected string-dump policy (1), Linux rotation frames (1), pinned catalog
  blob/loading inventory (1), referenced parser coverage (1), and plugin
  eligibility (1). Only the named synthetic/pure cases ran.
- The isolated managed `GraphBench ... upstream068` probe, using a separately
  built .NET 8/SDK 0.6.7 fixture DLL. It checks 532 original public/protected
  SDK members, legacy direct implementors and constructors/setters, Internal
  ordering, duplicate/retained pressure on native and managed continuations,
  async/filter-state injection, and rotation contracts.
- Linux x64 and macOS arm64 Rust target compilation checks, using the explicit
  rustup compiler (the system PATH compiler has only the Windows target).
- The Windows managed package closure: 79 runtime/dependency files, restored
  in locked mode, including the source SDK and its transitive dependencies.
- The historical parity ledger validator. This is metadata validation only.

The broad test, formatting and clippy suites were not run. No interactive UI,
physical tablet, live output, application, latency or native Unix runtime
acceptance was exercised. The synthetic DLL fixtures do not qualify the full
third-party plugin corpus. Optional background-rotation presentation remains
deferred; asynchronous Internal filters are explicitly rejected as above.

The original 0.6.7 inventory/acceptance ledger remains historical: its 339
configurations, 52 parser types and 98 catalog records must not be confused with
acceptance of the new 359-configuration catalog. Validating that historical
ledger does not execute the new fixtures or certify the RC additions.

Keep the integration PR unmerged. Stage the Windows package in
`E:/OTD RUST TEST`, preserving destination-owned files/settings and recording
source provenance. No active driver, app, game or device is to be started,
stopped or probed for this adoption. No release publication or GitHub Actions
is authorized by this handoff. Manual tablet/application testing remains with
the owner.
