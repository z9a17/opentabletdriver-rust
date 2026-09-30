# Reference

The full command-line, profile, daemon and build reference. For a short introduction, see the [README](../README.md).


A Windows 11 USB tablet driver written in Rust. It was built on the Wacom PTH-660 and, since 0.11.0, runs every tablet in OpenTabletDriver's database: all 52 of its report parsers are ported and checked against OpenTabletDriver's own parsers. Only the PTH-660 has been tested on hardware; reports from other tablets are welcome, and `opentabletdriver-rust.exe tablets --list` shows every supported model. It supports absolute/relative cursor movement, tip clicks, a native Windows control panel, native filter DLLs, and unchanged OpenTabletDriver .NET synchronous filters before and after mapping. Managed filters can suppress reports or emit several reports; PostTransform sees desktop pixels in Absolute Mode and motion deltas in Relative Mode. Absolute Mode can also drive a Windows Ink pen with pressure, tilt, eraser and hover ([pen output](PEN_OUTPUT.md)); it has not yet been used with a tablet and a drawing application. Pen side buttons can click mouse buttons, press keys/chords or drive barrel buttons on pen output; see [pen side buttons](PEN_BUTTONS.md). Live side-button validation is pending. See [plugin/UI support and compatibility limits](PLUGINS_AND_UI.md) and the [porting status](PORTING_STATUS.md).

For the path to full OpenTabletDriver parity, start with the [full parity roadmap](FULL_PARITY_PLAN.md). It includes [65 implementation tasks](parity/WORK_ITEMS.md), a pinned device/plugin inventory, dependencies, performance and validation gates, and an [agent handoff guide](parity/AGENT_HANDOFF.md). Claim work through the [GitHub workstream tracker](parity/GITHUB_TRACKING.md). These planned capabilities are not yet implemented.

The combined executable is licensed under GPL-3.0-only because its built-in Radial Follow filter is a Rust port of [AbstractQbit's RadialFollow 0.3.0](https://github.com/AbstractQbit/AbstractOTDPlugins/tree/0.3.0/RadialFollow), which is GPL-3.0-only. The earlier driver source retains its LGPL-3.0-only terms in [LICENSE.LGPL-3.0](../LICENSE.LGPL-3.0); the combined executable uses [LICENSE](../LICENSE). The PTH-660 USB identification and report layout were researched from [OpenTabletDriver 0.6.x](https://github.com/OpenTabletDriver/OpenTabletDriver/tree/fdeaa7b0c6d6f5260f511f19fb693ed33524af4e). The [implementation plan](IMPLEMENTATION_PLAN.md) records source links and acceptance criteria.

Release packages also include a [Linux CLI](../crates/otd-linux/README.md) with hidraw/uinput and a [macOS CLI](../crates/otd-macos/README.md) with IOKit/CoreGraphics. Both share the core pen-button mouse/key bindings. Linux capture and device discovery have run on the connected PTH-660; live cursor/drawing behavior remains pending. macOS native hardware validation remains pending. The Windows panel, daemon and external plugin host are not included in these CLI packages.

## Open the driver

For the GUI, extract the complete release ZIP and open **opentabletdriver-rust-ui.exe**. The control panel follows OpenTabletDriver's layout: **Output** has the graphical display and tablet area editors and the relative-mode settings, **Filters** holds the built-in Radial Follow port and native/.NET plugins, **Pen Settings** has the tip and eraser thresholds, and **Console** shows driver messages. Use **Save** and **Apply** in the bottom bar; **Tablets > Start driver** starts input when automatic start is off. The panel follows the Windows light or dark mode; **View > Theme** overrides it. Profiles save separately under `%LOCALAPPDATA%\OpenTabletDriverRust` by default. The panel first attaches to an existing daemon and loads its active configuration; otherwise it can launch the daemon without a console window and start the driver; **Tablets > Start driver when the panel opens** turns this off. **Tablets > Tablet debugger** shows the connected tablet, its parser, the report rate, each raw packet and its decoded values, and the pen on an outline of the tablet; `opentabletdriver-rust.exe debug` prints the same snapshot. **Tablets > Settings for tablet** chooses which tablet the settings are for; the area editor and pressure thresholds then use that tablet's size and range.

- **Plugins > Plugin manager** lists OpenTabletDriver's plugin catalog for version 0.6.7. It installs, updates and removes plugins after checking each download against the catalog's SHA-256, and **Add to settings** adds a plugin's filters and tools. **From file...** installs a plugin zip or DLL you downloaded yourself; it has no catalog hash, so install only files you trust.
- OpenTabletDriver **tools** (`ITool` plugins, such as tray or overlay helpers) start while the driver runs and stop with it. They appear in the Filters list like filters.
- .NET filters run on every supported tablet, including **timer-driven filters** such as interpolators, which tick on the report thread at their Frequency setting.
- **Tablets > Device string reader** shows the connected tablets' USB strings, and **Help > Export diagnostics** saves or copies a redacted diagnostic report.
- When several supported tablets are connected, the driver runs all of them, as OpenTabletDriver does. The chosen tablet keeps the panel's settings; each other tablet uses its OpenTabletDriver profile, or its full area when it has none.
- **File > Presets** loads and saves named settings, and **File > Save console log** writes the Console to a file.
- **Tablets > Start with Windows** opens the panel in the tray when you sign in.
- The panel checks for updates in the background on launch by default. When a newer release is available, it opens the **Update available** screen, including launches minimized to the tray. Choose whether to download and install, then whether to restart. Up-to-date launches show no prompt; failed startup checks report in the Console. **Help > Check for updates** checks manually and reports every outcome. Turn off the launch check in the same menu.
- On the command line, `update [--check]`, `plugins catalog|installed|install NAME|install-file PATH|remove NAME` and `device-strings VID PID` do the same. `device-strings` reads a tablet's USB strings for writing a configuration. Stop the original OpenTabletDriver through its own panel before starting Rust output. Minimizing the panel hides it in the notification area: click the tray icon to bring it back, or right-click it for **Start driver** and **Close**. Closing the panel waits for tablet input to stop; minimizing keeps it running. CLI `stop`/`shutdown` remain available. .NET plugins require the bundled `compat` folder and an installed x64 .NET 8+ runtime; Rust-only profiles do not initialize .NET.

On Windows, double-click **opentabletdriver-rust.exe**. It is a console application and runs the driver in the foreground with no arguments, showing the active mapping, connection, and reconnection status. Before injecting cursor input, it checks that the original OpenTabletDriver is stopped. Stop it through its own panel, and stop any other tablet daemon yourself. Press **Ctrl+C** for a normal stop. Keep the console open while using the tablet. It does not modify Windows HID drivers or startup registration.

With no explicit configuration, the driver first loads `driver.toml` from the Rust settings directory. If absent, it imports `%LOCALAPPDATA%\OpenTabletDriver\settings.json` and selects its **Wacom PTH-660** profile. It follows the enabled absolute or relative output mode and enabled tip/eraser bindings with their pressure activation thresholds. Absolute mode imports tablet and display area sizes and centers, tablet rotation, clipping, and area limiting. Relative mode imports per-axis sensitivity, rotation, and reset delay, preserving fractional motion between reports. Use `--otd-settings PATH` to import an OTD file explicitly. Profiles are read at worker startup; Apply/Save restarts an attached worker.

New OpenTabletDriver imports honor each filter's `Enable` flag. Enabled **Radial Follow Smoothing (Tablet coordinates)** entries become the native Rust filter; disabled entries remain in the preserved source archive and are not activated. Existing saved native `[[radial_follow]]` entries stay active when an older Rust profile is loaded. The explicit legacy import option restores the earlier force-enable behavior for that import only. The native filter keeps its original settings, curve and 50 ms reset without loading .NET. Other unsupported enabled filters and bindings produce diagnostics and remain archived; add supported filter DLLs explicitly to run them. The Windows Ink and Windows Pen Pointer plugins' absolute modes import as the native [pen output](PEN_OUTPUT.md); other unsupported output modes are rejected. Pen side buttons import supported mouse, key, chord and barrel bindings; express keys remain unimplemented.

For relative mode without OpenTabletDriver settings, use `opentabletdriver-rust.exe run --config driver.relative.example.toml`. [Relative mode details](RELATIVE_MODE.md) document reset behavior, performance checks, and upstream compatibility. Windows pointer speed and acceleration affect relative motion. Relative mode has automated replay coverage but still needs live tablet validation.

## Profiles and headless control

Schema-1 TOML profiles preserve the full original OpenTabletDriver settings text, source revision, disabled stores and unknown fields. Unsupported native TOML fields are archived under `preserved_fields`; preservation does not execute those features. The profile collection and import/export commands can inspect other tablets' settings, but the driver still runs only a supported USB PTH-660 configuration. Export reconciles representable edits into a copy and leaves the original source archive unchanged. See [settings contracts](parity/BEHAVIOR_CONTRACTS.md#settings-import).

Offline profile commands do not open devices or load plugins. Profile indexes start at zero, and output files must be new:

```text
opentabletdriver-rust.exe profiles list settings.json
opentabletdriver-rust.exe profiles preview settings.json --profile 0
opentabletdriver-rust.exe profiles import settings.json --profile 0 --output imported.toml
opentabletdriver-rust.exe profiles export imported.toml --output exported-settings.json
opentabletdriver-rust.exe profiles select collection.toml --name Gaming --output selected-collection.toml
opentabletdriver-rust.exe profiles recover driver.toml --output recovered.toml
opentabletdriver-rust.exe profiles get driver.toml --section areas
opentabletdriver-rust.exe profiles set driver.toml --output faster.toml --sensitivity 12,12 --reset-time 100
opentabletdriver-rust.exe profiles paths
```

Use `--legacy-force-radial-follow` on OTD preview/import only when that earlier behavior is intended. Collection selection does not switch a running driver; extract the desired entry with `profiles import` before starting it.

`profiles get` prints one settings section (`all`, `output`, `areas`, `sensitivity`, `bindings`, `filters` or `misc`) as JSON, like the upstream console's `get*` commands but for a file. `profiles set` changes relative sensitivity, rotation and reset time and writes a new profile; start it with `restart --config`. The [command matrix](parity/CLI_COMMAND_MATRIX.md) maps every upstream console command to its Rust equivalent or open task.

Offline named presets support `presets list`, `presets show NAME`, `presets save NAME --config FILE` and `presets export NAME --output NEWFILE`. Save creates a new name; explicit `--replace` uses a loaded-byte conflict guard and retains a backup. Names preserve spelling and reject unsafe paths or case-only aliases. These commands do not select/apply runtime settings. See [named presets](NAMED_PRESETS.md).

The default Rust settings directory is `%LOCALAPPDATA%\OpenTabletDriverRust`. Set `OTD_RUST_PORTABLE_DIR` to an absolute directory for portable profiles/preferences. Saves stage and flush a sibling file, retain the previous bytes in the reserved `.bak` sidecar, and reject changes made since the editor loaded the destination. Save As requires a new name. Recovery validates the backup and writes a new file. Relative plugin locations within the destination/portable tree are retained; other locations stay absolute. See [persistence limits](parity/BEHAVIOR_CONTRACTS.md#profile-persistence).

An optional headless daemon stays open independently of CLI clients:

```text
opentabletdriver-rust.exe daemon --background
opentabletdriver-rust.exe start --config driver.toml
opentabletdriver-rust.exe status
opentabletdriver-rust.exe configuration
opentabletdriver-rust.exe restart --config driver.toml
opentabletdriver-rust.exe stop
opentabletdriver-rust.exe shutdown
```

`daemon --background` creates no console window and starts an idle control service. `start` requests a driver worker; inspect `status` for startup errors. `stop` can return `stopping` until cleanup finishes; `shutdown` exits the service after worker cleanup. Inspect state as well as generation: `starting` can mean waiting for a tablet or activation, and an accepted apply keeps the old generation until commit. The GUI attaches to this service, fetches its active configuration and reconnects in the background. Closing the panel requests a guarded stop and waits for worker cleanup before closing. The idle service remains available; `shutdown` exits it. A timeout leaves the panel open with an error. Apply/Save uses a generation-guarded restart and preserves unsaved editor changes when another client changes the active profile. Restart prepares the replacement while current input continues, then drains the old reader/output before activating it. A pre-commit activation failure can resume the retained old worker; failures after commit stop the new generation. See [runtime reconfiguration](RUNTIME_RECONFIGURATION.md) for rollback and cancellation limits. The instance guard prevents simultaneous Rust injectors; foreground `run` and no-argument operation remain available.

`configuration` returns the daemon's active TOML in a JSON reply. `restart` without a profile reuses that configuration; `--config FILE` or `--otd-settings FILE` supplies a replacement. Retrieval/restart checks daemon instance and worker generation and reports conflicts without automatically retrying. Configuration replies can contain private plugin paths/settings; diagnostics export provides a redacted alternative.

Control uses a bounded, current-user Windows named pipe with protocol version 2; it rejects remote clients. A v1 daemon from 0.8.0 has a separate endpoint: automatic launch refuses to proceed while it is present. Stop and shut down that service with its original executable before starting this version; it is not killed or adopted automatically. After a timeout, check status before retrying because a command may already have been accepted. Device initialization checks cancellation between operations, but synchronous HID string/feature calls cannot be cancelled in flight. Interactive UI, daemon IPC, plugin and hardware validation for 0.10.0 remain pending.

Offline area previews and diagnostics are also available:

```text
opentabletdriver-rust.exe area convert percentage 0 0 1 1
opentabletdriver-rust.exe area full --tablet "Wacom PTH-660"
opentabletdriver-rust.exe area fit 224 148 1.7777777778
opentabletdriver-rust.exe diagnostics --output diagnostics.json --config driver.toml
```

Area commands return millimetre JSON previews without changing profiles or starting devices. Converters include percentage, Wacom/VEIKK, XP-Pen and separately named pinned/corrected Gaomon V2 behavior; `area --help` gives input order and units. Diagnostics captures static build/display/profile summaries and an optional daemon status snapshot without opening HID or loading plugins. Paths, plugin property values, imported archives and free-form logs are excluded by default. `--include-private-details` adds paths and messages for explicit review before sharing. Diagnostic output must be a new file.

## Build and diagnostics

The tablet area's **Convert area from...** action offers the same conversion formulas with a validated preview. **Use area** changes unsaved tablet settings only. See [area conversion](AREA_CONVERSIONS.md) for lock behavior and units.

`decode --parser intuos-v3 --hex "11 00 00 00 00 00"` prints a checked JSON report without opening a device or running an output pipeline. `--input FILE` accepts bounded hex packets one per line, preserving touch state within that input. The 27 decoder names cover 23 newly ported stateless dispatchers plus existing PTH-660/auxiliary/touch paths. Input is capped at 65,535 bytes per packet, 4,096 reports and 4 MiB. Parser names and raw-byte semantics are listed in [offline report decoding](OFFLINE_REPORT_DECODING.md). The Windows runtime can select configured tablets with supported parsers and specifications. Current hardware evidence is limited to PTH-660; configuration/parser coverage does not establish physical compatibility. See [PTK-470 and tablet-label qualification](TABLET_IDENTITY_0.14.4.md).

In **Filters**, select an entry and use **Enabled** to switch it on or off. **Add .NET**, **Add native**, **Remove** and **Defaults** form a compact two-row toolbar; the Move up / Move down controls are removed. Existing profiles keep their saved order. Built-in Radial Follow entries run before DLL filters; DLLs run in saved order within their declared stage. **Defaults** restores the selected built-in or inspected .NET filter's settings without changing enabled state, DLL path or class. .NET defaults come from the DLL's default-value attributes; settings without attributes revert to constructor behavior when applied. Native DLLs have no default-settings contract, so their Defaults button is disabled. **Save** persists settings; **Apply** activates them.

The settings pane shows the controls immediately below **Enabled**. Hover the filter entry for its full name, DLL path/type and settings information. The plugin manager uses the panel palette for its header, rows, controls and borders; plugin names fill the available width. Clicking an open dropdown's button again closes it. See [0.14.3 UI cleanup](UI_CLEANUP_0.14.3.md).

Inspected .NET filters show declared properties and default information on hover. Boolean and named enum properties offer choices; flags remain editable as text. Supported scalar edits use the declared CLR type and numeric bounds. The per-property Use default buttons and raw JSON editor have been removed. The selected-filter **Defaults** action remains. Merely viewing an omitted property keeps it omitted and does not construct the plugin. Long lists use Previous/Next pages without discarding edits.

Unknown keys and structured values are preserved in the profile. Scalar native/settings-without-metadata values can use normal fields; unsupported custom, structured, decimal and untyped-null settings are read-only. Invalid edits block Save/Apply. Matching daemon responses after Save/Apply keep the selected filter, page and list position. Dynamic validation and full plugin compatibility remain open. See [0.14.2 fixes and native/.NET feel comparison](DEBUGGER_AND_UI_0.14.2.md).

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

`settings` shows the effective mapping and bindings. `tablets` summarizes the embedded OpenTabletDriver tablet database and any override files in `%LOCALAPPDATA%\OpenTabletDriver\Configurations`. Add `--list` to print every tablet with its USB IDs and parser support. The driver still drives only the USB PTH-660. `capture` reads a short pen trace without cursor injection. [Input latency](INPUT_LATENCY.md) describes the report thread's scheduling, hover tracking and the benchmark commands. [Performance](PERFORMANCE.md) compares per-report cost, `SendInput`, idle CPU and memory with OpenTabletDriver 0.6.7 on the same machine. HID paths and serial identifiers are omitted unless you explicitly use `list --paths`.

For an independent Rust TOML profile instead of the active OpenTabletDriver settings, use `opentabletdriver-rust.exe run --config driver.toml`; see [driver.example.toml](../driver.example.toml). To test a different OpenTabletDriver settings file, use `--otd-settings path\to\settings.json`. To uninstall, stop the console and remove the executable.

Windows mouse injection does not carry pressure, tilt, or eraser data to drawing applications. Those values are decoded from the tablet, but native Windows Ink output is a later milestone.

The real PTH-660 reported a readable 192-byte pen collection and a 44-byte auxiliary collection. Hover, contact, lift, and proximity-loss reports were captured. The user confirmed that the earlier 0.2.0 console build moved the cursor and clicked with the pen using the active OpenTabletDriver profile. The 0.3.0 built-in Radial Follow port matches the original C# curve in automated reference tests; live cursor feel with the filter has not been checked yet, because the running 0.2.0 driver was left untouched. The driver also detected an unplug and reopened the HID collection after replug. Pen movement after that replug, sleep/wake, and display changes remain open hardware checks in the [validation guide](HARDWARE_VALIDATION.md).
