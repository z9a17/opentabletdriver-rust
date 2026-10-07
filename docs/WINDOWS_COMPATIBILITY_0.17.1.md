# Windows compatibility workflows, 0.17.1

This release extends the [0.17.0 device and editor workflows](WINDOWS_COMPATIBILITY_0.17.0.md).
The unchanged compatibility baseline is OpenTabletDriver 0.6.7,
revision `736003ed72c8bbb28033b039d5a0bb76c344145c`.

## Toggle and native preset bindings

Select a pen, mouse, express-key, wheel or scroll action in the Windows binding
editor. The toggle option retains a native key, mouse-button or barrel-button
hold until the next press. Repeated reports while held do not flip it again.
Stop, loss of range, endpoint removal and profile replacement release its output.
Toggle supports held native actions; scroll, managed actions, nested toggles
and presets cannot be wrapped.

The preset selector lists saved native presets. A native preset replaces only
the physical session that triggered it, using the existing guarded replacement
and cleanup transaction. The source button must physically release before the
replacement can trigger that held slot again. Other inputs remain available.
Profile reads and replacement run outside report processing. Native preset
switching requires the daemon; foreground-only sessions report it unavailable.

CLI action values use `toggle:keys:Control+Z`, `toggle:mouse:left`,
`toggle:barrel:1` or `preset:NAME`. These native extensions round-trip through
TOML. Original OTD JSON export rejects them explicitly because the pinned
original stores have no equivalent native toggle/per-device-preset format.
See [the binding implementation](parity/B04_BINDINGS_0.17.1.md).

## Original managed plugins and presets

Windows downloads include the actual original Desktop assembly, Core, Native,
Plugin and Configurations assemblies plus their pinned transitive dependencies,
resource satellites and licenses. The compatibility host supplies original
driver/configuration/parser/device metadata interfaces, the original plugin
manager and asynchronous `IDriverDaemon` operations backed by the native owner.
Successful task completion follows the real operation; queue admission alone
does not report settings as applied.

Unchanged `OpenTabletDriver.Desktop.Binding.PresetBinding` is discoverable without
installing a second Desktop DLL. Its original preset manager reads whole OTD
Settings collections from `.json` files in the preset directory reported by
`GetApplicationInfo`. Native presets there use `.toml`. The original preset
changes the collection across applicable tablets; it is never converted into
the single-session native preset action. A source handoff waits for the actual
button release or zero raw pressure for a tip/eraser binding.

Native-only profiles leave CLR uninitialized. Managed service polling and
device enumeration run on background/setup owners, outside native report
processing. Synchronous service waits are rejected from managed report,
timer, parser and setup callbacks. Queued settings mutations retain native
identity guards and cannot silently target a replacement daemon.

Concrete Core.Driver/RootHub/InputDevice readers, custom managed hub attachment,
shared endpoint streams/read/write/feature I/O and the managed daemon DeviceReport
event remain unavailable. Metadata providers do not establish those capabilities.
Some original diagnostic/update operations also require unavailable host behavior.
See [the provider contracts and gaps](parity/P07_SERVICES_0.17.1.md).

## Idle original settings

Original-format Get/Set/Reset works while no tablet is active. Disconnected rows
and unknown JSON fields remain in the collection. Set changes memory; explicit
Load/Save uses `upstream-rpc-settings.json` under the native data directory,
advertised as `GetApplicationInfo.SettingsFile`. Save checks the previously
observed file, backs up replacement and reports conflicts. Native per-physical
saved profiles and explicit native Apply take precedence over retained model rows.

Settings publication rejects stale collection revisions, daemon generations,
connected devices or preparation races. Active collections still use guarded
per-device application and rollback; this is not an atomic all-tablet transaction.
Enabled global tools without an active primary owner and settings with no exact
original representation retain explicit errors.
See [the collection contract](parity/S03_IDLE_SETTINGS_0.17.1.md).

## Evidence boundary

The owner deferred behavioral validation. No CI, tests, format/Clippy suites,
GUI, plugin, daemon, driver, hardware, game or OBS sessions were run for this
release. Actual compilation and archive/source/hash verification produce the
four platform downloads and `E:/OTD RUST TEST`; they do not prove physical input,
plugin runtime compatibility or full parity. Native Linux/macOS work is deferred.
The [acceptance backlog](parity/WORK_ITEMS.md) and
[evidence ledger](parity/EVIDENCE_LEDGER.md) retain their unverified criteria.
