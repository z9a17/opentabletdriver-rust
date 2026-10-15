# Documentation

Use the [project README](../README.md) for downloads and quick setup. These guides describe the current source and released workflows; historical snapshots retain their own version and validation limits.

## Setup and use

- [Windows setup](GETTING_STARTED_WINDOWS.md), [Linux setup](../crates/otd-linux/README.md), [macOS setup](../crates/otd-macos/README.md)
- [Command/profile reference](REFERENCE.md), including the [Windows 0.18 Console migration](REFERENCE.md#windows-console-migration-in-018)
- [Plugins and Windows panel](PLUGINS_AND_UI.md), [desktop collection/preset workflows](parity/DESKTOP_WORKFLOWS_0.18.md)
- [Pen output](PEN_OUTPUT.md), [pen buttons](PEN_BUTTONS.md), [express keys and wheels](EXPRESS_KEYS_AND_WHEELS.md), [relative mode](RELATIVE_MODE.md)
- [Experimental scheduling/affinity settings](EXPERIMENTAL_SETTINGS.md), [disconnected tablet canvas behavior](DISCONNECTED_TABLET_AREA.md)

## Compatibility and evidence

- [Current source and evidence map](parity/CURRENT_STATUS.md), separating selected RC components, offline results and historical acceptance
- [Autonomous compatibility and recovery follow-up](parity/AUTONOMOUS_FOLLOWUP_2026-10-15.md), including the [unchanged filter corpus](parity/OFFLINE_PLUGIN_CORPUS_2026-10-15.md)
- [0.6.8 RC adoption and deferred acceptance](parity/UPSTREAM_0.6.8_RC1.md), building on the [0.18 implementation scope](parity/IMPLEMENTATION_0.18.md); full verified parity remains open
- [Current tablet catalog coverage](TABLET_CATALOG_0.6.8_RC1.md): 359 configurations and 54 referenced parser names, with hardware limits
- [Historical hardware checklist and test scenarios](HARDWARE_VALIDATION.md): includes early-release limitations; use the current implementation report for present capability scope
- [Evidence ledger](parity/EVIDENCE_LEDGER.md), [validation rules](parity/VALIDATION.md)
- [Performance](PERFORMANCE.md), [input latency](INPUT_LATENCY.md): measurement methods and limits
- [13 October reliability review](BACKGROUND_AUDIT_2026-10-13.md): reader fairness, shutdown, cleanup and offline regression results
- [15 October driver overhead review](PERFORMANCE_IMPROVEMENTS_2026-10-15.md): managed graph, snapshot, wake and debugger improvements with offline measurements
- [0.19.1 release notes](RELEASE_0.19.1.md), [0.19.0 release notes](RELEASE_0.19.0.md), [porting status and historical release snapshots](PORTING_STATUS.md)

## Development

- [Full parity plan](FULL_PARITY_PLAN.md), [work items](parity/WORK_ITEMS.md), [agent handoff](parity/AGENT_HANDOFF.md)
- [Behavior contracts](parity/BEHAVIOR_CONTRACTS.md), [capability matrix](parity/CAPABILITY_MATRIX.md), [source inventory](parity/upstream-inventory.json)
- [Release/package procedure](RELEASING.md), [Unix runtime implementation](parity/UNIX_RUNTIME_COMPLETION.md)

The [0.17.1 audit](parity/PARITY_AUDIT_2026-10-07.md) and [original implementation plan](IMPLEMENTATION_PLAN.md) are historical. Consult the RC adoption report and its 0.18 baseline before treating an older missing-feature list as current.
