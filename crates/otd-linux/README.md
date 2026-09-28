# Linux backend slice (experimental)

`opentabletdriver-rust-linux` runs one tablet through the same portable core
as the Windows driver: hidraw input, the pinned upstream device database and
parsers, the profile's area/filters/pressure settings, and a uinput pointer
for output.

**Status:** it has been compiled and unit-tested, but it has never run on a
Linux machine with a tablet. Treat it as a starting point for testers, not a
working driver.

Not there yet:

- a daemon, the control pipe, the UI and the CLI commands of the Windows build;
- .NET or native DLL plugins (the built-in Radial Follow does run);
- desktop layout discovery: pass `--screen WIDTHxHEIGHT` for the whole virtual
  screen;
- hotplug notifications: it rescans every two seconds;
- pad buttons and express keys, and several tablets at once.

## Use

```sh
opentabletdriver-rust-linux list
opentabletdriver-rust-linux run --screen 2560x1440
opentabletdriver-rust-linux run --profile ~/profile.toml --tablet "Wacom PTH-660"
```

Without `--profile` it imports `~/.config/OpenTabletDriver/settings.json`
(or `$XDG_CONFIG_HOME/OpenTabletDriver`) as upstream stores it. Stop
OpenTabletDriver's daemon and unload a kernel driver that grabs the tablet
first, or both will move the pointer.

## Pen side buttons

The profile's [pen button actions](../../docs/PEN_BUTTONS.md) work here too:
right and middle click by default, or mouse buttons and keys of your choice.
Keys go through a second virtual device, "OpenTabletDriver Rust keyboard",
which is created only when a profile has a key binding. The mouse buttons use
the pointer. Nothing here has been run with a tablet.

## Artist Mode

A profile with `output = "pen"`, or an imported OpenTabletDriver profile in
Artist Mode, creates upstream's virtual tablet instead of the pointer:
"OpenTabletDriver Virtual Artist Tablet", with positions in thousandths of a
pixel, pressure 0-65535 with `BTN_TOUCH` while touching, tilt -64..63 and the
pen/eraser tool keys held while in range. Imported Artist Mode profiles touch
whenever pressure is above zero, as upstream does; native profiles follow
their tip and eraser thresholds. Pen side buttons are `BTN_STYLUS`, `BTN_STYLUS2` and `BTN_STYLUS3`. The event
framing is unit-tested, and the virtual devices have been created on a Linux
machine (without a tablet or any input sent).

## Permissions

It needs read/write access to the tablet's `/dev/hidraw*` node and to
`/dev/uinput`, and, for tablets with initialization strings or device-string
matches, to its `/dev/bus/usb` node. A udev rule such as this grants them to
the logged-in user (replace the vendor ID):

```
KERNEL=="hidraw*", ATTRS{idVendor}=="056a", TAG+="uaccess"
SUBSYSTEM=="usb", ATTRS{idVendor}=="056a", TAG+="uaccess"
KERNEL=="uinput", SUBSYSTEM=="misc", TAG+="uaccess", OPTIONS+="static_node=uinput"
```

Running as root is not a supported substitute.
