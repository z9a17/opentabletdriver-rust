# Windows application icon

`opentabletdriver.ico` restores this project's blue rounded-tablet icon from
[v0.15.5 canvas.rs](https://github.com/z9a17/opentabletdriver-rust/blob/v0.15.5/src/ui/canvas.rs).
It contains 16, 32, 48, 64, 128 and 256 pixel images, with Windows blue
`#0078D7`, a white active-area outline and a white center dot. It uses this
project's GPL-3.0-only license. Regenerate it with
`python scripts/generate-app-icon.py`; the script preserves the earlier geometry.

`build.rs` embeds it as Windows icon resource 1 in both executables. The native
panel loads owned copies at the current DPI for its window, taskbar, tray and
plugin manager. Explorer and shortcuts read the same embedded resource.

MSVC builds require the Windows SDK resource compiler, found through `RC`, PATH
or the SDK installation. GNU Windows builds use `WINDRES`. Resource compilation
errors fail the build instead of silently shipping a generic app icon.
