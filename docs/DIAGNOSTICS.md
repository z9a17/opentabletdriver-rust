# Diagnostic bundles

Create a JSON bundle without opening HID devices or loading plugin DLLs:

```text
opentabletdriver-rust.exe diagnostics --output diagnostics.json
opentabletdriver-rust.exe diagnostics --output profile-diagnostics.json --config driver.toml
```

The output must be a new file. A bundle records the driver version, architecture, pinned upstream revision, backend scope, display rectangles and a bounded status snapshot from an already-running control daemon. It does not start a service or change the running profile. An absent daemon is recorded in the bundle rather than treated as a fatal export error.

With `--config`, the bundle adds supported mapping/contact/filter-number settings, plugin kinds and enabled flags, and preservation/diagnostic counts. It does not execute that profile.

By default the bundle excludes filesystem and device paths, plugin class names and property values, archived OTD settings, environment variables, raw tablet reports and free-form daemon/diagnostic text. `--include-private-details` explicitly includes profile paths, plugin identities and free-form status/diagnostic messages; review those bundles before sharing. Plugin property values and the imported source archive are omitted even in that mode.

The bundle also lists up to ten recent crash records (time, version, role, kind, thread and source location); their messages are included only with `--include-private-details`, because they can name files.

## Crash records

The daemon and the panel run without a console. When one of them panics, or the daemon, panel or foreground driver stops with a fatal error, it appends one JSON line to `crash.log` in the settings directory (`%LOCALAPPDATA%\OpenTabletDriverRust`, or `OTD_RUST_PORTABLE_DIR`). A file larger than 256 KiB is renamed to `crash.log.old` before the next record. Release builds abort on a panic; the record is written before the abort.

The panel keeps a handle to the daemon process it is attached to. When the daemon disappears with a nonzero exit code or a crash record, the console shows the exit code and the record, for example a panic's thread, source location and message. A crash that is not a Rust panic, such as an access violation inside an in-process plugin, has no record; its exit code (0xC0000005) is still shown, and Windows Event Viewer (Windows Logs > Application) names the faulting module.

This is an explicit snapshot export. Live raw-report subscriptions, report statistics and the graphical tablet debugger remain separate S05 work. No command, driver, UI or hardware execution was performed while implementing this increment.
