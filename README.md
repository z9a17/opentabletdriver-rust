# opentabletdriver-rust

A Windows 11 USB driver for the Wacom PTH-660, written in Rust. It supports absolute/relative cursor movement, tip clicks, a native Windows control panel, native filter DLLs, and unchanged OpenTabletDriver .NET synchronous position filters before mapping or, in Absolute Mode, in screen pixels after mapping. Side-button output and Windows Ink remain unimplemented. See [plugin/UI support and compatibility limits](docs/PLUGINS_AND_UI.md) and the [porting status](docs/PORTING_STATUS.md).

For the path to full OpenTabletDriver parity, start with the [full parity roadmap](docs/FULL_PARITY_PLAN.md). It includes [65 implementation tasks](docs/parity/WORK_ITEMS.md), a pinned device/plugin inventory, dependencies, performance and validation gates, and an [agent handoff guide](docs/parity/AGENT_HANDOFF.md). Claim work through the [GitHub workstream tracker](docs/parity/GITHUB_TRACKING.md). These planned capabilities are not yet implemented.

The combined executable is licensed under GPL-3.0-only because its built-in Radial Follow filter is a Rust port of [AbstractQbit's RadialFollow 0.3.0](https://github.com/AbstractQbit/AbstractOTDPlugins/tree/0.3.0/RadialFollow), which is GPL-3.0-only. The earlier driver source retains its LGPL-3.0-only terms in [LICENSE.LGPL-3.0](LICENSE.LGPL-3.0); the combined executable uses [LICENSE](LICENSE). The PTH-660 USB identification and report layout were researched from [OpenTabletDriver 0.6.x](https://github.com/OpenTabletDriver/OpenTabletDriver/tree/fdeaa7b0c6d6f5260f511f19fb693ed33524af4e). The [implementation plan](docs/IMPLEMENTATION_PLAN.md) records source links and acceptance criteria.

## Open the driver

For the GUI, extract the complete release ZIP and open **opentabletdriver-rust-ui.exe**. The control panel follows OpenTabletDriver's layout: **Output** has the graphical display and tablet area editors and the relative-mode settings, **Filters** holds the built-in Radial Follow port and native/.NET plugins, **Pen Settings** has the tip and eraser thresholds, and **Console** shows driver messages. Use **Start driver** / **Stop driver**, **Save** and **Apply** in the bottom bar. The panel follows the Windows light or dark mode; **View > Theme** overrides it. Profiles save separately under `%LOCALAPPDATA%\OpenTabletDriverRust` by default. Like OpenTabletDriver, the panel starts the driver when it opens; **Tablets > Start driver when the panel opens** turns this off. While the driver runs, it pauses the original OpenTabletDriver, as the console daemon below does. Minimizing the panel hides it in the notification area: click the tray icon to bring it back, or right-click it for **Start driver** / **Stop driver** and **Close**. Closing the panel stops the driver. .NET plugins require the bundled `compat` folder and an installed x64 .NET 8+ runtime; Rust-only profiles do not initialize .NET.

On Windows, double-click **opentabletdriver-rust.exe**. It is a console application and starts the daemon with no arguments, showing the active mapping, connection, and reconnection status. Before injecting cursor input, it temporarily pauses any running original OpenTabletDriver daemon and UX; stop any other tablet daemon yourself. Press **Ctrl+C** for a normal stop; the Rust process restores those original processes afterward. Keep the console open while using the tablet. It does not modify Windows HID drivers or startup registration.

With no arguments, the driver reads `%LOCALAPPDATA%\OpenTabletDriver\settings.json` and selects its **Wacom PTH-660** profile. It follows the enabled absolute or relative output mode and enabled tip/eraser bindings with their pressure activation thresholds. Absolute mode imports tablet and display area sizes and centers, tablet rotation, clipping, and area limiting. Relative mode imports per-axis sensitivity, rotation, and reset delay, preserving fractional motion between reports. The current development-machine absolute profile maps an 85 × 47.8125 mm tablet area to a 2560 × 1440 display area with clipping and a 1% tip threshold. Profile values are read at launch, so restart the daemon after changing them in OpenTabletDriver.

New OpenTabletDriver imports honor each filter's `Enable` flag. Enabled **Radial Follow Smoothing (Tablet coordinates)** entries become the native Rust filter; disabled entries remain in the preserved source archive and are not activated. Existing saved native `[[radial_follow]]` entries stay active when an older Rust profile is loaded. The explicit legacy import option restores the earlier force-enable behavior for that import only. The native filter keeps its original settings, curve and 50 ms reset without loading .NET. Other unsupported enabled filters and bindings produce diagnostics and remain archived; add supported filter DLLs explicitly to run them. Unsupported output modes are rejected. Pen side buttons, express keys and Windows Ink output remain unimplemented.

For relative mode without OpenTabletDriver settings, use `opentabletdriver-rust.exe run --config driver.relative.example.toml`. [Relative mode details](docs/RELATIVE_MODE.md) document reset behavior, performance checks, and upstream compatibility. Windows pointer speed and acceleration affect relative motion. Relative mode has automated replay coverage but still needs live tablet validation.

## Profiles and headless control

Schema-1 TOML profiles preserve the full original OpenTabletDriver settings text, source revision, disabled stores and unknown fields. Unsupported native TOML fields are archived under `preserved_fields`; preservation does not execute those features. The profile collection and import/export commands can inspect other tablets' settings, but the driver still runs only a supported USB PTH-660 configuration. Export reconciles representable edits into a copy and leaves the original source archive unchanged. See [settings contracts](docs/parity/BEHAVIOR_CONTRACTS.md#settings-import).

Offline profile commands do not open devices or load plugins. Profile indexes start at zero, and output files must be new:

```text
opentabletdriver-rust.exe profiles list settings.json
opentabletdriver-rust.exe profiles preview settings.json --profile 0
opentabletdriver-rust.exe profiles import settings.json --profile 0 --output imported.toml
opentabletdriver-rust.exe profiles export imported.toml --output exported-settings.json
opentabletdriver-rust.exe profiles select collection.toml --name Gaming --output selected-collection.toml
```

Use `--legacy-force-radial-follow` on OTD preview/import only when that earlier behavior is intended. Collection selection does not switch a running driver; extract the desired entry with `profiles import` before starting it.

An optional headless daemon stays open independently of CLI clients:

```text
opentabletdriver-rust.exe daemon --background
opentabletdriver-rust.exe start --config driver.toml
opentabletdriver-rust.exe status
opentabletdriver-rust.exe stop
opentabletdriver-rust.exe shutdown
```

`daemon --background` creates no console window and starts an idle control service. `start` requests a driver worker; inspect `status` for startup errors. `stop` can return `stopping` until cleanup finishes; `shutdown` exits the service after worker cleanup. A running worker may be waiting for a tablet. The GUI still owns its own worker: it does not attach to this service yet, and closing it stops that worker. The existing single-instance guard prevents two Rust workers from injecting simultaneously. The console's no-argument foreground behavior is retained.

Control uses a versioned, bounded, current-user Windows named pipe; it rejects remote clients. After a timeout, check status before retrying a command because the daemon may already have accepted it. Device initialization checks cancellation between operations, but synchronous HID string/feature calls cannot be cancelled in flight. Interactive UI, daemon IPC and hardware validation for 0.8.0 remain pending.

## Build and diagnostics

In **Filters**, use **Move up** / **Move down** to change saved filter order. Built-in Radial Follow entries stay before DLL filters; DLL filters execute in saved order within their declared stage (tablet coordinates before mapping, pixels after mapping). Disabled entries keep their place. **Defaults** restores the selected built-in or inspected .NET filter's settings without changing its enabled state, DLL path or class. .NET defaults come from the DLL's default-value attributes; settings without attributes revert to constructor behavior when applied. Native DLLs have no default-settings contract, so their Defaults button is disabled. These edits take effect on **Apply** or the next driver start; **Save** persists them.

Inspected .NET filters show every declared property, including settings not yet saved. Boolean and named enum properties offer choices; flags remain editable as text. Supported scalar edits use the declared CLR type and numeric bounds. Decimal values use the JSON editor (quoted decimal strings retain their digits for .NET conversion); custom types receive JSON syntax checks only. **Use default** resets one property without changing the filter's identity or enabled state: an explicit null selects its default-value attribute, or leaves constructor behavior when no attribute exists. Merely viewing an omitted property keeps it omitted and does not construct the plugin. Long lists use Previous/Next pages without discarding edits.

Switch between **Properties** and **Raw JSON** to edit settings. Unknown keys and structured values are retained; invalid edits block the switch so they can be corrected. Native DLLs and .NET filters without available metadata keep the raw JSON editor. These controls are a partial plugin-editor implementation; actions, dynamic validation and full plugin compatibility remain open.

Use the stable Rust MSVC toolchain on Windows 11:

    cargo build --locked --workspace --release
    cargo test --locked --workspace

To build the optional .NET bridge and package the full release, use a .NET 8+ SDK and run `pwsh -File scripts/package.ps1`. The native UI executable is `target\release\opentabletdriver-rust-ui.exe`.

The executable is `target\release\opentabletdriver-rust.exe`. Read-only commands are:

    opentabletdriver-rust.exe settings
    opentabletdriver-rust.exe list
    opentabletdriver-rust.exe displays
    opentabletdriver-rust.exe tablets
    opentabletdriver-rust.exe capture --seconds 10

`settings` shows the effective mapping and bindings. `tablets` summarizes the embedded OpenTabletDriver tablet database and any override files in `%LOCALAPPDATA%\OpenTabletDriver\Configurations`. Add `--list` to print every tablet with its USB IDs and parser support. The driver still drives only the USB PTH-660. `capture` reads a short pen trace without cursor injection. [Input latency](docs/INPUT_LATENCY.md) describes the report thread's scheduling, hover tracking and the benchmark commands. [Performance](docs/PERFORMANCE.md) compares per-report cost, `SendInput`, idle CPU and memory with OpenTabletDriver 0.6.7 on the same machine. HID paths and serial identifiers are omitted unless you explicitly use `list --paths`.

For an independent Rust TOML profile instead of the active OpenTabletDriver settings, use `opentabletdriver-rust.exe run --config driver.toml`; see [driver.example.toml](driver.example.toml). To test a different OpenTabletDriver settings file, use `--otd-settings path\to\settings.json`. To uninstall, stop the console and remove the executable.

Windows mouse injection does not carry pressure, tilt, or eraser data to drawing applications. Those values are decoded from the tablet, but native Windows Ink output is a later milestone.

The real PTH-660 reported a readable 192-byte pen collection and a 44-byte auxiliary collection. Hover, contact, lift, and proximity-loss reports were captured. The user confirmed that the earlier 0.2.0 console build moved the cursor and clicked with the pen using the active OpenTabletDriver profile. The 0.3.0 built-in Radial Follow port matches the original C# curve in automated reference tests; live cursor feel with the filter has not been checked yet, because the running 0.2.0 driver was left untouched. The driver also detected an unplug and reopened the HID collection after replug. Pen movement after that replug, sleep/wake, and display changes remain open hardware checks in the [validation guide](docs/HARDWARE_VALIDATION.md).
