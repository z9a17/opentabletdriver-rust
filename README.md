# OpenTabletDriver Rust

A fast, lightweight drawing-tablet driver written in Rust, compatible with [OpenTabletDriver](https://github.com/OpenTabletDriver/OpenTabletDriver) settings, tablets and plugins. Releases include Windows x64, Linux x64, Intel Mac and Apple Silicon builds. Windows has a native control panel; Linux and macOS currently use a command-line driver.

It turns your pen tablet into a precise mouse or a pressure-sensitive pen. Core native report processing keeps storage inline and borrowed; managed plugins can add allocation and processing costs. If you already use OpenTabletDriver, it imports supported settings.

> **Status: early.** It includes all 357 configurations from OpenTabletDriver's current 0.6.x catalog and native entries for all 53 referenced parsers ([coverage and remaining limits](docs/TABLET_CATALOG_0.14.5.md)). Only the **Wacom PTH-660 (Intuos Pro M)** has been confirmed on real hardware on Windows; parser coverage does not establish working transport, initialization or output on every tablet. If you try another tablet, please open an issue and tell us how it went.

## What it does

The Windows build offers:

- **Absolute mode:** the pen position on the tablet maps to a position on your screen. Set the tablet area, screen area, rotation and clipping in a graphical editor.
- **Relative mode:** the tablet moves the cursor like a mouse, with sensitivity, rotation and a reset delay.
- **Pen pressure, tilt and eraser** in drawing apps, through Windows Ink. *Newly added and not yet tried with a real tablet in a drawing app.*
- **Filters** to smooth the pen: a built-in Radial Follow smoothing filter, native filter DLLs, and supported existing OpenTabletDriver `.NET` filters, unchanged. Arbitrary plugin-owned threads and dependencies remain compatibility limits.
- **Plugins:** browse and install from OpenTabletDriver's plugin catalog (checked against its SHA-256 hash), or install one from a file.
- **Several tablets at once,** each with its own profile.
- **Native control panel** in OpenTabletDriver's layout, with light and dark themes, a tray icon, and a tablet debugger that shows raw and decoded pen data live.
- **Command line and background service** for scripting: profiles, presets, diagnostics and area conversion all work headless.
- **Pen side buttons** click or press keys and shortcuts, like OpenTabletDriver's: right and middle click by default, or anything you set. See [pen side buttons](docs/PEN_BUTTONS.md).

Not done yet: tablet express keys, and a settings panel editor for the side buttons.

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

You need the stable Rust toolchain. Use MSVC on Windows; Linux and macOS builds select their backend crate as documented in [the release procedure](docs/RELEASING.md).

```text
cargo build --locked --release -p opentabletdriver-rust -p otd-ema-filter  # Windows
cargo build --locked --release -p otd-linux                         # Linux
cargo build --locked --release -p otd-macos                         # macOS
```

To build the Windows .NET bridge and ZIP, install the .NET 8+ SDK and run `pwsh -File scripts/package.ps1`. Linux and macOS packages use `scripts/package-unix.sh`; all-platform publishing follows [the release procedure](docs/RELEASING.md).

## Get started (Linux)

Download `opentabletdriver-rust-v<version>-linux-x64.tar.gz` from the [latest release](https://github.com/z9a17/opentabletdriver-rust/releases/latest), extract it, and open a terminal in its folder:

```sh
sudo ./setup/install.sh install
# Reconnect the tablet after installing permissions.
./opentabletdriver-rust-linux list
./opentabletdriver-rust-linux run
```

Run the driver as your normal user. Hyprland, Sway and X11 screen layouts are detected at startup. For other desktops, use `run --screen WIDTHxHEIGHT` in desktop coordinates. Press Ctrl+C to stop.

Linux uses hidraw for tablet input and uinput for absolute/relative mouse output or pressure-sensitive Artist Mode. It imports your existing OpenTabletDriver settings, or accepts `--profile FILE`. External plugins, the control panel and tablet express keys are not available on Linux yet. Restart after changing the monitor layout.

Stop other tablet drivers before running it. If the kernel Wacom/UCLogic driver also moves the pointer, `sudo ./setup/install.sh ignore-input VVVV PPPP` installs a scoped libinput rule that suppresses that physical tablet's desktop input after reconnecting it. `restore` removes the rule; reconnect again to restore native desktop input. This keeps hidraw available to the Rust driver and never installs a global driver blacklist.

Use `capture --seconds 10` to inspect pen reports without injecting input. See the [Linux guide](crates/otd-linux/README.md) for setup, removal and validation limits.

## Get started (macOS)

Download `opentabletdriver-rust-v<version>-macos-arm64.tar.gz` for Apple Silicon or `macos-x64.tar.gz` for an Intel Mac from the [latest release](https://github.com/z9a17/opentabletdriver-rust/releases/latest). macOS 11 or newer is required. Extract it and open a terminal in its folder:

```sh
./opentabletdriver-rust-macos list
./opentabletdriver-rust-macos displays
./opentabletdriver-rust-macos run
```

Grant your terminal Input Monitoring and Accessibility access in System Settings, then restart it. The driver uses IOKit HID input and CoreGraphics output for absolute/relative positioning, tip clicks and pen side-button mouse/key/chord bindings. `Application` means Command and `Alt` means Option; physical pen-button and shortcut validation remains pending. Profiles and built-in Radial Follow use the shared Rust core. Press Ctrl+C to stop.

The macOS backend is experimental. These packages are cross-built, unsigned and not notarized; we have no Mac hardware validation yet. There is no macOS control panel, external plugin host, pressure-sensitive drawing output or express-key support. See the [macOS guide](crates/otd-macos/README.md) for supported device initialization and permission troubleshooting.

Every future release must include Windows, Linux and both Mac architectures. The [release procedure](docs/RELEASING.md) checks the complete set before publication.

## Learn more

- [Release 0.15.1](docs/RELEASE_0.15.1.md): pen side buttons, startup update prompts and all-platform packages
- [Reference](docs/REFERENCE.md): every command, profile format, the daemon and troubleshooting notes
- [Plugins and UI](docs/PLUGINS_AND_UI.md) Â· [Pen output](docs/PEN_OUTPUT.md) Â· [Relative mode](docs/RELATIVE_MODE.md)
- [Performance](docs/PERFORMANCE.md) and [input latency](docs/INPUT_LATENCY.md), compared with OpenTabletDriver 0.6.7
- [Performance and usability audit, 0.14.1](docs/AUDIT_FIXES_0.14.1.md): source findings, fixes and remaining limits
- [Debugger crash and panel cleanup, 0.14.2](docs/DEBUGGER_AND_UI_0.14.2.md): captured crash cause, UI changes and native/.NET settings comparison
- [Plugin manager and menu cleanup, 0.14.3](docs/UI_CLEANUP_0.14.3.md): header theme, compact filter controls and dropdown click-to-close
- [Tablet identity correction, 0.14.4](docs/TABLET_IDENTITY_0.14.4.md): detected labels, editor dimensions and PTK-470 support limits
- [Hardware validation checklist](docs/HARDWARE_VALIDATION.md): what has and hasn't been tested on a real tablet
- [Porting status](docs/PORTING_STATUS.md) and the [full parity roadmap](docs/FULL_PARITY_PLAN.md) (65 tasks, [tracked on GitHub](docs/parity/GITHUB_TRACKING.md))

## License

The combined program is **GPL-3.0-only** ([LICENSE](LICENSE)), because its built-in Radial Follow filter is a Rust port of [AbstractQbit's RadialFollow](https://github.com/AbstractQbit/AbstractOTDPlugins/tree/0.3.0/RadialFollow). The earlier driver code keeps its LGPL-3.0-only terms ([LICENSE.LGPL-3.0](LICENSE.LGPL-3.0)). See [NOTICE.md](NOTICE.md) for credits. Tablet identification and report layouts come from [OpenTabletDriver](https://github.com/OpenTabletDriver/OpenTabletDriver).
