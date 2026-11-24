# OpenTabletDriver Rust 0.22.1

## Device removal fix and fast-path verification

This patch release fixes the targeted device-removal watcher introduced in
0.22.0 and adds tests that check the 0.22.0 fast report paths against the
original report pipeline.

### Fixes

- **Targeted device removal now fires (Windows):** 0.22.0's
  `DeviceRemovalNotification` only reacted to
  `CM_NOTIFY_ACTION_DEVICEREMOVECOMPLETE`, which Windows sends only to
  device-handle registrations. The watcher registers a device-interface
  filter, which reports removal as
  `CM_NOTIFY_ACTION_DEVICEINTERFACEREMOVAL`
  ([CM_NOTIFY_ACTION](https://learn.microsoft.com/windows/win32/api/cfgmgr32/ne-cfgmgr32-cm_notify_action)),
  so the callback never signalled. Unplugging a tablet still ended its
  session through the failed overlapped read, but the watcher did nothing.
  The general HID arrival/removal notification had the same mismatch and only
  ever woke on arrivals; it now also wakes on removals and then checks whether
  the tablet is still present, as intended.

### Tests

- The existing compact pen parity test compared the fast path with itself,
  because `process_with_raw` now takes the compact fast path too. Its
  reference pipelines are now forced onto the original full path.
- New `fast_paths_match_forced_full_path_on_randomized_reports`: about 20,000
  randomized PTH-660 reports (moves, pressure crossings, barrel buttons, range
  loss and timing gaps) through absolute, relative and Windows Ink output, each
  with and without Radial Follow. Every mouse packet, button action and pen
  packet must match the forced full path. They do.
- New `compact_decoders_match_generic_reduction_on_fuzzed_packets`: 200,000
  fuzzed packets through each of the 17 compact vendor decoders, compared field
  by field with the generic decode as the full path reduces it. They match.

There are no changes to the report path itself, so the 0.22.0 offline
benchmark results still apply.

## Checks and limits

CI (format, Clippy, and tests on Windows, Linux and macOS) passed on the pull
request. Locally on Windows 11, `cargo fmt --check` and
`cargo clippy --workspace --all-targets --locked` passed, and otd-core's
262 tests passed. The only failing local test,
`download::tests::a_setup_script_picks_the_proxy_per_host`, depends on the
machine's WinHTTP proxy configuration; it fails the same way on 0.22.0 and its
file is unchanged. The four release packages are built by the Release build
workflow from the merged commit and checked with `scripts/release.py verify`
before publishing.

No physical tablet, live driver, panel or game was started for this release.
Device unplug/replug with the fixed watcher, physical aim feel, end-to-end
latency, sleep/wake, Windows Ink drawing, and native Linux/macOS runtime
execution still require manual testing on real hardware. Managed features
require the CPU-matching .NET 10 runtime; Linux targets glibc 2.31+ and its
frontend needs GTK3; native macOS targets 11.0 while packaged .NET apphosts
declare 12.0 and remain unsigned/not notarized.
