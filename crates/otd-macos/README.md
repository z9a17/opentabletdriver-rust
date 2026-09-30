# macOS CLI backend

This crate adds a native macOS CLI using IOKit USB HID input and CoreGraphics mouse output. It uses the shared tablet database, native report decoders, profiles, mapping and session engine. It is a bounded implementation of [X04](../../docs/parity/WORK_ITEMS.md#x04). macOS runtime and physical tablet validation are still pending on Intel and Apple Silicon.

The binaries require macOS 11 or later on Intel and Apple Silicon. Release binaries are unsigned and are not notarized. The build links Apple's IOKit, CoreFoundation, CoreGraphics and ApplicationServices frameworks. A native build requires Xcode Command Line Tools or an equivalent Apple SDK and linker. No third-party native library is required.

## Use

Extract the archive for your architecture, open Terminal in the extracted directory, and run:

```sh
./opentabletdriver-rust-macos --version
./opentabletdriver-rust-macos list
./opentabletdriver-rust-macos displays
./opentabletdriver-rust-macos capture --seconds 10 --limit 2000
./opentabletdriver-rust-macos run
./opentabletdriver-rust-macos run --profile /path/to/profile.toml --tablet "Wacom PTH-660"
```

Grant Input Monitoring to Terminal, or the application responsible for launching the CLI, under System Settings > Privacy & Security > Input Monitoring. Quit and restart that application after changing permissions. `run` also requires Accessibility permission in the same settings area. `capture` does not create or post mouse events. It may send the selected tablet's required feature/output initialization reports.

macOS may block unsigned downloaded executables through Gatekeeper. Review the downloaded source and release before allowing it through the system's Open Anyway workflow. Distribution signing and notarization remain open work.

`run` selects one matching USB digitizer and waits for supported hardware. It rescans every two seconds after a disconnect. Ctrl+C and SIGTERM end the session and request release of held contact. `capture` stops after its report limit or its deadline, including the time spent waiting for hardware. `--seconds` accepts 1 to 60 seconds and defaults to 10; `--limit` defaults to 2000 reports. It prints the shared capture diagnostics to stderr. Capture proves received/decoded reports only; it is not proof of live cursor output.

Display mapping uses the CoreGraphics global desktop coordinate space, including negative monitor origins and Retina logical points. `displays` prints the sorted monitor order used by profile monitor indices. `--screen WIDTHxHEIGHT` overrides discovery with one rectangle at the desktop origin. The session checks aggregate desktop bounds and monitor count once a second. Rearrangements that preserve both may need a driver restart.

## Implemented behavior and limits

- USB HID service enumeration and matching on VID/PID, normalized report lengths and USB interface number where the registry provides it.
- Shared decoder support, absolute and relative mouse movement, tip/eraser contact as the left mouse button, and physical keyboard modifiers sampled when a mouse event is posted.
- Pen side buttons use the shared profile bindings. Defaults click right and middle; explicit bindings support all five mouse buttons and ordinary physical keys/chords through CoreGraphics. Pointer movement uses the appropriate left/right/other drag event while a button is held. Tip contact and a side-button left binding share ownership, so releasing either preserves the other's press.
- Feature and output initialization writes through asynchronous IOKit calls, with a native one-second timeout and a two-second callback deadline. Report callbacks use a fixed 32-report queue and 4096-byte report bound. Oversized reports and overflow stop the session instead of losing reports silently. Callbacks allocate no Rust heap memory.
- Input Monitoring and Accessibility diagnostics. HID services are enumerated without opening unrelated keyboards or mice. Only the selected digitizer is opened, with upstream-style shared access.

Indexed USB string descriptors and initialization string requests are unsupported. Configurations that require them cannot run. The CLI reports the missing operation instead of guessing descriptor indices or skipping initialization. Bluetooth and other transports, auxiliary endpoints, pad bindings, multi-tablet sessions, GUI, daemon, external plugins, tablet pressure/tilt output, Artist Mode, startup registration and native sleep/wake handling remain unimplemented. Mouse clicks use a single-click count; native double-click counting is not implemented. Competing tablet drivers can still consume input because the CLI does not seize the device.

Keyboard bindings use physical positions. `Application` means Command, `Alt` means Option, and `Control` stays Control. Use `keys:Application+Z` for the usual Mac undo shortcut. Letters, punctuation, navigation, keypad keys, F1 through F20 and left/right modifiers are available. CapsLock, NumberLock, Insert, ContextMenu, Help, PrintScreen, ScrollLock, Pause, F21 through F24 and media keys are rejected with a startup diagnostic. Both left/right modifiers can be held independently. Synthetic modifier flags are tracked immediately because the CoreGraphics system snapshot can lag; after a release, stale flags are suppressed until the snapshot clears, for at most 50 ms. A newly pressed physical modifier may briefly share that suppression window. Holding the same physical and synthetic modifier simultaneously is unsupported because the combined snapshot cannot distinguish them. Modifier and shortcut delivery require native validation.

Mouse and keyboard output use CGEventPost, which has no success return value. Permission preflight cannot prove that an application accepted an event. Relative output queries the system cursor with a native CGEvent allocation, and each key transition creates a native keyboard event so the current keyboard layout translates it correctly. The successful Rust report callback and report processing path do not allocate Rust heap memory. Native framework allocation behavior and performance require macOS measurements.

No format, clippy or test checks were run for this change, following the repository's release policy. Cross-linking a release binary does not validate native permissions, pen input, cursor movement, disconnect/replug, Retina mapping, sleep/wake or application behavior. Those are still required macOS validation tasks.

## Upstream references

The backend follows OpenTabletDriver 0.6.7 at commit `736003ed72c8bbb28033b039d5a0bb76c344145c`:

- [HidSharpEndpoint and macOS interface matching](https://github.com/OpenTabletDriver/OpenTabletDriver/blob/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver/Devices/HidSharpBackend/HidSharpEndpoint.cs) and [HidSharpEndpointStream](https://github.com/OpenTabletDriver/OpenTabletDriver/blob/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver/Devices/HidSharpBackend/HidSharpEndpointStream.cs).
- [Absolute pointer](https://github.com/OpenTabletDriver/OpenTabletDriver/blob/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.Desktop/Interop/Input/Absolute/MacOSAbsolutePointer.cs), [relative pointer](https://github.com/OpenTabletDriver/OpenTabletDriver/blob/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.Desktop/Interop/Input/Relative/MacOSRelativePointer.cs), [virtual mouse](https://github.com/OpenTabletDriver/OpenTabletDriver/blob/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.Desktop/Interop/Input/MacOSVirtualMouse.cs) and [displays](https://github.com/OpenTabletDriver/OpenTabletDriver/blob/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.Desktop/Interop/Display/MacOSDisplay.cs).
- [PermissionHelper and responsible application permissions](https://github.com/OpenTabletDriver/OpenTabletDriver/blob/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.UX.MacOS/PermissionHelper.cs).

The descriptor-length parser was copied from the repository's Linux backend. Its leading zero-byte normalization follows HidSharp's macOS stream behavior for unnumbered reports. This CLI does not implement the upstream pressure/tilt/proximity event additions or Cocoa permission dialogs.
