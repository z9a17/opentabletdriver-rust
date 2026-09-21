# Capability matrix and source map

Baseline: [OTD v0.6.7](https://github.com/OpenTabletDriver/OpenTabletDriver/tree/736003ed72c8bbb28033b039d5a0bb76c344145c), audited against Rust 0.4.0 on 2026-09-21. This is a gap assessment, not a list of completed work. See the [roadmap](../FULL_PARITY_PLAN.md), [tasks](WORK_ITEMS.md), and [evidence rules](VALIDATION.md).

`Partial` means a subset is implemented. `Missing` means the inspected Rust implementation has no equivalent workflow. `Pending evidence` is independent of implementation status. None of these labels imply a physical test on every device.

## Core and device capabilities

| ID | Upstream capability | Rust 0.4.0 assessment | Work items |
| --- | --- | --- | --- |
| CAP-01 | Configuration database, specifications, parser selection, custom configuration files | Partial: hard-coded PTH-660 and one parser family | D01, D06-D08 |
| CAP-02 | VID/PID plus input/output/feature lengths, device strings, attributes and match precedence | Partial: fixed Windows HID selection | D01-D02 |
| CAP-03 | Feature/output initialization reports and initialization strings | Missing: PTH-660 USB currently needs none in its selected configuration | D02, D09 |
| CAP-04 | Physical endpoint grouping, digitizer plus auxiliary collections | Partial: pen endpoint only | D02, D04-D05 |
| CAP-05 | Multiple tablet models with independent settings and pipelines | Missing | D05, C02 |
| CAP-06 | HID, supported WinUSB endpoints and wireless/Bluetooth variants | Partial: Windows USB HID only | D09, X02, X04 |
| CAP-07 | Hotplug, cancellation, suspend/resume and orderly disposal | Partial; reconnect reopen observed, resumed pen input pending | F06, D05, V02, V04 |
| CAP-08 | Position/pressure/eraser/tilt/proximity/tool/raw report contracts | Partial: PTH-660 values decoded, bridge exposes a subset | D03, P03 |
| CAP-09 | Auxiliary, mouse/puck, analog, wheel and touch reports | Missing from runtime pipeline | D03-D04, B03, O05 |
| CAP-10 | Vendor/parser variants, truncated/unknown reports and parser extensions | Partial: IntuosV2 pen 0x10/0x1e; 0x1e lacks live capture | D06-D08, F05, P07 |
| CAP-11 | Absolute area/rotation/clipping/limiting | Partial: implemented and replay-tested for current model | O01, V04 |
| CAP-12 | Relative sensitivity/rotation/reset behavior | Partial: implemented with fractional carry; live validation pending | O01, F06 |
| CAP-13 | Monitor selection, virtual desktop, DPI/topology changes | Partial: Windows implementation; broader validation pending | O02, X03-X04, V04 |
| CAP-14 | Adaptive tip/eraser/pen bindings, thresholds, drag-only bindings | Partial: limited tip/eraser left-click behavior | B01-B02, B04 |
| CAP-15 | Auxiliary/mouse buttons, keyboard chords, scroll and wheel bindings | Missing | B02-B04 |
| CAP-16 | Preset actions and binding extension types | Missing | B04, C04, P06 |
| CAP-17 | Shared action ownership and reliable release on failure | Partial: current single left-button state; broader ownership needed | B01, V02 |
| CAP-18 | Platform mouse and keyboard output | Partial: Windows absolute/relative mouse only | O02, X03-X04 |
| CAP-19 | Pressure/tilt disable settings, proximity/eraser output, synchronous pointer flush/reset | Missing beyond mouse contact output | O03-O05, P06 |
| CAP-20 | Linux Artist Mode and virtual tablet/pad | Missing | O04, X03 |
| CAP-21 | Windows drawing output via compatible Windows Ink/pen/VMulti plugins | Missing; not upstream core's default mouse mode | O03, P06, V04 |

## Configuration, control, and user interface

| ID | Upstream capability | Rust 0.4.0 assessment | Work items |
| --- | --- | --- | --- |
| CAP-22 | Settings revision, all tablet profiles, tools and serialized plugin stores | Partial: selected PTH-660 import, limited TOML schema | C01-C02 |
| CAP-23 | Preserve disabled entries, order, defaults and unknown settings | Partial: Radial Follow override and skipped filters are explicit differences | C01-C02, P02 |
| CAP-24 | Runtime settings apply/reset and resynchronization | Missing: edits take effect after restart | C03, S02 |
| CAP-25 | Named presets, settings import/export, default paths/portable mode | Partial: single-profile file save/load only | C02, C04, R02 |
| CAP-26 | Area conversion, aspect locks, usable area locks and numeric validation | Missing beyond raw numeric editing | C05, U02 |
| CAP-27 | Persistent daemon separate from GUI and control clients | Missing: worker lives in launching process | S01-S02 |
| CAP-28 | OTD daemon RPC methods, event subscriptions and client reconnect | Missing | S03 |
| CAP-29 | Console load/save/set/get/list/preset/plugin/update/stdio/edit commands | Partial: run/settings/list/displays/capture/plugin diagnostics only | S04 |
| CAP-30 | Logs, diagnostics export, tablet debugger, device-string requests | Partial: bounded panel log and console capture | S05, U06 |
| CAP-31 | Graphical display/tablet area editor with position, size, rotation and monitor selection | Missing: TOML editor | U01-U02 |
| CAP-32 | Output, pen, auxiliary, mouse, wheel, filters and tools editors | Partial: plugin list plus JSON settings | U03-U04 |
| CAP-33 | Attribute-generated plugin properties/actions/tooltips/validation | Missing | P02, U04 |
| CAP-34 | Tablet switcher and device-specific visibility | Missing | D05, U03 |
| CAP-35 | Tray, startup greeter/help, daemon watchdog, update UI | Missing | U05-U06, S01, R02 |
| CAP-36 | Platform-native UX, scaling, keyboard navigation and usable error handling | Partial: native Win32 controls; no cross-platform UX | U01, U06, X05 |

## Plugin and platform capabilities

| ID | Upstream capability | Rust 0.4.0 assessment | Work items |
| --- | --- | --- | --- |
| CAP-37 | Existing .NET assembly discovery, dependencies, supported-platform/ignore metadata | Partial: explicit DLL/type loading with narrow discovery | P01-P02 |
| CAP-38 | Settings defaults/conversion/validation, property and field injection, dependency callbacks | Partial: property settings and PTH-660 TabletReference only | P02, P07 |
| CAP-39 | Full report interfaces, concrete types where needed, raw bytes and report mutations | Partial: reusable position/pressure/eraser adapter; only X/Y returned | P03 |
| CAP-40 | PreTransform and PostTransform ordering, suppression and multiple emissions | Partial: exactly one synchronous PreTransform output | P04 |
| CAP-41 | Async filters, timers, resampling, plugin-owned threads | Missing | P05 |
| CAP-42 | Output-mode, binding, state-binding and tool plugins | Missing | P06 |
| CAP-43 | Report-parser, configuration/provider, device-hub, driver and platform services | Missing | P07 |
| CAP-44 | Install/update/uninstall from local archives and online catalog; metadata version checks | Missing: file selection does not install a package | P08 |
| CAP-45 | Plugin disposal/reload, dependency isolation, failed initialization and missing runtime diagnostics | Partial: basic load/dispose; no general lifecycle compatibility | P02, P05, P08-P09 |
| CAP-46 | Catalog-wide unchanged binaries and manually installed baseline-compatible plugins | Partial: unchanged RadialFollow tablet filter tested | P01, P09, V03 |
| CAP-47 | Linux HID permissions, input/display/timer and desktop behavior | Missing | X01-X03, X05 |
| CAP-48 | macOS device permissions, input/display/timer and desktop behavior | Missing | X01, X04-X05 |
| CAP-49 | Platform installers/packages/autostart/updating/uninstall | Partial: portable Windows x64 ZIP, checksums | R01-R02, X05 |
| CAP-50 | Provenance, license/source obligations, reproducible release checks | Partial: notices and Windows CI already exist | F03, R01, R03-R04 |
| CAP-51 | Device/parser/plugin/report performance and validation evidence | Partial: unit/replay checks and limited older-build hardware evidence | F04-F06, V01-V06 |
| CAP-52 | Rust native plugin ABI (project extension) | Version 1 position-only API and EMA DLL available | P10; extension must not displace CAP-37 through CAP-46 |

## Pinned source map

Use these groups in work-item implementation notes. All OTD links resolve to the same v0.6.7 commit. Inspect the relevant methods, not just class names, before copying semantics.

| Source group | Reference files and directories |
| --- | --- |
| UP-DEVICE | [Driver matching](https://github.com/OpenTabletDriver/OpenTabletDriver/blob/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver/Driver.cs), [input device](https://github.com/OpenTabletDriver/OpenTabletDriver/blob/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver/InputDevice.cs), [device tree](https://github.com/OpenTabletDriver/OpenTabletDriver/blob/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver/InputDeviceTree.cs), [device backends](https://github.com/OpenTabletDriver/OpenTabletDriver/tree/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver/Devices) |
| UP-CONFIG | [configuration data and parsers](https://github.com/OpenTabletDriver/OpenTabletDriver/tree/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.Configurations), [DeviceIdentifier](https://github.com/OpenTabletDriver/OpenTabletDriver/blob/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.Plugin/Tablet/DeviceIdentifier.cs), [TabletConfiguration](https://github.com/OpenTabletDriver/OpenTabletDriver/blob/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.Plugin/Tablet/TabletConfiguration.cs) |
| UP-REPORT | [tablet report interfaces, specifications and parsers](https://github.com/OpenTabletDriver/OpenTabletDriver/tree/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.Plugin/Tablet), [DeviceReader](https://github.com/OpenTabletDriver/OpenTabletDriver/blob/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver/Devices/DeviceReader.cs) |
| UP-PIPELINE | [OutputMode](https://github.com/OpenTabletDriver/OpenTabletDriver/blob/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.Plugin/Output/OutputMode.cs), [pipeline, absolute/relative and async classes](https://github.com/OpenTabletDriver/OpenTabletDriver/tree/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.Plugin/Output) |
| UP-BINDING | [binding implementations and states](https://github.com/OpenTabletDriver/OpenTabletDriver/tree/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.Desktop/Binding), [IBinding](https://github.com/OpenTabletDriver/OpenTabletDriver/blob/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.Plugin/IBinding.cs), [IStateBinding](https://github.com/OpenTabletDriver/OpenTabletDriver/blob/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.Plugin/IStateBinding.cs) |
| UP-OUTPUT | [desktop modes](https://github.com/OpenTabletDriver/OpenTabletDriver/tree/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.Desktop/Output), [pointer contracts](https://github.com/OpenTabletDriver/OpenTabletDriver/tree/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.Plugin/Platform), [platform input implementations](https://github.com/OpenTabletDriver/OpenTabletDriver/tree/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.Desktop/Interop/Input) |
| UP-SETTINGS | [Settings](https://github.com/OpenTabletDriver/OpenTabletDriver/blob/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.Desktop/Settings.cs), [profiles](https://github.com/OpenTabletDriver/OpenTabletDriver/tree/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.Desktop/Profiles), [PresetManager](https://github.com/OpenTabletDriver/OpenTabletDriver/blob/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.Desktop/PresetManager.cs), [area converters](https://github.com/OpenTabletDriver/OpenTabletDriver/tree/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.Desktop/Conversion) |
| UP-PLUGIN | [reflection, setting stores and service manager](https://github.com/OpenTabletDriver/OpenTabletDriver/tree/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.Desktop/Reflection), [attributes](https://github.com/OpenTabletDriver/OpenTabletDriver/tree/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.Plugin/Attributes), [dependency injection](https://github.com/OpenTabletDriver/OpenTabletDriver/tree/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.Plugin/DependencyInjection), [ITool](https://github.com/OpenTabletDriver/OpenTabletDriver/blob/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.Plugin/ITool.cs), [component contracts](https://github.com/OpenTabletDriver/OpenTabletDriver/tree/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.Plugin/Components) |
| UP-CATALOG | [catalog snapshot](https://github.com/OpenTabletDriver/Plugin-Repository/tree/2dfdff1cd77d274eb19c4d359b240167537c4db2), [catalog version/hash metadata logic](https://github.com/OpenTabletDriver/OpenTabletDriver/blob/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.Desktop/Reflection/Metadata/PluginMetadata.cs), [package lifecycle](https://github.com/OpenTabletDriver/OpenTabletDriver/blob/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.Desktop/Reflection/DesktopPluginManager.cs) |
| UP-DAEMON | [daemon settings/pipeline/service orchestration](https://github.com/OpenTabletDriver/OpenTabletDriver/blob/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.Daemon/DriverDaemon.cs), [IDriverDaemon](https://github.com/OpenTabletDriver/OpenTabletDriver/blob/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.Desktop/Contracts/IDriverDaemon.cs), [RPC](https://github.com/OpenTabletDriver/OpenTabletDriver/tree/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.Desktop/RPC) |
| UP-CLI | [command declarations](https://github.com/OpenTabletDriver/OpenTabletDriver/blob/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.Console/Program.cs), [command implementations](https://github.com/OpenTabletDriver/OpenTabletDriver/tree/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.Console) |
| UP-UI | [control panel](https://github.com/OpenTabletDriver/OpenTabletDriver/blob/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.UX/Controls/ControlPanel.cs), [area and property controls](https://github.com/OpenTabletDriver/OpenTabletDriver/tree/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.UX/Controls), [plugin manager, debugger, greeter and dialogs](https://github.com/OpenTabletDriver/OpenTabletDriver/tree/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.UX/Windows), [tray](https://github.com/OpenTabletDriver/OpenTabletDriver/blob/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.UX/TrayIcon.cs) |
| UP-PLATFORM | [display/input/timer/sleep interop](https://github.com/OpenTabletDriver/OpenTabletDriver/tree/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.Desktop/Interop), [native platform APIs](https://github.com/OpenTabletDriver/OpenTabletDriver/tree/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.Native), [system driver diagnostics](https://github.com/OpenTabletDriver/OpenTabletDriver/tree/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver/SystemDrivers) |
| UP-RELEASE | [updater](https://github.com/OpenTabletDriver/OpenTabletDriver/tree/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.Desktop/Updater), [packaging](https://github.com/OpenTabletDriver/OpenTabletDriver/tree/736003ed72c8bbb28033b039d5a0bb76c344145c/eng), [tests](https://github.com/OpenTabletDriver/OpenTabletDriver/tree/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.Tests), [license](https://github.com/OpenTabletDriver/OpenTabletDriver/blob/736003ed72c8bbb28033b039d5a0bb76c344145c/LICENSE) |

## Regenerating the inventory

The generator reads source JSON and file names; it does not download or execute plugins, open devices, or claim compatibility. It requires PowerShell 7, Git, and ripgrep. Source checkouts must be clean. Configuration `git_blob` values identify Git content independently of checkout line endings. Catalog archive SHA-256 values are copied from metadata and are not newly verified downloads.

```powershell
git clone --depth 1 --branch v0.6.7 https://github.com/OpenTabletDriver/OpenTabletDriver.git target/parity-upstream-0.6.7
git clone https://github.com/OpenTabletDriver/Plugin-Repository.git target/parity-plugin-repository
git -C target/parity-plugin-repository checkout --detach 2dfdff1cd77d274eb19c4d359b240167537c4db2
pwsh -File scripts/parity-inventory.ps1 -UpstreamRoot target/parity-upstream-0.6.7 -CatalogRoot target/parity-plugin-repository
```

If the directories already exist, verify their remotes, revisions and clean status instead of cloning over them. The generator records actual commit IDs. A different snapshot is a baseline change requiring review, not an automatic inventory refresh. The metadata eligibility calculation mirrors `PluginMetadata.IsSupportedBy(0.6.7.0)`; it does not infer OS support, download availability, dependency compatibility, or which plugin classes are exported.

F03 extends this source inventory with a separate evidence ledger. Keep source facts separate from implementation/test status so regeneration cannot erase claims, hardware qualifications, or blockers.
