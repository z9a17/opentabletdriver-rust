# Diagnostic bundles

Create a JSON bundle without opening HID devices or loading plugin DLLs:

```text
opentabletdriver-rust.exe diagnostics --output diagnostics.json
opentabletdriver-rust.exe diagnostics --output profile-diagnostics.json --config driver.toml
```

The output must be a new file. A bundle records the driver version, architecture, pinned upstream revision, backend scope, display rectangles and a bounded status snapshot from an already-running control daemon. It does not start a service or change the running profile. An absent daemon is recorded in the bundle rather than treated as a fatal export error.

With `--config`, the bundle adds supported mapping/contact/filter-number settings, plugin kinds and enabled flags, and preservation/diagnostic counts. It does not execute that profile.

By default the bundle excludes filesystem and device paths, plugin class names and property values, archived OTD settings, environment variables, raw tablet reports and free-form daemon/diagnostic text. `--include-private-details` explicitly includes profile paths, plugin identities and free-form status/diagnostic messages; review those bundles before sharing. Plugin property values and the imported source archive are omitted even in that mode.

This is an explicit snapshot export. Live raw-report subscriptions, report statistics and the graphical tablet debugger remain separate S05 work. No command, driver, UI or hardware execution was performed while implementing this increment.
