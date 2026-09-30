# Native driver feel investigation

Observed saved settings on 2026-09-30, with source review only. No active driver, HID device, input injection, UI, plugin execution or userdata modification occurred. No tests, format, Clippy or build checks ran in this subtask. The release integrator owns compilation.

## Main finding

The saved Rust and original OpenTabletDriver profiles use different active Radial Follow parameters and different tablet/display areas. This is a direct explanation for different output. It does not prove which difference caused the user's physical sensation.

Rust's saved driver.toml has one active built-in tablet-space filter with the original defaults. Its disabled managed tablet-space entry retains the user's custom original settings. Both managed Radial Follow entries are disabled. Original OTD settings.json enables the custom tablet-space settings and disables the screen-space filter.

| Property | Active Rust native | Active original OTD tablet-space |
| --- | ---: | ---: |
| OuterRadius, mm | 1.0 | 0.7039 |
| InnerRadius, mm | 0.0 | 0.302 |
| SmoothingCoefficient | 0.95 | 0.302 |
| SoftKneeScale | 1.0 | 0.603 |
| SmoothingLeakCoefficient | 0.0 | 0.201 |
| Tablet area Width, mm | 85.0 | 85.0 |
| Tablet area Height, mm | 47.8125 | 47.8125 |
| Tablet area X, mm | 110.0 | 110.0 |
| Tablet area Y, mm | 25.3125 | 23.90625 |
| Display Width, px | 2560.0 | 2560.0 |
| Display Height, px | 1440.0 | 1440.0 |
| Display X, px | 1280.0 | 1285.6917 |
| Display Y, px | 720.0 | 720.0 |
| Clipping | true | false |
| Area limiting | false | false |
| Eraser enabled | false | true |

The higher Rust smoothing coefficient slows descent towards the inner radius according to the original plugin's tooltip. Original settings have a 0.302 mm deadzone, while the active Rust settings have none. Original settings also enable leak, changing the curve outside the outer radius. The center changes shift cursor positioning by about 42.35 px vertically and 5.69 px horizontally at these dimensions. Equal area sizes mean equal nominal sensitivity, but the centers and edge behavior differ.

Rust tip threshold 82 raw matches the original 1 percent activation threshold for PTH-660 max pressure 8191. This does not explain movement smoothing. Original eraser is enabled while Rust's is disabled.

Read-only sources were %LOCALAPPDATA%/OpenTabletDriverRust/driver.toml, settings revision 11, modified 2026-09-30 00:07:22 UTC, and %LOCALAPPDATA%/OpenTabletDriver/settings.json, modified 2026-09-29 16:03:32 UTC. Saved files establish persisted configuration, not any unsaved UI state or the exact currently running pipeline.

## Native algorithm versus original source

Read original AbstractQbit release 0.3.0 source through GitHub API and the pinned OpenTabletDriver 0.6.7 source at commit 736003ed72c8bbb28033b039d5a0bb76c344145c from the local upstream checkout.

The native core and original tablet-space implementation share the same five defaults, property clamps, radial curve, f64/double scalar calculations, f32/Vector2 report positions, tablet millimetre conversion and 50 ms timeout/nonfinite reset rule. Rust preserves fractional report positions after filtering. No meaningful curve mismatch was established. Floating-point implementation details can differ by rounding; no runtime equivalence claim follows from source review.

Tablet-space is PreTransform and uses millimetres. Screen-space is Pixels after mapping and uses pixels, with default OuterRadius 5.0 instead of 1.0. At this area mapping, 1 mm corresponds to about 30.12 px, so 1 mm and 5 px represent different smoothing distances. Changing variants with the same numeric properties does not preserve smoothing behavior.

For PTH-660 IntuosV2, raw X/Y byte layouts and mm scaling agree with the pinned parser. Upstream processes every IntuosV2 positional report. Rust suppresses movement when neither In Range nor Sense is present. Rust also rejects malformed values outside the configured digitizer/pressure range. These are documented behavior differences, not evidence that normal in-range smoothing differs.

Original Radial Follow resets based on elapsed time when Consume reaches the plugin. Native Rust uses the report processing timestamp captured by the owner. Both use a 50 ms boundary. A backlog or differently timed pipeline can change reset timing near that boundary; no live timing evidence was collected.

## Other source-confirmed differences

The native built-ins always precede all DLL PreTransform filters. Original OTD preserves the configured order of PreTransform entries. This matters for multiple active filters. The currently saved original PTH-660 profile has only Radial Follow active, so the evidence does not attribute this symptom to ordering.

The Rust mapper uses f64 transformation and rounds normalized output with screen dimension minus one. Upstream WindowsAbsolutePointer uses f32 position divided by screen dimension divided by 65535, then truncates. The Rust clipping endpoint is one pixel before the original inclusive far-edge coordinate. Existing behavior contracts track these as BC-10, BC-11 and BC-13, and golden fixtures assert the current integer coordinates. These differences can alter final pixel placement, but are not evidence of an altered Radial Follow curve.

The integrator chose to leave mapper and curve behavior unchanged in this release. A normalization change requires deliberate fixture and documentation updates. Matching the current saved smoothing parameters is the first useful comparison.

## Existing path to matching settings

Settings > Import OpenTabletDriver settings already imports the enabled tablet-space Radial Follow values through Profile::load_connected and radial_settings. It also imports the mapping and supported thresholds. The import handler replaces the entire editor profile, so old built-in defaults and old disabled managed rows do not survive the import. It sets the editor dirty and requires Save or Apply to persist or run the new configuration. The original OTD source file remains unchanged.

For the original PTH-660 profile observed here, import would produce exactly the custom native Radial Follow parameters above, the original display/tablet centers and clipping disabled. No other original filter is enabled, so no enabled filter is skipped in this particular profile. The code warns when other enabled filters cannot be imported as runnable native filters.

To compare only smoothing while keeping the user's chosen mapping, enter native values OuterRadius 0.7039, InnerRadius 0.302, SmoothingCoefficient 0.302, SoftKneeScale 0.603 and SmoothingLeakCoefficient 0.201. The existing disabled managed tablet entry already stores those numbers. Running the native entry and the managed tablet-space entry together is rejected by duplicate_radial_follow/profile validation, because it would smooth twice.

The UI import enumerates connected tablets before choosing the OTD profile. This path was inspected in source only and was not invoked in the investigation.

## Source links

- [RadialFollowCore 0.3.0](https://github.com/AbstractQbit/AbstractOTDPlugins/blob/0.3.0/RadialFollow/RadialFollowCore.cs)
- [Tablet-space adapter and defaults](https://github.com/AbstractQbit/AbstractOTDPlugins/blob/0.3.0/RadialFollow/RadialFollowSmoothingTabletSpace.cs)
- [Screen-space adapter and defaults](https://github.com/AbstractQbit/AbstractOTDPlugins/blob/0.3.0/RadialFollow/RadialFollowSmoothingScreenSpace.cs)
- [Upstream absolute mapping](https://github.com/OpenTabletDriver/OpenTabletDriver/blob/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.Plugin/Output/AbsoluteOutputMode.cs)
- [Upstream Windows absolute normalization](https://github.com/OpenTabletDriver/OpenTabletDriver/blob/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.Desktop/Interop/Input/Absolute/WindowsAbsolutePointer.cs)
- [Upstream IntuosV2 report](https://github.com/OpenTabletDriver/OpenTabletDriver/blob/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.Configurations/Parsers/Wacom/IntuosV2/IntuosV2Report.cs)

Local source evidence is in crates/otd-core/src/radial_follow.rs, pipeline.rs, mapping.rs, config.rs, src/plugins/graph.rs, src/ui/model.rs and docs/parity/BEHAVIOR_CONTRACTS.md. The latter already records limits from earlier differential fixture checks. Those earlier checks were not rerun.

## Investigation limit

The diagnosing-bugs skill normally requires an executable reproduction of the exact symptom before hypotheses and a regression check afterwards. The task expressly forbids driver/plugin/HID/input execution, and repository policy skips automated checks. This subtask therefore audits persisted settings and source behavior, and cannot confirm a physical-feel diagnosis or verify a physical-feel fix.

Memory used for historical context only: MEMORY.md lines 481-507 and rollout_summaries/2026-09-15T18-47-31-MMYG-opentabletdriver_rust_pth660_radial_follow_port.md. Rollout ID 01a0a665-4b83-7f82-b84a-253f09801e92. Current settings and source observations were independently refreshed.

## Independent UI diff review

Reviewed the root's current app, client, command, debugger, layout, model, paint, canvas and tray changes, with plugin manager interactions, through static source and diff inspection only. No UI or driver execution and no compilation occurred in this subtask.

Found a compile error in the first stop_for_close implementation: Reply::Ack does not exist. The actual StopIf handler returns Reply::Stopped or Reply::Status. Reported it to the integrator, who corrected the match to those variants.

Also reported that treating every inactive status as successful cleanup includes DriverState::Failed. The daemon sets Failed after worker joins when cleanup_error exists. Source therefore proves worker input processing has ended, but cannot prove every OS release succeeded. The integrator owns the final failure reporting behavior.

Reviewed load_active_profile against Save and initial daemon snapshots. Save updates the local settings_revision before restart; matching serialized snapshots skip all rebuilding, preserving the selected filter, property page, scroll position and disabled native stash. A different active configuration still replaces a clean editor. Dirty or invalid local controls prevent replacement in the snapshot caller. Changed configurations restore a selected plugin by kind, path and type_name, then clamp the row and page through the existing list/layout code. No further defect was established in this path.

Reviewed removed raw JSON, per-property default and main Start/Stop controls for remaining source references. The integrator removed the stale StartStop paint case. Manager close exists and destroys its window without retaining the manager borrow. Debugger palette/font ownership avoids dangling references to main-window fonts; its close path drops the slot before DestroyWindow, and theme/DPI Win32 calls occur after the slot borrow ends. Cancellation and dropped receivers stop debugger polling emissions. Runtime behavior remains unverified.
