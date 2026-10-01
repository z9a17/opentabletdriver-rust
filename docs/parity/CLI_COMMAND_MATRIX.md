# Command-line command matrix (S04)

This maps every command of the upstream console client to the Rust command that covers it, or to the open task that owns the gap. It is the command inventory that S04 acceptance asks for. It is not evidence that a mapped command behaves identically; the notes column records known differences.

Upstream source: [OpenTabletDriver.Console/Program.cs](https://github.com/OpenTabletDriver/OpenTabletDriver/blob/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.Console/Program.cs) and [Program.Commands.cs](https://github.com/OpenTabletDriver/OpenTabletDriver/blob/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.Console/Program.Commands.cs) at the pinned revision `736003e`. Upstream names are the lowercased method names unless the source gives an explicit name ([CommandTools.cs](https://github.com/OpenTabletDriver/OpenTabletDriver/blob/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.Console/CommandTools.cs)). Upstream commands talk to a running daemon; most Rust equivalents below work on profile files offline instead.

Status values:

- **Covered:** a Rust command performs the same task, with the differences listed.
- **Partial:** a Rust command covers part of the task.
- **Open:** no Rust command yet; the task ID owns the gap.

Rust commands are subcommands of `opentabletdriver-rust.exe`.

## Settings files and presets

| Upstream | Rust | Status | Notes |
| --- | --- | --- | --- |
| `load FILE` | `restart --config FILE` / `restart --otd-settings FILE` | Partial | Validates and replaces the daemon configuration through guarded restart. Upstream applies settings without restarting the tablet session. C03/S02. |
| `save FILE` | `configuration` | Partial | Prints the daemon's active configuration; the user redirects it to a file. No direct save-to-path. S02. |
| `save-defaults` | — | Open | The GUI Save writes the default profile; no CLI command writes the daemon's active settings to it. S04. |
| `preset NAME` | — | Open | `presets` has no apply command; runtime preset activation is not implemented. C03/C04. |
| `savepreset NAME` | `presets save NAME --config FILE` | Partial | Saves a profile file, not the daemon's active settings. [Named presets](../NAMED_PRESETS.md). |

## Actions

| Upstream | Rust | Status | Notes |
| --- | --- | --- | --- |
| `detect` | `restart`, `list`, `tablets` | Partial | `restart` re-runs detection in the daemon worker; `list` enumerates matching HID collections; `tablets` reports configuration-database matches. Live support is limited to the PTH-660. D02/D05. |
| `installplugin PATH` | — | Open | Plugins are referenced by path in the profile; no package install. P08. |
| `uninstallplugin FOLDER` | — | Open | P08. |

## Debugging

| Upstream | Rust | Status | Notes |
| --- | --- | --- | --- |
| `getstring VID PID INDEX` | — | Open | Indexed HID strings are read during matching but not exposed. S05. |

## Setters

Upstream setters change one tablet profile in the running daemon. `profiles set` changes a profile file and writes a new file with the next settings revision; applying it is a separate `restart --config`.

| Upstream | Rust | Status | Notes |
| --- | --- | --- | --- |
| `setoutputmode TABLET PATH` | — | Open | Output mode is chosen by editing the profile or in the GUI. S04. |
| `enabletabletfilters TABLET FILTERS...` | — | Open | GUI only. S04/U04. |
| `disabletabletfilters TABLET FILTERS...` | — | Open | GUI only. S04/U04. |
| `resettabletfilters TABLET FILTERS...` | — | Open | GUI only (reset to defaults). S04/U04. |
| `enabletools TOOLS...` | — | Open | Tools are not hosted. P06. |
| `disabletools TOOLS...` | — | Open | P06. |
| `setdisplayarea TABLET W H X Y` | — | Open | GUI area editor only. S04/U02. |
| `maptodisplayindex TABLET INDEX` | — | Open | The profile `monitor` key covers the simple mapping; no CLI setter. `displays` lists indexes. S04. |
| `settabletarea TABLET W H X Y [ROTATION]` | `area full`/`area fit` (preview) | Partial | Area commands compute areas offline; they do not write profiles. [Area conversions](../AREA_CONVERSIONS.md). C05/S04. |
| `setsensitivity TABLET X Y [ROTATION]` | `profiles set FILE --output NEW --sensitivity X,Y [--relative-rotation DEG]` | Covered | Offline, relative profiles only; values pass the same validation as a loaded profile. No tablet argument; the file is one profile. |
| `settipbinding TABLET NAME THRESHOLD` | — | Open | B02. |
| `setpenbinding TABLET NAME INDEX` | `profiles set FILE --output NEW --pen-button NUMBER=ACTION` | Partial | Offline, absolute or relative profiles; button numbers start at 1. Mouse, barrel, key/chord and none actions; unsupported bindings remain open under B02/B04. [Editing and export](../PEN_BUTTONS.md#offline-command-line-editing). |
| `setauxbinding TABLET NAME INDEX` | — | Open | B02. |
| `setresettime TABLET MS` | `profiles set FILE --output NEW --reset-time MS` | Covered | Offline, relative profiles only. Whole milliseconds, as upstream. |
| `setenableclipping TABLET BOOL` | — | Open | S04. |
| `setenablearealimiting TABLET BOOL` | — | Open | S04. |
| `setlockaspectratio TABLET BOOL` | — | Open | GUI only. S04/U02. |

## Getters

Upstream getters print text for the running daemon's settings. `profiles get` prints JSON for a profile file, a profile collection (default: its selected profile) or an OTD settings file (`--profile INDEX` required).

| Upstream | Rust | Status | Notes |
| --- | --- | --- | --- |
| `log` | — | Open | The daemon has no log retrieval. S05. |
| `getallsettings` | `profiles get INPUT` / `configuration` | Covered | `--section all` is the default. The archived OTD document and preserved unknown fields are summarized, not printed. |
| `getoutputmode TABLET` | `profiles get INPUT --section output` | Covered | Rust modes are `absolute` and `relative`; `output` is `mouse` or `pen` (Windows Ink/Artist Mode, absolute only). Managed output modes are not hosted (P06). |
| `getareas TABLET` | `profiles get INPUT --section areas` | Covered | Prints `monitor`/`rotation`/`crop` for simple profiles and `absolute` for OTD mappings. |
| `getsensitivity TABLET` | `profiles get INPUT --section sensitivity` | Covered | |
| `getbindings TABLET` | `profiles get INPUT --section bindings` | Partial | Contact policy and pen side buttons, including defaults. Auxiliary, mouse, wheel and plugin bindings remain open under B02-B04. |
| `getmiscsettings TABLET` | `profiles get INPUT --section misc` | Partial | Device path, schema version and settings revision. Clipping/limiting live in `absolute`. |
| `getfilters TABLET` | `profiles get INPUT --section filters` | Covered | Radial Follow entries and plugin references, including disabled ones. |
| `gettools` | — | Open | P06. |
| `listplugins` | — | Open | No plugin install directory. P08. |

## Updates

| Upstream | Rust | Status | Notes |
| --- | --- | --- | --- |
| `hasupdate` | — | Open | Must delegate to R02. |
| `installupdate` | — | Open | Must delegate to R02. |

## Lists

| Upstream | Rust | Status | Notes |
| --- | --- | --- | --- |
| `listoutputmodes` | — | Open | P06. |
| `listfilters` | `inspect-plugin DLL` | Partial | Inspects one given managed DLL; no discovery of installed filters. P08. |
| `listtools` | — | Open | P06. |
| `listbindings` | — | Open | B02. |
| `listpresets` | `presets list` | Covered | |
| `listdisplays` | `displays` | Covered | |

## Scripting

| Upstream | Rust | Status | Notes |
| --- | --- | --- | --- |
| `getdiagnostics` | `diagnostics --output FILE` | Partial | Writes a redacted JSON bundle instead of printing. [Diagnostics](../DIAGNOSTICS.md). S05. |
| `stdio` | — | Open | S04. |
| `edit` | — | Open | Upstream opens `$EDITOR` on a temporary copy and applies it when the hash changes. S04. |

## Rust-only commands

`run`, `daemon`, `start`, `status`, `stop`, `shutdown`, `ui`, `settings`, `capture`, `check-plugins`, `profiles list|preview|import|export|select|recover|paths`, `presets show|export`, `area convert`, `decode` and `version` have no upstream console counterpart. See `opentabletdriver-rust.exe help`.

## Summary

47 upstream commands: 9 covered, 10 partial, 28 open.
