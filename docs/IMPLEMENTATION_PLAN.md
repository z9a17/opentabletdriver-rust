# Implementation plan: Windows 11 USB driver for Wacom PTH-660

Status: planning only. No Rust code has been written.

## 1. Objective and limits

Build a standalone, user-mode Rust executable that reads a Wacom PTH-660 connected by USB and drives the Windows 11 cursor. The first usable release must provide absolute pen position across the desktop, pen contact as left-button down/up, two pen side buttons, and reliable startup, shutdown, unplug, and replug behavior.

This is a focused replacement for the hardware-to-pointer part of the OpenTabletDriver daemon, not an implementation of its full RPC or plugin ecosystem. It should run in the signed-in user's desktop session. Its normal operation should require neither a background GUI nor administrator elevation.

**Included in the first release**

- USB PTH-660 identification and HID input access.
- Pen reports, proximity, position, pressure capture, contact transition, eraser flag capture, tilt capture, and two side-button transitions.
- Absolute mapping to one selected display or the full virtual desktop.
- Mouse event injection for cursor movement and buttons.
- A small fixed device/profile configuration, diagnostics, graceful recovery, and packaging instructions.

**Excluded from the first release**

- Bluetooth PTH-660, other tablets, touch input, express keys, touch wheel, gestures, smoothing filters, plugin loading, scripts, a GUI, and OpenTabletDriver RPC compatibility.
- Pressure, tilt, and eraser *output* to Windows Ink or drawing applications. The daemon may decode and display those values diagnostically, but SendInput mouse events do not expose pen pressure. Native pen output is a separate later milestone.
- Kernel driver development or blanket HID/WinUSB driver replacement.

The project should be named and packaged distinctly enough that users can keep the original OpenTabletDriver installation for comparison or rollback. Only one tablet driver should actively inject cursor events during a functional test.

## 2. Pinned reference and facts to verify on hardware

Use OpenTabletDriver's default branch, **0.6.x at fdeaa7b0c6d6f5260f511f19fb693ed33524af4e**, as the source snapshot for the initial protocol and behavior study. Link every protocol assertion in future PRs to an upstream file or to a captured report. Recheck the upstream branch when implementation starts rather than silently mixing revisions.

The upstream PTH-660 configuration records:

| Property | USB target |
| --- | --- |
| Vendor / product | 0x056A / 0x0357 |
| Pen collection max input report length | 192 bytes |
| Auxiliary collection max input report length | 44 bytes |
| Digitizer range | X 0..44800; Y 0..29600 |
| Pressure range | 0..8191 |
| Nominal active size | 224 x 148 mm |
| Pen side buttons | 2 |
| Bluetooth product ID | 0x0360, excluded |

Both USB collections use the IntuosV2 parser in upstream configuration. The parser dispatches report IDs **0x10** (pen), **0x1E** (offset pen), **0x11** (auxiliary), **0x21** and **0xD2** (touch). The first release processes 0x10 and 0x1E for pen input. It recognizes but ignores touch and auxiliary reports. The auxiliary collection can remain unopened initially; it must never be mistaken for the pen collection.

The current upstream 0x10 parser reads X from bytes 2..4, Y from bytes 5..7, pressure from bytes 8..9, tilt from bytes 10..11, rotation from bytes 12..13, proximity and side-button flags from byte 1, and hover distance from byte 16. The 0x1E offset report has a different layout. These offsets are **reference facts, not permission to assume every report is valid**. The Rust parser must check report ID and returned byte count before indexing, reject out-of-range data, and confirm the layout with a real PTH-660 capture.

The upstream configuration lists no feature or output initialization reports for this USB device. Do not send undocumented initialization data. If the user's firmware actually needs a mode switch, capture and document the evidence before adding it.

**Hardware questions to settle in the protocol-validation milestone**

1. Which HID collection(s) are present on this Windows 11 machine, and are 192/44-byte capabilities reported exactly?
2. Does the pen stream deliver both 0x10 and 0x1E, and under what conditions?
3. Which flag or pressure transition reliably signals tip contact? Upstream exposes pressure and proximity but does not by itself specify the first release's mouse-click policy.
4. What happens when the pen leaves proximity, the cable is unplugged, or reports are truncated?
5. Is the Windows Wacom driver or another tablet daemon holding the intended collection open? Report this clearly without changing system drivers automatically.

References: [PTH-660 configuration](https://github.com/OpenTabletDriver/OpenTabletDriver/blob/fdeaa7b0c6d6f5260f511f19fb693ed33524af4e/OpenTabletDriver.Configurations/Configurations/Wacom/PTH-660.json), [IntuosV2 dispatch](https://github.com/OpenTabletDriver/OpenTabletDriver/blob/fdeaa7b0c6d6f5260f511f19fb693ed33524af4e/OpenTabletDriver.Configurations/Parsers/Wacom/IntuosV2/IntuosV2ReportParser.cs), [0x10 layout](https://github.com/OpenTabletDriver/OpenTabletDriver/blob/fdeaa7b0c6d6f5260f511f19fb693ed33524af4e/OpenTabletDriver.Configurations/Parsers/Wacom/IntuosV2/IntuosV2Report.cs), and [0x1E layout](https://github.com/OpenTabletDriver/OpenTabletDriver/blob/fdeaa7b0c6d6f5260f511f19fb693ed33524af4e/OpenTabletDriver.Configurations/Parsers/Wacom/IntuosV2/IntuosV2OffsetReport.cs).

## 3. Architecture

Use one executable with a small number of explicit modules. The production data path is:

**HID collection -> reusable read buffer -> checked report parser -> pen state machine -> coordinate transform -> Windows mouse event -> SendInput.**

The pen read worker should parse, transform, and emit output in order on the same thread for the first release. This avoids an inter-thread queue, report copies, wakeups, and reordered button transitions. The supervisor owns device discovery and session lifecycle. A separate low-frequency control path may handle notifications, configuration changes, and diagnostics; it must not block the pen read path.

Suggested module boundaries for later implementation:

| Module | Responsibility |
| --- | --- |
| Device discovery | Enumerate present HID collections, inspect vendor/product and capabilities, choose exactly one pen collection, and report why candidates fail. |
| Device session | Own the HID handle, read operation, reusable report buffer, cancellation, and connection state. |
| Protocol | Pure checked parsing into fixed-size pen report data; no Windows API calls. |
| Pen state | Convert reports into cursor and button transitions; release held buttons on loss of proximity or session end. |
| Mapping | Transform raw X/Y into a configured physical desktop rectangle, clamp safely, and respond to display topology changes. |
| Output | Own SendInput calls and return-value handling; preserve event order. |
| Supervisor | Discover, connect, wait, reconnect, stop, and emit health status. |
| Configuration/diagnostics | Parse a minimal startup profile, show selected device and report counters, and support opt-in raw capture with sensitive identifiers redacted. |

Do not introduce a generic tablet abstraction, plugin interface, dependency-injection framework, async runtime, or cross-platform backend in the first release. Keep Windows-specific unsafe FFI in narrow wrappers and parsing/mapping/state logic safe and testable.

## 4. Windows HID acquisition

1. Register for HID interface arrival/removal notifications, then enumerate already-present interfaces. Deduplicate the same arrival that may appear in both places.
2. Enumerate **GUID_DEVINTERFACE_HID** collections using supported Windows device-interface APIs. Obtain each device path without relying on the string format of that path.
3. Open candidate collections, obtain HID attributes and preparsed capabilities, and filter by vendor 0x056A, product 0x0357, and **InputReportByteLength = 192** for the pen collection. Treat the 44-byte collection as auxiliary. Record usage page/usage, serial/device-instance information, and the selected path for diagnostics; do not use a path regex as the sole identity test.
4. On a machine with multiple PTH-660 devices, require an explicit device selection or a stable documented tie-break. Never mix two devices' pen/button state.
5. Open the selected pen collection for input with an appropriate sharing mode and a dedicated cancellable read operation. Validate what actual Windows HID access permits on the target machine. An access-denied or sharing violation is a recoverable state with actionable diagnostics.
6. Allocate the report buffer from the collection's reported maximum input length. That length includes the report ID. Reuse the buffer rather than allocate per report.
7. Read on event completion, not by periodic polling. One outstanding read is sufficient initially. Keep buffer and overlapped-operation storage alive until completion, including after cancellation.
8. Never install WinUSB automatically. Upstream's PTH-660 entry is not marked as requiring WinUSB; altering a device driver is outside this milestone.

Microsoft documents [HID collection discovery/opening](https://learn.microsoft.com/en-us/windows-hardware/drivers/hid/finding-and-opening-a-hid-collection), [HID capabilities and report lengths](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/hidpi/ns-hidpi-_hidp_caps), [arrival/removal notifications](https://learn.microsoft.com/en-us/windows-hardware/drivers/install/registering-for-notification-of-device-interface-arrival-and-device-removal), and [cancellable I/O](https://learn.microsoft.com/en-us/windows/win32/api/ioapiset/nf-ioapiset-cancelioex).

## 5. Protocol and pen state

The parser should return a compact, typed result with report kind, raw X/Y, pressure, proximity, contact candidate, two side-button bits, eraser, tilt, rotation where available, and hover distance where available. Use explicit little-endian reads and checked slices. Unknown IDs are counted and ignored; malformed known IDs are counted and ignored. No malformed report may inject a cursor move or button transition.

Clamp or reject raw coordinates outside the configured hardware range using a policy chosen after captures. Do not wrap integers, index beyond a short report, or trust a report merely because its buffer has the collection's maximum size. Maintain a clear distinction between raw report length, returned byte count, and ID-specific minimum length.

The pen state machine owns the previous proximity/contact/side-button state. It emits only changes for buttons, preserves the order of cursor movement and button transitions within one report, and never repeats button-down for a held button. A tip-contact policy may use pressure > 0 if captures confirm that this matches the tablet's contact behavior; otherwise use the verified report flag. Configure side-button mappings as right and middle mouse buttons by default, with the option to disable either.

When proximity is lost, or the HID session ends, synthesize releases for every button held by this daemon exactly once. Avoid moving the cursor from stale/out-of-proximity coordinates. After reconnect, start from a neutral state rather than carrying over an old pressure or button bit. Eraser/tilt/rotation/hover distance remain parsed state and diagnostics in release one, not fabricated mouse events.

Protocol tests should be fixture-driven: captured 0x10 and 0x1E samples for hover, contact, both side buttons, release, edge coordinates, and proximity loss. Add short/truncated/unknown/out-of-range cases. Test button-transition sequences separately from report decoding. Do not use fixtures copied from the parser implementation as the only evidence.

## 6. Desktop mapping and output

Default mapping: full tablet active range to the full Windows virtual desktop rectangle. The minimal startup profile may choose one display and an input crop/rotation, but full-area/full-desktop must work with no configuration. Explicitly clamp transformed coordinates to the selected output rectangle.

Retrieve virtual-screen origin and size, including negative monitor coordinates. Convert pixel positions to the normalized 0..65535 absolute mouse coordinate space and set the virtual-desktop flag. Define and test edge rounding so all four tablet corners reach the corresponding desktop edges. Recompute mapping when monitor topology or resolution changes; do not require the user to restart the daemon to fix a changed layout.

Send one ordered batch of movement and button events per report when possible, and no event when nothing changed. Check SendInput's returned event count. Track requested button state separately from successfully emitted button state so a failed batch can be retried or reconciled without a silent stuck-button assumption; log failures at a controlled rate. Run in the user's desktop session at ordinary integrity level; SendInput may not inject into applications at higher integrity level.

For release one, mouse output is an intentional capability boundary. The raw tablet pressure range should still be decoded correctly, but Windows Ink pressure/tilt/eraser support requires a separate output design. Do not label the mouse-only deliverable as a full pen-pressure driver.

References: [OpenTabletDriver's Windows absolute pointer](https://github.com/OpenTabletDriver/OpenTabletDriver/blob/fdeaa7b0c6d6f5260f511f19fb693ed33524af4e/OpenTabletDriver.Desktop/Interop/Input/Absolute/WindowsAbsolutePointer.cs), [mouse injection wrapper](https://github.com/OpenTabletDriver/OpenTabletDriver/blob/fdeaa7b0c6d6f5260f511f19fb693ed33524af4e/OpenTabletDriver.Desktop/Interop/Input/WindowsVirtualMouse.cs), [MOUSEINPUT coordinate semantics](https://learn.microsoft.com/en-us/windows/win32/api/winuser/ns-winuser-mouseinput), [virtual-screen metrics](https://learn.microsoft.com/en-us/windows/win32/gdi/multiple-monitor-system-metrics), [SendInput behavior and integrity restriction](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-sendinput), and [OpenTabletDriver's Windows pressure FAQ](https://opentabletdriver.net/Wiki/FAQ/Windows).

## 7. Session lifecycle and failure handling

Use explicit states: **waiting -> opening -> reading -> disconnected -> waiting**, with a separate stopping state. The supervisor starts listening for notifications before initial enumeration. Opening failures are reported and retried when a relevant device-change event occurs or after a modest backoff; the daemon must not spin while a device is unavailable.

Removal, failed reads, or a stale handle must cancel/complete outstanding I/O, release held mouse buttons, close the device handle, clear parser and pen state, and return to waiting. On reconnect, inspect attributes/capabilities again. The daemon should not crash when unplug happens mid-read or during shutdown. The cancellation path must wait for I/O completion before freeing its buffer and operation memory.

Ctrl+C and normal process termination should request stop, cancel reads, release buttons, unregister notifications, close handles, and exit. A forced process kill cannot guarantee a release event; document that limit and ensure normal stop paths are reliable. Sleep/wake should revalidate the handle and rediscover if it is stale.

Only one instance may inject for a selected device. An instance guard should prevent duplicate launches of this Rust daemon. Packaging instructions must tell the user to stop OpenTabletDriver and any other tablet daemon before live testing, because two active pointer injectors produce duplicate movement.

## 8. Performance design

Efficiency is an architectural requirement for the implementation, with concrete invariants rather than a comparison to the C# daemon:

- No allocation, heap cloning, formatting, or logging on the successful per-report path.
- No periodic read polling or artificial sleep in the active pen path.
- One reusable HID buffer and fixed-size parsed report/state structures.
- No unbounded queue. Prefer direct read-to-output processing; if a queue becomes necessary, bound it and preserve button transitions even when superseded position reports are discarded.
- Avoid redundant SendInput calls when position and button state have not changed.
- Keep discovery, configuration parsing, verbose diagnostics, and monitor enumeration off the hot path.
- Do not raise process priority or alter timer resolution by default. First prove any such setting solves a specific scheduling problem.

Instrument internal counters for reports read, accepted, malformed, ignored IDs, emitted mouse events, read failures, reconnects, and maximum processing time. Keep tracing opt-in and separate from normal operation. Functional acceptance is based on correct real-device behavior and the hot-path invariants above.

## 9. Configuration and user operation

The first executable should start with sensible defaults and a small documented profile. Limit configuration to the device selection, output target (full desktop or chosen monitor), optional input crop/rotation, side-button actions, and diagnostics level. Parse it once at startup; dynamic settings reload can be deferred. Reject invalid crop dimensions, monitor IDs, and button mappings with a clear error before opening the device.

Provide read-only commands or modes for listing detected candidate collections and showing their VID/PID, input report length, usage, and openability; for showing the active mapping; and for collecting a bounded, opt-in report trace. Raw traces should omit serial numbers and user-specific paths by default. No GUI, RPC, plugin settings, or network service is needed.

The initial install package should contain the single executable, example profile, a concise Windows 11 setup guide, and uninstall/rollback instructions. Run as the signed-in user. Do not change Windows HID drivers, startup registration, or other tablet-driver installations during normal install. Autostart is an optional later packaging feature.

## 10. Work breakdown for agents

Each item can become a narrow PR with a clear review boundary. Agent work should depend on the preceding protocol and API contracts instead of all agents editing the executable entrypoint.

| Order | Deliverable | Completion gate |
| --- | --- | --- |
| P0 | Repository foundation: Rust workspace, toolchain policy, license decision, CI, format/lint/build checks, and module contract notes. | Clean Windows build and documented source provenance; no device behavior claimed. |
| P1 | HID discovery/diagnostic mode and hardware capture procedure. | Lists the real PTH-660 pen collection and distinguishes it from the 44-byte auxiliary collection; records actual report IDs and byte counts. |
| P2 | Pure protocol parser plus captured fixtures. | Correct typed values and robust rejection of short, unknown, and invalid reports. |
| P3 | Pen state machine and desktop mapping. | Deterministic corner mapping and exactly-once button transitions/release in sequence tests. |
| P4 | Windows HID read session and SendInput output. | Cursor moves, tip click, side buttons work with the real tablet; no per-report allocation or busy loop. |
| P5 | Notifications, replug, shutdown, sleep/wake, and failure diagnostics. | Survives repeated cable unplug/replug and shutdown during a pending read; releases held buttons. |
| P6 | Configuration, package, user guide, and release checklist. | A fresh Windows 11 user can run and remove it without changing system HID drivers. |

Separate agents may work on pure protocol fixtures, Windows HID API wrappers, and coordinate/state tests after the P1 hardware findings and shared contracts are published. Only one PR should own the read-to-output integration at a time.

## 11. Acceptance checklist for the first usable release

**Device and input**

- Detects only USB PTH-660 product 0x0357, selects the 192-byte pen collection, and does not select Bluetooth product 0x0360 or the 44-byte auxiliary collection as the pen.
- A no-config launch moves the cursor smoothly over the whole virtual desktop when the pen is in proximity.
- Tablet corners map to desktop edges; a negative-origin secondary monitor works.
- Tip contact causes one left-button down, lift causes one up; each side button causes one mapped down/up; releases occur on proximity loss and normal disconnect.
- Pressure, tilt, eraser, proximity, and coordinates decoded from captured reports match the observed device values, even though only mouse events are emitted.

**Recovery**

- Starting without a tablet waits without spinning; plugging it in starts input.
- Repeated unplug/replug works without restart; unplug during contact does not leave a mouse button held.
- Normal stop during a pending read completes cleanly and does not leave a mouse button held.
- Unknown IDs, short reports, access denied, and SendInput failure are counted or reported without panic.
- Display layout changes refresh the mapping.

**Implementation quality**

- Parser/state/mapping tests use independent captures and sequence fixtures.
- Windows API wrappers handle every owned handle and registration exactly once.
- Format, lint, build, and relevant tests pass in CI.
- A release build has no successful-report-path allocation, no active-path sleep, and no unbounded input queue.
- The README and setup guide state the mouse-only pressure limitation and how to stop another tablet daemon during use.

## 12. Decisions deliberately reserved for evidence

- Exact contact semantics: pressure > 0 versus a device flag, to be settled from real hover/contact captures.
- Exact HID open mode and overlapped-read wrapper, to be validated on Windows 11 with this device.
- Whether 0x1E is needed during normal operation or only recognized for robustness.
- Whether a Rust Windows API crate or a very small direct FFI layer gives the clearest audited implementation. Select after the API surface is fixed; pin versions.
- License for this private repository and treatment of any upstream code reuse. Record the upstream LGPL-3.0 source and make an explicit decision before copying code or publishing binaries.

The first release is complete when the acceptance checklist passes on an actual PTH-660/Windows 11 setup. Any future Linux, Bluetooth, Windows Ink, tablet-key, or plugin work should be planned as a distinct milestone after that.
