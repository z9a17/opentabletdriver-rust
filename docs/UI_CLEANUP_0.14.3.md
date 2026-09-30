# Plugin manager and menu cleanup: 0.14.3

The owner supplied screenshots of a white plugin-catalog header in dark mode, unnecessary filter reorder controls and a long description above the settings. They also reported that clicking an open dropdown's button again did not leave it closed. This release addresses those Windows UI paths. The review baseline is v0.14.2, commit f8a1b75ab90c680e1456e20831b66e819dd26210.

## Plugin manager

The header custom-draw handler painted the whole header during CDDS_PREPAINT and returned CDRF_SKIPDEFAULT. For header controls, Microsoft documents that suppression result at CDDS_ITEMPREPAINT, so native painting could overwrite the custom colors. The handler now requests CDDS_POSTPAINT and paints the complete header after native drawing, including the unused tail. It uses the same light/dark/high-contrast palette as the panel. See Microsoft's [header drawing contract](https://learn.microsoft.com/en-us/windows/win32/controls/nm-customdraw-header).

The manager also uses the panel's application icon at the current DPI, gives remaining table width to the plugin name column and shows a short selection hint in the details pane. Other columns retain native resizing. Owned icons are replaced on DPI changes and released after window destruction. Existing font lifetime, theme updates, list selection and background package operations remain in place.

## Filters

Move up and Move down are removed with their handlers and editor-only movement code. Add .NET, Add native, Remove and Defaults occupy two rows. The obsolete movement test is removed with that API; no new tests were added. Profile order and pipeline execution are unchanged.

The settings pane starts with Enabled followed by property controls. The long DLL path/type and pipeline paragraph is removed from the top. Hovering an entry retains its full name, DLL identity, help/default information and settings; built-in Radial Follow hover text identifies tablet coordinates and legacy import behavior. Selected-filter Defaults, property validation and Save selection retention remain.

## Dropdowns

Anchored menus use Win32's modal menu loop. A temporary WH_MSGFILTER hook on the UI thread detects a second left click inside the current anchor, ends that menu and consumes the click before the button can send another open command. Every other message continues through the hook chain. The hook is removed when tracking returns and does not run while the UI is idle or on the driver thread. Escape, item selection and clicks on other controls retain native handling. See [MessageProc](https://learn.microsoft.com/en-us/windows/win32/winmsg/messageproc) and [EndMenu](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-endmenu).

## Evidence and limits

The supplied screenshots are the visual baseline. Source review traced header notifications through the list subclass and menu clicks through the shared popup function. No panel, daemon, HID/input session or third-party plugin was launched, and live userdata was not changed.

The owner disables CI and the pre-publish format/Clippy/test/build-check suite; none was run. Windows release compilation produces the downloadable artifacts. Package contents and SHA-256 are compared to the produced files before publication. Compilation and source review do not establish interactive behavior.

Manual release checks remain: manager header colors in light/dark/high-contrast modes, column resizing and DPI changes, filter add/remove/defaults/Save behavior, and second-click dismissal of menu-bar, output-mode, binding and property dropdowns. No measured latency improvement or new device/plugin parity is claimed.
