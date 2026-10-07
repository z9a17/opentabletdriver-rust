# OpenTabletDriver Rust 0.18.0

This release finishes the remaining implementation work for the pinned
OpenTabletDriver 0.6.7 baseline. Native Linux/macOS, tablet/application and
unchanged plugin corpus validation remain deferred at the owner's request.

- Original plugins can access concrete Core/device/hub services, shared reader
  streams, actual feature/write/string I/O and registry contexts without a
  second physical reader. Global tools run independently of tablet attachment.
- Background managed reports are owned and scheduled on the report thread.
  Input holders share acknowledged platform ownership; retirement receipts
  cover readers, timers, callbacks, tools and delegated output.
- Windows adds original collection/default/preset workflows, a setup guide,
  device-string reader, original diagnostics, complete original key handling
  and foreground daemon services. Blue remains the default accent.
- Linux and both Macs include native multi-device ownership, auxiliary/custom
  sources, original Gtk/Eto frontends, original Console and managed hosting.
  Linux uses GNU dynamic linking; .NET/GTK and platform permissions are required
  for their respective managed/graphical workflows.
- Generic Wayland desktops use the original read-only display provider when
  native discovery is unavailable. Unix discovery includes Bluetooth HID;
  macOS ordinary mouse modes carry tablet attributes and native click counts.
- Updates drain actual readers and tools before replacing files and retire the
  daemon after the successful original RPC response has finished writing.

See [implementation scope, commands and boundaries](https://github.com/z9a17/opentabletdriver-rust/blob/v0.18.0/docs/parity/IMPLEMENTATION_0.18.md).

Windows CLI migration: original Console names now use the live original JSON
collection: `save`, `save-defaults`, `stdio`, `preset`, `savepreset`,
`listpresets`, `log`, `detect`, `getstring`, original `get*`/`list*` commands,
`hasupdate`, `installupdate` and non-TOML `load`. Native scripts retain
`native-save`, `native-save-defaults`, `native-stdio`, and the existing
`profiles`/`presets`/`devices` subcommands; explicit `.toml` load/save stays
native. See the [migration table](https://github.com/z9a17/opentabletdriver-rust/blob/v0.18.0/docs/REFERENCE.md#windows-console-migration-in-018).

The default explicitly enabled `--upstream-rpc` listener is now
`OpenTabletDriver.Daemon` on Windows and Unix. To retain the earlier custom
endpoint, pass `--upstream-pipe OpenTabletDriverRust.Compat` (on Windows also
pass `--upstream-rpc`). Unchanged original clients use the fixed default name.
Original fixed RPC hosting, registered types/default constructors, Unix
Console/frontends and managed plugins require .NET 8; native-only profiles/UI
remain CLR-free unless a managed feature is requested. Generic Wayland display
discovery uses the original managed provider; native Hyprland/Sway/X11 or
explicit `--screen` bypasses it. Source builds require a real Git checkout.

Four archives are built from one clean merged commit: Windows x64, Linux x64,
macOS Intel x64 and macOS Apple Silicon arm64. Their hashes, binary architecture,
managed dependency/resource/license closure and source provenance are checked.

No tests, format/Clippy/build-check suites or CI were run. No driver, daemon,
UI, plugin, game, OBS or hardware session was launched. The packages are compiled
for manual validation; this release is not a certificate of fully tested parity
or a claim of measured input latency improvement. macOS packages are unsigned
and not notarized. Existing plugin platform and external-driver prerequisites
still apply.
