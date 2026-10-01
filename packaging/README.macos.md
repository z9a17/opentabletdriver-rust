# OpenTabletDriver Rust for macOS

Use the archive for your Mac's processor, Intel x64 or Apple Silicon arm64.
macOS 11 or newer is required. Extract it and open a terminal in its directory.

Grant Input Monitoring and Accessibility permissions in System Settings, then:

```sh
./opentabletdriver-rust-macos run
```

Stop other tablet drivers before using this driver. This package provides a
command-line driver, without the Windows control panel. The binaries are
unsigned and not notarized, and native hardware validation remains pending.

See [the macOS guide](data/MACOS.md) for permissions, profiles, Gatekeeper and
current limitations. Keep the `data` folder for the guide and license notices.
Licenses and source notices are in [data/licenses](data/licenses).
Full documentation and source: [project repository](https://github.com/z9a17/opentabletdriver-rust).
