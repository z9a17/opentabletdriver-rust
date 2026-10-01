# OpenTabletDriver Rust for Linux

Extract the archive, then open a terminal in its directory:

```sh
sudo ./setup/install.sh install
```

Reconnect your tablet and run the driver as your normal user:

```sh
./opentabletdriver-rust-linux run
```

Stop other tablet drivers before using this driver. Keep the `setup` and `data`
folders. Setup changes require sudo; the driver runs without it. This package
provides a command-line driver, without the Windows control panel.

See [the Linux guide](data/LINUX.md) for permissions, supported desktops, profiles
and removal. Native pen/cursor validation remains incomplete.
Licenses and source notices are in [data/licenses](data/licenses).
Full documentation and source: [project repository](https://github.com/z9a17/opentabletdriver-rust).
