# Windows application icon

`opentabletdriver.svg` is the application icon: a pen with a rust-orange grip
drawing on a tablet's mapped active area, on a Windows-blue tile.
`opentabletdriver-small.svg` is a simplified drawing of the same icon with
thicker strokes, so it stays legible at 16-24 pixels in the notification area
and title bar. Both use this project's GPL-3.0-only license.

`opentabletdriver.ico` is generated from them with
`python scripts/generate-app-icon.py` (requires `resvg`, installed with
`cargo install resvg`). It contains PNG images at 16, 20 and 24 pixels from the
small drawing and 32, 40, 48, 64, 96, 128 and 256 pixels from the full one.
Edit the SVGs, then regenerate the icon; do not edit the `.ico` by hand.

`build.rs` embeds it as Windows icon resource 1 in both executables. The native
panel loads owned copies at the current DPI for its window, taskbar, tray and
plugin manager. Explorer and shortcuts read the same embedded resource.

MSVC builds require the Windows SDK resource compiler, found through `RC`, PATH
or the SDK installation. GNU Windows builds use `WINDRES`. Resource compilation
errors fail the build instead of silently shipping a generic app icon.
