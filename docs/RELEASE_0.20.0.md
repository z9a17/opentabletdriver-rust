# OpenTabletDriver Rust 0.20.0

## Faster pen click for imported settings

Settings imported from OpenTabletDriver usually bind the pen tip to
OpenTabletDriver's own "Mouse Button Binding: Left" (or "Adaptive Binding:
Tip"). Until now the driver ran that binding through the .NET bridge, so
every pen report, about 200 per second on a PTH-660, crossed into .NET and
back, and the .NET runtime was loaded into the report thread. With mouse
output these bindings press the left button exactly like the built-in tip,
so the driver now runs them natively:

- When the driver starts, such a tip or eraser binding from
  `OpenTabletDriver.Desktop.dll` runs as the built-in left click. The Console
  logs it. Saved settings are not rewritten. `OTD_DISABLE_NATIVE_PORTS=1`
  keeps the .NET binding, as for Radial Follow.
- New imports store it as the built-in click.
- The Pen tab shows it as **Left click** instead of "Managed: Mouse Button
  Binding". Other plugin bindings, buttons and output modes show as
  "Plugin: <name>" instead of "Managed: <name>".

Together with the existing native Radial Follow port, a typical imported osu!
profile (absolute mode, tablet-space Radial Follow, left-click tip) no longer
loads .NET at all. No physical pen-to-display latency was measured; this
removes work from every report, it is not a measured latency claim.

## Clearer panel

- The status bar shows short messages such as "Starting the driver...",
  "Applying settings...", "Settings applied." and "Settings saved.". Routine
  technical driver messages go to the Console tab only. Warnings and errors
  still appear in the status bar.
- "Daemon connection unavailable" now reads "Driver not running".
- **Apply** starts the driver with the current settings when it is stopped,
  instead of showing a message.
- Several warnings use plainer wording.

## Also since 0.19.1

- OpenTabletDriver 0.6.8.1 and later upstream fixes: 363 tablet configurations
  (new: CTK-4120, XP-Pen Artist Ultra 14, Gaomon GM156HD 16K, Huion H950P 16K),
  signed IntuosV3 tilt, Giano eraser, scheduler and WinUSB fixes. See
  [0.6.8.1 adoption](parity/UPSTREAM_0.6.8.1.md) and
  [later fixes](parity/UPSTREAM_AFTER_0.6.8.1.md).
- GitHub Actions CI runs format, Clippy and tests on Windows, Linux and macOS.
- A manual "Release build" workflow builds all four packages from one commit
  on GitHub-hosted runners. Linux links against glibc 2.31 with
  cargo-zigbuild; both Mac packages build on an Apple Silicon runner.

## Checks and limits

CI (format, Clippy, tests on Windows, Linux and macOS) passed on the merged
commit, and the packages were built by the Release build workflow and checked
by `scripts/release.py verify` (archive contents, architecture, version,
source revision and SHA-256). New tests cover the native left-click
substitution.

No driver, panel or tablet was started for this release; the panel changes
were not checked on screen. Physical input, hotplug and sleep/wake, Windows Ink
drawing, native Linux/macOS execution and end-to-end latency remain
unverified. Managed features still require the CPU-matching .NET 10 runtime.
Linux targets glibc 2.31+ and its frontend needs GTK3. Native macOS targets
11.0; the packaged .NET apphosts declare 12.0. macOS packages are unsigned and
not notarized. The Windows package is now built with MSVC on a GitHub runner.
