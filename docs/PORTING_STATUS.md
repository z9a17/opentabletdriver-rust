# OpenTabletDriver Rust port status

The long-term goal is broader OpenTabletDriver functionality with efficient native Rust report processing. The [original implementation plan](IMPLEMENTATION_PLAN.md) describes the first PTH-660 milestone; its exclusions are historical milestone boundaries, not the final project scope.

The [full parity roadmap](FULL_PARITY_PLAN.md) now defines the complete target against OpenTabletDriver v0.6.7, with [65 scoped tasks](parity/WORK_ITEMS.md), a [capability/source matrix](parity/CAPABILITY_MATRIX.md), a reproducible upstream inventory and [GitHub tracking](parity/GITHUB_TRACKING.md). The 0.4.1 planning release changes documentation/tooling and the package version; driver functionality and plugin compatibility remain as in 0.4.0. 0.5.0 replaces the control panel with one laid out like OpenTabletDriver's, with light and dark themes; driver behavior and plugin compatibility are unchanged. The table below records implemented behavior, not roadmap completion.

| Capability | Current Rust status | Remaining work |
| --- | --- | --- |
| USB discovery and recovery | Windows PTH-660 pen collection, cancellable reads, reconnect supervisor | Hardware coverage for sleep/wake and repeated reconnects |
| Pen decoding | PTH-660 IntuosV2 0x10/0x1e, position, pressure, proximity, eraser, tilt, rotation | More devices and parsers; real 0x1e capture |
| Absolute mouse output | Areas, rotation, clipping, limiting, display selection and refresh | Broader display/hardware validation |
| Relative mouse output | Per-axis sensitivity, rotation, fractional carry, reset delay, OTD/TOML import | Live hardware validation |
| Bindings | Tip/eraser mapped to left mouse with pressure thresholds | Pen side buttons, express keys, keyboard/mouse actions, wheel/scroll, shared-action ownership |
| Filters | Native tablet-space Radial Follow, native DLL API/sample, unchanged synchronous .NET PreTransform position filters | Async/pixel-space filters and additional report properties |
| Pen output | Mouse injection only; pressure/tilt/eraser are decoded | Native Windows pen/Ink output and application compatibility |
| Device configuration | One fixed USB PTH-660 model | Upstream configuration database, parser selection, documented initialization reports, multi-device support |
| Configuration and UI | Native Windows control panel in OpenTabletDriver's layout: graphical display/tablet area editors, relative settings, filter and plugin property editors, pen thresholds, console, light/dark/high-contrast themes; OTD mapping import, TOML save/load, start/stop/apply, diagnostics | Automatic reload, presets, tray icon, tablet debugger, plugin display names and units from .NET metadata |
| Platforms/transports | Windows USB | Linux/macOS backends, Bluetooth where documented |
| Plugins | Native C ABI and optional in-process .NET compatibility bridge; unchanged RadialFollow DLL verified | Output modes, tools, bindings, async filters, online catalog and automatic plugin-settings migration |

Suggested next increments are button bindings with reliable shared-button ownership, auxiliary collection handling, native pen output, and configuration-driven device support. Each increment needs upstream source references, replay/error tests, and device validation where hardware behavior matters. Preserve fixed-size report/state structures, reuse buffers, avoid report queues and hot-path allocation, and measure CPU work separately from device and OS latency.
