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

Since 0.15.5, `profiles export` writes supported pen-button edits into a copy of
the archived OpenTabletDriver document. It preserves unchanged bindings, other
profiles and settings. A disabled button keeps its original store with `Enable`
set to false; new empty slots are null. Replacing unsupported stores, changing a
binding type that has unknown properties, or shortening the archived button list
returns an error. Use `none` for unwanted buttons. A standalone TOML profile still
needs an imported OTD archive to export.

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

## Offline command-line editing

The Windows CLI can inspect and edit pen buttons without starting the driver:

```text
opentabletdriver-rust.exe profiles get driver.toml --section bindings
opentabletdriver-rust.exe profiles set driver.toml --output undo.toml --pen-button "1=keys:Control+Z" --pen-button "2=mouse:right"
opentabletdriver-rust.exe profiles export undo.toml --output exported-settings.json
```

Button numbers start at 1, while profile indexes start at 0. Repeat `--pen-button`
for different buttons, up to 64; duplicate numbers and unsupported actions are
errors. Missing slots between the current list and the edited button become
`none`. Each successful command increments the settings revision once and
requires a new output filename. It never changes the source or applies the
profile to a running driver. Button edits work in absolute and relative profiles;
relative sensitivity/rotation/reset options still require relative output.

`profiles get --section bindings` and the default `all` section always include
`pen_buttons`, including the default barrel assignments omitted from TOML. Key
names are printed in canonical form, such as `keys:LeftControl+Z`.

The Linux and macOS driver CLIs can load the resulting TOML but do not yet expose
these profile-editing commands. A key accepted by the profile format may still
be unavailable on the target platform; see [platform output](#platform-output).

Export uses the pinned upstream [AdaptiveBinding](https://github.com/OpenTabletDriver/OpenTabletDriver/blob/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.Desktop/Binding/AdaptiveBinding.cs),
[MouseBinding](https://github.com/OpenTabletDriver/OpenTabletDriver/blob/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.Desktop/Binding/MouseBinding.cs),
[KeyBinding](https://github.com/OpenTabletDriver/OpenTabletDriver/blob/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.Desktop/Binding/KeyBinding.cs)
and [MultiKeyBinding](https://github.com/OpenTabletDriver/OpenTabletDriver/blob/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.Desktop/Binding/MultiKeyBinding.cs)
store paths and property names. This establishes format support, not physical
button or application compatibility.

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

## Platform output

Windows sends keys with `SendInput` as scan codes, so they are physical key
positions: the keyboard layout decides the character. Linux presses evdev keys on
a virtual keyboard, created when a profile has a key binding. Not every name works
on every platform: the driver lists the ones it cannot press at start-up
(`Pen button not applied: ...`) and those buttons do nothing. `F13` to `F24`, `Help`, `Pause`
and `KeypadEqual` are supported on Linux. macOS also supports F13 through F20
and `KeypadEqual`, but rejects F21 through F24, Help, Pause, PrintScreen,
ScrollLock, CapsLock, NumberLock, Insert and ContextMenu at startup. macOS uses
physical Carbon key positions through CoreGraphics. `Application` is Command,
`Alt` is Option, and `Control` stays Control; use `keys:Application+Z` for Mac
undo. Mouse movement carries held right/middle/other buttons as drag events.
Native Mac validation of shortcuts, modifiers and dragging remains pending.

On all three platforms, a left-click side binding and tip contact share
ownership. Releasing one does not release the other. macOS tracks synthetic
modifier flags because system snapshots may lag. Physical/synthetic overlap
of the same modifier cannot be distinguished from that snapshot and needs
native validation; see [the macOS guide](../crates/otd-macos/README.md).

## Not done

- A settings panel editor for pen buttons.
- Toggle, preset, scroll and drag-only ("Enable drag bindings") behavior, and
  managed (`IBinding`) plugin bindings (B04, P06).
- Express keys and tablet mouse buttons (B02 auxiliary/mouse, D04).
- Barrel buttons 2 and 3 as separate buttons on Windows pen output.
