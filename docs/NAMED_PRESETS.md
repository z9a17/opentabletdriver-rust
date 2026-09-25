# Named presets

Named presets are native TOML profiles stored in `presets/` below the selected Rust data directory. The default is `%LOCALAPPDATA%\OpenTabletDriverRust\presets` on Windows; `OTD_RUST_PORTABLE_DIR` selects an explicit portable root. See [profile storage](PROFILE_STORAGE.md) for other platforms and durability limits.

```text
opentabletdriver-rust.exe presets list
opentabletdriver-rust.exe presets save "Gaming Low Latency" --config driver.toml
opentabletdriver-rust.exe presets show "Gaming Low Latency"
opentabletdriver-rust.exe presets save "Gaming Low Latency" --config revised.toml --replace
opentabletdriver-rust.exe presets export "Gaming Low Latency" --output exported.toml
```

All commands operate offline. They do not open a device, load a plugin, connect to the daemon, select an active profile or apply settings. GUI selection and transactional preset activation remain separate work. There is no delete command.

Names preserve spelling and internal spacing and become `<NAME>.toml`. Allowed names contain 1–64 ASCII letters/digits, internal spaces, hyphens, underscores or parentheses. Leading/trailing spaces, dots, path separators and Windows reserved device names are rejected. Names are never trimmed, sanitized or silently renamed. Case-only alternatives are rejected on every platform: use the exact spelling returned by `list`. Existing files that collide by case produce an ambiguity error instead of selecting an arbitrary preset.

`list` reports names, saved revisions and errors for invalid or conflicting presets; unrelated backup/lock files are excluded, and invalid preset filenames produce warnings. A missing preset directory returns an empty list without creating it. `show` returns JSON containing the native profile TOML with resolved plugin paths. It includes full settings and is not a redacted diagnostic artifact.

`save` creates a new preset by default. `--replace` requires an existing valid preset and captures its exact file bytes before source preparation. Replacement rechecks that snapshot under the storage lock, keeps previous bytes in the reserved `<NAME>.toml.bak`, and advances the revision above both source and previous preset revisions. A removed or externally edited target is an error. The source file is unchanged. Invalid existing presets can be recovered with `profiles recover` into a new file before saving under another name.

Source plugin locations resolve against the source profile's absolute directory before serialization. Saving and export preserve unknown native fields and the original imported settings archive, with the same destination/portable-tree relocation rules as ordinary profile saves. `export` requires a new output filename and does not replace the preset. Preserving settings does not make unsupported plugins or tablets executable.

Cooperating preset writers also hold `presets/.presets.lock`, so simultaneous creation cannot introduce case-only aliases on case-sensitive filesystems. A crashed writer can leave that lock; remove it only after confirming no preset save is running. Filesystem changes by tools that ignore these locks can still race publication. Existing `.bak` rotation, metadata/ACL and post-publication flush limitations apply unchanged. Preset primary files must be regular files; symbolic-link presets are rejected.

The core API accepts source-resolved `Profile` values and rejects unresolved relative plugin paths. Replacing a preset requires the immutable `LoadedPreset` returned by that store's load operation; a snapshot from another name/directory cannot replace it.

This slice was source-reviewed and formatted. No local tests, preset CLI runs, plugin execution, GUI or device validation were performed; compilation belongs to integration. C03 runtime transactions and broader C04/UI acceptance remain open.
