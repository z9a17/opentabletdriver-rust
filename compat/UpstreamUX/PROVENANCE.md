# Original desktop frontend

The C# frontend, original assets, GTK launcher (`../UpstreamUX.Gtk`) and macOS
launcher (`../UpstreamUX.MacOS`) originate from OpenTabletDriver revision
`736003ed72c8bbb28033b039d5a0bb76c344145c` (0.6.7). They retain upstream
copyrights and LGPL-3.0-or-later attribution; the upstream license is included as
`LICENSE`. This is a selectively updated source host, not an unchanged upstream
frontend or complete copy of the RC tree.

The maintained `App.cs` integration adds `PluginPlatform.Linux` to the existing
`EnableDaemonWatchdog` platform mask. The packaged GTK frontend can therefore
start the native daemon forwarder when the original daemon instance is absent;
Windows/macOS watchdog policy is retained. Projects reference the vendored
Desktop/Core/Plugin hosts and now target net10.0 with assembly version 0.6.8.
Original assembly and resource names are retained. GTK still uses Eto.Platform.Gtk
2.5.11; shared UX and the macOS launcher retain their Eto 2.5.10 dependencies.
Lock files record the actual closure.

## Selective RC adoption

Reference: [OpenTabletDriver v0.6.8-rc1, `126f4828564abb673d39e97a093fd815aab335c2`](https://github.com/OpenTabletDriver/OpenTabletDriver/tree/126f4828564abb673d39e97a093fd815aab335c2).

- The area editor recomputes aspect locking from the final display dimensions
  and fits Full/Quarter tablet areas to rotated physical bounds. Compound resize
  operations suppress intermediate aspect corrections; missing bounds leave
  tablet edits intact. The fit uses equivalent finite edge constraints at
  cardinal angles, avoiding infinite slope calculations.
- Binding editors use cloned cancellation snapshots and private drafts. The
  advanced editor restores original settings when switching back to the original
  binding type. The `PluginSettingStore.Clone()` helper lives in the sibling
  Desktop source host.
- Device string dumps skip VID 10429/index 200 by default, with an unchecked
  dangerous-string opt-in and fixed device IDs/opt-in during each bulk operation.
  Deliberate single-index requests retain their existing behavior.
- Unvalidated string properties display the SDK placeholder attribute without
  changing stored/default settings.

The RC's rotated-background presentation is intentionally not adopted; existing
canvas appearance is preserved. The native Windows frontend implements its own
corresponding controls and daemon version warning; those Rust changes do not
imply every new upstream UX feature was copied here.

Project artwork overrides: `Assets/otd.ico` is a copy of the project's
`resources/opentabletdriver.ico`, and `Assets/otd.png` is its 256-pixel PNG image.
Both derive from the project's SVG sources and use GPL-3.0-only, as recorded in
the root `NOTICE.md`. The existing manifest resource names are retained so
the original frontend uses the new logo without changing its code contracts.

`compat/NativeDaemonLauncher` is Rust-project code. It occupies the daemon
executable name expected by the original watchdog and launches the native Rust
multi-device owner with the exact `OpenTabletDriver.Daemon` hosted RPC pipe
and `--owner-stdin` lifetime contract. If the watchdog kills the forwarder,
its redirected input pipe closes and the native owner drains readers/output.
It does not contain the upstream hardware daemon.

GTK requires its real native GTK3 dependencies. The macOS Eto/MonoMac native
assets must support the target architecture. An x64 or arm64 cross-build is
not native GUI, permission, input or hardware validation. No frontend was
launched or validated during this implementation.
