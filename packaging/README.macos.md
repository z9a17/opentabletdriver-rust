# OpenTabletDriver Rust for macOS

Use the archive for your Mac's processor, Intel x64 or Apple Silicon arm64.
macOS 11 or newer is required. Extract it and open a terminal in its directory.

Install the .NET 8 runtime for this CPU.
Grant Input Monitoring and Accessibility permissions in System Settings, then
start the original desktop frontend:

```sh
./opentabletdriver-rust-macos ui
```

For foreground operation without the frontend:

```sh
./opentabletdriver-rust-macos run
```

The pinned original Console client is included beside the native executable:

```sh
./opentabletdriver-rust-macos original-console --help
```

Its commands connect to the original compatibility service on the daemon; use
the original client only with that listener enabled. It runs through .NET 8 and
retains the original command names and API overloads. This does not establish
runtime compatibility of every managed plugin or original client operation.

Stop other tablet drivers before using this driver. Keep the adjacent original
frontend, Rust daemon watchdog launcher and managed assemblies beside the native
executable. The binaries are unsigned and not notarized. The shipped apphosts
and libnethost match this CPU; original MonoMac/Objective-C behavior and native
hardware validation remain pending.

See [the macOS guide](data/MACOS.md) for permissions, profiles, Gatekeeper and
current limitations. Keep the `data` folder for the guide and license notices.
Licenses and source notices are in [data/licenses](data/licenses).
Full documentation and source: [project repository](https://github.com/z9a17/opentabletdriver-rust).
