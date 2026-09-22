# OpenTabletDriver Rust port status

The long-term goal is broader OpenTabletDriver functionality with efficient native Rust report processing. The [original implementation plan](IMPLEMENTATION_PLAN.md) describes the first PTH-660 milestone; its exclusions are historical milestone boundaries, not the final project scope.

The [full parity roadmap](FULL_PARITY_PLAN.md) defines the target against OpenTabletDriver v0.6.7, with [65 scoped tasks](parity/WORK_ITEMS.md), a [capability/source matrix](parity/CAPABILITY_MATRIX.md), a reproducible upstream inventory, an [evidence ledger](parity/EVIDENCE_LEDGER.md) and [GitHub tracking](parity/GITHUB_TRACKING.md).

Release 0.5.0 redesigned the control panel, 0.6.0 added tray behavior, and 0.7.0 improved report-thread scheduling and hover/tip handling; see [input latency](INPUT_LATENCY.md). Release 0.7.1 added behavior contracts and nine golden traces without changing pen handling, mapping, clicks or filtering. Release 0.7.2 added validation bookkeeping and CI checks without changing driver behavior. Release 0.7.3 runs unchanged synchronous .NET Pixels/PostTransform position filters after absolute mapping; the original RadialFollow screen-space class passes an integration replay. Release 0.7.4 shows .NET plugin names, property labels, units and tooltips from unchanged DLL metadata in the Windows Filters editor. The table below records implemented behavior, not roadmap completion.

| Capability | Current Rust status | Remaining work |
| --- | --- | --- |
| USB discovery and recovery | Windows PTH-660 pen collection, cancellable reads, reconnect supervisor | Hardware coverage for sleep/wake and repeated reconnects |
| Pen decoding | PTH-660 IntuosV2 0x10/0x1e, position, pressure, In Range and Sense hover flags, eraser, tilt, rotation | More devices and parsers; real 0x1e capture |
| Absolute mouse output | Areas, rotation, clipping, limiting, display selection and refresh | Broader display/hardware validation |
| Relative mouse output | Per-axis sensitivity, rotation, fractional carry, reset delay, OTD/TOML import | Live hardware validation |
| Bindings | Tip/eraser mapped to left mouse with pressure thresholds | Pen side buttons, express keys, keyboard/mouse actions, wheel/scroll, shared-action ownership |
| Filters | Native tablet-space Radial Follow, native DLL API/sample, unchanged synchronous .NET PreTransform filters and Pixels/PostTransform position filters on absolute mouse output | Async filters, zero/multiple outputs, additional report properties and wider stage coverage |
| Pen output | Mouse injection only; pressure/tilt/eraser are decoded | Native Windows pen/Ink output and application compatibility |
| Device configuration | One fixed USB PTH-660 model | Upstream configuration database, parser selection, documented initialization reports, multi-device support |
| Configuration and UI | Native Windows control panel in OpenTabletDriver's layout: graphical display/tablet area editors, relative settings, filter and plugin property editors with .NET names/labels/units/tooltips, pen thresholds, console, light/dark/high-contrast themes, tray icon; driver start with the panel, OTD mapping import, TOML save/load, start/stop/apply, diagnostics | Automatic reload, presets, tablet debugger, complete typed plugin controls/actions/validation |
| Platforms/transports | Windows USB | Linux/macOS backends, Bluetooth where documented |
| Plugins | Native C ABI and optional in-process .NET compatibility bridge; unchanged RadialFollow DLL verified | Output modes, tools, bindings, async filters, online catalog and automatic plugin-settings migration |

Suggested next increments are button bindings with reliable shared-button ownership, auxiliary collection handling, native pen output, and configuration-driven device support. Each increment needs upstream source references, replay/error tests, and device validation where hardware behavior matters. Preserve fixed-size report/state structures, reuse buffers, avoid report queues and hot-path allocation, and measure CPU work separately from device and OS latency.
