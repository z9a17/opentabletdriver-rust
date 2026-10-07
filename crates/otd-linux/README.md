# Linux runtime and original desktop frontend

`opentabletdriver-rust-linux` reads a USB tablet through hidraw and uses the
portable Rust core, pinned upstream device database and report parsers. It
creates a uinput pointer for mouse output or a virtual tablet for Artist Mode.
The Linux package includes the native multi-device daemon, original Gtk frontend,
original Console client, managed plugin host, setup files and source notices.
It uses GNU dynamic linking. Native input with native display discovery or an
explicit `--screen` needs no .NET runtime; the original
frontend, Console and managed plugins need the x64 .NET 8 runtime, and the
frontend additionally needs GTK3. Explicit network operations need system curl.
Keep the adjacent managed assemblies and the `data/compat` directory intact.

Implementation and hardware validation are separate. The connected Wacom
PTH-660 was identified through read-only sysfs metadata, including its 192-byte
input report length and active `wacom` kernel driver. That observation does not
prove pen movement, pressure, output, unplug/replug or suspend/resume. Consult
the release notes for the hardware evidence collected for that release.

## Setup

Extract the release archive, open a terminal in its directory, and run:

```sh
sudo ./setup/install.sh install
```

Replug the tablet, then run the driver as your normal user. On an active
systemd-logind desktop session, the installed udev rules give that user access
to the tablet's hidraw and USB nodes and `/dev/uinput`. The rules include only
device VID/PID pairs from the pinned database. They run before the seat ACL
rules. Access to the USB node is needed for configurations that request USB
strings. Capture does not require uinput access.

Setup installs `/etc/udev/rules.d/70-opentabletdriver-rust.rules` and
`/etc/modules-load.d/opentabletdriver-rust.conf`, then loads `uinput`. It records
file hashes under `/var/lib/opentabletdriver-rust-setup` so later releases can
update unchanged files and preserve local edits. It never installs a global
kernel driver blacklist, starts the driver or changes compositor settings.
For sessions without logind access grants, arrange equivalent device access
with your distribution's administrator; root operation is not the normal
workflow.

Check access and kernel conflicts with:

```sh
./setup/install.sh status
./opentabletdriver-rust-linux list
```

Stop an existing OpenTabletDriver daemon before running the Rust driver. If
`list` reports `wacom` or `hid_uclogic`, both the kernel and Rust driver can emit
input. On desktops using libinput, you can opt in to ignoring physical input
from the connected USB PTH-660 while preserving hidraw access:

```sh
sudo ./setup/install.sh ignore-input 056a 0357
```

This installs `/etc/udev/rules.d/71-opentabletdriver-rust-ignore-056a-0357.rules`
with `LIBINPUT_IGNORE_DEVICE=1` only for physical input devices belonging to
that USB VID/PID. It records the file hash under the setup directory. Replug
the tablet afterwards. The compositor must rediscover the devices before the
rule takes effect; no live effect is claimed. Applications reading raw evdev
can still receive the physical tablet's input. The virtual Rust output device
does not inherit this USB match. Remove unchanged owned ignore rules with:

```sh
sudo ./setup/install.sh restore
```

Replug afterwards to restore normal libinput handling. The command affects
every USB tablet with that exact VID/PID until its rule is removed. Other
models need their actual identifiers from `list`. This is optional and is not
performed by `install`.

Kernel bindings remain unchanged. The older `detach` command now fails before
changing anything: a loaded specialized driver prevents `hid-generic` from
matching the device, and generic binding can still create physical input.
`restore` retains support for old detachment records under
`/run/opentabletdriver-rust-detached` so existing users can recover them.

Remove this application's unchanged setup files with:

```sh
sudo ./setup/install.sh uninstall
```

Uninstall removes unchanged owned ignore rules and restores old recorded HID
bindings. It preserves edited setup files
and leaves the uinput module loaded until reboot. Profiles and other drivers'
files are preserved.

## Use

```sh
./opentabletdriver-rust-linux --help
./opentabletdriver-rust-linux --version
./opentabletdriver-rust-linux list
./opentabletdriver-rust-linux capture --seconds 10 --tablet "Wacom PTH-660"
./opentabletdriver-rust-linux run
./opentabletdriver-rust-linux run --screen 2560x1440
./opentabletdriver-rust-linux run --profile ~/profile.toml --tablet "Wacom PTH-660"
./opentabletdriver-rust-linux ui
./opentabletdriver-rust-linux original-console --help
```

`ui` launches the adjacent original Gtk assembly. Its watchdog launches the
packaged `OpenTabletDriver.Daemon` forwarder, which owns a separate native Rust
daemon and its lifetime. To run original Console operations without the frontend,
start `./opentabletdriver-rust-linux daemon --upstream-rpc` in another terminal.
The original listener uses `OpenTabletDriver.Daemon`; `--upstream-pipe NAME`
selects a custom endpoint for clients that support it. Plain `run` owns the native
daemon services without enabling that original listener. Native `status`,
`start`, `stop`, `shutdown`, `detect`, `request` and `console` commands use the
separate native control channel. Start only one driver owner at a time.

Capture is bounded to 1 through 60 seconds and 100,000 reports. It initializes
the tablet according to its configuration, then logs decoded/raw report
samples and counts through the shared capture session. It never creates a
uinput device or injects input. A tablet may need initialization writes even
for capture. Move the pen during capture to collect evidence; an empty capture
is not evidence of working pen input. Raw output can contain identifying
information, so review it before sharing.

Run discovers monitor rectangles once at startup from Hyprland's `hyprctl`,
Sway's `swaymsg` or X11's `xrandr`. Hyprland uses logical desktop coordinates,
including output scaling and rotation. Sway provides its logical rectangles.
Xwayland's monitor list is not used for a Wayland session. Other Wayland
compositors use the original display-provider fallback, which needs .NET 8.
If discovery fails, supply `--screen WIDTHxHEIGHT`; there is no assumed desktop
size. Explicit
`--screen` describes a single rectangle at the origin. Restart after changing
display topology, scale or rotation. Compositor device-to-output assignment can
still affect how a virtual tablet maps; validate mapping in your desktop.

Without `--profile`, run imports `~/.config/OpenTabletDriver/settings.json`,
or the equivalent path under `$XDG_CONFIG_HOME`, if a profile exists for the
detected tablet. Otherwise it uses the defaults for that tablet. Original
settings collections retain disconnected profile rows and global tools; live
changes use the daemon-owned lifecycle. An explicit native profile takes
precedence for its selected model. Installed managed filters, output modes,
bindings, tools, parsers and device providers use the shared .NET host. Their
platform and external-driver prerequisites still apply; unsupported dependencies
are reported rather than ignored. The built-in Radial Follow filter is available.
Capture uses native defaults and does not import or execute plugins.

Missing permissions are reported as errors rather than retried indefinitely.
Run waits for supported tablets and owns independent primary/auxiliary sessions.
Disconnect/reconfigure retires only the affected worker. Shared input ownership
prevents one tablet or managed scope from releasing another's held actions.
Ctrl+C or SIGTERM drains the owned sessions, tools and virtual input devices.

While a session drives output, its report loop requests real-time priority
(`SCHED_FIFO` 40) to reduce competition from ordinary scheduled work
([measurements](../../docs/REALTIME_SCHEDULING_2026-10-05.md)). This needs an
rtprio limit, which audio packages often grant to a group such as `audio`,
`realtime` or `pipewire`. Check yours with `ulimit -r`. If it is 0, add a file
such as `/etc/security/limits.d/99-opentabletdriver-rust.conf` containing
`<user> - rtprio 40`, then sign out and in. Without a limit the driver logs a
hint and keeps normal priority. `OTD_RUST_REALTIME=0` keeps normal priority.

## Artist Mode and remaining scope

The profile's [pen button actions](../../docs/PEN_BUTTONS.md) use the virtual
pointer for mouse buttons and a virtual keyboard for keys and chords. The
keyboard is created before tablet initialization when a key binding is
configured. Mouse output defaults to right and middle click. A side button
bound to left click and the tip share the same held state; releasing either
one preserves the other's press. Physical side-button and shortcut validation
is still pending.

A native profile with `output = "pen"`, or an imported Artist Mode profile,
creates `OpenTabletDriver Virtual Artist Tablet`. It advertises position,
pressure, tilt, touch and pen/eraser tool keys. Positions use thousandths of a
pixel, pressure uses 0 through 65535, and tilt uses -64 through 63. Imported
Artist Mode profiles touch at any positive pressure; native profiles follow
their configured tip and eraser thresholds. Default barrel bindings emit
`BTN_STYLUS`, `BTN_STYLUS2` and `BTN_STYLUS3` on the virtual tablet. Explicit
mouse or key bindings use separate virtual pointer/keyboard output as needed.

Pad/wheel bindings, auxiliary endpoints, simultaneous tablets, original desktop
workflows and managed hosting are implemented in source. Native TOML keys retain
physical positions; imported original bindings retain the pinned Evdev names and
aliases. They persist with the `evdev:` key prefix when saved as native TOML.
The original Console retains the upstream command names and collection workflows.
Native `plugins` and `update` commands perform explicit installation/update work;
they do not run as part of passive discovery.

Native Linux input, Gtk/Wayland/X11 behavior, plugin binaries and hardware remain
unqualified. Parser availability does not establish working transport,
initialization or output for every database entry, including Bluetooth devices.
Restart after changing display topology. Replug, cancellation, held-input
failure cleanup, sleep/wake and simultaneous-tablet behavior still need native
acceptance. No tests, format/Clippy/check suites, runtime, frontend, plugin or
hardware sessions were run for this source update.

The setup rules follow [upstream generate-rules.sh at the pinned revision](https://github.com/OpenTabletDriver/OpenTabletDriver/blob/736003ed72c8bbb28033b039d5a0bb76c344145c/generate-rules.sh).
The opt-in per-tablet libinput rule deliberately avoids
[upstream's global modprobe overrides](https://github.com/OpenTabletDriver/OpenTabletDriver/blob/736003ed72c8bbb28033b039d5a0bb76c344145c/eng/bash/Generic/usr/lib/modprobe.d/99-opentabletdriver.conf).
Regenerate the shipped rules after a database update with
`python3 packaging/linux/generate-rules.py` in the source checkout.
The source checkout's setup script is `packaging/linux/install.sh`; release
archives place the same script and its setup assets under `setup/`.
