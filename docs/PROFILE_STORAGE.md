# Profile storage and recovery

The panel, foreground commands and daemon use the saved Rust `driver.toml` in the selected data directory. If it does not exist, normal mode can import the existing OpenTabletDriver settings. An unreadable or invalid saved Rust profile is an error; it is not silently replaced by a different active mapping.

`profiles paths` prints the selected directory and file locations without creating them. Defaults are `%LOCALAPPDATA%\OpenTabletDriverRust` on Windows, `$XDG_CONFIG_HOME/opentabletdriver-rust` (or `~/.config/opentabletdriver-rust`) on Linux, and `~/Library/Application Support/OpenTabletDriverRust` on macOS. These portable storage paths do not imply runtime driver support on every platform.

Set `OTD_RUST_PORTABLE_DIR` to an absolute directory to use a portable tree. Automatic import from global OpenTabletDriver settings is disabled in that mode; explicit `profiles import` or `--otd-settings` remains available. Relative plugin references within the destination or explicitly selected portable tree are retained when saving. Plugins outside that tree keep absolute references. Portable storage does not create another independently running daemon for the same user.

## Saving

The editor remembers the exact bytes it loaded. Save compares them with the destination, stages and flushes the replacement beside it, retains the previous bytes as `driver.toml.bak`, and publishes the new file. If another editor changed the destination, Save fails with a reload/Save As choice instead of overwriting that change. Save As and offline export require a new filename.

Writers using this implementation coordinate through a `.lock` sidecar. Applications that ignore it can still race the final comparison and rename; this is optimistic conflict detection, not an operating-system compare-and-swap. A crash may leave a stale lock. Remove it only after confirming that no save is running. Staged files are cleaned up only if this writer created them.

The `.bak` name is reserved for the previous primary content. It may already have been replaced when a later conflict or publication failure is reported. Do not keep unrelated files under that name. A Unix directory-sync error can be reported after the new primary has been published; inspect or reload the file before retrying.

Windows publication uses a same-directory write-through rename. Basic filesystem permissions are copied to the staged primary file; existing Windows DACLs, owner metadata and alternate streams are not fully preserved by this implementation. Unix new-file publication requires hard-link support and synchronizes the directory. Read-only destinations and failed backup creation are reported as errors.

## Recovery

In the panel, **File > Recover backup as unsaved settings** loads the current profile's backup into the editor. A failed Load also offers recovery when a backup exists. Recovery leaves the original and backup files intact, resolves plugin references against the original profile directory, and requires **Save As** to a new filename. It does not apply settings to the daemon.

Recover the backup into a new, validated file:

```text
opentabletdriver-rust.exe profiles recover driver.toml --output recovered.toml
```

This reads `driver.toml.bak`, validates the native profile or collection, and writes the recovered copy without overwriting either source. Open the recovered file in the panel to inspect it before applying it.

Relocation operates on resolved plugin paths from loaded profiles. API callers constructing profiles with relative references must first resolve them against their source directory. Existing symlink/junction aliases may be canonicalized to their current targets when saved.

Saving and runtime activation remain separate guarantees. The daemon now prepares a replacement before quiescing the old worker and can resume retained instances after a pre-commit activation failure; see [runtime reconfiguration](RUNTIME_RECONFIGURATION.md). File saves are not rolled back when runtime activation fails. GUI preset selection, broader C03/C04 behavior and physical validation remain open.

The [named preset store](NAMED_PRESETS.md) uses these same snapshot, backup and relocation services under `presets/`. Its offline list/show/save/export commands preserve explicit names and require an existing loaded snapshot for replacement. They do not activate presets or change the running driver.
