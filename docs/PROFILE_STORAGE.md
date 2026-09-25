# Profile storage and recovery

The panel, foreground commands and daemon use the saved Rust `driver.toml` in the selected data directory. If it does not exist, normal mode can import the existing OpenTabletDriver settings. An unreadable or invalid saved Rust profile is an error; it is not silently replaced by a different active mapping.

`profiles paths` prints the selected directory and file locations without creating them. Defaults are `%LOCALAPPDATA%\OpenTabletDriverRust` on Windows, `$XDG_CONFIG_HOME/opentabletdriver-rust` (or `~/.config/opentabletdriver-rust`) on Linux, and `~/Library/Application Support/OpenTabletDriverRust` on macOS. These portable storage paths do not imply runtime driver support on every platform.

Set `OTD_RUST_PORTABLE_DIR` to an absolute directory to use a portable tree. Automatic import from global OpenTabletDriver settings is disabled in that mode; explicit `profiles import` or `--otd-settings` remains available. Relative plugin references within the destination or explicitly selected portable tree are retained when saving. Plugins outside that tree keep absolute references. Portable storage does not create another independently running daemon for the same user.

## Saving

The editor remembers the exact bytes it loaded. Save compares them with the destination, stages and flushes the replacement beside it, retains the previous bytes as `driver.toml.bak`, and publishes the new file. If another editor changed the destination, Save fails with a reload/Save As choice instead of overwriting that change. Save As and offline export require a new filename.

Writers using this implementation coordinate through a `.lock` sidecar. Applications that ignore it can still race the final comparison and rename; this is optimistic conflict detection, not an operating-system compare-and-swap. A crash may leave a stale lock. Remove it only after confirming that no save is running. Staged files are cleaned up only if this writer created them.

Windows publication uses a same-directory write-through rename. Basic filesystem permissions are copied to the staged primary file; existing Windows DACLs, owner metadata and alternate streams are not fully preserved by this implementation. Unix new-file publication requires hard-link support and synchronizes the directory. Read-only destinations and failed backup creation are reported as errors.

## Recovery

Recover the backup into a new, validated file:

```text
opentabletdriver-rust.exe profiles recover driver.toml --output recovered.toml
```

This reads `driver.toml.bak`, validates the native profile or collection, and writes the recovered copy without overwriting either source. Open the recovered file in the panel to inspect it before applying it.

Saving and runtime activation remain separate guarantees. The daemon's current Apply operation validates then stops/restarts a worker; it is not yet a transactional hot swap that retains a prepared pipeline after every plugin or activation failure. Full C03/C04 preset transactions and physical validation remain open.
