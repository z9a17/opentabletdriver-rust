# Windows application icon

`opentabletdriver.ico` is copied unchanged from OpenTabletDriver's
[otd.ico at revision 736003ed72c8bbb28033b039d5a0bb76c344145c](https://github.com/OpenTabletDriver/OpenTabletDriver/blob/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.UX/Assets/otd.ico).
It contains 16, 32, 48, 64, 128 and 256 pixel images. The upstream project is
licensed under LGPL-3.0; see the source notice in [NOTICE.md](../NOTICE.md).

`build.rs` embeds it as Windows icon resource 1 in both executables. The native
panel loads owned copies at the current DPI for its window, taskbar, tray and
plugin manager. Explorer and shortcuts read the same embedded resource.

MSVC builds require the Windows SDK resource compiler, found through `RC`, PATH
or the SDK installation. GNU Windows builds use `WINDRES`. Resource compilation
errors fail the build instead of silently shipping a generic app icon.
