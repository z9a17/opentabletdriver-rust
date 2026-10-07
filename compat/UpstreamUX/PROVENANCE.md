# Original desktop frontend

The C# frontend, assets, GTK launcher and macOS launcher are copied from
OpenTabletDriver revision `736003ed72c8bbb28033b039d5a0bb76c344145c`
(0.6.7). Their C# source is unchanged. Project files replace upstream-relative
references with the vendored Desktop project, set the same .NET 8 framework,
preserve original assembly/resource names and add package lock generation.
The upstream LGPL-3.0-or-later license is included as `LICENSE`.

`compat/NativeDaemonLauncher` is Rust-project code. It occupies the daemon
executable name expected by the original watchdog and launches the native Rust
multi-device owner. It does not contain the upstream hardware daemon.

GTK requires its real native GTK3 dependencies. The macOS Eto/MonoMac native
assets must support the target architecture. An x64 or arm64 cross-build is
not native GUI, permission, input or hardware validation. No frontend was
launched or validated during this implementation.
