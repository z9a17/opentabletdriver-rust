# Radial Follow source notice

`crates/otd-core/src/radial_follow.rs` is a Rust port of AbstractQbit's `RadialFollowCore.cs` and `RadialFollowSmoothingTabletSpace.cs` from the [RadialFollow 0.3.0 release](https://github.com/AbstractQbit/AbstractOTDPlugins/tree/0.3.0/RadialFollow). The original plugin is by AbstractQbit and is licensed under GPL-3.0-only. This port keeps the original tablet-space filter name, OpenTabletDriver configuration path, setting names, curve, and 50 ms reset behavior. Its changes are the Rust representation, `std::time::Instant` timing, and direct integration into this driver without the .NET plugin interface.

The pre-existing driver files were released under LGPL-3.0-only. Their license text is retained in [LICENSE.LGPL-3.0](LICENSE.LGPL-3.0). The combined 0.3.0 executable includes the GPL-3.0-only filter and is conveyed under [GPL-3.0-only](LICENSE).

## Relative output source notice

`crates/otd-core/src/relative.rs` ports behavior from OpenTabletDriver contributors' LGPL-3.0-licensed `RelativeOutputMode.cs` and `WindowsRelativePointer.cs` at revision `fdeaa7b0c6d6f5260f511f19fb693ed33524af4e`. [Relative mode documentation](docs/RELATIVE_MODE.md) links the original sources and records the Rust implementation's changes. This module uses LGPL-3.0-only; the combined executable remains GPL-3.0-only.

## Optional .NET compatibility bridge

The release's `compat` directory includes OpenTabletDriver.Plugin 0.6.7 (OpenTabletDriver contributors, LGPL-3.0-or-later; [source](https://github.com/OpenTabletDriver/OpenTabletDriver/tree/736003ed72c8bbb28033b039d5a0bb76c344145c)), Newtonsoft.Json (James Newton-King, MIT; [license](https://github.com/JamesNK/Newtonsoft.Json/blob/master/LICENSE.md)), and Microsoft/JetBrains dependencies listed in `compat/OtdCompat/packages.lock.json`. The .NET hosting library `nethost.dll` is distributed under Microsoft's MIT license; its SDK license and third-party notices accompany the packaged bridge. The .NET runtime itself is not bundled. The compatibility bridge and sample native plugin source are in this repository. The original third-party RadialFollow DLL is used unchanged for tests and is not included in releases; users retain their original plugin and its licensing terms.
