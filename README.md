<img src="resources/opentabletdriver.svg" width="72" height="72" alt="OpenTabletDriver Rust tablet and pen logo">

# OpenTabletDriver Rust

A Rust drawing-tablet driver based on [OpenTabletDriver](https://github.com/OpenTabletDriver/OpenTabletDriver), with a native Windows control panel, Linux/macOS command-line drivers, and the original Gtk/Eto frontends on Linux/macOS. It imports OpenTabletDriver settings and hosts supported existing .NET plugins unchanged.

**Experimental compatibility:** the project includes 359 tablet configurations and native entries for all 54 referenced parsers. Only the **Wacom PTH-660 (Intuos Pro M) on Windows** has confirmed hardware evidence. Catalog/parser coverage does not establish working initialization, transport or output for every tablet. Linux/macOS runtime and hardware acceptance, the unchanged plugin corpus, and full verified parity remain open. See [RC adoption and evidence limits](docs/parity/UPSTREAM_0.6.8_RC1.md).

## Download

Get the archives and SHA-256 checksums from [the latest release](https://github.com/z9a17/opentabletdriver-rust/releases/latest). Extract the entire archive and keep its folders and dependencies together.

| Platform | Archive suffix | Requirements and setup |
| --- | --- | --- |
| Windows x64 | `win-x64.zip` | Windows 11; [Windows guide](docs/GETTING_STARTED_WINDOWS.md) |
| Linux x64 | `linux-x64.tar.gz` | glibc 2.31+, udev, hidraw/uinput; [Linux guide](crates/otd-linux/README.md) |
| macOS Intel | `macos-x64.tar.gz` | Native driver: macOS 11+; managed features: see [macOS guide](crates/otd-macos/README.md) |
| macOS Apple Silicon | `macos-arm64.tar.gz` | Native driver: macOS 11+; managed features: see [macOS guide](crates/otd-macos/README.md) |

Archive names begin with `opentabletdriver-rust-v<version>-`. Upstream distribution packages and Flatpak install the original driver; use this project's archives for the Rust version.

Native-only input needs no .NET runtime. Original .NET plugins, registered type/default-store requests, original RPC hosting and original frontends/Console need the **architecture-matching .NET 10 runtime or newer**. Linux's frontend also needs GTK3; generic Wayland display discovery needs .NET, while Hyprland/Sway/X11 discovery and explicit `--screen` remain native. Unix network operations need system curl. macOS packages are unsigned and not notarized.

The native driver baselines above do not extend [.NET 10's supported OS versions](https://github.com/dotnet/core/blob/main/release-notes/10.0/supported-os.md). The packaged Mac managed launchers and host library require macOS 12 at the binary level; managed features also need an OS supported by the installed .NET runtime.

## Get started

Stop other tablet drivers before starting this one. Preserve your existing settings and plugins when switching or upgrading.

**Windows:** extract the ZIP, open `opentabletdriver-rust-ui.exe` as your normal user, connect the tablet, then choose your tablet/screen areas and press **Save** and **Apply**. The panel starts or attaches to a hidden daemon. Closing the panel shuts down that daemon; minimizing keeps it running in the tray. Supported existing settings are imported on first run. Settings live in `%LOCALAPPDATA%\OpenTabletDriverRust`.

**Linux:** extract the archive and open a terminal in its folder:

```sh
sudo ./setup/install.sh install
# Reconnect the tablet after installing permissions, then run as your normal user:
./opentabletdriver-rust-linux list
./opentabletdriver-rust-linux run
```

Use `./opentabletdriver-rust-linux ui` for the original Gtk frontend. The [Linux guide](crates/otd-linux/README.md) covers permissions, competing kernel input, screen discovery, updates and removal.

**macOS:** extract the matching archive, open Terminal in its folder, and grant the terminal **Input Monitoring** and **Accessibility** access in System Settings. Restart the terminal after changing permissions, then run:

```sh
./opentabletdriver-rust-macos list
./opentabletdriver-rust-macos run
```

Use `./opentabletdriver-rust-macos ui` for the original Eto frontend. See the [macOS guide](crates/otd-macos/README.md) for Gatekeeper, permissions, display mapping and validation limits. Press Ctrl+C to stop either Unix command-line driver.

## Features and scope

- Absolute and relative mapping, tablet/screen areas, rotation, clipping and per-device profiles.
- Built-in Radial Follow smoothing, native plugins and supported original .NET filters, output modes, bindings and tools. Plugins retain their platform and external-driver prerequisites; the full unchanged-DLL corpus is unqualified.
- Pen and auxiliary buttons, keyboard shortcuts, express keys, wheels and scroll bindings.
- Multiple tablet sessions, native daemon control, original settings collections/presets, original Console/RPC workflows and plugin installation.
- Windows native panel with light/dark/system themes, accent colors, tray controls and a tablet debugger. Linux/macOS packages include the original graphical frontends.
- Windows Ink pressure/tilt/rotation/eraser and Linux Artist Mode are implemented but still need application/hardware acceptance. macOS follows the original mouse/keyboard output contract.

Native report processing uses inline/borrowed storage; managed compatibility paths may allocate and add work. [Performance measurements](docs/PERFORMANCE.md) and [latency analysis](docs/INPUT_LATENCY.md) distinguish offline processing from physical pen-to-display latency. No general low-latency or osu!/OBS improvement is established by source coverage or package builds.

Windows 0.18 original Console names such as `save`, `preset` and `stdio` address the daemon's original JSON collection. Native TOML scripts should follow the [0.18 command migration table](docs/REFERENCE.md#windows-console-migration-in-018). The [reference](docs/REFERENCE.md) documents commands and profiles.

## Documentation and development

Start with the [documentation index](docs/README.md), [current source and evidence map](docs/parity/CURRENT_STATUS.md), [hardware validation checklist](docs/HARDWARE_VALIDATION.md), or [release notes](docs/RELEASE_0.19.1.md). Historical audits and release snapshots remain in [porting status](docs/PORTING_STATUS.md).

Build from a real **Git checkout**, using stable Rust. Windows needs MSVC and the Windows SDK; macOS needs an Apple SDK/linker. Source archives without Git metadata cannot be built as-is. Run the matching command on its target platform:

```text
cargo build --locked --release -p opentabletdriver-rust -p otd-ema-filter
cargo build --locked --release -p otd-linux
cargo build --locked --release -p otd-macos
```

Complete packages also need the .NET 10+ SDK and the managed dependencies. Use `pwsh -File scripts/package.ps1` on Windows or `scripts/package-unix.sh <platform>` on Unix. See [releasing](docs/RELEASING.md) for dependency/toolchain details and all four platform packages, and [the parity plan](docs/FULL_PARITY_PLAN.md) for contribution contracts and deferred acceptance gates.

## License

The combined program is **GPL-3.0-only** ([LICENSE](LICENSE)), because its built-in Radial Follow filter ports [AbstractQbit's RadialFollow](https://github.com/AbstractQbit/AbstractOTDPlugins/tree/0.3.0/RadialFollow). Earlier driver code retains its [LGPL-3.0-only terms](LICENSE.LGPL-3.0). See [NOTICE.md](NOTICE.md) for credits and upstream source notices.
