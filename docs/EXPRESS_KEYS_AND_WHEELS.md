# Express keys, wheels and the binding editors

Express keys (auxiliary buttons), wheels, rings and dials can click, press keys
or do nothing, like pen side buttons. This follows OpenTabletDriver's
`BindingHandler`, `WheelBindings` and `DeltaThresholdBindingState`
([pinned sources](https://github.com/OpenTabletDriver/OpenTabletDriver/tree/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.Desktop/Binding)).
Tasks: B02 (auxiliary), B03 (wheels) and D04 (PTH-660 auxiliary collection).

**Status:** implemented on branch `parity/b02-aux-wheel-bindings`, covered by
replay, profile and offscreen panel tests, not yet tried with a real tablet.
See [the validation checklist](HARDWARE_VALIDATION.md#express-keys-and-wheels-pending).

## Behavior

- An express key or wheel button is held while it is down, like a pen button.
  A pen leaving range does not release it (upstream releases pen buttons only).
- Each rotation threshold a wheel turns through presses and releases its
  action once. The default threshold is one wheel step (360° divided by the
  configuration's step count: 5° on the PTH-660's 72-position ring). A
  threshold that is not positive becomes 1°, as upstream forces it.
- Absolute rings wrap the short way round: on the PTH-660, 71 to 0 is one step
  clockwise. The first reading after touching the ring only records where the
  finger is.
- Lifting the finger, or a relative wheel reporting no movement, drops a partial
  turn. Turning back clears the other direction's partial turn.
- Barrel-button actions on express keys and wheels click the mouse (button 1
  right, button 2 middle), also with pen output.
- Everything shares the session's key and button ownership with the pen
  buttons: two bindings holding the same key press it once and release it once.

## Windows auxiliary collection

Some tablets, the PTH-660 among them, report express keys and the ring on a
separate HID collection (`AuxiliaryDeviceIdentifiers`). On Windows the driver
opens it beside the pen collection, initializes it from its own identifier and
decodes it with that identifier's parser. If it cannot be opened, initialized
or decoded, or it fails later, the Console says so, its keys are released and
pen input continues. Tablets whose express keys arrive on the pen collection
need nothing extra; that also works on Linux and macOS. Linux and macOS do not
yet open a separate auxiliary collection.

## Profiles

```toml
aux_buttons = ["keys:Control+Z", "none", "mouse:right"]

[[wheels]]
clockwise = "keys:Control+Equal"
counter_clockwise = "keys:Control+Minus"
clockwise_threshold = 10.0          # degrees; omit for one wheel step
counter_clockwise_threshold = 10.0
buttons = ["keys:Space"]
```

Actions use the [pen button syntax](PEN_BUTTONS.md#rust-toml-profiles).
Imported OpenTabletDriver profiles bring `Bindings.AuxButtons` and
`Bindings.WheelBindings` (rotation stores, thresholds and wheel buttons);
unsupported bindings such as presets, scrolling and plugin bindings become
import diagnostics and do nothing. `profiles export` writes edits back into the
archived document the same way as pen buttons. A wheel the source document did
not list gets upstream's one-step thresholds.

## Command line

```text
opentabletdriver-rust.exe profiles set driver.toml --output keys.toml --aux-button "1=keys:Control+Z" --wheel-clockwise "1=keys:Control+Equal" --wheel-counter-clockwise "1=keys:Control+Minus" --wheel-threshold 1=10
opentabletdriver-rust.exe profiles get keys.toml --section bindings
```

Express key numbers go up to 64 and wheel numbers up to 8. Wheel buttons are
set in TOML or the panel.

## Panel

**Pen Settings** lists the pen buttons under the tip and eraser settings, and
**Auxiliary Settings** lists the express keys and, per wheel, the clockwise and
counter-clockwise bindings, their thresholds and the wheel buttons. The rows
follow the detected tablet's declared buttons and wheels; with no tablet
detected they show what the profile binds. Each binding's dropdown offers None,
the pen's barrel buttons (pen buttons only), mouse buttons and **Key or
Shortcut…**, which opens a dialog that captures every key including Alt, Tab,
Enter and Escape. Once a shortcut is captured and released, Enter accepts it and
Escape cancels. Save or Apply sends the profile to the driver.

## Not done

- Scroll output (upstream's Mouse Scroll Binding), presets, toggles and
  managed `IBinding` plugins as actions (B04, P06).
- Tablet mouse (puck) buttons and scroll bindings, strips, touch.
- A separate auxiliary collection on Linux and macOS.
