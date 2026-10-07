# OpenTabletDriver Rust for Linux

Extract the archive, then open a terminal in its directory:

```sh
sudo ./setup/install.sh install
```

Install the .NET 8 runtime and native GTK3
for the original desktop frontend/managed plugins. The native GNU Linux binary
also requires your distribution's glibc. Reconnect your tablet and start the
original desktop frontend as your normal user:

```sh
./opentabletdriver-rust-linux ui
```

For foreground operation without the frontend:

```sh
./opentabletdriver-rust-linux run
```

Stop other tablet drivers before using this driver. Keep the `setup` and `data`
folders and all adjacent managed/frontend files. Setup changes require sudo;
the driver runs without it. The frontend watchdog owns a separate native daemon.
Keep the original launcher and managed assemblies beside the Rust executable.
Native GTK/Wayland/X11, plugin and input behavior has not been qualified on the
owner's Linux hardware.

See [the Linux guide](data/LINUX.md) for permissions, supported desktops, profiles
and removal. Native pen/cursor validation remains incomplete.
Licenses and source notices are in [data/licenses](data/licenses).
Full documentation and source: [project repository](https://github.com/z9a17/opentabletdriver-rust).
