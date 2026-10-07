# OpenTabletDriver.Desktop 0.6.7

All C# source files in this directory are copied unchanged from
https://github.com/OpenTabletDriver/OpenTabletDriver/tree/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.Desktop
and retain upstream copyrights and LGPL-3.0-or-later licensing (LICENSE).

The local project supplies the upstream net8.0/0.6.7 assembly identity and
references sibling Core through the pinned source-host project and Native/
Configurations through their exact original NuGet 0.6.7 packages. All other package versions are the pinned upstream project versions.
Original concrete Core readers consume host-owned tee streams; no duplicate
physical endpoint reader is opened. See ../UpstreamCore/PROVENANCE.md.
