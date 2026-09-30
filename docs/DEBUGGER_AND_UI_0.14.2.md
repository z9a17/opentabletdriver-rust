# Debugger crash and panel cleanup, 0.14.2

## Captured crash

The reported debugger crash was an access violation in `opentabletdriver-rust-ui.exe` 0.14.1. The existing Windows Application event and matching minidump identify `user32.dll`, exception `0xc0000005`, reading address `0x2`. The captured return address in the shipped UI binary follows a call to `DrawTextW` with `DT_CALCRECT | DT_WORDBREAK | DT_NOPREFIX`, the flags used by `canvas::wrapped_height`.

Opening the debugger can receive an empty raw-packet snapshot before the first report. Its empty UTF-16 vector has a dangling pointer of `0x2`. The height measurement passed that pointer to Windows with a zero character count. Windows dereferenced it. This was a native UI fault, not evidence of a daemon or .NET plugin crash.

Height measurement now returns zero for empty text before calling Windows. Both `DrawTextW` helpers also provide allocated, NUL-terminated UTF-16 storage. The debugger keeps its own fonts and DPI state, follows the main palette, clears stale tablet data after connection errors and stops polling when closed. Closing the main panel closes its debugger too. Its report rate, raw/decoded values, tablet outline and pressure display follow the existing OTD-style layout.

Sanitized evidence: event report `ae8a50be-d438-4ff3-9833-350f203698d2`; matching dump SHA-256 `07aa4e75f8dce06760c063d70afce465aea56df59817ee822b528cbfe342fa1d`. The dump remains local and is not included in source or release assets.

## Panel behavior

- Plugin manager rows, column header, details, buttons, borders and status use the main light/dark/high-contrast palette. It handles its own DPI and fonts. Window closing and update guards also cover modal install dialogs.
- Per-property **Use default** buttons and the **Edit JSON** / raw JSON editor are removed. Existing settings remain in the profile. Supported scalar fields and choices remain editable; unsupported structured/custom/decimal values remain preserved and read-only. The selected-filter **Defaults** action remains available.
- A matching daemon configuration response after **Save** or **Apply** skips rebuilding the editor. Changed clean snapshots retain the matching selected filter, property page and list position where possible. Explicit file loads and Reset keep their normal reset behavior.
- The bottom driver button and menu/tray **Stop driver** action are removed. **Start driver** remains in the Tablets/tray menus for recovery or disabled autostart.
- Closing the panel or selecting tray **Close** queues a generation-guarded worker stop, waits for the worker to finish on the IPC thread and then closes. It does not block the window thread. Pending Start/Apply commands finish before that stop is sent. A timeout or unconfirmed stop leaves the panel open with an error. Failed-but-joined worker cleanup is logged separately from successful cleanup. Minimizing continues to hide the panel in the tray and keeps input running. The idle daemon service remains available; CLI `shutdown` exits it.
- An approved updater restart uses its existing shutdown handoff and bypasses normal close-stop, so the old panel cannot stop the replacement it just launched.

The panel close policy intentionally replaces the earlier detach-on-close behavior at the owner's request. CLI clients and foreground operation remain independent.

## Why native and .NET can feel different

The read-only settings comparison found different saved configurations. Rust's active native Radial Follow entry uses the defaults; original OTD enables custom tablet-space values. Both managed entries in the saved Rust profile are disabled, although the managed tablet entry retains the custom values.

| Radial Follow property | Saved active Rust native | Saved active original OTD |
| --- | ---: | ---: |
| Outer radius, mm | 1.0 | 0.7039 |
| Inner radius, mm | 0.0 | 0.302 |
| Initial smoothing coefficient | 0.95 | 0.302 |
| Soft knee scale | 1.0 | 0.603 |
| Smoothing leak coefficient | 0.0 | 0.201 |

The tablet area's Y center is also different, `25.3125` versus `23.90625` mm. Display X center is `1280` versus `1285.6917` pixels. Rust enables clipping and the original saved OTD profile disables it. These files establish persisted settings, not unsaved edits or a live pipeline snapshot.

Source comparison against [RadialFollow 0.3.0](https://github.com/AbstractQbit/AbstractOTDPlugins/tree/0.3.0/RadialFollow) found matching defaults, clamps, curve structure, tablet millimetre conversion and the 50 ms reset rule. Tablet-space and screen-space variants are different: the latter runs after mapping in pixels and defaults to a 5-pixel outer radius. Matching numeric radii across the two variants does not match smoothing distance.

**File > Import OpenTabletDriver settings** already loads the original enabled Radial Follow parameters and supported mapping into unsaved editor contents. Save/Apply then uses those settings. The original settings file is unchanged by import. For a smoothing-only comparison, the native filter can use the five custom values above while retaining the chosen mapping.

Existing mapping precision, clipping endpoint and multi-filter ordering differences remain documented in the behavior contracts. This release does not change the core smoothing or output mathematics. Matching settings is necessary before attributing a feel difference to runtime latency.

## Evidence and limits

The crash diagnosis uses captured event, minidump and shipped-binary evidence. The fixes and configuration paths received source review by GPT-6.1 Sol agents at High, as requested. No other model family was used in this follow-up.

The owner disabled the pre-publish format, Clippy, test and build-check suite. Those checks were not run. Release compilation is only for producing the Windows executables and managed bridge. Package contents and SHA-256 are checked against the produced files before publication. No panel, daemon, tablet input or third-party plugin was launched during this work. The active driver and saved user settings were not changed.

The fixed debugger, manager visuals, Save selection, close lifecycle and physical native/.NET feel still require manual testing on the release. No measured latency improvement or full OTD parity is claimed.
