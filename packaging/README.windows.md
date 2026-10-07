# OpenTabletDriver Rust for Windows

1. Extract the entire ZIP into its own folder.
2. Open **opentabletdriver-rust-ui.exe** to start the panel and a separate hidden daemon.
3. Connect your tablet, choose your areas, then Save and Apply.

Keep the `data` folder beside the apps. It contains the plugin dependencies and
license notices. You can create a shortcut to the UI executable on your desktop.

The Experimental tab lets you choose logical CPUs separately for the
GUI and driver. Use All for automatic scheduling or a list such as `0,2,4-7`.
Save and apply persists the choices without restarting tablet input. Closing
the panel stops input and shuts down the daemon process. Minimizing keeps both
running. Opening the panel again starts a new daemon with the saved choices.

Stop other tablet drivers before using this driver. Only the Wacom PTH-660 has
confirmed Windows hardware support; other tablets remain experimental.

The native panel and driver need no .NET runtime. Existing OpenTabletDriver
.NET filters, output modes, bindings, tools and custom report parsers need
Microsoft's **x64 .NET 8 runtime or newer**:
[download .NET](https://dotnet.microsoft.com/en-us/download/dotnet/8.0).

Settings are stored in `%LOCALAPPDATA%\OpenTabletDriverRust`. Updating or moving
this extracted folder does not move your settings. To update, use the panel's
update command or extract the latest release into a new folder and open its UI.

`opentabletdriver-rust.exe` is the command-line app. Run it with `--help` for
commands. Full guides, examples and source are in the
[project repository](https://github.com/z9a17/opentabletdriver-rust).
Licenses and source notices are in [data/licenses](data/licenses).
