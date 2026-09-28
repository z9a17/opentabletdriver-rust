# Pen side buttons

The buttons on the side of the pen (barrel buttons) can click, press keys or
do nothing. This follows OpenTabletDriver's `BindingHandler`, which presses a
button's binding when its state in a report changes and releases every pen
button when the pen leaves range. Tablet express keys, wheels and rings are a
separate task (B02 auxiliary, B03).

**Status:** implemented and covered by replay tests, but not yet tried with a
real tablet. See [the validation checklist](HARDWARE_VALIDATION.md#pen-side-buttons-pending).

## Defaults

Like OpenTabletDriver, a profile starts with **Button 1 = right click** and
**Button 2 = middle click**. Button 3 does nothing on a mouse. With pen output
(Windows Ink, Linux Artist Mode) the buttons are the pen's own barrel buttons
instead, see below. The PTH-660 has two side buttons in `0x10` reports and
three in `0x1E` reports; other tablets report their own count.

## Imported OpenTabletDriver settings

`Bindings.PenButtons` is imported as it is, entry by entry:

| OpenTabletDriver binding | Setting | Result |
| --- | --- | --- |
| Adaptive Binding | `Button 1`, `Button 2`, `Button 3` | that barrel button |
| Adaptive Binding | `Tip`, `Eraser` | left button (mouse output only) |
| Mouse Button Binding | `Left`, `Middle`, `Right`, `Backward`, `Forward` | that mouse button |
| Key Binding | one key name such as `Escape` or `F5` | that key |
| Multi-Key Binding | keys joined by `+`, such as `Control+Shift+Z` | all held together |

A disabled or empty entry does nothing. Anything else (preset, toggle or scroll
bindings, plugin bindings, key names this driver cannot press) is reported in the
import diagnostics, and that button does nothing. Key names are OpenTabletDriver's
(`D1` for the 1 key, `Control`, `Shift`, `Alt`, `LeftApplication`, `Keypad5`,
...); `Control`, `Shift`, `Alt` and `Application` mean the left key. The media
keys (`Mute`, `VolumeUp`, `PlayPause`, ...) are not supported.

Changes made to pen buttons in this driver cannot be written back to an
OpenTabletDriver settings file (`profiles export` refuses); keep the Rust TOML
profile for them.

## Rust TOML profiles

```toml
# One entry per pen button, button 1 first. Omit for the defaults.
pen_buttons = ["mouse:right", "keys:Control+Z", "none"]
```

| Entry | Meaning |
| --- | --- |
| `none` | nothing |
| `barrel:1` .. `barrel:3` | what the output does for that barrel button (the default) |
| `mouse:left`, `mouse:right`, `mouse:middle`, `mouse:backward`, `mouse:forward` | a mouse button |
| `keys:Control+Shift+Z` | a key or a chord, with OpenTabletDriver's key names |

Buttons past the end of the list do nothing. The settings panel has no editor
for pen buttons yet; it keeps them when it saves.

## How a button is carried out

- **Mouse output** (absolute and relative): `barrel:1` clicks right, `barrel:2`
  middle, `barrel:3` nothing. The pointer moves first and the button follows in
  the same report, so a click lands where the pen is.
- **Pen output:** `barrel:n` is the pen's barrel button. Windows pens have one
  barrel button, and, like the Windows Pen Pointer plugin, all three set it. On
  Linux they are `BTN_STYLUS`, `BTN_STYLUS2` and `BTN_STYLUS3` of the virtual
  tablet. Mouse and key entries still click and type, through a mouse or
  keyboard device.
- **Leaving range** releases everything the pen holds. So do a stop, a
  reconnect and a display change that pauses the mapping.
- Two buttons (or two tablets) holding the same key press it once and release it
  once. A failed press is retried on the next report and a failed release is
  retried until it succeeds; neither marks the key as sent.
- A chord presses its modifiers first and releases them last.
- Report processing stays allocation-free (there is a test for it).

## Windows and Linux

Windows sends keys with `SendInput` as scan codes, so they are physical key
positions: the keyboard layout decides the character. Linux presses evdev keys on
a virtual keyboard, created when a profile has a key binding. Not every name works
on every platform: the driver lists the ones it cannot press at start-up
(`Pen button not applied: ...`) and those buttons do nothing. `F13` to `F24`, `Help`, `Pause`
and `KeypadEqual` are supported on Linux only.

## Not done

- A settings panel editor for pen buttons.
- Toggle, preset, scroll and drag-only ("Enable drag bindings") behavior, and
  managed (`IBinding`) plugin bindings (B04, P06).
- Express keys and tablet mouse buttons (B02 auxiliary/mouse, D04).
- Barrel buttons 2 and 3 as separate buttons on Windows pen output.
