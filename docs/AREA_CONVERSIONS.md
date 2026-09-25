# Area editing and conversion

The graphical editor and offline commands share the portable area, aspect-ratio, alignment, flip and usable-bound helpers in `otd_core::areas`. Existing graphical actions keep their behavior. These services do not start a tablet or apply settings to a driver.

```text
opentabletdriver-rust.exe area full --tablet "Wacom PTH-660"
opentabletdriver-rust.exe area fit 224 148 1.7777777777777777
opentabletdriver-rust.exe area convert percentage 0.25 0.25 0.75 0.75
opentabletdriver-rust.exe area convert wacom-veikk 1000 2000 20000 40000
```

Results are JSON containing the area's width, height, center X/Y and rotation, in millimetres. `fit` centers the largest rectangle of the requested width/height ratio inside the supplied bounds. Converter results have zero rotation. Inputs and output sizes must be finite; dimensions must be positive. Conversion does not automatically clamp an area to physical tablet bounds.

`--tablet NAME` chooses a valid named entry from the pinned device database. The default is Wacom PTH-660. `--configurations DIRECTORY` applies that directory's configuration overrides for conversion only. A configuration being available for conversion does not imply driver support for the tablet.

| Format | Four inputs, in order | Convention |
| --- | --- | --- |
| `percentage` | Up, Left, Down, Right | Fractions: `1` means 100 percent, not `100` |
| `wacom-veikk` | Top, Left, Bottom, Right | Raw tablet coordinates; both axes use MaxX / Width |
| `xp-pen` | W, H, X, Y | XP Pen driver units; pinned factor `3.937f` |
| `gaomon-v2-otd067` | Width, Height, X, Y | Matches the pinned upstream formula, including its X-for-Y offset quirk |
| `gaomon-v2-corrected` | Width, Height, X, Y | Explicitly uses the supplied Y for the Y offset |

The baseline is [OTD 0.6.7's conversion sources](https://github.com/OpenTabletDriver/OpenTabletDriver/tree/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.Desktop/Conversion). In its Gaomon V2 converter, the fourth input is unused: both center offsets depend on X. The Rust command exposes that behavior under the explicit `otd067` name and reports a notice. The corrected variant is separate; it is not silently substituted. Specification and result rounding follows the upstream Single-valued properties and Area fields.

These commands preview a result and never rewrite profiles.

## Graphical conversion

In Absolute Mode, right-click the tablet area and choose **Convert area from...**, or use **Tablets > Convert tablet area from...**. Select a source format and enter its four values. Labels, units and the pinned/corrected Gaomon notice follow the selected format. **Preview** validates finite inputs and positive output dimensions and shows width, height, center and zero rotation in millimetres. Editing a value or changing format invalidates the preview; **Use area** stays disabled until another valid preview exists.

**Use area** replaces only the unsaved tablet area with the displayed conversion result. It deliberately does not enforce aspect/usable-area locks or change those preferences; the dialog states this before acceptance. Display area, clipping, limiting, filters and other settings stay intact. Save and runtime Apply remain separate actions. Cancel, invalid inputs or a profile change while the dialog is open leave the selected profile unchanged. An older simple crop/monitor profile becomes the equivalent area mapping only after acceptance, as with other graphical area edits.

The graphical editor currently uses pinned PTH-660 specifications and rejects other stored tablet profiles explicitly. The offline CLI can preview other configured tablets; conversion does not imply runtime support. No dependencies, device opens or automatic saves are introduced by this workflow.

The graphical slice has been source-reviewed and compiled with strict Clippy, without local tests, UI or hardware execution. Interactive accessibility, DPI/layout and conversion/runtime evidence remain open. Panel Save follows its normal behavior: after saving, it also applies settings when a worker is attached.
