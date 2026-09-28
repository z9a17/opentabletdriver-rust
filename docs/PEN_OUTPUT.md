# Pen output (Windows Ink)

Absolute Mode can drive a pen instead of the mouse. Drawing applications that use Windows Ink then receive pressure, tilt, the eraser and hover; other applications see the pen's ordinary mouse emulation. Choose **Windows Ink Mode (pen)** from the output mode menu on the **Output** tab, or set it in a profile:

```toml
output = "pen"
```

The default, `output = "mouse"`, is not written to saved profiles. Pen output is absolute only; choosing Relative Mode switches back to the mouse, and a profile with both `output = "pen"` and `[relative]` is rejected.

## Behavior

The area, rotation, clipping, area limiting and filters work as in Absolute Mode. The pen position is the mapped desktop pixel, kept inside the virtual screen.

| Pen state | What Windows receives |
| --- | --- |
| In range, not touching | Hover at the pen position, pressure 0 |
| Tip binding pressed | Contact with the report's pressure (0-1024 of the tablet's maximum) |
| Tip binding released | Contact ends; the pen stays in range |
| Eraser end | The pen is inverted; with the eraser binding pressed it also erases |
| Switching between tip and eraser | The pen leaves range and enters again with the other end |
| Out of range, disconnect, stop | Contact ends, then the pen leaves range |

Contact follows the **Pen Settings** tip and eraser bindings and their pressure thresholds, as the mouse's left button does. Pressure is sent only while the binding holds contact, as the Windows Ink plugin does. Tilt is sent in degrees when the tablet reports it, limited to ±90. Filters that change pressure, tilt or the eraser flag affect the pen.

Windows implements the pen with a synthetic pointer device ([`InjectSyntheticPointerInput`](https://learn.microsoft.com/windows/win32/api/winuser/nf-winuser-injectsyntheticpointerinput), Windows 10 1809 or later). The device setup follows the catalog's [Windows Pen Pointer](https://github.com/Kuuuube/VoiDPlugins/tree/02c3ed3a54937e39157f984c42400a755b82eb9e/src/OutputMode/WindowsPenPointer) plugin, which uses the same API from OpenTabletDriver: pointer ID 1, indirect feedback, an initial `NEW` injection, the primary flag and the foreground window as the target. Unlike that plugin, `DOWN` and `UP` are sent only on the contact change, as the pointer documentation describes. No driver needs to be installed; VMulti is not used.

The report thread sends each packet directly, with no queue and no allocation after the device is created. Each connected tablet in pen output creates its own pen device.

## Importing OpenTabletDriver settings

| OpenTabletDriver output mode | Imported as |
| --- | --- |
| `VoiDPlugins.OutputMode.WinInkAbsoluteMode` (Windows Ink plugin) | Pen output. Tip and eraser bindings of the plugin's **Pen Tip** action mean contact; its other actions (**Pen Button**, **Eraser (Toggle)**, **Eraser (Hold)**) are reported and not applied. The plugin's Sync settings are preserved but not applied. |
| `VoiDPlugins.OutputMode.WindowsPenPointerOutputMode` (Windows Pen Pointer plugin) | Pen output that touches whenever pressure is above zero, as the plugin does. The profile's bindings and thresholds are preserved but not applied. |
| `VoiDPlugins.OutputMode.WinInkRelativeMode` | Rejected: pen output is absolute. |

Mouse bindings (`Tip`/`Eraser` adaptive bindings) also mean contact in pen output, and Windows Ink bindings without a Windows Ink output mode are reported and not applied. Exporting a pen profile back to OpenTabletDriver keeps the Windows Pen Pointer mode when the profile came from it and otherwise writes the Windows Ink plugin's mode with **Pen Tip** contact bindings; OpenTabletDriver needs that plugin installed to use the export.

## Compatibility and validation

This is a native pen output, not the Windows Ink or Windows Pen Pointer plugin DLL running unchanged. Running those DLLs needs managed output-mode hosting (P06), and the Windows Ink plugin also needs the VMulti driver; that compatibility remains open (CAP-21, O03).

Automated tests cover the pen lifecycle, tool switching, retry after a refused packet, allocation-free processing, the Windows pointer fields for every state, profile round trips and the imports above. **The pen output has not yet been used with a tablet or a drawing application.** Pen side buttons (barrel), express keys, WinTab applications and pen output on the secure desktop are not covered. Report results, including the application and Windows version, in [issue #1](https://github.com/z9a17/opentabletdriver-rust/issues/1).
