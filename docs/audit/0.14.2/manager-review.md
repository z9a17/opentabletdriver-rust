# Plugin manager independent source review

Reviewed `src/ui/plugin_manager.rs` and its main-window theme/shutdown hooks read-only. No tests, builds, UI, daemon, HID, plugin or settings actions ran during this review. No application source was edited.

## Finding fixed after review

The initial P2 finding was that closing the main panel while the manager's install dialog is open could skip manager shutdown and leave a pending action able to resume after the close request.

`with_manager` at line 231 holds `MANAGER.try_borrow_mut()` throughout its callback. `Manager::command` at line 539 enters `MessageBoxW` for Install inside that callback, and `install_from_file` at line 602 enters a file dialog and confirmation inside the same borrow. Those calls run modal message loops. The manager was created with a null owner at line 116, so the main panel remains independently accessible.

When the main panel receives a close during that loop, `App::begin_close` calls `plugin_manager::set_restart_pending(true)`. The helper at line 691 attempts another mutable borrow through `with_manager`; that fails and the manager is not disabled. After daemon cleanup, main `WM_DESTROY` calls `plugin_manager::close`. The close helper at line 695 attempts an immutable borrow, which also fails while the dialog owns the mutable borrow, so it does not call `DestroyWindow`. The source then allows an accepted Install dialog to call `Manager::start` without checking whether the main panel is closing. This is a reachable reentrancy path from the source, not a reproduced runtime failure.

Suggested correction for the integrator: make the manager an owned main-panel window and keep its native window handle available independently of the mutable Manager borrow. Disable/close should use that independent handle. After a modal dialog returns, refuse to start work if its native window was destroyed or the panel is closing. Releasing the Manager borrow before showing dialogs is another valid fix, but requires moving dialog results back into the command flow.

Root received this finding before any edit. The manager agent then implemented the correction, and this reviewer read the final source. The manager now has the main HWND as owner. Separate HWND, active, close-pending and restart-pending Cells let shutdown and disablement run while the Manager borrow is held. `close()` disables the native window immediately and defers destruction until the outer callback releases its borrow. Native owner-driven destruction clears the HWND independently, and post-dialog/command/start guards reject actions once either window is gone or the panel is closing. Restart disablement is reapplied after a dialog returns, because a native dialog may re-enable its owner.

The manager agent also added a theme-pending Cell. A theme change that arrives during a modal borrow now retries after that borrow ends. The final resource-order correction keeps Manager-owned fonts until parent `WM_NCDESTROY`, after native child controls have been destroyed. Parent `WM_DESTROY` only clears the independent HWND and marks close pending. Microsoft's ordering documentation supports this distinction at https://learn.microsoft.com/en-us/windows/win32/winmsg/wm-ncdestroy.

The final source addresses the reported modal path. No further actionable crash or drawing defect was found in the reviewed changes. This is source review, without a runtime reproduction, compiled verification or visual confirmation. Ownership of the application source remains with the integrator/manager agent.

## Areas reviewed without a new defect

- `draw_catalog_row` at line 744 uses a 1,024-unit UTF-16 buffer, passes its exact capacity to `LVM_GETITEMTEXTW`, clamps the returned length to 1,023 and decodes only that bounded slice. Empty cells use the guarded `Canvas::text` path.
- `draw_catalog_header` at line 797 uses a 256-unit buffer, passes that capacity to `HDM_GETITEMW`, and finds the terminator within the same array. A failed retrieval leaves the zero-initialized buffer empty. No invalid raw UTF-16 pointer is handed to DrawText.
- Header notifications are intercepted by the list subclass, whose callback signature matches its existing windows-sys imports. `WM_NCDESTROY` removes the subclass before calling `DefSubclassProc`.
- `set_dpi` at line 333 creates new fonts, updates every manager control and the header, and only then replaces `_fonts`. Custom row/header/button drawing retrieves the native control font with `WM_GETFONT`, so synchronous child repaint during this sequence uses a live font.
- Custom drawing reads LOOK through `with_look`, which uses `try_borrow`, and does not take a Manager borrow. The brush cache's mutable borrow is confined to lookup or `CreateSolidBrush`; the returned brush reaches `FillRect` after that borrow ends. Main theme changes remove cached brushes before requesting redraw.
- `App::apply_theme` updates LOOK before the manager/debugger refresh hooks. The manager's theme method copies the palette before calling native theme functions. Nested `WM_THEMECHANGED` cannot recursively obtain the current Manager mutable borrow.
- Main `WM_DESTROY` closes the manager/debugger before the message loop drops APP and LOOK. The modal-dialog finding above is the case where the manager close helper cannot obtain its handle.

## Decision trail audit

At review time `decisions.tsv` contained its header and one initial scope row at `2026-09-30T00:16:02Z`. The row says source review only and release compilation planned. That does not claim tests or a successful runtime fix and matches the work scope I observed. I did not independently verify the row's issue #9 or screenshot references.

The trail is incomplete as a final artifact. Before handoff/publication it should append the captured crash evidence, empty DrawText boundary fix, debugger theme/font ownership changes, root shutdown/filter decisions, manager source review and any release compilation/publication results. Captured dump inspection is evidence analysis, not a live reproduction or regression test. Release compilation should be described as asset production, without implying a format/Clippy/test/build check suite ran. Runtime and visual behavior remain unverified.
