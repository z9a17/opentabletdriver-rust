# Parity implementation audit, 2026-10-07

Baseline: OpenTabletDriver 0.6.7, revision `736003ed72c8bbb28033b039d5a0bb76c344145c`.
This audit separates available implementations from proof of full compatibility.
The current device catalog remains a separate snapshot; this work does not advance
the pinned device, parser or plugin baseline.

## Changes in this branch

| Area | Added behavior | Remaining boundary |
| --- | --- | --- |
| Settings and bindings | Mouse/puck buttons, pressure percentage thresholds, drag-only pen bindings, pressure/tilt suppression, native scroll bindings and Mouse Scroll Up/Down stores | Extension binding DLLs and preset bindings remain open; imported unsupported stores retain diagnostics |
| Windows panel | Tools tab with typed settings, separate Filters list, pen policies, Mouse Settings and pagination for all 32 mouse buttons | Interactive DPI/theme and hardware behavior have not been exercised |
| Console | Active-profile getters/setters, guarded settings saves, preset apply/save-active and bounded v2 JSON stdio | This is the Rust control protocol; unchanged upstream StreamJsonRpc clients do not work |
| Plugin hosting | Exact-Type service lookup, Windows virtual-screen injection, aligned settings/tablet/tool lifetime and timer provider support | Driver/device/input providers, unchanged output-mode/binding/parser extensions and a complete binary corpus remain open |
| Plugin installation | Bounded archive preflight/extraction and full repository identity in catalog and installed-plugin workflows | Archive integrity and metadata eligibility do not establish plugin execution compatibility |
| macOS | Indexed USB descriptor matching and initialization, explicit device-string inspection, layout-change detection and native scroll output | Native permissions, transport, display and output behavior remain unverified |
| Linux | High-resolution wheel output and binding resource discovery across pen/auxiliary/mouse/wheel groups | Native desktop, uinput and hardware behavior remain unverified |

## Full parity still requires implementation

1. **Unchanged plugin contracts (P06-P09).** Filters and tools have a managed host,
   but custom output modes, bindings/state bindings, report-parser extensions,
   device hubs and driver/platform providers still need their real contracts.
   Every eligible plugin package needs a complete class inventory and execution
   evidence for the exact unchanged DLL. A native replacement is not that evidence.
2. **Multiple active devices (D05/C02).** Independent stored profiles and a panel
   tablet selector exist. Concurrent independently owned device sessions and their
   action/output lifetimes remain open.
3. **Upstream control compatibility (S03-S04).** The bounded Rust daemon protocol
   and its clients exist. Upstream RPC framing, subscriptions and original clients
   remain incompatible; console command coverage is tracked in
   [CLI_COMMAND_MATRIX.md](CLI_COMMAND_MATRIX.md).
4. **Cross-platform desktop services (X03-X05).** Linux/macOS CLI backends are
   available. Their GUI, daemon integration and unchanged .NET plugin hosting are
   still missing; macOS also lacks drawing/Artist Mode output.
5. **Remaining transport and report integration (D04-D09/O05).** WinUSB,
   Bluetooth/wireless variants and device-specific touch/analog extension behavior
   need implementation and qualified captures.

## Validation boundary

The owner has no native Linux or macOS test machines available for this work.
Cross-compiling a package checks its source and linked artifact, not native
permissions, input delivery, application behavior or tablet hardware. Those
checks stay open. No driver, GUI, game, OBS session or live input is launched.

Regression fixtures are added for changed semantics. The owner-requested
format/clippy/test/build-check suites are not run; compiling the actual manual
test package and release artifacts is a separate packaging requirement.
The Windows manual build is installed by `scripts/update-test-build.py`, with
its exact revision recorded in `E:/OTD RUST TEST/data/TEST-BUILD.json`.

Historical evidence is retained. This unmerged work does not turn a capability,
device configuration, parser or plugin package into `implemented` or `verified`
in the ledger. Full parity is **not complete**.
