# Parity implementation audit, 2026-10-07

Baseline: OpenTabletDriver 0.6.7, revision
`736003ed72c8bbb28033b039d5a0bb76c344145c`.
Windows is the current priority. The owner deferred native Linux/macOS work
because those machines are unavailable. The independent catalog snapshot and
historical evidence are unchanged.

This is a source/workflow checkpoint for 0.17.0, not a full-parity certificate.
See [Windows workflows](../WINDOWS_COMPATIBILITY_0.17.0.md), the
[acceptance backlog](WORK_ITEMS.md) and [evidence ledger](EVIDENCE_LEDGER.md).

## Source delivered

| Area / stable task IDs | Implemented source | Qualification |
| --- | --- | --- |
| Bindings B01-B04, output O03/O05, UI U02/U03 | Pen/mouse/auxiliary/wheel/scroll editors, percentage contact and pen policies, separate Tools tab, typed unchanged managed output/binding selectors | Complete toggle/preset bindings and per-binary/device-specific behavior remain open |
| Devices D05/D09, settings C02 | Independent physical sessions, guarded profiles, per-device CLI/panel controls, HID and cancel-safe WinUSB transport | Multi-device, firmware, unplug/reconnect, transport and input-release hardware checks remain unrun |
| Plugins P06/P07 | Unchanged IOutputMode/IBinding/IStateBinding host, actual opened identifiers, retained registry generations, installed store resolution, original Configurations and custom parser reports driving the managed graph | Original Desktop/core/device-provider services and unchanged binary corpus remain open |
| Daemon S03/S04 | Opt-in persistent upstream framing, 19 method dispatches, four event paths, multi-device settings/defaults/discovery, typed logs, registry and checked update ownership | Original clients unrun; bounded logs, per-client debug enable, idle settings storage and nonrepresentable native settings retain explicit limits |
| Diagnostics S05/U05 | Full-rate raw recording, independent all-session subscriptions, stateful endpoint decoding, sequence/gap/loss reporting | Visualizer sampled; losses before the tap unmeasured; decoded history begins at capture start |
| Native filters P02/P04 | PR80 atomic whole-chain RadialFollow substitution eligibility and tablet-report interface gating; puck reports do not change native pen history | Native substitution is separate from unchanged-DLL evidence; new fixtures remain unrun |
| Packaging R02-R04 | Original Plugin/Configurations dependencies and notices, guarded manual copy, complete four-platform packaging workflow | Architecture/checksum/source inspection does not prove runtime behavior |

## Required continuation work

The broad GitHub issues #2-#13 are consolidated at the owner's request.
Administrative closure does not close these acceptance criteria or certify
an implementation as tested. Stable task IDs remain the continuation backlog:

1. **P01/P03/P07/P09 and V03:** supply actual original Desktop/core/device/control
   provider types/services required by unchanged binaries. Finish per-assembly
   class/dependency inventory and execute the exact unchanged DLL corpus.
   Registry metadata and replacement Rust algorithms are separate evidence.
2. **B04, O05 and D04/D06-D09:** finish remaining toggle/preset binding semantics,
   device-specific analog/touch/output behavior and qualify Bluetooth/wireless
   and WinUSB variants. Catalog parser coverage does not prove every tablet works.
3. **C03-C05, S03-S05 and U01-U06:** complete original-client/provider fidelity,
   idle upstream settings collections and nonrepresentable native extensions.
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
identifiers, external unsaved Apply origin, dynamic managed pen storage,
parser/graph reload identity and original-import gaps. The new
registry compilation error was fixed before any package was copied.

Fixtures were written but not executed. CI, format/Clippy/test/build-check
suites were not run under the owner's policy. No driver, GUI, plugin, daemon,
game, OBS session or live input was launched. Native Linux/macOS validation is
deferred. Compilation, source review, archive verification and hardware/plugin
behavior are separate evidence layers. Full parity remains unverified and
has the implementation gaps listed above.
