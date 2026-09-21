# Control panel and plugins (0.4.0)

Extract the complete Windows release ZIP and open **opentabletdriver-rust-ui.exe**. The native Windows control panel has a TOML profile editor, active-OpenTabletDriver import, absolute/relative templates, validation, saving, a plugin list, and Start/Stop controls. The console executable retains its existing no-argument daemon behavior; `opentabletdriver-rust.exe ui` also opens the panel.

The panel starts stopped. It opens `%LOCALAPPDATA%\OpenTabletDriverRust\driver.toml` if present, otherwise imports the current OpenTabletDriver mapping and built-in filter settings. **Save profile** writes the path shown at the top; it does not edit OpenTabletDriver's `settings.json`. **Start driver** uses the current editor contents. Edits while running apply on the next stop/start. Closing the panel stops its worker and waits for normal input cleanup. The existing instance guard prevents another Rust daemon from injecting simultaneously. A small status area reports connection/startup errors. This first UI uses text editors for settings; graphical area dragging and property-specific sliders are future work.

## Existing OpenTabletDriver .NET plugins

Use **Add .NET DLL** to select an unchanged plugin assembly. Its exported OTD filter classes are added as disabled entries. Select a class, edit its property/value JSON, click **Apply plugin settings**, then **Enable / disable**. The plugin's adjacent dependencies must remain alongside its DLL. Add only trusted plugins: both native and managed DLLs execute with the driver's permissions.

This release supports **synchronous PreTransform position filters** implementing OpenTabletDriver's `IPositionedPipelineElement<IDeviceReport>`. It uses the official `OpenTabletDriver.Plugin` **0.6.7** assembly, sets `[Property]` values/defaults, injects the PTH-660 `[TabletReference]`, and invokes `[OnDependencyLoad]` methods. An existing filter can run without recompilation. The shipped bridge has been tested with the original, unchanged **RadialFollow 0.3.0 DLL**, including a configured tablet-space dead zone.

Compatibility is deliberately explicit:

- Supported filters consume one report and synchronously emit one positional report on the same thread. Only returned X/Y changes are applied; pressure and button transformations are outside this ABI.
- Pixel/PostTransform filters, asynchronous interpolators, output-mode plugins (including Windows Ink), tools, bindings, and filters requiring unresolved services are not supported in 0.4.0. They fail initialization or are disabled if they violate the synchronous contract. DLL discovery lists available filter classes; discovery does not guarantee each class is supported.
- The report adapter supplies position, pressure, eraser state and an empty raw-report array; it does not emulate device-specific report types, tilt, rotation, or pen-button input. Plugins that depend on those need further adapter work.
- Unknown enabled filters in imported OTD settings are still reported as skipped. Add their DLL/type/settings explicitly in the panel. Plugin installation from the upstream online catalog and automatic settings migration are not implemented yet.
- The existing native Radial Follow port remains the efficient default for imported profiles. To use the original .NET Radial Follow instead, remove its `[[radial_follow]]` block before enabling the DLL, so smoothing is not applied twice.

Keep the release's **compat/** folder beside both executables and install the **Windows x64 .NET 8 runtime or newer**. The bridge is framework-dependent; it does not download or install a runtime. Rust-only/native-plugin profiles do not start .NET. Hosts with no .NET runtime can still use the native UI, Rust built-ins, and native plugins.

Diagnostic commands, which do not open HID or inject input:

```text
opentabletdriver-rust.exe inspect-plugin C:/plugins/RadialFollow.dll
opentabletdriver-rust.exe check-plugins driver.toml
```

These commands load executable plugin code. Plain `settings` validates configuration without loading DLLs. `inspect-plugin` prints class names/default properties; `check-plugins` checks all enabled plugins and reports initialization errors.

## Native Rust plugins

The release also includes `otd_ema_filter.dll`, a small exponential-smoothing example. [driver.plugins.example.toml](../driver.plugins.example.toml) enables it. Its `alpha` is in 0..1 (1 follows the pen immediately) and `reset_ms` sets the inactivity reset. Relative DLL paths resolve against the profile's directory, not the process's working directory.

The versioned C ABI is defined in [otd-plugin-api](../crates/otd-plugin-api/src/lib.rs); the buildable example is in [plugins/ema](../plugins/ema). Plugins export `otd_filter_v1`, returning a version/size-checked table with a fixed-size name and create/process/reset/destroy callbacks. JSON is decoded only on creation. A context is owned and freed by its DLL; Rust-owned allocations never cross the boundary. Callbacks run in report order on one driver thread. They must not unwind, block, retain sample pointers, or allocate per report.

The pipeline runs built-in Radial Follow first, then enabled DLL entries in profile order, then absolute/relative mapping. A failing callback or nonfinite output disables that plugin for the current run and preserves the prior valid position. Proximity loss/reconnect resets healthy plugin instances; restart the driver to retry a disabled plugin. Native ABI version 1 filters position only.

## Efficiency and verification

Native callbacks use fixed-size samples and function pointers. The .NET bridge hosts CoreCLR in-process using Microsoft's `nethost`/`hostfxr` interfaces, with a cached managed report and GCHandle per plugin. There is no per-report JSON, reflection, subprocess IPC, or report queue. Reflection, configuration, DLL loading and dependency resolution happen at startup. Third-party managed plugins can still allocate or pause for GC; Rust-only measurements do not characterize .NET plugin latency. The UI uses a separate worker and polls status only while the driver is running.

The native DLL integration test loads the compiled sample, verifies its smoothing and reset, and checks the Rust-side report call for allocations. The .NET integration test loads the unmodified upstream RadialFollow DLL, applies settings, injects tablet specifications, and checks actual filter output. Its archive is checksum-pinned in CI. Profile serialization/mapping tests cover UI saves and fractional output from simple TOML profiles. UI checks cover layout, validation, saving, plugin toggling and duplicate-instance refusal. Live pen feel, successful device start/stop and plugin behavior across hardware reconnects remain manual checks.

Build and package on Windows with Rust stable and a .NET 8+ SDK:

```powershell
cargo fmt --all --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace
pwsh -File scripts/package.ps1
```

The package script builds both executables, the sample DLL, the compatibility bridge and notices, then produces a versioned ZIP and SHA-256 sidecar in `target/`.

References: [official OTD plugin package](https://www.nuget.org/packages/OpenTabletDriver.Plugin/0.6.7), [OTD setting/dependency initialization](https://github.com/OpenTabletDriver/OpenTabletDriver/blob/fdeaa7b0c6d6f5260f511f19fb693ed33524af4e/OpenTabletDriver.Desktop/Reflection/PluginSettingStore.cs), [Microsoft native .NET hosting](https://learn.microsoft.com/en-us/dotnet/core/tutorials/netcore-hosting), and [original RadialFollow plugin](https://github.com/AbstractQbit/AbstractOTDPlugins/tree/0.3.0/RadialFollow).
