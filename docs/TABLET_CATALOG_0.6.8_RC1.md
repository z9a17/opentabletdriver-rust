# Tablet catalog and device changes from OpenTabletDriver 0.6.8-rc1

The embedded catalog now follows [v0.6.8-rc1, commit
126f4828564abb673d39e97a093fd815aab335c2](https://github.com/OpenTabletDriver/OpenTabletDriver/tree/126f4828564abb673d39e97a093fd815aab335c2).
It contains **359 configurations and 54 referenced native parser types**.
The previous catalog at a126f7b contained 357 configurations and 53 referenced
types; 354 existing definitions remain semantically identical. The original
0.6.7 source inventory and hardware acceptance ledger remain historical records.

- Add Parblo A610 Plus using the existing XP-Pen parser.
- Add Silvercrest SGT 10.6 A2 with native pen decoding, signed wheel movement,
  three selector buttons and its wheel push button.
- Correct CTH-460 PID 214 auxiliary matching to the 20-byte BambooV2 auxiliary
  parser. Existing other CTH-460 identifiers are retained.
- Describe five auxiliary buttons for CTL-4100WL and CTL-6100WL.
- Ignore XP-Pen initialization echoes, idle notifications and wireless-on
  status packets in the affected base/offset-pressure variants, matching
  [#4989](https://github.com/OpenTabletDriver/OpenTabletDriver/pull/4989).
- Preserve partial wheel movement across zero-delta reports in native and
  managed bindings, matching [#5006](https://github.com/OpenTabletDriver/OpenTabletDriver/pull/5006).
- Exclude Google fastboot and existing Quest devices from native WinUSB
  enumeration/string requests and guard interface setup even without reliable
  instance metadata. The copied managed interface has the Google exclusion too
  ([#4798](https://github.com/OpenTabletDriver/OpenTabletDriver/pull/4798)).
- Recognize the renamed Linux `hid_digimend_uclogic` driver when reporting
  kernel-driver conflicts ([#4955](https://github.com/OpenTabletDriver/OpenTabletDriver/pull/4955)).
- Cache managed endpoint strings for each detection attempt, including null
  results, and materialize matches once. Redetection clears the cache
  ([#4934](https://github.com/OpenTabletDriver/OpenTabletDriver/pull/4934)).
  Native discovery already reads each required index once per endpoint.
  Managed matching now applies the HID_REPORTS predicate already present in
  native matching ([#4836](https://github.com/OpenTabletDriver/OpenTabletDriver/pull/4836)).
- Parse managed reports once, deliver raw observers before filters, recover
  from per-packet index exceptions and include raw bytes in failure logs
  ([#5054](https://github.com/OpenTabletDriver/OpenTabletDriver/pull/5054),
  [#5008](https://github.com/OpenTabletDriver/OpenTabletDriver/pull/5008)).
  Existing hosted callback scopes and completion signals are preserved.
- Implement the 0.6.8 endpoint feature/output initialization helpers in the
  copied HidSharp and WinUSB backends. Temporary streams are disposed after the
  request, including failure paths; existing reader streams and endpoint caches
  retain their ownership.

Pen rotation now uses typed `MinRotation` / `MaxRotation` configuration bounds
and upstream's `(raw - min) / (max - min)` normalization. Native packets retain
the full orientation alongside pressure/tilt; changes in rotation can emit an
otherwise stationary pen packet. A report without a position can update the
last accepted pen orientation, and absence of a rotation field preserves it.
The native position-filter DLL `Sample`, `Header` and `FilterApi` ABI is unchanged.

Capable providers receive the normalized value in their own units:
Windows synthetic pen uses
[`PEN_MASK_ROTATION` and 0..359 degrees](https://learn.microsoft.com/windows/win32/api/winuser/ns-winuser-pointer_pen_info);
Linux Artist Mode declares and emits the RC `ABS_Z` axis at 0..65535;
the macOS tablet mouse writes
[`kCGTabletEventRotation`](https://developer.apple.com/documentation/coregraphics/cgeventfield/tableteventrotation)
in [degrees](https://developer.apple.com/documentation/appkit/nsevent/rotation).
Ordinary Windows/Linux mouse APIs have no pen-rotation channel. macOS's existing
tablet mouse path can carry orientation; this does not add a macOS Artist Mode
or synthetic pen backend. Physical Art Pen orientation and application behavior
remain unverified on every platform.

`docs/parity/device-catalog.json` records each exact upstream configuration blob;
`scripts/update-tablet-database.py` can reproduce the embedded files from the
pinned checkout. Added fixture cases cover status-only packets, Silvercrest pen
and wheel decoding, malformed packets, stationary wheel accumulation, retained
orientation and duplicate suppression, Windows rotation masks/ranges, and the
Linux rotation axis within the fixed event capacity. Rotation cases use mock
sinks or pure platform packet structures and never create an input device.
These fixtures have not been executed during this adoption work. No physical
device, driver, UI, or platform behavior was exercised. Configuration/parser
coverage does not certify hardware support or establish lower latency.
