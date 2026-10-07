# OpenTabletDriver.Desktop 0.6.7

C# source files in this directory originate from
https://github.com/OpenTabletDriver/OpenTabletDriver/tree/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.Desktop
and retain upstream copyrights and LGPL-3.0-or-later licensing (LICENSE).

The local project supplies the upstream net8.0/0.6.7 assembly identity and
references sibling Core through the pinned source-host project and Native/
Configurations through their exact original NuGet 0.6.7 packages. All other package versions are the pinned upstream project versions.
Internal host patches: DiagnosticInfo accepts actual native version/build
provenance only when managed assembly metadata is absent. RpcHost.Run owns and
drains client tasks during cancellation; connected pipe disposal is guaranteed.
Internal PluginContext/DesktopPluginContext constructors attach the real loaded
collectible registry contexts without loading DLLs twice. GetLoadedPlugins then
returns actual context objects with actual Assemblies/Directory/GetMetadata.
Public type/member/assembly identities remain pinned; helper bytes are modified.

Original concrete Core readers consume host-owned tee streams; no duplicate
physical endpoint reader is opened. See ../UpstreamCore/PROVENANCE.md.
