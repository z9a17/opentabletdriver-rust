# PTH-660 Windows 11 hardware validation

The automated parser, state, and mapping tests do not prove that a specific physical tablet moves the Windows cursor correctly. Use this checklist on the PTH-660 over USB before calling the first release complete.

1. Run the list command. Expect a readable 192-byte pen collection, a 44-byte auxiliary collection, and possibly an unreadable 4-byte mouse collection. The pen collection must have vendor/product 056a:0357.
2. Run capture for 10 to 20 seconds. Hover and move the pen, then tap, hold, and lift. Check that X/Y change within 0..44800 and 0..29600, pressure rises during contact and returns to zero on lift, and `in_range` and then `sense` clear as the pen rises and leaves. Save only the relevant report prefixes as fixtures and avoid publishing HID paths or serial numbers.
3. Run `opentabletdriver-rust.exe settings` and compare its effective tablet area, display area, clipping, and tip threshold with the active OpenTabletDriver PTH-660 profile. Double-click the console executable to pause the original daemon/UX and run Rust. Check that the current cropped tablet area maps to the intended display edges; use an independent TOML profile for full-virtual-desktop and negative-origin monitor tests.
4. Tap and lift repeatedly. Check exactly one left-button down/up pair per contact and a release after leaving proximity.
5. Unplug and replug during hover and during tip hold. Confirm that the process stays alive, resumes without a restart, and releases a held left mouse button. Stop with Ctrl+C during contact and during an idle pending read.
6. Change monitor resolution or topology while the process runs. Confirm the mapping refreshes within a few seconds. If a profile targets a monitor that disappears, verify the driver does not move into a stale rectangle.
7. Confirm that pressure, tilt, and eraser values are diagnostic only. Drawing applications should not be told that this first release provides Windows Ink pressure.

Record the Windows build, PTH-660 firmware/connection mode, whether another tablet driver was running, and the result of each step in a hardware-validation issue or PR. A failed step is a release blocker for the first usable driver.

## Relative mode validation (pending)

1. Preview `settings --config driver.relative.example.toml`, then run the same profile. Confirm the first hover does not jump to an absolute screen position and tip clicks still work.
2. Compare one-mm horizontal/vertical movements at 10/10 sensitivity, then unequal sensitivities and 90-degree rotation. Record Windows pointer speed and acceleration settings; they affect relative output.
3. Move slowly to verify sub-count movement accumulates; repeat equal steps to check that equal nonzero deltas are not suppressed.
4. Leave proximity and return at another tablet position, both quickly and after 100 ms. Confirm no cross-tablet jump or leftover fractional movement. Test pauses longer than the configured reset delay as well.
5. Hold the tip, unplug, reconnect, and stop with Ctrl+C. Confirm releases and that the first report after reconnect establishes a new origin.
6. Import an OpenTabletDriver relative profile with Radial Follow enabled and compare filtering and pressure thresholds. Change display topology during relative output and verify continuous motion without a reset.

Automated recorded-report replay and allocation checks cover the new CPU path, but these live relative-mode checks have not yet been performed.

## Input latency and hover validation (pending)

These checks cover the 0.7.0 changes described in [input latency](INPUT_LATENCY.md). Run OpenTabletDriver and the Rust driver one at a time; starting the Rust driver pauses OpenTabletDriver.

1. Raise the pen slowly from the surface and note the height at which the cursor stops following. Repeat with OpenTabletDriver. The heights should match; before 0.7.0 the Rust driver stopped lower.
2. Run `capture --seconds 20` while raising and lowering the pen. Record the `in_range`/`sense` transitions and whether any report arrives with both false; if one does, record its coordinates.
3. Load every CPU (for example, a parallel build) and move the pen quickly. Compare the cursor with OpenTabletDriver under the same load. Stop the driver and record the Console's `Processed ... reports` line.
4. With a 1 % tip threshold, press lightly and compare the click point with OpenTabletDriver.

## Development-machine results

- Windows 11 USB PTH-660 enumeration: one readable 192-byte pen collection, one readable 44-byte auxiliary collection, and one unreadable 4-byte mouse collection, all 056a:0357.
- A read-only pen capture verified hover, tip contact, lift, and proximity loss. Captured 0x10 report prefixes are checked into parser tests. A 0x1e report has not yet been observed on this tablet.
- For a live run, OpenTabletDriver was stopped temporarily and restored afterward. The user confirmed that the Rust driver moved the cursor with the pen and produced a normal click on tap. The session counted 1,492 reads, 1,472 accepted pen reports, 20 ignored reports, no malformed reports, 1,413 mouse injections, and no SendInput failures.
- The renamed console build read the active OpenTabletDriver profile, reported its 85 × 47.8125 mm input area, 2560 × 1440 px output area, clipping, 1% tip threshold, and one skipped enabled filter. A no-argument console start paused the original daemon and restored it after Ctrl+C. With the packaged build running in a visible console, the user confirmed that pen movement followed the current tablet mapping and tapping produced a click.
- One unplug/replug test showed the daemon entering its waiting state and reopening the 192-byte pen collection when Windows presented it again. Windows briefly reset the collection after the first return; the daemon reopened it a second time without exiting. The pen was not moved after replug, so resumed cursor input is not confirmed by that test.
- Repeated unplug/replug with pen input after reconnect, hold-contact unplug, sleep/wake, and display-topology changes have not yet been validated on hardware.
- The 0.3.0 Radial Follow port uses the current saved tablet-space parameters and matches the original C# core's radial curve at ten reference distances. It has not been run against live pen movement or clicks: the active 0.2.0 driver was left running at the user's request. On the first 0.3.0 launch, compare slow movements inside the 0.302 mm inner radius, larger jumps past the 0.7039 mm outer radius, and pen redetection after at least 50 ms with OpenTabletDriver's original filter.


## Pen side buttons (pending)

Needs a Windows build with [pen side buttons](PEN_BUTTONS.md). Replay tests cover the logic; these steps check it on a tablet.

1. With the default profile, press the first side button while hovering over a text field: a context menu opens where the pen is. The second button should middle click (paste on Linux, autoscroll in browsers).
2. Hold a side button, move the pen out of range and back. The click must have been released when the pen left: no stuck menu, no drag.
3. Hold a side button while touching with the tip. The button must not change the tip's click.
4. Set `pen_buttons = ["keys:Control+Z", "keys:Escape", "mouse:forward"]` and check each in a text editor or browser. A chord must not leave Ctrl held after release, including when the pen leaves range or you press Ctrl+C in the console.
5. In pen output, open a drawing application: a side button should arrive as the pen's barrel button (most applications list it as a stylus or pen button you can assign). Windows pens have one barrel button, so buttons 1 to 3 all act as it.
6. Unplug the tablet while a button is held and confirm nothing stays pressed.
7. Report the tablet model, the Windows build, and any button that does the wrong thing.
