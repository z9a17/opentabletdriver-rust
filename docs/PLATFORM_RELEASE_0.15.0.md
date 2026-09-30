# Platform release 0.15.0

This release adds usable Linux and macOS CLI backends and publishes their packages with Windows. It does not close the platform parity gate. Linux/macOS have no graphical panel, persistent daemon, external plugin host, express-key binding or multi-tablet runtime yet. macOS additionally lacks indexed USB string operations, native double-click counts and pressure-sensitive tablet output.

## Linux

The package uses a static x86-64 musl executable so it does not inherit Arch's glibc minimum. It reads hidraw reports, requests configured initialization, and sends uinput mouse or Artist Mode events through the existing shared session/core. Startup geometry comes from Hyprland, Sway or X11; other desktops need `--screen`. Capture runs without uinput or injection. Discovery distinguishes inaccessible hardware from unmatched hardware and reports USB-string permission failures.

Setup follows [upstream's generated permissions](https://github.com/OpenTabletDriver/OpenTabletDriver/blob/736003ed72c8bbb28033b039d5a0bb76c344145c/generate-rules.sh). The rules grant active-session access only to database VID/PID pairs. The setup script owns its rule/module-load files, preserves local edits, and offers an opt-in libinput suppression rule scoped to one physical VID/PID. Reconnecting the tablet applies or removes that rule. It avoids upstream's global kernel-driver blacklists.

Observed on this Arch/Hyprland laptop on 2026-10-01:

- USB `056a:0357`, Wacom PTH-660, reports `Intuos Pro M [cfw]`; firmware is not established by that name.
- hidraw input length 192, feature length 2561; read/write access granted by the user-installed udev rules. The kernel `wacom` driver was still bound at the observation, so duplicate native/Rust output remains a potential conflict.
- Capture opened and initialized the device and ended after two seconds, with `read=0 accepted=0 ignored=0 malformed=0 output_commits=0 output_failures=0`.
- A three-second driver startup created its uinput pointer, detected Hyprland's 1536x864 logical desktop from a 1920x1080 display at scale 1.25, opened and initialized the PTH-660, and ended cleanly on SIGINT. All report/output counters were zero.

The user requested no manual pen interaction. These observations prove access, initialization and idle lifecycle only. They do not prove pen movement, tip/eraser, pressure, cursor output, drawing behavior, unplug/replug or suspend/resume. Sway/X11 discovery is implemented but was not exercised here.

An earlier capture aborted while build-tool downloads exceeded the temporary filesystem quota and its stderr log stayed empty. After moving the toolchains to disk, bounded capture and startup completed. The core showed a Rust abort but the stripped executable did not provide a symbolized cause, so storage exhaustion is the inferred trigger, not a confirmed backend defect.

## macOS

The new backend follows the pinned upstream [HID endpoint](https://github.com/OpenTabletDriver/OpenTabletDriver/blob/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver/Devices/HidSharpBackend/HidSharpEndpoint.cs), [absolute mouse](https://github.com/OpenTabletDriver/OpenTabletDriver/blob/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.Desktop/Interop/Input/Absolute/MacOSAbsolutePointer.cs), [relative mouse](https://github.com/OpenTabletDriver/OpenTabletDriver/blob/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.Desktop/Interop/Input/Relative/MacOSRelativePointer.cs), [displays](https://github.com/OpenTabletDriver/OpenTabletDriver/blob/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.Desktop/Interop/Display/MacOSDisplay.cs), and [permissions](https://github.com/OpenTabletDriver/OpenTabletDriver/blob/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.UX.MacOS/PermissionHelper.cs).

IOKit discovers USB digitizers and delivers reports through a bounded, allocation-free Rust callback queue on the owning CoreFoundation run loop. CoreGraphics posts absolute/relative mouse and tip/eraser contact events. Input Monitoring and Accessibility are checked separately. The shared native decoders, profiles and Radial Follow run directly; no original OpenTabletDriver daemon is invoked.

Both macOS packages target macOS 11 or newer and link system frameworks. They are cross-built from Linux using Apple's 11.3 SDK and Rust's Mach-O linker. They are not developer-signed or notarized. Apple Silicon requires the linker's ad-hoc code signature; this is not a distribution identity or notarization.

No Mac is available for a native runtime or physical test. Compilation and Mach-O inspection do not establish Input Monitoring/Accessibility behavior, event delivery, Retina mapping, initialization, report framing, disconnect or sleep/wake reliability. Configurations requiring indexed USB strings or initialization strings return an unsupported-operation error.

## Windows and publication

The Windows driver, panel, sample EMA DLL and rebuilt .NET bridge retain the current source behavior. The Windows package is cross-built with GNU MinGW-w64 rather than MSVC. Its imports are Windows system/UCRT DLLs, with no toolchain DLL dependency. Native Windows UI/driver/plugin execution was not performed on this Linux host.

The release matrix requires Windows x64, Linux x64, Intel Mac and Apple Silicon. The local build/publish scripts record the source state around each build, reject stale or altered binaries, verify archive modes/content/architecture/version/source/checksums, upload a draft, compare GitHub's asset digests and publish only the complete set. No GitHub Actions runners are used. See [the release procedure](RELEASING.md).

Per repository policy, no separate format/Clippy/test/build-check suite or CI runs for this release. Actual release compilation, shellcheck, CLI/device idle observations and archive inspection are recorded separately. CAP-47, CAP-48, CAP-49 and X02-X05 remain open in the evidence ledger because their wider automated and physical acceptance criteria are not met.
