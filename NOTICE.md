# Radial Follow source notice

`src/radial_follow.rs` is a Rust port of AbstractQbit's `RadialFollowCore.cs` and `RadialFollowSmoothingTabletSpace.cs` from the [RadialFollow 0.3.0 release](https://github.com/AbstractQbit/AbstractOTDPlugins/tree/0.3.0/RadialFollow). The original plugin is by AbstractQbit and is licensed under GPL-3.0-only. This port keeps the original tablet-space filter name, OpenTabletDriver configuration path, setting names, curve, and 50 ms reset behavior. Its changes are the Rust representation, Windows `Instant` timing, and direct integration into this driver without the .NET plugin interface.

The pre-existing driver files were released under LGPL-3.0-only. Their license text is retained in [LICENSE.LGPL-3.0](LICENSE.LGPL-3.0). The combined 0.3.0 executable includes the GPL-3.0-only filter and is conveyed under [GPL-3.0-only](LICENSE).

## Relative output source notice

`src/relative.rs` ports behavior from OpenTabletDriver contributors' LGPL-3.0-licensed `RelativeOutputMode.cs` and `WindowsRelativePointer.cs` at revision `fdeaa7b0c6d6f5260f511f19fb693ed33524af4e`. [Relative mode documentation](docs/RELATIVE_MODE.md) links the original sources and records the Rust implementation's changes. This module uses LGPL-3.0-only; the combined executable remains GPL-3.0-only.
