# Command-line command matrix (S04)

This maps every command of the upstream console client to the Rust command that covers it, or to the open task that owns the gap. It is the command inventory that S04 acceptance asks for. It is not evidence that a mapped command behaves identically; the notes column records known differences.

Upstream source: [OpenTabletDriver.Console/Program.cs](https://github.com/OpenTabletDriver/OpenTabletDriver/blob/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.Console/Program.cs) and [Program.Commands.cs](https://github.com/OpenTabletDriver/OpenTabletDriver/blob/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.Console/Program.Commands.cs) at the pinned revision `736003e`. Upstream names are the lowercased method names unless the source gives an explicit name ([CommandTools.cs](https://github.com/OpenTabletDriver/OpenTabletDriver/blob/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.Console/CommandTools.cs)). Upstream commands talk to a running daemon; most Rust equivalents below work on profile files offline instead.

Mappings describe implementation, not executed evidence. Focused regression cases were added; the owner-disabled pre-publication format/Clippy/test/build-check suite was not run. Original-client, native Unix CLI and hardware evidence remain separate.

Status values:

- **Covered:** a Rust command performs the same task, with the differences listed.
- **Partial:** a Rust command covers part of the task.
- **Open:** no Rust command yet; the task ID owns the gap.

Rust commands are subcommands of `opentabletdriver-rust.exe`.

## Settings files and presets

| Upstream | Rust | Status | Notes |
| --- | --- | --- | --- |
| `load FILE` | `restart --config FILE` / `restart --otd-settings FILE` | Partial | Validates and replaces the daemon configuration through guarded restart. Upstream applies settings without restarting the tablet session. C03/S02. |
| `save FILE` | `save FILE [--replace]` | Covered | Coherent active native TOML snapshot, atomic save, explicit replacement with snapshot conflict detection and .bak; paths relocate. |
| `save-defaults` | `save-defaults` | Covered | Saves active native settings to the selected data directory driver.toml with conflict detection and backup. |
| `preset NAME` | `preset NAME / presets apply NAME` | Partial | Validates preset then requests guarded restart. Reply is acceptance; query status for completion. Differs from upstream in-place SetSettings. Binding-triggered preset selection remains B04. |
| `savepreset NAME` | `savepreset NAME / presets save-active NAME [--replace]` | Covered | Durable active-snapshot save; existing presets require explicit replacement. |

## Actions

| Upstream | Rust | Status | Notes |
| --- | --- | --- | --- |
| `detect` | `detect / restart; list; tablets` | Partial | Restart re-runs worker detection; enumeration/database coverage does not prove physical device behavior. |
| `installplugin PATH` | `plugins install-file PATH / plugins install NAME` | Partial | Existing Rust package validation/recovery; installation does not prove unchanged plugin execution. P08. |
| `uninstallplugin FOLDER` | `plugins remove NAME` | Partial | Existing Rust package removal by installed identity, not arbitrary upstream folder. P08. |

## Debugging

| Upstream | Rust | Status | Notes |
| --- | --- | --- | --- |
| `getstring VID PID INDEX` | `getstring VID PID INDEX / device-strings VID PID INDEX` | Covered | Existing indexed HID string query. Requires device presence and permissions. |

## Setters

Upstream setters change one tablet profile in the running daemon. `profiles set` changes a profile file and writes a new file with the next settings revision; applying it is a separate `restart --config`.

| Upstream | Rust | Status | Notes |
| --- | --- | --- | --- |
| `setoutputmode TABLET PATH` | `profiles set FILE --output NEW --output-mode absolute|relative|pen` | Partial | Offline native modes. Relative defaults match pinned source; arbitrary managed output modes remain P06. |
| `enabletabletfilters TABLET FILTERS...` | `profiles set FILE --output NEW --plugin-enabled NUMBER=true / --radial-follow enable` | Partial | Existing one-based entries preserve order; native filter restores retained values. Adding stores by type path remains open. |
| `disabletabletfilters TABLET FILTERS...` | `profiles set FILE --output NEW --plugin-enabled NUMBER=false / --radial-follow disable` | Partial | Disables existing entries offline. One native filter retains values; multiple native entries are rejected. |
| `resettabletfilters TABLET FILTERS...` | `profiles set FILE --output NEW --radial-follow reset` | Partial | Native defaults reset preserves enabled/disabled state. Arbitrary DLL defaults and upstream remove/append ordering remain open. |
| `enabletools TOOLS...` | `profiles set FILE --output NEW --plugin-enabled NUMBER=true` | Partial | Enables existing dotnet_tool entries offline. Adding stores by type path and per-binary validation remain P06. |
| `disabletools TOOLS...` | `profiles set FILE --output NEW --plugin-enabled NUMBER=false` | Partial | Disables existing tools offline; lifecycle changes require explicit load/restart. |
| `setdisplayarea TABLET W H X Y` | `profiles set FILE --output NEW --display-area W,H,X,Y` | Covered | Offline centered desktop pixels. Simple profiles require both display and tablet areas together. |
| `maptodisplayindex TABLET INDEX` | `profiles set FILE --output NEW --monitor INDEX|all` | Partial | Offline zero-based simple absolute display selection; explicit area geometry remains separate. |
| `settabletarea TABLET W H X Y [ROTATION]` | `profiles set FILE --output NEW --tablet-area W,H,X,Y[,ROTATION]` | Covered | Offline centered millimeters/degrees; preserves existing display area. Simple profiles require both areas. |
| `setsensitivity TABLET X Y [ROTATION]` | `profiles set FILE --output NEW --sensitivity X,Y [--relative-rotation DEG]` | Covered | Offline, relative profiles only; values pass the same validation as a loaded profile. No tablet argument; the file is one profile. |
| `settipbinding TABLET NAME THRESHOLD` | `profiles set FILE --output NEW --tip-enabled BOOL --tip-threshold PERCENT` | Partial | Native contact and exact percentage. Arbitrary/toggle/preset/managed bindings remain B04/P06. |
| `setpenbinding TABLET NAME INDEX` | `profiles set FILE --output NEW --pen-button NUMBER=ACTION` | Partial | Offline, absolute or relative profiles; button numbers start at 1. Mouse, barrel, key/chord and none actions; unsupported bindings remain open under B02/B04. [Editing and export](../PEN_BUTTONS.md#offline-command-line-editing). |
| `setauxbinding TABLET NAME INDEX` | `profiles set FILE --output NEW --aux-button NUMBER=ACTION` | Partial | One-based native mouse/barrel/key/chord/none express-key actions. Managed bindings remain B04/P06. |
| `setresettime TABLET MS` | `profiles set FILE --output NEW --reset-time MS` | Covered | Offline, relative profiles only. Whole milliseconds, as upstream. |
| `setenableclipping TABLET BOOL` | `profiles set FILE --output NEW --clipping BOOL` | Covered | Offline explicit absolute mappings; simple profiles require both areas. |
| `setenablearealimiting TABLET BOOL` | `profiles set FILE --output NEW --limiting BOOL` | Covered | Offline explicit absolute mappings; relative edits rejected. |
| `setlockaspectratio TABLET BOOL` | — | Open | GUI only. S04/U02. |

## Getters

Upstream getters print text for the running daemon's settings. `profiles get` prints JSON for a profile file, a profile collection (default: its selected profile) or an OTD settings file (`--profile INDEX` required).

| Upstream | Rust | Status | Notes |
| --- | --- | --- | --- |
| `log` | `log / status` | Partial | Bounded recent native daemon logs and sequence. Original log/event stream remains S03/S05. |
| `getallsettings` | `getallsettings / profiles get INPUT / configuration` | Covered | Coherent active native profile or offline inspection; original archive is summarized. |
| `getoutputmode TABLET` | `getoutputmode / profiles get INPUT --section output` | Partial | Native active getters take no tablet argument; arbitrary managed output remains P06. |
| `getareas TABLET` | `getareas / profiles get INPUT --section areas` | Covered | Active/offline native profile inspection; active getter takes no tablet argument. |
| `getsensitivity TABLET` | `getsensitivity / profiles get INPUT --section sensitivity` | Covered | Active/offline native profile inspection. |
| `getbindings TABLET` | `getbindings / profiles get INPUT --section bindings` | Partial | Contact/drag/pressure/tilt, pen/aux/mouse and wheels. Scroll/toggle/preset/managed bindings remain open. |
| `getmiscsettings TABLET` | `getmiscsettings / profiles get INPUT --section misc` | Partial | Device path/schema/revision. Clipping/limiting live in absolute areas; lock aspect metadata remains open. |
| `getfilters TABLET` | `getfilters / profiles get INPUT --section filters` | Covered | Active/offline native filters, disabled retained values and plugin entries; inspection does not execute them. |
| `gettools` | `gettools / profiles get INPUT --section tools` | Covered | Active/offline configured dotnet_tool entries filtered from other plugins; no code execution. |
| `listplugins` | `plugins installed` | Partial | Rust installed package identities; complete unchanged-plugin classification remains P08. |

## Updates

| Upstream | Rust | Status | Notes |
| --- | --- | --- | --- |
| `hasupdate` | `hasupdate / update --check` | Covered | Existing validated updater. |
| `installupdate` | `installupdate / update` | Covered | Existing updater and packaging/recovery checks. |

## Lists

| Upstream | Rust | Status | Notes |
| --- | --- | --- | --- |
| `listoutputmodes` | `listoutputmodes` | Partial | Native mode JSON list; managed output remains P06. |
| `listfilters` | `inspect-plugin DLL` | Partial | Inspects one given managed DLL; no discovery of installed filters. P08. |
| `listtools` | `inspect-plugin DLL` | Partial | One explicitly supplied trusted assembly, including tools. Inspection executes code; installed discovery remains open. |
| `listbindings` | `listbindings` | Partial | Native action syntax JSON; managed and preset bindings explicitly not hosted. |
| `listpresets` | `listpresets / presets list` | Covered | Named native presets. |
| `listdisplays` | `listdisplays / displays` | Covered | Existing platform display inspection. |

## Scripting

| Upstream | Rust | Status | Notes |
| --- | --- | --- | --- |
| `getdiagnostics` | `diagnostics --output FILE` | Partial | Writes a redacted JSON bundle instead of printing. [Diagnostics](../DIAGNOSTICS.md). S05. |
| `stdio` | `stdio` | Partial | Bounded native protocol-v2 Request/Response JSON lines, validates before IPC, no implicit daemon launch. Upstream text-console and StreamJsonRpc are different contracts. |
| `edit` | — | Open | Upstream opens `$EDITOR` on a temporary copy and applies it when the hash changes. S04. |

## Rust-only commands

`run`, `daemon`, `start`, `status`, `stop`, `shutdown`, `ui`, `settings`, `capture`, `check-plugins`, `profiles list|preview|import|export|select|recover|paths`, `presets show|export`, `area convert`, `decode` and `version` have no upstream console counterpart. See `opentabletdriver-rust.exe help`.

## Summary

47 upstream commands: 19 covered, 26 partial, 2 open. Mappings do not establish full live-protocol or unchanged-plugin parity.

Additional native setters: `--mouse-button NUMBER=ACTION`, wheel directions/thresholds,
`--eraser-enabled`, `--eraser-threshold`, `--drag-only`, `--disable-pressure` and
`--disable-tilt`. Drag-only applies the pinned threshold remap before gating its
rising edge. Imported float percentages are retained; raw-only native thresholds
use the midpoint percent representation used by OTD export. Existing native output
pressure math remains unchanged. Losing pressure does not release an already held
pen button. Mouse scroll, toggle and binding-triggered preset actions remain open.

Active getters take no tablet argument because the native daemon exposes one
active worker configuration. Preset apply uses guarded restart and reports
acceptance. Stdio accepts one native Request per line, for example:

```json
{"version":2,"id":1,"command":{"method":"status"}}
```

Errors are Response JSON; oversized input stops without dispatch. This endpoint
does not implement the original IDriverDaemon/StreamJsonRpc contract (S03).
