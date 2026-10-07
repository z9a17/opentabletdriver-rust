# Windows bindings and plugin services, 0.17.1

Integration owner: `parity/windows-bindings-services`, based on v0.17.0
(`4595ee5ab04a5b45bd1f74f2f3bf30f177912ff9`).

The owner authorized implementation and publication, with behavioral validation
deferred to their manual testing. Linux/macOS native parity work stays deferred.
The compatibility baseline remains OpenTabletDriver 0.6.7, revision
`736003ed72c8bbb28033b039d5a0bb76c344145c`.

## Claimed implementation

- B04: `parity/b04-toggle-presets` owns native toggle and preset binding
  configuration, import, editors, runtime dispatch and held-input cleanup.
- P07: `parity/p07-driver-services` owns original managed assembly/provider
  services, dependency injection, host access and lifetime contracts.
- S03/C03-C05: `parity/p07-desktop-providers` owns retained idle upstream
  settings collections and original settings load/save boundaries.
- The integrator owns cross-slice profile selection, documentation, packaging,
  version creation, the reviewed PR and publication.

Each slice records its exact delivered scope and remaining gaps separately.
Native successful report processing must retain its allocation-free design;
profile reads, conversion and replacement happen on control/setup owners.

## Evidence boundary

No CI, format, Clippy, tests, behavioral probes, GUI, driver, daemon, plugin,
tablet input, game or OBS sessions run for this work. Actual binary compilation
and archive/provenance verification produce the distributable packages and
manual test folder. Those operations do not establish runtime compatibility,
performance or full parity. Implementation and acceptance evidence remain
separate in the backlog and ledger.
