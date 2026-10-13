# Documentation

Use the [project README](../README.md) for downloads and quick setup. These guides describe the current source and released workflows; historical snapshots retain their own version and validation limits.

## Setup and use

- [Windows setup](GETTING_STARTED_WINDOWS.md), [Linux setup](../crates/otd-linux/README.md), [macOS setup](../crates/otd-macos/README.md)
- [Command/profile reference](REFERENCE.md), including the [Windows 0.18 Console migration](REFERENCE.md#windows-console-migration-in-018)
- [Plugins and Windows panel](PLUGINS_AND_UI.md), [desktop collection/preset workflows](parity/DESKTOP_WORKFLOWS_0.18.md)
- [Pen output](PEN_OUTPUT.md), [pen buttons](PEN_BUTTONS.md), [express keys and wheels](EXPRESS_KEYS_AND_WHEELS.md), [relative mode](RELATIVE_MODE.md)
- [Experimental scheduling/affinity settings](EXPERIMENTAL_SETTINGS.md), [disconnected tablet canvas behavior](DISCONNECTED_TABLET_AREA.md)

## Compatibility and evidence

- [0.18 implementation scope and deferred acceptance](parity/IMPLEMENTATION_0.18.md): current source completion against pinned OpenTabletDriver 0.6.7; full verified parity remains open
- [Tablet catalog coverage](TABLET_CATALOG_0.14.5.md): 357 configurations and 53 referenced parser names, with hardware limits
- [Hardware validation checklist](HARDWARE_VALIDATION.md), [evidence ledger](parity/EVIDENCE_LEDGER.md), [validation rules](parity/VALIDATION.md)
- [Performance](PERFORMANCE.md), [input latency](INPUT_LATENCY.md): measurement methods and limits
- [13 October reliability review](BACKGROUND_AUDIT_2026-10-13.md): reader fairness, shutdown, cleanup and offline regression results
- [0.18 release notes](RELEASE_0.18.0.md), [porting status and historical release snapshots](PORTING_STATUS.md)

## Development

- [Full parity plan](FULL_PARITY_PLAN.md), [work items](parity/WORK_ITEMS.md), [agent handoff](parity/AGENT_HANDOFF.md)
- [Behavior contracts](parity/BEHAVIOR_CONTRACTS.md), [capability matrix](parity/CAPABILITY_MATRIX.md), [source inventory](parity/upstream-inventory.json)
- [Release/package procedure](RELEASING.md), [Unix runtime implementation](parity/UNIX_RUNTIME_COMPLETION.md)

The [0.17.1 audit](parity/PARITY_AUDIT_2026-10-07.md) and [original implementation plan](IMPLEMENTATION_PLAN.md) are historical. Consult the current 0.18 implementation report before treating an older missing-feature list as current.
