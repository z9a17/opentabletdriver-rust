# Control panel and plugins

Extract the complete Windows release ZIP and open **opentabletdriver-rust-ui.exe**. The control panel follows OpenTabletDriver's desktop layout: a **File / Tablets / Plugins / View / Help** menu bar, **Output**, **Filters**, **Pen Settings** and **Console** tabs, and a bottom bar with the tablet status, **Start driver**, **Save** and **Apply**. The console executable retains its existing no-argument daemon behavior; `opentabletdriver-rust.exe ui` also opens the panel.

- **Output** draws the display area over your monitors and the tablet area over the PTH-660's 224 × 148 mm surface, as upstream's area editors do. Drag an area, type its size, center and tablet rotation, or right-click it for alignment, resizing, flipping, **Set to display**, **Lock to usable area**, **Lock aspect ratio**, **Clamp input outside area** (clipping) and **Ignore input outside area** (area limiting). The dropdown below the editors switches between **Absolute Mode** and **Relative Mode** (X/Y sensitivity, rotation and reset time).
- **Filters** lists the built-in Radial Follow port and the profile's native and .NET plugin entries. Each has an **Enable** check box and its settings.
- **Pen Settings** sets the tip and eraser bindings (left click or none) and their activation thresholds in percent. An empty threshold uses the pen's own tip switch.
- **Console** lists driver, settings and plugin messages with their time and level. When a device session ends it also shows how long reports took from read to output; see [input latency](INPUT_LATENCY.md). **Copy All**, or Ctrl+C on selected rows, copies them.

The panel follows the Windows light or dark app mode and switches when it changes. **View > Theme** picks Light or Dark instead; high-contrast themes use the system colors. The theme, the area locks, the window size and the **Start driver when the panel opens** setting are stored in `ui.toml` beside the profile, never in the profile itself.

The panel opens `%LOCALAPPDATA%\OpenTabletDriverRust\driver.toml` if present, otherwise imports the current OpenTabletDriver mapping and built-in filter settings. Like OpenTabletDriver's UX, which starts its daemon, the panel then starts the driver with those settings. If the settings fail to load, or another Rust driver such as the console daemon is already running, it leaves the driver stopped and says why in the Console. **Tablets > Start driver when the panel opens** turns the automatic start off. **Save** (Ctrl+S) writes that file and **File > Save settings as...** writes another; neither edits OpenTabletDriver's `settings.json`. **Start driver** uses the current settings. As in OpenTabletDriver, **Apply** (Ctrl+Enter) and **Save** restart a running driver with the new settings; while the driver is stopped they take effect at the next start. Closing the panel stops its worker and waits for normal input cleanup. The existing instance guard prevents another Rust daemon from injecting simultaneously. The status bar and the Console report connection and startup errors.

As in OpenTabletDriver, the panel keeps an icon in the notification area while it runs. Minimizing the panel hides it there and removes its taskbar button; clicking the icon, or **Show Window** in its menu, brings the panel back. The icon's tooltip shows the driver state. Its menu also has **Start driver** / **Stop driver**, which upstream's lacks, and **Close**, which closes the panel and stops the driver. Opening the panel again while it runs brings the running panel forward instead of starting a second one. A shortcut set to run minimized starts the panel in the tray.

Profiles that use the older crop/monitor keys appear as the equivalent areas; the file keeps that form until you edit an area. `device_path` has no editor and is preserved.

## Existing OpenTabletDriver .NET plugins

Use **Plugins > Add .NET plugin...**, or **Add .NET...** on the Filters tab, to select an unchanged plugin assembly. Its exported OTD filter classes are added as disabled entries. Select a class, edit its settings, then check **Enable**. The Filters tab reads `[PluginName]`, `[Property]` display names, `[Unit]` and `[ToolTip]` from the DLL, including for entries loaded from a saved profile. The editor shows scalar settings already present in the profile or supplied by DLL defaults; a property without either does not appear until it is added to the profile's `settings_json`. Unknown settings retain generic labels, and nested values open in the JSON editor. Metadata is read when the Filters tab opens and kept only in memory; saving does not change the profile format. The plugin's adjacent dependencies must remain alongside its DLL. Add only trusted plugins: both native and managed DLLs execute with the driver's permissions.

Discovery omits filter classes marked `[PluginIgnore]` or restricted to another OS by `[SupportedPlatform]`. If a saved profile enables such a class explicitly, loading reports why it was rejected; no other filter is substituted. This follows upstream's [platform and ignore checks](https://github.com/OpenTabletDriver/OpenTabletDriver/blob/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.Desktop/Reflection/DesktopPluginManager.cs) and [platform flag contract](https://github.com/OpenTabletDriver/OpenTabletDriver/blob/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.Plugin/Attributes/SupportedPlatformAttribute.cs).

The bridge supports **synchronous, one-output position filters** implementing OpenTabletDriver's `IPositionedPipelineElement<IDeviceReport>`. PreTransform filters run on tablet coordinates before mapping. Pixels/PostTransform filters run on desktop pixel coordinates after mapping when the output is Absolute Mode. The bridge uses the official `OpenTabletDriver.Plugin` **0.6.7** assembly, sets `[Property]` values/defaults, injects the PTH-660 `[TabletReference]`, and invokes `[OnDependencyLoad]` methods. The original, unchanged **RadialFollow 0.3.0 DLL** now passes integration tests for both its tablet-space and screen-space classes. Select either class in the panel's Filters tab; screen-space radius settings are in pixels.

Compatibility is deliberately explicit:

- Supported filters consume one report and synchronously emit one positional report on the same thread. Only returned X/Y changes are applied; pressure and button transformations are outside this ABI. Native DLL filters remain PreTransform only.
- Pixel/PostTransform filters need Absolute Mode; `check-plugins` and driver startup reject them in Relative Mode. Asynchronous interpolators, output-mode plugins (including Windows Ink), tools, bindings, and filters requiring unresolved services are not supported. They fail initialization or are disabled if they violate the synchronous contract. DLL discovery lists available filter classes; discovery does not guarantee each class is supported.
- The report adapter supplies position, pressure, eraser state and an empty raw-report array; it does not emulate device-specific report types, tilt, rotation, or pen-button input. Plugins that depend on those need further adapter work.
- Unknown enabled filters in imported OTD settings are still reported as skipped. Add their DLL/type/settings explicitly in the panel. Plugin installation from the upstream online catalog and automatic settings migration are not implemented yet.
- The existing native Radial Follow port remains the efficient default for imported profiles. To use the original .NET Radial Follow instead, uncheck the built-in entry on the Filters tab (this removes its `[[radial_follow]]` block) before enabling the DLL, so smoothing is not applied twice. The panel warns while both are enabled.

Keep the release's **compat/** folder beside both executables and install the **Windows x64 .NET 8 runtime or newer**. The bridge is framework-dependent; it does not download or install a runtime. Rust-only/native-plugin profiles do not start .NET. Hosts with no .NET runtime can still use the native UI, Rust built-ins, and native plugins.

Diagnostic commands, which do not open HID or inject input:

```text
opentabletdriver-rust.exe inspect-plugin C:/plugins/RadialFollow.dll
opentabletdriver-rust.exe check-plugins driver.toml
```

These commands load executable plugin code. Plain `settings` validates configuration without loading DLLs. `inspect-plugin` prints class names/default properties; `check-plugins` checks all enabled plugins and reports initialization errors.

## Native Rust plugins

The release also includes `otd_ema_filter.dll`, a small exponential-smoothing example. [driver.plugins.example.toml](../driver.plugins.example.toml) enables it. Its `alpha` is in 0..1 (1 follows the pen immediately) and `reset_ms` sets the inactivity reset. Relative DLL paths resolve against the profile's directory, not the process's working directory. In the panel, **Add native...** adds a DLL entry; its settings are edited as JSON because native DLLs do not describe their properties.

The versioned C ABI is defined in [otd-plugin-api](../crates/otd-plugin-api/src/lib.rs); the buildable example is in [plugins/ema](../plugins/ema). Plugins export `otd_filter_v1`, returning a version/size-checked table with a fixed-size name and create/process/reset/destroy callbacks. JSON is decoded only on creation. A context is owned and freed by its DLL; Rust-owned allocations never cross the boundary. Callbacks run in report order on one driver thread. They must not unwind, block, retain sample pointers, or allocate per report.

The pipeline runs built-in Radial Follow first, then PreTransform DLL entries in profile order, then mapping, then Pixels/PostTransform DLL entries in profile order before mouse output. A failing callback or nonfinite output disables that plugin for the current run and preserves the prior valid position. Losing the pen (neither hover bit set) or reconnecting resets healthy plugin instances; restart the driver to retry a disabled plugin. Native ABI version 1 filters position only.

## Efficiency and verification

Native callbacks use fixed-size samples and function pointers. The .NET bridge hosts CoreCLR in-process using Microsoft's `nethost`/`hostfxr` interfaces, with a cached managed report and GCHandle per plugin. There is no per-report JSON, reflection, subprocess IPC, or report queue. Driver reflection, configuration, DLL loading and dependency resolution happen at startup; UI metadata inspection happens when the Filters tab opens. Third-party managed plugins can still allocate or pause for GC; Rust-only measurements do not characterize .NET plugin latency. The UI runs the driver on a separate worker thread, which posts its status messages to the window; the panel uses no timers. The panel renders at the monitor's DPI, while the driver thread keeps the process's default DPI awareness so its display coordinates match the console daemon's.

The native DLL integration test loads the compiled sample, verifies its smoothing and reset, and checks the Rust-side report call for allocations. The .NET integration test loads the unmodified upstream RadialFollow DLL, checks tablet and screen dead zones, verifies labels/units/tooltips, and replays the screen class through mapping and a simulated mouse output. Its archive is checksum-pinned in CI. A mixed-stage native test checks ordering and no Rust allocation during a report. Profile serialization/mapping tests cover UI saves and fractional output from simple TOML profiles. Unit tests cover the panel's editing model: area locks, alignment and flips, crop-to-area conversion, threshold percentages, output-mode switching and plugin settings. The window itself has no automated test. Earlier releases were checked with scripted runs against sandboxed profiles: every tab in both themes, field edits and validation, area dragging, context and dropdown menus, OpenTabletDriver import, saving, and Start/Apply/Stop with a device path that does not exist. The metadata display added in 0.7.4 has not had a visual window check. Live pen feel, successful device start/stop and plugin behavior across hardware reconnects remain manual checks.

Build and package on Windows with Rust stable and a .NET 8+ SDK:

```powershell
cargo fmt --all --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace
pwsh -File scripts/package.ps1
```

The package script builds both executables, the sample DLL, the compatibility bridge and notices, then produces a versioned ZIP and SHA-256 sidecar in `target/`.

References: [official OTD plugin package](https://www.nuget.org/packages/OpenTabletDriver.Plugin/0.6.7), [OTD property labels](https://github.com/OpenTabletDriver/OpenTabletDriver/blob/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.Plugin/Attributes/PropertyAttribute.cs), [units](https://github.com/OpenTabletDriver/OpenTabletDriver/blob/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.Plugin/Attributes/UnitAttribute.cs), [tooltips](https://github.com/OpenTabletDriver/OpenTabletDriver/blob/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.Plugin/Attributes/ToolTipAttribute.cs), [OTD setting/dependency initialization](https://github.com/OpenTabletDriver/OpenTabletDriver/blob/fdeaa7b0c6d6f5260f511f19fb693ed33524af4e/OpenTabletDriver.Desktop/Reflection/PluginSettingStore.cs), [Microsoft native .NET hosting](https://learn.microsoft.com/en-us/dotnet/core/tutorials/netcore-hosting), and [original RadialFollow plugin](https://github.com/AbstractQbit/AbstractOTDPlugins/tree/0.3.0/RadialFollow).
