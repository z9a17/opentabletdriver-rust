# PTH-660 Windows 11 hardware validation

The automated parser, state, and mapping tests do not prove that a specific physical tablet moves the Windows cursor correctly. Use this checklist on the PTH-660 over USB before calling the first release complete.

1. Run the list command. Expect a readable 192-byte pen collection, a 44-byte auxiliary collection, and possibly an unreadable 4-byte mouse collection. The pen collection must have vendor/product 056a:0357.
2. Run capture for 10 to 20 seconds. Hover and move the pen, then tap, hold, and lift. Check that X/Y change within 0..44800 and 0..29600, pressure rises during contact and returns to zero on lift, and proximity eventually clears. Save only the relevant report prefixes as fixtures and avoid publishing HID paths or serial numbers.
3. Run `opentabletdriver-rust.exe settings` and compare its effective tablet area, display area, clipping, and tip threshold with the active OpenTabletDriver PTH-660 profile. Double-click the console executable to pause the original daemon/UX and run Rust. Check that the current cropped tablet area maps to the intended display edges; use an independent TOML profile for full-virtual-desktop and negative-origin monitor tests.
4. Tap and lift repeatedly. Check exactly one left-button down/up pair per contact and a release after leaving proximity.
5. Unplug and replug during hover and during tip hold. Confirm that the process stays alive, resumes without a restart, and releases a held left mouse button. Stop with Ctrl+C during contact and during an idle pending read.
6. Change monitor resolution or topology while the process runs. Confirm the mapping refreshes within a few seconds. If a profile targets a monitor that disappears, verify the driver does not move into a stale rectangle.
7. Confirm that pressure, tilt, and eraser values are diagnostic only. Drawing applications should not be told that this first release provides Windows Ink pressure.

Record the Windows build, PTH-660 firmware/connection mode, whether another tablet driver was running, and the result of each step in a hardware-validation issue or PR. A failed step is a release blocker for the first usable driver.

## Development-machine results

- Windows 11 USB PTH-660 enumeration: one readable 192-byte pen collection, one readable 44-byte auxiliary collection, and one unreadable 4-byte mouse collection, all 056a:0357.
- A read-only pen capture verified hover, tip contact, lift, and proximity loss. Captured 0x10 report prefixes are checked into parser tests. A 0x1e report has not yet been observed on this tablet.
- For a live run, OpenTabletDriver was stopped temporarily and restored afterward. The user confirmed that the Rust driver moved the cursor with the pen and produced a normal click on tap. The session counted 1,492 reads, 1,472 accepted pen reports, 20 ignored reports, no malformed reports, 1,413 mouse injections, and no SendInput failures.
- The renamed console build read the active OpenTabletDriver profile, reported its 85 × 47.8125 mm input area, 2560 × 1440 px output area, clipping, 1% tip threshold, and one skipped enabled filter. A no-argument console start paused the original daemon and restored it after Ctrl+C. Pen movement and click with this active-profile build have not yet been confirmed.
- One unplug/replug test showed the daemon entering its waiting state and reopening the 192-byte pen collection when Windows presented it again. Windows briefly reset the collection after the first return; the daemon reopened it a second time without exiting. The pen was not moved after replug, so resumed cursor input is not confirmed by that test.
- Repeated unplug/replug with pen input after reconnect, hold-contact unplug, sleep/wake, and display-topology changes have not yet been validated on hardware.
