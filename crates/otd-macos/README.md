# macOS runtime and original desktop frontend

This crate provides native multi-device ownership using IOKit USB HID input and CoreGraphics mouse/keyboard output, the shared tablet database, native/original parsers, profiles, mapping and session engine. Packages also include the original Eto frontend, original Console, native daemon watchdog forwarder and shared managed host. macOS runtime, frontend and physical tablet validation remain deferred on Intel and Apple Silicon.

The binaries require macOS 11 or later on Intel and Apple Silicon. Release binaries are unsigned and are not notarized. Native input links Apple's IOKit, CoreFoundation, CoreGraphics and ApplicationServices frameworks and needs no .NET runtime. Original frontend, Console and managed plugins need the CPU-matching .NET 8 runtime; plugins retain their platform and external-driver prerequisites. Explicit network operations need system curl. A native build requires Xcode Command Line Tools or an equivalent Apple SDK and linker. Keep the adjacent frontend/launcher assemblies and `data/compat` intact.

## Use

Extract the archive for your architecture, open Terminal in the extracted directory, and run:

```sh
./opentabletdriver-rust-macos --version
./opentabletdriver-rust-macos list
./opentabletdriver-rust-macos displays
./opentabletdriver-rust-macos device-strings 056a 0357 1 2 3
./opentabletdriver-rust-macos capture --seconds 10 --limit 2000
./opentabletdriver-rust-macos run
./opentabletdriver-rust-macos run --profile /path/to/profile.toml --tablet "Wacom PTH-660"
./opentabletdriver-rust-macos ui
./opentabletdriver-rust-macos original-console --help
```

`ui` starts the adjacent original Eto apphost. Its watchdog launches the packaged native daemon forwarder and owns that daemon's lifetime. Without the frontend, start `./opentabletdriver-rust-macos daemon --upstream-rpc` in another terminal before original Console operations. The default original listener is `OpenTabletDriver.Daemon`; `--upstream-pipe NAME` selects a custom endpoint for configurable clients. Plain `run` supplies native daemon services without enabling that original listener. Native `status`, `start`, `stop`, `shutdown`, `detect`, `request` and `console` use the separate native control channel. Start only one driver owner at a time.

Grant Input Monitoring to Terminal, or the application responsible for launching the CLI, under System Settings > Privacy & Security > Input Monitoring. Quit and restart that application after changing permissions. `run` also requires Accessibility permission in the same settings area. `capture` does not create or post mouse events. It may send the selected tablet's required feature/output initialization reports.

macOS may block unsigned downloaded executables through Gatekeeper. Review the downloaded source and release before allowing it through the system's Open Anyway workflow. Distribution signing and notarization remain open work.

`run` waits for supported hardware and supervises independent tablet/auxiliary workers. Disconnect or reconfigure retires the affected worker while peers remain owned. Ctrl+C and SIGTERM drain readers, tools and held input. `capture` selects a matching device and stops after its report limit or deadline, including time spent waiting for hardware. `--seconds` accepts 1 to 60 seconds and defaults to 10; `--limit` defaults to 2000 reports. It prints shared capture diagnostics to stderr. Capture proves received/decoded reports only; it is not proof of live cursor output.

Without an explicit native profile, startup imports original settings for the detected tablet. Original collections retain disconnected rows and global tools. Installed managed filters, output modes, bindings, tools, parsers and custom providers use the shared host, with actual platform prerequisites. Original managed readers consume owned tees from the native reader rather than opening it again. The pinned original macOS output contract is mouse/keyboard, not Linux Artist Mode or Windows Ink.

Display mapping uses the CoreGraphics global desktop coordinate space, including negative monitor origins and Retina logical points. `displays` prints the sorted monitor order used by profile monitor indices. `--screen WIDTHxHEIGHT` overrides discovery with one rectangle at the desktop origin. The session checks monitor rectangles once a second, including rearrangements that preserve the aggregate desktop bounds and monitor count, and refreshes absolute mapping when they change.

## Implemented behavior and limits

- USB HID service enumeration and matching on VID/PID, normalized report lengths and USB interface number where the registry provides it.
- Indexed USB descriptor predicates and initialization string requests use Apple's IOUSBDeviceInterface245. Discovery requests only indices declared by plausible matching tablet configurations. Strings use a language advertised by the device, preferring English (US), and checked UTF-16 decoding. Each native descriptor request has a one-second timeout; cancellation and capture deadlines are checked before endpoints and each matching/initialization descriptor request. A request already in flight may finish up to one second after cancellation or the deadline. USB handles and plugin references are released on every success/error path. `device-strings` reads explicitly requested indices from each matching physical USB device without initializing it or injecting input.
- Shared decoder support, absolute and relative mouse movement, tip/eraser contact as the left mouse button, and physical keyboard modifiers sampled when a mouse event is posted.
- Pen side buttons use the shared profile bindings. Defaults click right and middle; explicit bindings support all five mouse buttons and ordinary physical keys/chords through CoreGraphics. Pointer movement uses the appropriate left/right/other drag event while a button is held. Tip contact and a side-button left binding share ownership, so releasing either preserves the other's press.
- Feature and output initialization writes through asynchronous IOKit calls, with a native one-second timeout and a two-second callback deadline. Report callbacks use a fixed 32-report queue and 4096-byte report bound. Oversized reports and overflow stop the session instead of losing reports silently. Callbacks allocate no Rust heap memory.
- While a session drives output, the report thread uses the Mach time-constraint policy, 1 ms of computation within 2 ms, as upstream's pull request 5071 proposes for its device reader ([real-time scheduling](../../docs/REALTIME_SCHEDULING_2026-10-05.md)). `OTD_RUST_REALTIME=0` keeps normal scheduling. It has not run on a Mac yet.
- Input Monitoring and Accessibility diagnostics. HID services are enumerated without opening unrelated keyboards or mice. Matched digitizer and auxiliary endpoints are independently owned, with upstream-style shared access.

Indexed USB string requests require access to the parent USB device. A competing driver may deny that access; matching/initialization reports the failed operation rather than guessing a value or skipping initialization. Native descriptor reads cannot be cancelled in flight, but each request is bounded. Auxiliary/pad bindings, simultaneous tablets, original frontend/Console workflows and managed services are implemented in source. Native hardware families and transports, including Bluetooth, remain unqualified. The native CLI does not register login startup. Mouse clicks use a single-click count; native double-click counting is not implemented. Competing tablet drivers can still consume input because the native HID backend does not seize the device.

Native TOML keyboard bindings retain physical positions. `Application` means Command, `Alt` means Option, and `Control` stays Control; `keys:Application+Z` is a native shortcut. Imported original bindings instead retain the exact pinned Mac dictionary, including its keypad aliases, CapsLock, Clear/NumberLock and Mute/VolumeUp/VolumeDown. Native persistence marks those original keys with `cg:`, for example `keys:cg:Application+cg:Z`; ordinary physical names keep their legacy mappings. Names absent from the original Mac dictionary are not advertised as original support. `None` is an explicit Rust no-op. Native and managed holders share platform ownership, including aliases; one holder's release preserves another's press. Physical user input and other injectors are outside this ownership model. CoreGraphics modifier snapshots may lag, and shortcut/application delivery still requires native validation.

Mouse and keyboard output use CGEventPost, which has no success return value. Permission preflight cannot prove that an application accepted an event. Relative output queries the system cursor with a native CGEvent allocation, and each key transition creates a native keyboard event so the current keyboard layout translates it correctly. The successful Rust report callback and report processing path do not allocate Rust heap memory. Native framework allocation behavior and performance require macOS measurements.

No format, clippy or test checks were run for this change, following the repository's release policy. Cross-linking a release binary does not validate native permissions, pen input, cursor movement, disconnect/replug, Retina mapping, sleep/wake or application behavior. Those are still required macOS validation tasks.

## Upstream references

The backend follows OpenTabletDriver 0.6.7 at commit `736003ed72c8bbb28033b039d5a0bb76c344145c`:

- [HidSharpEndpoint and macOS interface matching](https://github.com/OpenTabletDriver/OpenTabletDriver/blob/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver/Devices/HidSharpBackend/HidSharpEndpoint.cs) and [HidSharpEndpointStream](https://github.com/OpenTabletDriver/OpenTabletDriver/blob/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver/Devices/HidSharpBackend/HidSharpEndpointStream.cs).
- [Absolute pointer](https://github.com/OpenTabletDriver/OpenTabletDriver/blob/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.Desktop/Interop/Input/Absolute/MacOSAbsolutePointer.cs), [relative pointer](https://github.com/OpenTabletDriver/OpenTabletDriver/blob/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.Desktop/Interop/Input/Relative/MacOSRelativePointer.cs), [virtual mouse](https://github.com/OpenTabletDriver/OpenTabletDriver/blob/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.Desktop/Interop/Input/MacOSVirtualMouse.cs) and [displays](https://github.com/OpenTabletDriver/OpenTabletDriver/blob/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.Desktop/Interop/Display/MacOSDisplay.cs).
- [PermissionHelper and responsible application permissions](https://github.com/OpenTabletDriver/OpenTabletDriver/blob/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.UX.MacOS/PermissionHelper.cs).
- Apple's [IOUSBDeviceInterface245 ABI](https://github.com/apple-oss-distributions/IOUSBFamily/blob/IOUSBFamily-630.4.5/IOUSBFamily/Headers/IOUSBLib.h), [IOUSBDevRequestTO](https://github.com/apple-oss-distributions/IOUSBFamily/blob/IOUSBFamily-560.4.2/IOUSBFamily/Headers/USB.h), [IOCFPlugIn interface](https://github.com/apple-oss-distributions/IOKitUser/blob/main/IOCFPlugIn.h), and [CFPlugInCOM IUnknown layout](https://github.com/apple-oss-distributions/CF/blob/main/CFPlugInCOM.h) provide the native declarations; USB GET_DESCRIPTOR requests use the declared string indices.

The descriptor-length parser was copied from the repository's Linux backend. Its leading zero-byte normalization follows HidSharp's macOS stream behavior for unnumbered reports. Native CLI input remains mouse/keyboard output; the original Eto frontend retains its Cocoa permission dialogs. Native permission, frontend and input behavior were not exercised for this source update.
