# Tablet debugger crash and theme handoff

The captured v0.14.1 crash comes from `DrawTextW` reading the dangling pointer of an empty UTF-16 vector in `Canvas::wrapped_height`. The debugger measures the raw packet text even before any packet exists. `spaced_hex("")` returns empty text; the old wrapping helper passed an empty `Vec<u16>` to Windows without the empty guard already present in `Canvas::text`.

## Captured evidence

- Windows Application event 1000 at 2026-09-30 02:00:40 identifies `opentabletdriver-rust-ui.exe`, PID 5348, from the Downloads v0.14.1 folder. The fault module is `user32.dll` version 10.0.26100.9444, offset `0x24a82`, exception `0xc0000005`. Report ID is `ae8a50be-d438-4ff3-9833-350f203698d2`.
- The matching dump is `%LOCALAPPDATA%/CrashDumps/opentabletdriver-rust-ui.exe.5348.dmp`, 2,180,129 bytes. SHA-256 is `07AA4E75F8DCE06760C063D70AFCE465AEA56DF59817EE822B528CBFE342FA1D`.
- The dump exception stream records a read from address `0x2`, register `RDI=0x2`. Its memory stream contains the fault instruction bytes `66 44 39 1f`, a word comparison through RDI. An empty `Vec<u16>` has aligned dangling address `0x2`.
- The stack contains return address `opentabletdriver-rust-ui.exe+0x127d8f`. Reading the shipped executable proves the preceding indirect call targets import-table RVA `0x30f9b8`, named `user32!DrawTextW`. That call sets flags `0x0c10`, exactly `DT_CALCRECT | DT_WORDBREAK | DT_NOPREFIX` used by `src/ui/canvas.rs::wrapped_height`, called from `src/ui/debugger.rs::Debugger::draw`.
- No `crash.log` exists in the local OpenTabletDriverRust data directory. The bounded event search found the panel fault, with no separate daemon fault. This evidence identifies the crashed process as the panel; it does not prove the daemon's state afterwards.

The inspection script is `read_dump.py`, and its output is `minidump-evidence.txt` in this folder. `python read_dump.py` ran successfully. It only reads the existing dump, shipped executable and system DLL. These artifacts may contain process memory; do not commit or distribute the dump.

## Changes ready for integration

Only `src/ui/canvas.rs` and `src/ui/debugger.rs` were edited by this agent.

- `Canvas::wrapped_height` returns zero for empty text before calling Win32. Both `DrawTextW` call sites use allocated NUL-terminated UTF-16 storage and pass the character count without the terminator.
- The debugger owns its `FontSet`, DPI and palette. Paint no longer holds borrowed main-window font handles, and moving the debugger between monitors recreates its fonts and adjusts its bounds.
- `debugger::refresh_theme()` copies the main palette, updates the title bar and invalidates the client area. Root already integrated the hook in `App::apply_theme`.
- The debugger window is owned by the main panel, as pinned upstream `TabletDebugger : DesktopForm` calls `base(Application.Instance.MainForm)`. `debugger::close()` removes its state before `DestroyWindow`, and `Drop` always cancels polling. Root already integrated close in main `WM_DESTROY`.
- The poller stops after cancellation or receiver disconnection. An IPC error clears the old report and visualizer instead of showing stale data beside an error. Tablet/session changes reset the cached specification.

No changes were needed in `src/decode_cli.rs` or `crates/otd-core/src/debug.rs`. The capture and decoding boundary remains as before.

## Sources and limits

Upstream debugger source was fetched at pinned commit `736003ed72c8bbb28033b039d5a0bb76c344145c` and saved as `upstream-TabletDebugger.cs` in this folder. Its source URL is https://github.com/OpenTabletDriver/OpenTabletDriver/blob/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.UX/Windows/Tablet/TabletDebugger.cs. Microsoft documents the `DrawTextW` input pointer, explicit character count and formatting flags at https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-drawtextw. The address-0x2 failure itself comes from the user's captured dump, not an API-document inference.

Source and final diffs were reviewed. No format, Clippy, test or build checks ran, as required by the repository owner and parent task. No UI, driver, daemon, HID device, input or plugin was launched or stopped. Configuration artifacts were listed read-only, and no active settings changed. The new debugger has not been opened live, so crash recovery, theme changes, monitor DPI changes and shutdown remain manual release checks.

Root owns commit, release compilation, push and publication. This agent did not commit or publish anything.
