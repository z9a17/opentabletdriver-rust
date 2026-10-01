# Radial Follow source notice

`crates/otd-core/src/radial_follow.rs` is a Rust port of AbstractQbit's `RadialFollowCore.cs` and `RadialFollowSmoothingTabletSpace.cs` from the [RadialFollow 0.3.0 release](https://github.com/AbstractQbit/AbstractOTDPlugins/tree/0.3.0/RadialFollow). The original plugin is by AbstractQbit and is licensed under GPL-3.0-only. This port keeps the original tablet-space filter name, OpenTabletDriver configuration path, setting names, curve, and 50 ms reset behavior. Its changes are the Rust representation, `std::time::Instant` timing, and direct integration into this driver without the .NET plugin interface.

The pre-existing driver files were released under LGPL-3.0-only. Their license text is retained in [LICENSE.LGPL-3.0](LICENSE.LGPL-3.0). The combined 0.3.0 executable includes the GPL-3.0-only filter and is conveyed under [GPL-3.0-only](LICENSE).

## Relative output source notice

`crates/otd-core/src/relative.rs` ports behavior from OpenTabletDriver contributors' LGPL-3.0-licensed `RelativeOutputMode.cs` and `WindowsRelativePointer.cs` at revision `fdeaa7b0c6d6f5260f511f19fb693ed33524af4e`. [Relative mode documentation](https://github.com/z9a17/opentabletdriver-rust/blob/main/docs/RELATIVE_MODE.md) links the original sources and records the Rust implementation's changes. This module uses LGPL-3.0-only; the combined executable remains GPL-3.0-only.

## Tablet configuration data

`crates/otd-core/tablets` holds OpenTabletDriver's 357 tablet configuration files, copied unchanged from [`OpenTabletDriver.Configurations/Configurations`](https://github.com/OpenTabletDriver/OpenTabletDriver/tree/a126f7b241e417399be6c6a760c0a9d4b987ecfd/OpenTabletDriver.Configurations/Configurations) at revision `a126f7b` (OpenTabletDriver contributors, LGPL-3.0-or-later). The executables embed them. `scripts/update-tablet-database.py` copies them, and `crates/otd-core/tablets/SOURCE` records their origin.

## Optional .NET compatibility bridge

The release's `data/compat` directory includes OpenTabletDriver.Plugin 0.6.7 (OpenTabletDriver contributors, LGPL-3.0-or-later; [source](https://github.com/OpenTabletDriver/OpenTabletDriver/tree/736003ed72c8bbb28033b039d5a0bb76c344145c)), Newtonsoft.Json (James Newton-King, MIT; [license](https://github.com/JamesNK/Newtonsoft.Json/blob/master/LICENSE.md)), and Microsoft/JetBrains dependencies listed in `compat/OtdCompat/packages.lock.json`. The .NET hosting library `nethost.dll` is distributed under Microsoft's MIT license; its SDK license and third-party notices are under `data/licenses`. The .NET runtime itself is not bundled. The compatibility bridge and sample native plugin source are in this repository; the developer sample plugin is not included in runtime downloads. The original third-party RadialFollow DLL is used unchanged for tests and is not included in releases; users retain their original plugin and its licensing terms.

## Windows application icon

`resources/opentabletdriver.ico` is this project's blue tablet icon, generated from the native panel's earlier drawing in [v0.15.5 canvas.rs](https://github.com/z9a17/opentabletdriver-rust/blob/v0.15.5/src/ui/canvas.rs) by `scripts/generate-app-icon.py`. It is embedded in both Windows executables and used by the native panel, under this project's GPL-3.0-only license.

## Platform runtime licenses

Release packages include Rust's standard-library dependency notices and license texts in `data/licenses`. The static Linux package also includes musl's COPYRIGHT from [musl 1.2.5](https://git.musl-libc.org/cgit/musl/tree/COPYRIGHT?h=v1.2.5), obtained from [its source mirror](https://github.com/ifduyue/musl/blob/v1.2.5/COPYRIGHT). GNU Windows cross-builds include the MinGW-w64 runtime notices and GCC Runtime Library Exception. The 0.15.0 Windows build uses GCC 16.2.0 and MinGW-w64 14.0.0; their corresponding sources are available from [GCC](https://gcc.gnu.org/git/?p=gcc.git;a=tree;hb=releases/gcc-16.2.0) and [MinGW-w64](https://github.com/mingw-w64/mingw-w64/tree/v14.0.0). macOS packages link Apple system frameworks without redistributing the SDK or framework libraries.
