# Windows setup and usage

Download the Windows x64 ZIP from [the latest release](https://github.com/z9a17/opentabletdriver-rust/releases/latest). Stop other tablet drivers before starting it; keep your existing settings and plugins when switching drivers or upgrading. These instructions describe the Windows layout in 0.18.2 and newer.

## Prerequisites

Native-only profiles and the native panel run without .NET until a managed feature is requested. Existing OpenTabletDriver `.NET` plugins, original fixed RPC hosting, registered type discovery and default-store construction need the Windows **x64 .NET 10 or newer runtime** from [Microsoft's download page](https://dotnet.microsoft.com/en-us/download/dotnet/10.0). This includes registered type lists/selectors and Console commands that construct plugin stores; simple native Console collection reads/edits do not themselves require CLR. Choose the x64 installer under **.NET Runtime**; the x64 Desktop Runtime also includes it. Keep the release's `data` folder beside the executables; plugin dependencies are in `data/compat`.

## Install and start

1. Download `opentabletdriver-rust-v<version>-win-x64.zip`.
2. Right-click the ZIP and select **Extract All**. Extract the whole archive into its own folder, for example `C:\Users\<username>\OpenTabletDriverRust`. Open the extracted `opentabletdriver-rust-v<version>-win-x64` folder containing the two apps, a short README and the `data` folder.
3. Open **opentabletdriver-rust-ui.exe** as your normal user. It opens the panel and starts or attaches to a separate hidden daemon process.
4. Connect your tablet. The panel starts the driver. Set your screen and tablet areas in **Area**, then press **Save**. Save also applies the settings to a running driver; **Apply** applies changes without saving them.

Supported settings from an existing OpenTabletDriver installation are imported on first run. Explicit runtime imports resolve supported installed managed stores while preserving the original settings document. The Rust driver refuses to start alongside the original OpenTabletDriver daemon. Windows includes USB HID and WinUSB discovery and input transport; compilation and catalog coverage do not establish working hardware for every tablet.

You can create a shortcut to `opentabletdriver-rust-ui.exe`; set its **Start in** field to the extracted folder. Enable **Settings > Startup > Start with Windows (in the tray)** in the panel to launch it at sign-in. Closing the panel shuts down the daemon and waits for its process to exit after input cleanup; minimizing keeps both running in the tray.

Choose **Settings > Theme > Accent color** to change the blue highlights throughout the panel and its settings windows. Blue remains the default. Pick a preset, follow the Windows accent color, or enter RGB/hex values under **Custom**. The choice is saved for future launches. Windows high-contrast colors take precedence.

Use **Settings > Performance settings** to choose GUI and driver CPU affinity. **Save and apply** stores both choices without restarting input; **Reload saved** discards unsaved edits. These choices are separate from tablet profiles. See [performance settings](EXPERIMENTAL_SETTINGS.md).

Both CPU-affinity choices default to All CPUs; lists such as `0,2,4-7` are accepted. Optional MMCSS Pro Audio scheduling takes effect at the next tablet-input start. [Usage and limits](EXPERIMENTAL_SETTINGS.md).

## Console, updates and removal

Double-click **opentabletdriver-rust.exe** to run in a console, or use these commands from the extracted folder:

```text
.\opentabletdriver-rust.exe tablets --list
.\opentabletdriver-rust.exe settings
.\opentabletdriver-rust.exe capture --seconds 10
```

Press Ctrl+C to stop the console driver. The full command list is in the [reference](REFERENCE.md).

Use **Help > Check for updates** to install a newer release and restart. For a manual upgrade, close the panel and run `.\opentabletdriver-rust.exe shutdown` from its folder to stop the background daemon. Extract the new ZIP into a new folder and start it there. Normal settings remain in `%LOCALAPPDATA%\OpenTabletDriverRust`; preserve any profiles or plugins stored inside an older portable folder before removing it.

To uninstall, turn off **Settings > Startup > Start with Windows (in the tray)** if enabled, close the panel, run `.\opentabletdriver-rust.exe shutdown` and delete the extracted folder. Your saved settings remain in `%LOCALAPPDATA%\OpenTabletDriverRust`.
