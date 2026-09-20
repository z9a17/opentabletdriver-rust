# OpenTabletDriver Rust port status

The long-term goal is broader OpenTabletDriver functionality with efficient native Rust report processing. The [original implementation plan](IMPLEMENTATION_PLAN.md) describes the first PTH-660 milestone; its exclusions are historical milestone boundaries, not the final project scope.

| Capability | Current Rust status | Remaining work |
| --- | --- | --- |
| USB discovery and recovery | Windows PTH-660 pen collection, cancellable reads, reconnect supervisor | Hardware coverage for sleep/wake and repeated reconnects |
| Pen decoding | PTH-660 IntuosV2 0x10/0x1e, position, pressure, proximity, eraser, tilt, rotation | More devices and parsers; real 0x1e capture |
| Absolute mouse output | Areas, rotation, clipping, limiting, display selection and refresh | Broader display/hardware validation |
| Relative mouse output | Per-axis sensitivity, rotation, fractional carry, reset delay, OTD/TOML import | Live hardware validation |
| Bindings | Tip/eraser mapped to left mouse with pressure thresholds | Pen side buttons, express keys, keyboard/mouse actions, wheel/scroll, shared-action ownership |
| Filters | Native tablet-space Radial Follow | Other built-in/community filters and a Rust extension design |
| Pen output | Mouse injection only; pressure/tilt/eraser are decoded | Native Windows pen/Ink output and application compatibility |
| Device configuration | One fixed USB PTH-660 model | Upstream configuration database, parser selection, documented initialization reports, multi-device support |
| Configuration and UI | Startup OTD profile import, independent TOML, console diagnostics | Reload, GUI, IPC/control surface, profile management |
| Platforms/transports | Windows USB | Linux/macOS backends, Bluetooth where documented |
| Plugins | One statically compiled filter port | Explicit compatibility scope; existing .NET plugins cannot be loaded directly into Rust |

Suggested next increments are button bindings with reliable shared-button ownership, auxiliary collection handling, native pen output, and configuration-driven device support. Each increment needs upstream source references, replay/error tests, and device validation where hardware behavior matters. Preserve fixed-size report/state structures, reuse buffers, avoid report queues and hot-path allocation, and measure CPU work separately from device and OS latency.
