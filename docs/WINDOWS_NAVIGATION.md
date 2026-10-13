# Windows panel navigation

The owner requested a simpler presentation of the existing OpenTabletDriver
settings and utilities. The panel now groups everyday settings into four main
tabs, while keeping less-used pages and commands available through menus.

| Main tab | Existing settings |
| --- | --- |
| Area | Display and tablet areas, output mode and relative movement |
| Filters | Configured filters and their properties |
| Pen | Tip, eraser, pressure thresholds and pen buttons |
| Buttons | Tablet buttons for express keys and wheels; Mouse tool for mouse buttons and scrolling |

Buttons has two inline choices: **Tablet buttons** and **Mouse tool**.
The selected choice is remembered while switching main tabs. Binding dropdowns
keep None and Key or shortcut directly available, and group mouse, scroll, pen
and preset actions into submenus instead of one long list.

| Menu path | Existing page or commands |
| --- | --- |
| Plugins > Tool settings | Configured plugin tools |
| Settings > Performance settings | Advanced performance settings |
| Settings > Theme / Startup | Appearance and automatic startup preferences |
| Help > Diagnostics > Console | Driver messages and console actions |
| Help > Diagnostics | Tablet debugger, device string reader, log and diagnostics exports |
| File > Import & export | Settings import and export |
| File > Import & export > Original settings collection | Original OpenTabletDriver collection operations |
| File > Advanced | Backup recovery, reset and the settings folder |

Utility pages provide **Back** to return to the previous main page. Ctrl+Tab and
Ctrl+Shift+Tab navigate the main tabs. Save and Apply retain their existing roles.

This cleanup reorganizes the existing settings and commands. Profiles, bindings,
plugin compatibility and native theme preferences retain their existing behavior.

Command IDs for original OpenTabletDriver collection/diagnostic actions were
also separated from accent-color IDs. Their old overlapping ranges could route
color selections into settings operations, or diagnostics into color changes.

Validation used hidden native controls and menus without launching the panel,
daemon or computer-use tools. Ten UI layout/navigation tests, one binding-menu
test and one command-ID collision test passed. Offscreen dark/light renders were
inspected, with existing high-contrast/DPI layout coverage retained. Interactive
desktop and physical tablet behavior were not retested during that initial
cleanup. The subsequent [authorized Windows review](BACKGROUND_AUDIT_2026-10-13.md)
records interactive checks of the new navigation with a connected PTH-660;
physical pen-motion and latency acceptance remain open.
