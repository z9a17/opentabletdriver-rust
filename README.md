# OpenTabletDriver Rust

A fast, lightweight drawing-tablet driver written in Rust, compatible with [OpenTabletDriver](https://github.com/OpenTabletDriver/OpenTabletDriver) settings, tablets and plugins. It runs on Windows today, with an experimental Linux build.

It turns your pen tablet into a precise mouse or a pressure-sensitive pen. Core native report processing keeps storage inline and borrowed; managed plugins can add allocation and processing costs. If you already use OpenTabletDriver, it imports supported settings.

> **Status: early.** It includes OpenTabletDriver's pinned device database and report parsers. Only the **Wacom PTH-660 (Intuos Pro M)** has been confirmed on real hardware; parser coverage does not establish working transport, initialization or output on every tablet. If you try another tablet, please open an issue and tell us how it went.

## What it does

- **Absolute mode:** the pen position on the tablet maps to a position on your screen. Set the tablet area, screen area, rotation and clipping in a graphical editor.
- **Relative mode:** the tablet moves the cursor like a mouse, with sensitivity, rotation and a reset delay.
- **Pen pressure, tilt and eraser** in drawing apps, through Windows Ink. *Newly added and not yet tried with a real tablet in a drawing app.*
- **Filters** to smooth the pen: a built-in Radial Follow smoothing filter, native filter DLLs, and supported existing OpenTabletDriver `.NET` filters, unchanged. Arbitrary plugin-owned threads and dependencies remain compatibility limits.
- **Plugins:** browse and install from OpenTabletDriver's plugin catalog (checked against its SHA-256 hash), or install one from a file.
- **Several tablets at once,** each with its own profile.
- **Native control panel** in OpenTabletDriver's layout, with light and dark themes, a tray icon, and a tablet debugger that shows raw and decoded pen data live.
- **Command line and background service** for scripting: profiles, presets, diagnostics and area conversion all work headless.

Not done yet: pen side buttons and tablet express keys.

## Get started (Windows)

1. Download `opentabletdriver-rust-<version>-win-x64.zip` from the [latest release](https://github.com/z9a17/opentabletdriver-rust/releases/latest) and extract the **whole** zip.
2. Open **opentabletdriver-rust-ui.exe**.
3. Plug in your tablet. The panel starts the driver for you. Pick your screen and tablet areas in **Output**, then press **Save** and **Apply**.

If you have OpenTabletDriver installed, your settings are imported on first run. Stop its driver through its own panel before starting Rust output. The Rust driver refuses to start alongside the original driver; it leaves that process and its launch settings untouched. Stop any other tablet driver you have running too.

Using `.NET` plugins needs the bundled `compat` folder and an installed x64 [.NET 8 or newer runtime](https://dotnet.microsoft.com/download). Without plugins, or with only the built-in filter, .NET isn't needed.

Closing the panel stops tablet input. Minimizing keeps it running in the tray. To remove it, close the panel and delete the folder. It doesn't change Windows drivers or startup settings (unless you turn on **Tablets > Start with Windows**).

### Prefer the console?

Double-click **opentabletdriver-rust.exe** to run the driver in a console window. Press **Ctrl+C** to stop. Handy commands:

```text
opentabletdriver-rust.exe tablets --list   # every supported tablet model
opentabletdriver-rust.exe settings         # show the active mapping
opentabletdriver-rust.exe capture --seconds 10   # record a short pen trace
```

The full command list is in the [reference](docs/REFERENCE.md).

## Build from source

You need the stable Rust toolchain (MSVC on Windows).

```text
cargo build --locked --workspace --release
cargo test --locked --workspace
```

To also build the .NET bridge and make a full release zip, install the .NET 8+ SDK and run `pwsh -File scripts/package.ps1`.

## Linux and macOS

Windows 11 is the main platform. An experimental Linux build (hidraw input, uinput output, Artist Mode pen output) lives in [crates/otd-linux](crates/otd-linux/README.md); it hasn't been run on a real Linux machine yet. There is no macOS backend yet, though the core builds and passes its tests there.

## Learn more

- [Reference](docs/REFERENCE.md): every command, profile format, the daemon and troubleshooting notes
- [Plugins and UI](docs/PLUGINS_AND_UI.md) Â· [Pen output](docs/PEN_OUTPUT.md) Â· [Relative mode](docs/RELATIVE_MODE.md)
- [Performance](docs/PERFORMANCE.md) and [input latency](docs/INPUT_LATENCY.md), compared with OpenTabletDriver 0.6.7
- [Performance and usability audit, 0.14.1](docs/AUDIT_FIXES_0.14.1.md): source findings, fixes and remaining limits
- [Debugger crash and panel cleanup, 0.14.2](docs/DEBUGGER_AND_UI_0.14.2.md): captured crash cause, UI changes and native/.NET settings comparison
- [Hardware validation checklist](docs/HARDWARE_VALIDATION.md): what has and hasn't been tested on a real tablet
- [Porting status](docs/PORTING_STATUS.md) and the [full parity roadmap](docs/FULL_PARITY_PLAN.md) (65 tasks, [tracked on GitHub](docs/parity/GITHUB_TRACKING.md))

## License

The combined program is **GPL-3.0-only** ([LICENSE](LICENSE)), because its built-in Radial Follow filter is a Rust port of [AbstractQbit's RadialFollow](https://github.com/AbstractQbit/AbstractOTDPlugins/tree/0.3.0/RadialFollow). The earlier driver code keeps its LGPL-3.0-only terms ([LICENSE.LGPL-3.0](LICENSE.LGPL-3.0)). See [NOTICE.md](NOTICE.md) for credits. Tablet identification and report layouts come from [OpenTabletDriver](https://github.com/OpenTabletDriver/OpenTabletDriver).
