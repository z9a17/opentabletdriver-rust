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
- **Plugins:** browse and install from OpenTabletDriver's plugin catalog (checked against its SHA-256 hash), or install one from a file. Supported unchanged output-mode and binding classes have typed settings selectors alongside filters and tools.
- **Several tablets at once,** each with its own profile.
- **Native control panel** in OpenTabletDriver's layout, with light and dark themes, configurable accent colors, a tray icon, and a tablet debugger that shows raw and decoded pen data live.
- **Command line and background service** for scripting: profiles, presets, diagnostics and area conversion all work headless.
- **Pen side buttons** click or press keys and shortcuts, with right and middle click defaults. The Windows CLI can inspect defaults, edit individual buttons into a new profile and export supported edits back to OTD settings. See [pen side buttons](docs/PEN_BUTTONS.md).

The Windows panel includes pen, mouse, express-key, wheel and scroll binding editors. See [express keys and wheels](docs/EXPRESS_KEYS_AND_WHEELS.md) and [Windows compatibility workflows](docs/WINDOWS_COMPATIBILITY_0.17.0.md) for supported actions and remaining checks. Complete toggle and preset-switch binding compatibility remains open.

## Installation

Download from [this project's latest release](https://github.com/z9a17/opentabletdriver-rust/releases/latest). Choose the archive for your operating system and processor:

| Platform | Requirements | Download |
| --- | --- | --- |
| [Windows](#windows) | Windows 11, x64 | `opentabletdriver-rust-v<version>-win-x64.zip` |
| [Linux](#linux) | Linux x64, udev and kernel hidraw/uinput support | `opentabletdriver-rust-v<version>-linux-x64.tar.gz` |
| [macOS, Apple Silicon](#macos) | macOS 11 or newer, M-series processor | `opentabletdriver-rust-v<version>-macos-arm64.tar.gz` |
| [macOS, Intel](#macos) | macOS 11 or newer, Intel processor | `opentabletdriver-rust-v<version>-macos-x64.tar.gz` |

Stop OpenTabletDriver and other tablet drivers before starting this driver. If a vendor driver keeps running in the background, follow its removal instructions. Keep your existing settings and plugins when switching drivers or upgrading.

These instructions follow the original OpenTabletDriver website's [Windows](https://opentabletdriver.net/Wiki/Install/Windows), [Linux](https://opentabletdriver.net/Wiki/Install/Linux) and [macOS](https://opentabletdriver.net/Wiki/Install/MacOS) guides, adapted to our release packages. Distribution packages named `opentabletdriver`, upstream's Flatpak and upstream's macOS app install the original driver. Use the archives above for this Rust rewrite.

### Windows

#### Prerequisites

The driver and native panel run without .NET. To use existing OpenTabletDriver `.NET` plugins, install the Windows **x64 .NET 8 or newer runtime** from [Microsoft's download page](https://dotnet.microsoft.com/en-us/download/dotnet/8.0). Choose the x64 installer under **.NET Runtime**; the x64 Desktop Runtime also includes it. Keep the release's `data` folder beside the executables; plugin dependencies are in `data/compat`.

#### Install and start

1. Download `opentabletdriver-rust-v<version>-win-x64.zip`.
2. Right-click the ZIP and select **Extract All**. Extract the whole archive into its own folder, for example `C:\Users\<username>\OpenTabletDriverRust`. Open the extracted `opentabletdriver-rust-v<version>-win-x64` folder containing the two apps, a short README and the `data` folder.
3. Open **opentabletdriver-rust-ui.exe** as your normal user. It opens the panel and starts or attaches to a separate hidden daemon process.
4. Connect your tablet. The panel starts the driver. Set your screen and tablet areas in **Output**, then press **Save** and **Apply**.

Supported settings from an existing OpenTabletDriver installation are imported on first run. Explicit runtime imports resolve supported installed managed stores while preserving the original settings document. The Rust driver refuses to start alongside the original OpenTabletDriver daemon. Windows includes USB HID and WinUSB discovery and input transport; compilation and catalog coverage do not establish working hardware for every tablet.

You can create a shortcut to `opentabletdriver-rust-ui.exe`; set its **Start in** field to the extracted folder. Enable **Tablets > Start with Windows** in the panel to launch it at sign-in. Closing the panel shuts down the daemon and waits for its process to exit after input cleanup; minimizing keeps both running in the tray.

Choose **View > Theme > Accent color** to change the blue highlights throughout the panel and its settings windows. Blue remains the default. Pick a preset, follow the Windows accent color, or enter RGB/hex values under **Custom**. The choice is saved for future launches. Windows high-contrast colors take precedence.

Use the **Experimental** tab to choose GUI and driver CPU affinity. **Save and apply** stores both choices without restarting input; **Reload saved** discards unsaved edits. These choices are separate from tablet profiles. See [experimental settings](docs/EXPERIMENTAL_SETTINGS.md).

Both CPU-affinity choices default to All CPUs; lists such as `0,2,4-7` are accepted. Optional MMCSS Pro Audio scheduling takes effect at the next tablet-input start. [Usage and limits](docs/EXPERIMENTAL_SETTINGS.md).

#### Console, updates and removal

Double-click **opentabletdriver-rust.exe** to run in a console, or use these commands from the extracted folder:

```text
.\opentabletdriver-rust.exe tablets --list
.\opentabletdriver-rust.exe settings
.\opentabletdriver-rust.exe capture --seconds 10
```

Press Ctrl+C to stop the console driver. The full command list is in the [reference](docs/REFERENCE.md).

Use **Help > Check for updates** to install a newer release and restart. For a manual upgrade, close the panel and run `.\opentabletdriver-rust.exe shutdown` from its folder to stop the background daemon. Extract the new ZIP into a new folder and start it there. Normal settings remain in `%LOCALAPPDATA%\OpenTabletDriverRust`; preserve any profiles or plugins stored inside an older portable folder before removing it.

To uninstall, turn off **Tablets > Start with Windows** if enabled, close the panel, run `.\opentabletdriver-rust.exe shutdown` and delete the extracted folder. Your saved settings remain in `%LOCALAPPDATA%\OpenTabletDriverRust`.

### Linux

#### Prerequisites

The Linux x64 release is a static executable and needs no .NET runtime. Setup requires Bash and udev and loads the kernel's `uinput` module. Its permission rules grant device access to the active systemd-logind desktop user. Other session managers need equivalent device permissions; see the [Linux guide](crates/otd-linux/README.md).

#### Install and start

1. Download `opentabletdriver-rust-v<version>-linux-x64.tar.gz`.
2. Open a terminal in the download directory. Replace `<version>` with the downloaded release version, then extract it and enter its folder:

   ```sh
   tar -xzf "opentabletdriver-rust-v<version>-linux-x64.tar.gz"
   cd "opentabletdriver-rust-v<version>-linux-x64"
   ```

3. Install the device permissions:

   ```sh
   sudo ./setup/install.sh install
   ```

4. Unplug and reconnect the tablet, then check discovery and start the driver as your normal user:

   ```sh
   ./opentabletdriver-rust-linux --version
   ./opentabletdriver-rust-linux list
   ./opentabletdriver-rust-linux run
   ```

Use sudo for the setup script only. Press Ctrl+C to stop the driver. Setup does not register a background service or start the driver automatically.

Hyprland, Sway and X11 screen layouts are detected at startup. Other desktops need a desktop size, for example `./opentabletdriver-rust-linux run --screen 1920x1080`. Restart after changing monitor layout or scaling. Existing OpenTabletDriver settings are imported for the selected tablet; use `run --profile /path/to/profile.toml` for a native profile.

Linux supports absolute/relative mouse output and pressure-sensitive Artist Mode. It currently has no control panel or external plugin host. Enabled external plugins in an imported profile must be disabled or replaced with supported native settings.

#### Permissions, competing input and removal

If access is denied, run `./setup/install.sh status` and reconnect after installing permissions. If the kernel Wacom/UCLogic driver also moves the pointer, get the tablet's USB vendor/product IDs from `list` and run:

```sh
sudo ./setup/install.sh ignore-input VVVV PPPP
```

Replace `VVVV PPPP` with the two four-digit hexadecimal IDs. Reconnect the tablet to apply the scoped libinput rule. It suppresses that physical tablet's desktop input while preserving hidraw access. To undo it, run `sudo ./setup/install.sh restore` and reconnect. The setup uses this scoped rule instead of a global kernel-driver blacklist.

For an update, stop the driver, extract the new archive into a new folder, run its `sudo ./setup/install.sh install` and reconnect. Keep your profiles. To uninstall, stop the driver, run `sudo ./setup/install.sh uninstall` from its extracted folder, reconnect the tablet and delete that folder. Locally edited setup files and saved profiles are preserved.

Use `capture --seconds 10` to inspect pen reports without injecting input. See the [Linux guide](crates/otd-linux/README.md) for further setup and validation limits.

### macOS

#### Prerequisites

Use macOS 11 or newer. Download `macos-arm64` for Apple Silicon or `macos-x64` for Intel. The CLI needs no .NET runtime. Give the terminal application you use **Input Monitoring** and **Accessibility** access before running tablet output.

#### Install and start

1. Download the matching `opentabletdriver-rust-v<version>-macos-arm64.tar.gz` or `opentabletdriver-rust-v<version>-macos-x64.tar.gz`.
2. Double-click the archive in Finder to extract it. Keep the extracted folder in a stable location, such as `~/Applications/OpenTabletDriverRust`.
3. Open Terminal, type `cd `, drag the extracted folder from Finder into the terminal window and press Return.
4. Open **System Settings > Privacy & Security**, then add or enable your terminal application under both **Input Monitoring** and **Accessibility**. On macOS 11/12, use **System Preferences > Security & Privacy > Privacy**. If Input Monitoring does not list your terminal, try `./opentabletdriver-rust-macos run` once to request access. Quit and reopen the terminal after granting access, then return to the extracted folder.
5. Connect the tablet and run:

   ```sh
   ./opentabletdriver-rust-macos --version
   ./opentabletdriver-rust-macos list
   ./opentabletdriver-rust-macos displays
   ./opentabletdriver-rust-macos run
   ```

Press Ctrl+C to stop. If macOS blocks the downloaded executable, follow Apple's [Open Anyway instructions](https://support.apple.com/en-us/102445) after checking that you trust the download. These releases are unsigned and not notarized.

The driver supports absolute/relative mouse output, tip clicks and pen side-button mouse/key/chord bindings. `Application` means Command and `Alt` means Option. Profiles and built-in Radial Follow use the shared Rust core. Pass `run --profile /path/to/profile.toml` for explicit settings. Imported profiles using pressure-sensitive pen output or external plugins need a supported mouse-output profile instead.

#### Updates, removal and current limits

For an update, stop the driver, extract the new archive into a new folder and run it from there. Keep any saved profiles. To uninstall, stop it and delete its extracted folder; remove the terminal's privacy permissions only if you no longer need them for other applications. This CLI does not register startup or a background service.

The macOS backend is experimental and has no Mac hardware validation yet. There is no macOS control panel, external plugin host, pressure-sensitive drawing output or express-key support. See the [macOS guide](crates/otd-macos/README.md) for device initialization, permissions and other limits.

Every future release must include Windows, Linux and both Mac architectures. The [release procedure](docs/RELEASING.md) checks the complete set before publication.

## Build from source

You need the stable Rust toolchain. Use MSVC and the Windows SDK resource compiler on Windows; Linux and macOS builds select their backend crate as documented in [the release procedure](docs/RELEASING.md).

```text
cargo build --locked --release -p opentabletdriver-rust -p otd-ema-filter  # Windows
cargo build --locked --release -p otd-linux                         # Linux
cargo build --locked --release -p otd-macos                         # macOS
```

To build the Windows .NET bridge and ZIP, install the .NET 8+ SDK and run `pwsh -File scripts/package.ps1`. Linux and macOS packages use `scripts/package-unix.sh`; all-platform publishing follows [the release procedure](docs/RELEASING.md).

## Learn more

- [Windows compatibility workflows, 0.17.0](docs/WINDOWS_COMPATIBILITY_0.17.0.md): independent device profiles, managed selectors and upstream RPC
- [Current parity audit](docs/parity/PARITY_AUDIT_2026-10-07.md): implemented source, remaining gaps and validation boundaries
- [Update and packaging reliability, 0.15.4](docs/RELIABILITY_0.15.4.md): update lifecycle guards, clipboard preparation and Unix archive permissions
- [Release 0.15.1](docs/RELEASE_0.15.1.md): pen side buttons, startup update prompts and all-platform packages
- [Reference](docs/REFERENCE.md): every command, profile format, the daemon and troubleshooting notes
- [Plugins and UI](docs/PLUGINS_AND_UI.md) · [Pen output](docs/PEN_OUTPUT.md) · [Relative mode](docs/RELATIVE_MODE.md)
- [Performance](docs/PERFORMANCE.md) and [input latency](docs/INPUT_LATENCY.md), compared with OpenTabletDriver 0.6.7
- [Performance audit, 0.15.3](docs/AUDIT_FIXES_0.15.3.md): every release measured against 0.7.5 and OpenTabletDriver, and what was fixed
- [Performance and usability audit, 0.14.1](docs/AUDIT_FIXES_0.14.1.md): source findings, fixes and remaining limits
- [Debugger crash and panel cleanup, 0.14.2](docs/DEBUGGER_AND_UI_0.14.2.md): captured crash cause, UI changes and native/.NET settings comparison
- [Plugin manager and menu cleanup, 0.14.3](docs/UI_CLEANUP_0.14.3.md): header theme, compact filter controls and dropdown click-to-close
- [Tablet identity correction, 0.14.4](docs/TABLET_IDENTITY_0.14.4.md): detected labels, editor dimensions and PTK-470 support limits
- [Hardware validation checklist](docs/HARDWARE_VALIDATION.md): what has and hasn't been tested on a real tablet
- [Porting status](docs/PORTING_STATUS.md) and the [full parity roadmap](docs/FULL_PARITY_PLAN.md) (65 tasks, [tracked on GitHub](docs/parity/GITHUB_TRACKING.md))

## License

The combined program is **GPL-3.0-only** ([LICENSE](LICENSE)), because its built-in Radial Follow filter is a Rust port of [AbstractQbit's RadialFollow](https://github.com/AbstractQbit/AbstractOTDPlugins/tree/0.3.0/RadialFollow). The earlier driver code keeps its LGPL-3.0-only terms ([LICENSE.LGPL-3.0](LICENSE.LGPL-3.0)). See [NOTICE.md](NOTICE.md) for credits. Tablet identification and report layouts come from [OpenTabletDriver](https://github.com/OpenTabletDriver/OpenTabletDriver).
