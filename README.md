# opentabletdriver-rust

A Windows 11 USB driver for the Wacom PTH-660, written in Rust. It reads the pen HID collection and drives absolute or relative cursor movement and tip clicks. The port does not yet have a plugin system, side-button output, GUI, or Windows Ink pressure output. See the [porting status](docs/PORTING_STATUS.md) for completed functionality and remaining work toward broader OpenTabletDriver support.

The combined executable is licensed under GPL-3.0-only because its built-in Radial Follow filter is a Rust port of [AbstractQbit's RadialFollow 0.3.0](https://github.com/AbstractQbit/AbstractOTDPlugins/tree/0.3.0/RadialFollow), which is GPL-3.0-only. The earlier driver source retains its LGPL-3.0-only terms in [LICENSE.LGPL-3.0](LICENSE.LGPL-3.0); the combined executable uses [LICENSE](LICENSE). The PTH-660 USB identification and report layout were researched from [OpenTabletDriver 0.6.x](https://github.com/OpenTabletDriver/OpenTabletDriver/tree/fdeaa7b0c6d6f5260f511f19fb693ed33524af4e). The [implementation plan](docs/IMPLEMENTATION_PLAN.md) records source links and acceptance criteria.

## Open the driver

On Windows, double-click **opentabletdriver-rust.exe**. It is a console application and starts the daemon with no arguments, showing the active mapping, connection, and reconnection status. Before injecting cursor input, it temporarily pauses any running original OpenTabletDriver daemon and UX; stop any other tablet daemon yourself. Press **Ctrl+C** for a normal stop; the Rust process restores those original processes afterward. Keep the console open while using the tablet. It does not modify Windows HID drivers or startup registration.

With no arguments, the driver reads `%LOCALAPPDATA%\OpenTabletDriver\settings.json` and selects its **Wacom PTH-660** profile. It follows the enabled absolute or relative output mode and enabled tip/eraser bindings with their pressure activation thresholds. Absolute mode imports tablet and display area sizes and centers, tablet rotation, clipping, and area limiting. Relative mode imports per-axis sensitivity, rotation, and reset delay, preserving fractional motion between reports. The current development-machine absolute profile maps an 85 × 47.8125 mm tablet area to a 2560 × 1440 display area with clipping and a 1% tip threshold. Profile values are read at launch, so restart the daemon after changing them in OpenTabletDriver.

When the PTH-660 profile contains **AbstractQbit's Radial Follow Smoothing (Tablet coordinates)** (`RadialFollow.RadialFollowSmoothingTabletSpace`), the Rust implementation enables it automatically before absolute or relative mapping, even if OpenTabletDriver's `Enable` flag is false. This is an intentional override for the Rust build; the console reports it. It reads the original `OuterRadius`, `InnerRadius`, `SmoothingCoefficient`, `SoftKneeScale`, and `SmoothingLeakCoefficient` properties; the current profile uses 0.7039 mm, 0.302 mm, 0.302, 0.603, and 0.201 respectively. The filter keeps its original dead zone, radial curve, and 50 ms reset. No .NET DLL is loaded. Other enabled plugin filters are reported and skipped. Pen side buttons and tablet express keys remain unimplemented. Unsupported output modes and nonstandard tip actions are rejected with a clear message rather than silently using a different mapping. If OpenTabletDriver settings are absent, the built-in fallback maps the whole PTH-660 to the whole virtual desktop without filtering.

For relative mode without OpenTabletDriver settings, use `opentabletdriver-rust.exe run --config driver.relative.example.toml`. [Relative mode details](docs/RELATIVE_MODE.md) document reset behavior, performance checks, and upstream compatibility. Windows pointer speed and acceleration affect relative motion. Relative mode has automated replay coverage but still needs live tablet validation.

## Build and diagnostics

Use the stable Rust MSVC toolchain on Windows 11:

    cargo build --release
    cargo test

The executable is `target\release\opentabletdriver-rust.exe`. Read-only commands are:

    opentabletdriver-rust.exe settings
    opentabletdriver-rust.exe list
    opentabletdriver-rust.exe displays
    opentabletdriver-rust.exe capture --seconds 10

`settings` shows the effective mapping and bindings. `capture` reads a short pen trace without cursor injection. HID paths and serial identifiers are omitted unless you explicitly use `list --paths`.

For an independent Rust TOML profile instead of the active OpenTabletDriver settings, use `opentabletdriver-rust.exe run --config driver.toml`; see [driver.example.toml](driver.example.toml). To test a different OpenTabletDriver settings file, use `--otd-settings path\to\settings.json`. To uninstall, stop the console and remove the executable.

Windows mouse injection does not carry pressure, tilt, or eraser data to drawing applications. Those values are decoded from the tablet, but native Windows Ink output is a later milestone.

The real PTH-660 reported a readable 192-byte pen collection and a 44-byte auxiliary collection. Hover, contact, lift, and proximity-loss reports were captured. The user confirmed that the earlier 0.2.0 console build moved the cursor and clicked with the pen using the active OpenTabletDriver profile. The 0.3.0 built-in Radial Follow port matches the original C# curve in automated reference tests; live cursor feel with the filter has not been checked yet, because the running 0.2.0 driver was left untouched. The driver also detected an unplug and reopened the HID collection after replug. Pen movement after that replug, sleep/wake, and display changes remain open hardware checks in the [validation guide](docs/HARDWARE_VALIDATION.md).
