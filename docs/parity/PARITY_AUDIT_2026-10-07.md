# Parity implementation audit, 2026-10-07

Historical 0.17.1 checkpoint. For current source completion and deferred validation, use [the 0.18 implementation report](IMPLEMENTATION_0.18.md).

Baseline: OpenTabletDriver 0.6.7, revision
`736003ed72c8bbb28033b039d5a0bb76c344145c`.
Windows is the current priority. The owner deferred native Linux/macOS work
because those machines are unavailable. The independent catalog snapshot and
historical evidence are unchanged.

This is a source/workflow checkpoint for 0.17.1, not a full-parity certificate.
See [Windows workflows](../WINDOWS_COMPATIBILITY_0.17.1.md), the
[acceptance backlog](WORK_ITEMS.md) and [evidence ledger](EVIDENCE_LEDGER.md).

## Source delivered

| Area / stable task IDs | Implemented source | Qualification |
| --- | --- | --- |
| Bindings B01-B04, output O03/O05, UI U02/U03 | Pen/mouse/auxiliary/wheel/scroll editors, contact/pen policies, native toggle and guarded per-device preset actions, unchanged original JSON PresetBinding | Held-input/self-apply and unchanged-binary/device-specific runtime acceptance remain open; foreground native preset switching is unavailable |
| Devices D05/D09, settings C02 | Independent physical sessions, guarded profiles, per-device CLI/panel controls, HID and cancel-safe WinUSB transport | Multi-device, firmware, unplug/reconnect, transport and input-release hardware checks remain unrun |
| Plugins P06/P07 | Actual original Desktop/Core/Native helper assemblies, IDriver/config/parser/device metadata and asynchronous IDriverDaemon services, scoped lifetime/generation guards and callback wait rejection | Concrete Driver/RootHub/readers, custom hub attachment, shared streams/feature I/O, managed DeviceReport and native-hosted DiagnosticInfo remain gaps; original binary corpus unrun |
| Daemon S03/S04, settings C03-C05 | Original RPC framing/events, retained idle Settings collections, unknown/disconnected rows, explicit guarded Load/Save and setup/reconnect selection, actual bounded logs | Original clients unrun; enabled idle global tools, same-model differing physical profiles and nonrepresentable native extensions retain explicit limits; group Apply is not atomic |
| Diagnostics S05/U05 | Full-rate raw recording, independent all-session subscriptions, stateful endpoint decoding, sequence/gap/loss reporting | Visualizer sampled; losses before the tap unmeasured; decoded history begins at capture start |
| Native filters P02/P04 | PR80 atomic whole-chain RadialFollow substitution eligibility and tablet-report interface gating; puck reports do not change native pen history | Native substitution is separate from unchanged-DLL evidence; new fixtures remain unrun |
| Packaging R02-R04 | Complete locked Windows managed DLL/resource/license closure, guarded manual copy, complete four-platform packaging workflow | Architecture/checksum/source inspection does not prove runtime behavior |

## Required continuation work

The broad GitHub issues #2-#13 are consolidated at the owner's request.
Administrative closure does not close these acceptance criteria or certify
an implementation as tested. Stable task IDs remain the continuation backlog:

1. **P01/P03/P07/P09 and V03:** finish concrete Core/device reader and shared-I/O
   services required by unchanged binaries. Finish per-assembly
   class/dependency inventory and execute the exact unchanged DLL corpus.
   Registry metadata and replacement Rust algorithms are separate evidence.
2. **B04, O05 and D04/D06-D09:** qualify delivered native/original preset semantics,
   device-specific analog/touch/output behavior and qualify Bluetooth/wireless
   and WinUSB variants. Catalog parser coverage does not prove every tablet works.
3. **C03-C05, S03-S05 and U01-U06:** complete original-client/provider fidelity,
   idle enabled global-tool ownership and nonrepresentable native extensions.
   Verify editing, save/import/presets, theme/high contrast/DPI, device selection
   and async lifetime guards. Logs are bounded; update cancellation cannot
   interrupt synchronous Windows proxy discovery.
4. **F04-F06 and V01-V06:** execute current differential/malformed/ownership/async
   fixtures, allocation/performance measurements, original-client checks and
   Windows tablet/application validation. Replug, held-contact removal, sleep,
   monitor changes, multiple tablets and plugins need current observed evidence.
   The reported osu!lazer/OBS slowdown remains physically unmeasured.
5. **X01-X05, O04 and G5:** deferred Linux/macOS desktop services, GUI,
   daemon/plugin integration and macOS drawing output. Cross-built CLI packages
   remain available without claiming completion of that platform work.
6. **G6/R04:** reconcile the ledger only after acceptance evidence exists.
   Closing an issue or compiling an archive does not promote unsupported rows.

## Validation boundary

Windows packages compile the Rust executables and pinned managed bridge.
The manual updater verifies source/hashes and copies into `E:/OTD RUST TEST`,
preserving extras/settings. Final release archives must identify one clean
merged commit and lockfile.

Source review corrected managed Enable defaults, typed log field loss,
invalid timestamp acceptance, first-request event loss, incorrect opened-tablet
identifiers, external unsaved Apply origin, authored/native-execution separation,
dynamic managed pen storage,
parser/graph reload identity and original-import gaps. The new
registry compilation error was fixed before any package was copied.

The 0.17.1 source pass additionally corrected managed detection result types,
log delivery after ring rollover, subscription baselines, finalizer exception
containment, synchronous service waits from parser/report/timer callbacks,
scope retirement admission and physical source generation checks. Managed
metadata snapshots do not probe another input reader. Original Desktop cleanup
paths use dedicated native data folders rather than the system temporary root.

Fixtures were written but not executed. CI, format/Clippy/test/build-check
suites were not run under the owner's policy. No driver, GUI, plugin, daemon,
game, OBS session or live input was launched. Native Linux/macOS validation is
deferred. Compilation, source review, archive verification and hardware/plugin
behavior are separate evidence layers. Full parity remains unverified and
has the implementation gaps listed above.
