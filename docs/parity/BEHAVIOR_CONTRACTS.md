# Behavior contracts (F01)

This document records the observable behavior of the Windows PTH-660 driver through 0.9.0, and compares each stage with OpenTabletDriver 0.6.7 ([`736003e`][otd]). The [golden traces](#golden-traces) replay recorded report sequences through the driver's own pipeline and fail on any output change. Every difference from upstream is listed [at the end](#differences), with the backlog task that resolves it or the reason it stays.

Historical golden traces describe their recorded paths. The 0.8.0/0.9.0 additions below have no new local test or hardware evidence; updating this contract does not extend those earlier results.

## Report path and stage order

The Rust driver handles one device session on one thread, with no queue and no lock. The session loop (`crates/otd-core/src/session.rs`) is portable; the Windows adapter (`src/session.rs`) supplies the reads, the display layout and `SendInput`:

1. An overlapped `ReadFile` returns one report from the 192-byte pen collection. The Windows HID class driver buffers reports that arrive while the thread works.
2. `protocol::parse` decodes report `0x10` and `0x1E` into a `PenReport`. Other report IDs are ignored; a short report, or a position or pressure outside the digitizer's range, is malformed. Neither produces output nor changes any state.
3. `ReportPipeline` (`crates/otd-core/src/pipeline.rs`) runs the rest for each decoded report:
   1. Input/proximity and relative timeout gates, then built-in Radial Follow and enabled PreTransform DLL filters in profile order.
   2. For each synchronous emitted report, absolute or relative mapping, then enabled Pixels/PostTransform DLL filters in profile order. Zero emissions suppress downstream processing.
   3. Existing tip/eraser contact policy uses supported post-filter pressure/eraser values. General side-button/auxiliary binding integration remains open.
   4. At most one combined move/button `SendInput` packet per output emission. Several emissions can produce several packets from one input report.

Upstream runs each endpoint on a [`DeviceReader`][DeviceReader] thread. It parses the report, takes the tablet's [lock][InputDeviceTree], and passes it to the output mode ([`OutputMode.Read`][OutputMode]):

1. PreTransform elements, which are the enabled profile filters in [profile order][DriverDaemon-elements].
2. The transform.
3. PostTransform elements, ending with the [`BindingHandler`][BindingHandler-position].
4. `OnOutput`, which sets the pointer position and [flushes][WindowsVirtualMouse] one `SendInput` per report.

## Units and precision

| Quantity | Units and range | Notes |
| --- | --- | --- |
| Position | raw units, 0..44800 × 0..29600; 200 units per mm | from the report descriptor and upstream's `PTH-660.json` |
| Pressure | 0..8191 | |
| Tilt, twist, distance | −64..63, −900..899, 0..63 | decoded, not used for output |
| Tablet-space filters | mm (raw × 224/44800, raw × 148/29600) | same scale as upstream's `TabletReference` |
| Absolute transform | mm → display pixels in `f64` | upstream uses `float` `Matrix3x2` ([CalculateTransformation][AbsoluteOutputMode-transform]) |
| Normalized output | `round((px − left) × 65535 / (width − 1))` | upstream truncates `px × 65535 / width` ([WindowsAbsolutePointer][WindowsAbsolutePointer]) |
| Relative output | counts per mm; deltas of `f64` positions with a signed carry | see [relative mode](../RELATIVE_MODE.md) |

Rust and upstream normalized coordinates for the same transform result differ by up to about one pixel. The difference comes from `f64` against `float` and from rounding against truncation, and is largest at the right and bottom edges. Differential tests against upstream (F05) need that tolerance; the golden traces compare Rust's values exactly.

## Pen state, hover and contact

- **Detection.** The cursor follows the pen while byte-1 In Range (`0x20`) or Sense (`0x40`) is set, like upstream, which uses the position of every [IntuosV2 report][IntuosV2Report]. A report with neither bit set means the pen is not detected. It moves nothing, releases a held button, resets DLL filters and clears the relative origin. The bit meanings come from the device's report descriptor; see [input latency](../INPUT_LATENCY.md#hover-tracking).
- **Tip or eraser.** Invert (`0x10`) selects the eraser binding, as upstream's `Eraser` does. Each binding can be disabled; a disabled one never clicks.
- **Threshold.** With a raw threshold, contact means `pressure >= threshold`. An imported OpenTabletDriver percentage becomes the first raw value with `pressure / 8191 × 100 > percent` in single precision, and 100 % means full pressure; this reproduces [`ThresholdBindingState`][ThresholdBindingState]. For 1 % that is raw 82.
- **Tip switch.** A Rust TOML profile without a threshold uses the tip switch bit (`0x01`) instead of pressure.
- Managed filters can alter pressure/eraser before the contact policy; the driver still has no pressure-capable pen output. Without an explicit threshold, native input retains its tip switch; replacement managed reports lacking that switch use nonzero pressure.

## Filters and time

- The built-in Radial Follow port runs per report in tablet millimetres. It resets to the report when 50 ms or more have passed since the previous one, or when its output is not finite, as the original [`RadialFollowCore`][RadialFollowCore] does (`!(elapsed < 50)`).
- Time is the moment the report's read completed. The Radial Follow reset, the relative reset delay and the DLL filters' `time_ns` all use it. Upstream measures each element's elapsed time with its own `HPETDeltaStopwatch` when the report reaches it. Neither driver uses device timestamps.
- Native ABI version 1 filters position only. The managed graph preserves synchronous Emit ordering and the emitted object across downstream nodes; PostTransform sees desktop pixels or relative deltas. Async/off-thread Emit is rejected. The built-in Radial Follow retains its 50 ms reset rule; managed range-loss reports traverse the graph, while physical loss independently releases native held output even if the plugin suppresses that notification.

## Mapping

- **Absolute, OpenTabletDriver areas.** Areas are center-based. The transform converts raw units to mm, translates by the tablet area's center, rotates by the negated tablet rotation, scales by display size over tablet size, and translates to the display area's center; this is upstream's [order][AbsoluteOutputMode-transform]. Clipping clamps to the display area. Limiting drops a report outside it entirely: no move and no button change, as in upstream, where the binding handler never sees a report the transform discarded ([limiting][AbsoluteOutputMode-clamp]).
- **Absolute, built-in areas.** Rust TOML profiles can crop the tablet, rotate by 0/90/180/270 degrees and pick a monitor. Unfiltered positions are rounded to whole pixels before normalization.
- **Display topology.** The driver compares a display fingerprint once a second while reports flow, and the full monitor layout while idle; a changed layout rebuilds the mapping.
- **Relative.** See [relative mode](../RELATIVE_MODE.md). Upstream's reset rule is [strict][RelativeOutputMode-reset] and it [skips a stale first report][RelativeOutputMode-stale]; Rust matches both.

## Output

- At most one `SendInput` per output emission; one input may emit several reports. The move precedes the button change within the packet.
- Absolute packets use `MOVE | ABSOLUTE | VIRTUALDESK`. A report that would repeat the last normalized position without a button change sends nothing.
- The tip and the eraser both press the left button, as upstream's `AdaptiveBinding` does for a mouse pointer.
- If `SendInput` fails, the driver counts the failure and warns at most every 5 s. Button state is committed only after a successful call, so the next report retries the transition; relative movement from the failed packet is dropped rather than replayed.
- A held button is released when the pen is not detected, when a display change makes the absolute mapping invalid, and when the session ends.
- Failed release remains pending and is retried even while reports are paused. A terminal cleanup failure propagates out of the session/supervisor instead of reconnecting or reporting successful stop. Original-driver restoration failures are surfaced to daemon status. These error paths have no current live validation.

## Session lifecycle, cancellation and ownership

- A session opens the pen collection and ends on the stop event, removal of that device, a read error, or the capture deadline. Ending cancels the pending read with `CancelIoEx` and waits for it to complete before the buffer is reused. Then the session releases buttons and logs its counters and timing. The driver then waits for a PnP arrival or rechecks every 2 s.
- The session owns and reuses one read buffer. `PenReport` is a copied value and nothing retains it. A native DLL receives a copy of the sample that is valid only during its call. The .NET bridge imports an owned managed input report and arrays, then passes each emitted object directly downstream so retained references, identity and mutations remain intact. This adds managed allocation; performance and unchanged-plugin replay evidence for this path are pending. Native envelopes borrow raw bytes and use bounded inline values; retaining one explicitly creates an owned snapshot.
- Configuration is fixed for a worker: the GUI requests a validated, generation-guarded daemon restart on Apply/Save. Accepted restart waits for old-worker cleanup; a failure cancels the pending replacement. Close detaches the panel without stopping input. C03 transactional hot apply and rollback remain open.
- Upstream disposes the output mode when a device disconnects and has no explicit button release. Its per-tablet lock serializes pen and auxiliary endpoints; the Rust driver reads no auxiliary endpoint yet.

## Settings import

The importer reads the `Wacom PTH-660` profile from OpenTabletDriver's `settings.json`:

- Output mode: Absolute or Relative Mode; anything else is an error.
- Tip and eraser: only supported `AdaptiveBinding` actions become native clicks. Unsupported active actions are preserved and diagnosed, and are inactive in the native mapping. A disabled binding never clicks. Thresholds convert as described above.
- Filters: enabled `RadialFollow.RadialFollowSmoothingTabletSpace` entries become built-in filters. New imports honor `Enable`; the explicit legacy import option can force disabled entries for that import only. Existing native entries from older saved Rust profiles stay active. Other enabled filters remain archived and produce unsupported diagnostics; supported DLLs must be added explicitly. Enabling the same tablet-space Radial Follow natively and through .NET is rejected before execution.
- The complete original OTD settings text, selected profile index and source revision are preserved in schema-1 native profiles. Other profiles, disabled stores, null/missing values and unknown fields remain archived. Unsupported active features produce diagnostics; preservation does not run pen/auxiliary buttons, wheels, mouse buttons, pressure/tilt output options or tools. Unknown native TOML fields are archived under `preserved_fields` without attaching them to a different filter after reordering.

## Golden traces

`tests/golden/*.toml` hold the traces. `cargo test --locked golden` replays each one through `protocol::parse` and `ReportPipeline`, the code the device session runs, with a simulated output sink.

A fixture has a `description`, the `origin` of its reports (`captured`, `synthetic` or a mix, labeled per step in `note`), an optional `virtual_screen` and `monitors`, a Rust `profile` in TOML, and `[[step]]` entries. Each step gives a processing time `at_ms`, the report bytes in hex, and the expected output:

| Expected | Meaning |
| --- | --- |
| `move X Y` | absolute move to normalized X, Y |
| `delta DX DY` | relative move in counts |
| `down`, `up` | left button change, alone or after a move |
| `none` | decoded and processed, nothing sent |
| `ignored`, `malformed` | not a pen report, or a pen report that failed validation |

`end` is the output when the session ends. The pipeline is created just before 0 ms and traces start at 100 ms, so the first report arrives outside the filters' 50 ms window, as on a device. When a trace fails, the test prints the whole actual trace in fixture syntax.

| Fixture | Pins down |
| --- | --- |
| `absolute-otd-hover-contact` | OpenTabletDriver areas; hover, contact, lift; tracking in the Sense band; duplicate suppression; loss of detection |
| `absolute-thresholds` | raw 82 in both directions, full pressure, release on loss of detection, disabled eraser |
| `absolute-tip-switch` | built-in full-area mapping and corners; tip switch instead of pressure |
| `absolute-clipping`, `absolute-limiting` | clamping to the display edge; dropped reports and held buttons outside the area |
| `absolute-radial-follow` | reset on the first report, dead zone, partial moves, and the 50 ms boundary on both sides |
| `relative` | origin, contact, Sense band, loss of detection, fractional carry, timeout with a stale report |
| `non-positional` | status, auxiliary and touch IDs; short, out-of-range and empty reports |
| `offset-report-0x1e` | the `0x1E` layout, synthetic because this tablet has not sent one |

Replacing the tip threshold's `>=` with `>` makes `absolute-thresholds` fail, which shows the traces detect a changed transition.

## Differential tests

`tests/differential` holds report sequences with the outputs OpenTabletDriver 0.6.7 produced for them: its own parser, output modes and binding handler, and the unchanged RadialFollow DLL, assembled as its daemon does (`bench/upstream --reference`). `cargo test --locked differential` runs the same reports through this driver's pipeline and compares each report:

| Output | Comparison |
| --- | --- |
| Absolute position | within 0.05 px, in desktop pixels: `SendInput`'s 0..65535 resolution is 0.032 px on the fixtures' desktop, and BC-13 adds under 0.001 px |
| Position at the right or bottom edge | within 1.05 px (BC-11) |
| Inside or outside under area limiting | may differ within 0.001 px of an edge (BC-13) |
| Relative motion | running sums within one count plus 0.001 (BC-18: both drivers truncate each delta and carry the fraction) |
| Presses and releases | exactly, report by report |
| Tip threshold | the first pressing raw value for 16 activation thresholds, exactly |

The cases cover the development machine's osu! profile with and without Radial Follow, a 30° rotated area, area limiting, relative mode at 10 counts/mm, and at 12 × 8 counts/mm rotated 15°. They run over 2,000 synthetic osu!-style reports and 35 handwritten edge cases. The suite found no difference beyond BC-10, BC-11, BC-13 and BC-18. A self-test moves one press by a report and one position by 0.1 px and checks that the comparison reports both. [The fixtures' README](../../tests/differential/README.md) covers their origin, format and regeneration.

## Tablet configurations

The executables embed OpenTabletDriver's 339 tablet configuration files unchanged (`crates/otd-core/tablets`). When the driver starts, it loads them together with the files in `%LOCALAPPDATA%\OpenTabletDriver\Configurations`, as upstream's [configuration provider][DesktopDeviceConfigurationProvider] does:
- the first file with a configuration's `Name` replaces that configuration;
- a file with a new name is added;
- a later file with the same name is ignored.

`opentabletdriver-rust.exe tablets` shows the result. Live Windows snapshots now supply report lengths, device strings, attributes and physical identity to the matcher. Execution remains restricted to the PTH-660 parser, pen report length and specifications supported by this runtime; incompatible overrides are rejected. Multiple matching pen endpoints require an explicit `device_path`. Auxiliary pairing is recorded but does not start auxiliary reading. The selected configuration/identifier supplies managed TabletReference initialization.

## New report, action and control foundations

The native report envelope borrows raw transport bytes and distinguishes an absent capability from a present zero/released value. Fixed limits are 64 buttons, 16 analog channels, 8 wheels and 32 touch slots; exceeding them is an error rather than truncation. An IntuosV2 auxiliary decoder exposes button/analog values without modifying pen contact or emitting output. Stateful IntuosV2 touch decoding retains 16 slots across packets; a separate Wacom-driver variant removes its transport prefix while preserving the original bytes. These decoders are not wired to live touch or gesture output. These types do not establish support for additional devices or bindings.

The managed pen adapter supplies owned raw bytes, pressure, eraser, pen buttons, tilt and proximity where available. Rotation remains in raw bytes. Each synchronous Emit immediately traverses downstream filters with the same object, including zero/multiple emission, non-positional reports and supported capability mutations. Managed input vendor concrete types are not emulated. The graph supports at most 32 enabled nodes and the native capability limits above; it does not truncate excess data. Async remains rejected. A failed graph stops the current dispatch without replaying an already accepted output prefix, disables an identified failing node where possible, and attempts held-output cleanup before resuming. Imported snapshots/arrays allocate in managed code; current latency/allocation evidence is pending. The native-only design retains fixed report storage.

Shared action ownership records `(device generation, binding)` holds in fixed storage. Overlapping owners produce one initial press and one final release. Only successful adapter acceptance advances emitted state; failed transitions and cleanup remain pending. Releases precede new presses, with modifier presses first and modifier releases last. A Windows adapter maps supported USB keyboard usages and five mouse buttons; unsupported usages return errors. This foundation is not yet the complete binding engine, and the existing combined move/tip packet path remains in use.

The optional headless daemon keeps one worker independent of CLI connections. Version-2 JSON requests have nonzero IDs and bounded length-prefixed frames (256 KiB); profiles are limited to 128 KiB and status keeps at most 64 messages of 512 bytes. The SID-qualified local named pipe has a protected current-user/SYSTEM ACL and rejects remote clients. Server identity verification by clients, response acknowledgements and cancellable overlapped I/O keep malformed, slow or disconnected clients out of report processing. A connection has a five-second server budget. Request IDs correlate replies but do not deduplicate commands; after a timeout, query status before retrying. The GUI polls through a separate bounded client thread and deduplicates logs by daemon sequence. Guarded commands carry daemon instance plus worker generation; stale commands fail without stopping a newer worker. Active configuration retrieval updates a clean editor but preserves unsaved edits. Autostart is suppressed once the client has observed an active worker. GUI close cancels only its client. A detected v1 endpoint prevents automatic launch; migration requires stopping/shutting down the old service with its original executable.

Selected HID initialization runs indexed strings, delayed feature reports, then output writes. Failure or a partial write aborts the session before cursor output. This differs from upstream's warning-and-continue policy. Stop interrupts delays and pending output writes; synchronous string/feature calls are only cancellable between calls. Device-specific initialization and these IPC paths have not been exercised on hardware or through a live daemon in this development increment.

## Profile persistence

Core storage stages a sibling file, flushes it, and publishes by rename (Windows write-through) or a no-clobber new-file operation. Cooperating writers hold a `.lock` sidecar; exact loaded bytes are checked again before replacement. External tools ignoring that lock can still race the final check/publication. A crashed writer may leave a lock requiring deliberate removal after confirming no save is running.

`.bak` is a reserved, overwritten sidecar containing the previous destination bytes. Backup rotation can succeed before a later conflict/publication failure; an unsuccessful save does not guarantee the backup stayed unchanged. On Unix, a directory-sync error can be reported after publication already succeeded. Windows ACLs and full metadata are not preserved. These are file-persistence guarantees, not rollback of a running configuration.

The GUI retains a destination revision floor with its loaded-byte snapshot, even when the daemon supplies an older active profile; replacing that same destination increments above both revisions. Loading a new path resets the floor. Save As requires a new destination. Recovery validates a backup and writes a new file, preserving primary and backup. `OTD_RUST_PORTABLE_DIR` chooses an absolute settings directory. Plugin relocation uses relative paths within the destination/portable tree where possible and can canonicalize aliases; API callers must resolve relative plugin paths against their source before relocation.

## Area helpers and diagnostic export

C05 shares area validation, aspect/usable-area helpers and CLI previews for percentage, Wacom/VEIKK, XP-Pen and Gaomon V2 settings. The pinned Gaomon converter explicitly retains the upstream X-for-Y quirk; a separately named corrected converter uses Y. Preview results are millimetres and do not apply settings or activate another tablet.

S05 diagnostic JSON is an explicit static snapshot of build/display information, whitelisted profile fields and optional daemon status. Default redaction omits paths, free-form logs/errors, plugin property values and imported settings archives; `--include-private-details` explicitly adds paths/messages. Export does not open HID, load plugins or subscribe to reports. Current artifact/privacy execution validation remains open.

## Differences

"Record" means the difference is intended; its reason is given. A task ID means the difference is a gap that task closes.

| ID | Behavior | Upstream | Rust | Disposition |
| --- | --- | --- | --- | --- |
| BC-01 | Filter order | enabled PreTransform filters in profile order | built-in Radial Follow first, then DLL filters in profile order | P04 |
| BC-02 | Pixel-space filters | PostTransform (`Pixels`) filters run after the transform | synchronous zero/multiple Emit and PostTransform after absolute or relative mapping implemented; current execution evidence remains open | P04 |
| BC-03 | Binding input | `BindingHandler` runs after filters and the transform, so a filter can change the pressure it sees | existing tip contact uses supported post-filter pressure/eraser values; broad bindings remain open | P04, B02; current plugin execution evidence pending |
| BC-04 | Pen barrel buttons | bits `0x02`/`0x04` (`0x1E`: three buttons) drive pen bindings | ignored | B02 |
| BC-05 | Auxiliary and touch | `0x11` and `0x21`/`0xD2` parsed; auxiliary endpoint opened | auxiliary endpoint not opened; those IDs ignored | D04 |
| BC-06 | Report validation | any value accepted | out-of-range position or pressure is malformed and ignored | Record: a working tablet never sends them; ignoring one avoids a jump |
| BC-07 | Neither hover bit set | position used | not detected: no move, button released | Record: see [input latency](../INPUT_LATENCY.md#hover-tracking); not yet observed from this tablet |
| BC-08 | Release when the pen is not detected | only when pressure falls below the threshold | immediately | Record: a report without contact state cannot hold a click |
| BC-09 | Tip switch without threshold | always a threshold | Rust TOML profiles may use the tip switch bit | Record: Rust-only profile option; imports always use a threshold |
| BC-10 | Normalization | truncates `px × 65535 / width` | rounds with `width − 1` | O01: adopt a tolerance or match upstream |
| BC-11 | Clipping bound | clamps to the far edge coordinate, `X + W/2` | clamps one pixel short of it | O01 |
| BC-12 | Display coordinates | relative to the virtual screen's top-left | absolute desktop coordinates | O02; the two agree when no monitor sits left of or above the primary |
| BC-13 | Transform precision | `float` | `f64` | O01 tolerance |
| BC-14 | Display changes | virtual screen read once | refreshed within about a second | Record: follows monitor changes without a restart |
| BC-15 | Unchanged absolute position | sent every report | not sent | Record: no observable movement difference |
| BC-16 | `SendInput` failure | ignored | counted, warned; button change retried | Record: avoids a stuck or lost click |
| BC-17 | Session end | no release | held button released on stop, disconnect or invalid mapping | Record: avoids a stuck click |
| BC-18 | Relative details | as linked | immediate rebase on loss, carry cleared on reset, `f64` deltas, no zero-motion packets | Record, detailed in [relative mode](../RELATIVE_MODE.md); O01 revisits |
| BC-19 | Disabled Radial Follow on import | not run | new imports honor Enable; explicit legacy option restores the older override, and existing saved native entries stay active | C01 migration; current execution evidence pending |
| BC-20 | Other enabled filters on import | run | preserved with unsupported diagnostics; add supported DLLs explicitly | C02, P02 |
| BC-21 | Other bindings and output modes | run | unsupported bindings are preserved/diagnosed and inactive; unsupported output modes prevent runnable import | B02, O03, P06 |
| BC-22 | Pressure and tilt output | pressure-capable pointers receive remapped pressure | mouse only | O03 |
| BC-23 | `0x1E` hover distance | read from byte 11, which is also tilt X | not reported | Record: upstream defect with no effect on mouse output |
| BC-24 | Reader priority | High class, AboveNormal thread (14) | time-critical thread (15), rest of the process normal | Record: see [input latency](../INPUT_LATENCY.md#scheduling) |
| BC-25 | Configuration file that does not parse | the exception stops detection, so no tablet is found | reported and skipped; the other files still apply | Record: one bad file should not disable every tablet |
| BC-26 | Configuration file syntax | Newtonsoft.Json also accepts comments, single-quoted strings, unquoted property names, property names in any case and numbers written as strings | strict JSON with OpenTabletDriver's property names; anything else is an error or an unknown-field warning | Open: matters once override files select devices (D02) |
| BC-27 | Configuration order | built-ins in the assembly's resource order, then files in the file system's order | built-ins by path, then files breadth first in NTFS name order | Record: built-in names are unique, so order only decides between tablets that declare the same interface (D02) |
| BC-28 | Override of the PTH-660 configuration | used for detection and parsing | predicates and initialization apply; unsupported parser/report-length/specification changes are rejected | D02, D03; hardware evidence pending |
| BC-29 | Initialization failure | warning and continued initialization | failed or partial initialization aborts the session before output | Record: fail closed on partially initialized hardware; D02 evidence pending |

[otd]: https://github.com/OpenTabletDriver/OpenTabletDriver/tree/736003ed72c8bbb28033b039d5a0bb76c344145c
[DeviceReader]: https://github.com/OpenTabletDriver/OpenTabletDriver/blob/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver/Devices/DeviceReader.cs#L102-L113
[InputDeviceTree]: https://github.com/OpenTabletDriver/OpenTabletDriver/blob/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver/InputDeviceTree.cs#L59-L65
[OutputMode]: https://github.com/OpenTabletDriver/OpenTabletDriver/blob/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.Plugin/Output/OutputMode.cs#L112-L120
[DriverDaemon-elements]: https://github.com/OpenTabletDriver/OpenTabletDriver/blob/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.Daemon/DriverDaemon.cs#L406-L416
[BindingHandler-position]: https://github.com/OpenTabletDriver/OpenTabletDriver/blob/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.Desktop/Binding/BindingHandler.cs#L38
[WindowsVirtualMouse]: https://github.com/OpenTabletDriver/OpenTabletDriver/blob/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.Desktop/Interop/Input/WindowsVirtualMouse.cs#L93-L103
[AbsoluteOutputMode-transform]: https://github.com/OpenTabletDriver/OpenTabletDriver/blob/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.Plugin/Output/AbsoluteOutputMode.cs#L90-L113
[AbsoluteOutputMode-clamp]: https://github.com/OpenTabletDriver/OpenTabletDriver/blob/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.Plugin/Output/AbsoluteOutputMode.cs#L126-L128
[WindowsAbsolutePointer]: https://github.com/OpenTabletDriver/OpenTabletDriver/blob/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.Desktop/Interop/Input/Absolute/WindowsAbsolutePointer.cs#L9-L19
[IntuosV2Report]: https://github.com/OpenTabletDriver/OpenTabletDriver/blob/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.Configurations/Parsers/Wacom/IntuosV2/IntuosV2Report.cs#L13-L33
[ThresholdBindingState]: https://github.com/OpenTabletDriver/OpenTabletDriver/blob/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.Desktop/Binding/ThresholdBindingState.cs#L13-L21
[RelativeOutputMode-reset]: https://github.com/OpenTabletDriver/OpenTabletDriver/blob/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.Plugin/Output/RelativeOutputMode.cs#L97
[RelativeOutputMode-stale]: https://github.com/OpenTabletDriver/OpenTabletDriver/blob/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.Plugin/Output/RelativeOutputMode.cs#L114
[DesktopDeviceConfigurationProvider]: https://github.com/OpenTabletDriver/OpenTabletDriver/blob/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.Desktop/DesktopDeviceConfigurationProvider.cs#L20-L56
[RadialFollowCore]: https://github.com/AbstractQbit/AbstractOTDPlugins/blob/0.3.0/RadialFollow/RadialFollowCore.cs#L52-L63
