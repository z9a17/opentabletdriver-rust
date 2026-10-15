# OpenTabletDriver.Desktop: 0.6.7 baseline with selected 0.6.8-rc1 changes

C# source files in this directory originate from
https://github.com/OpenTabletDriver/OpenTabletDriver/tree/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.Desktop
and retain upstream copyrights and LGPL-3.0-or-later licensing (LICENSE).

The local project retains the `OpenTabletDriver.Desktop` assembly name and
namespaces, targets net10.0 and uses assembly version 0.6.8. Core is the sibling
modified source host; Plugin is the vendored RC SDK, not a substitute shim.
Native and Configurations are exact NuGet 0.6.8 packages identifying RC source
commit `126f4828564abb673d39e97a093fd815aab335c2`. Current dependency versions are
pinned in the project and lock files. This directory is a selectively updated
0.6.7 source host, not the complete RC Desktop source tree.
Internal host patches: DiagnosticInfo accepts actual native version/build
provenance only when managed assembly metadata is absent. RpcHost.Run owns and
drains client tasks during cancellation; connected pipe disposal is guaranteed.
Internal PluginContext/DesktopPluginContext constructors attach the real loaded
collectible registry contexts without loading DLLs twice. GetLoadedPlugins then
returns actual context objects with actual Assemblies/Directory/GetMetadata.
Original public type names and existing host integration remain; helper bytes
and assembly version are modified.

Original concrete Core readers consume host-owned tee streams; no duplicate
physical endpoint reader is opened. See ../UpstreamCore/PROVENANCE.md.

DesktopPluginManager internal hooks route direct Load/Install/Download/Update/
Uninstall/Unload/type-removal to the authoritative native installation and leased
registry owner. Context reads return immutable generation snapshots. RpcHost wraps
the original header-delimited handler to defer update retirement until its exact
success response completes WriteAsync (write and flush), keyed by RequestId.

DesktopInterop has an internal cold display factory using exactly its original
platform provider selection. The host can enumerate a fresh read-only screen
without changing the public cached VirtualScreen or constructing input services.
The fallback exports actual aggregate/child geometry; native conversion rejects
nonfinite, empty, excessive monitor counts and out-of-range rectangles.

## Selective RC changes

From [v0.6.8-rc1, `126f4828564abb673d39e97a093fd815aab335c2`](https://github.com/OpenTabletDriver/OpenTabletDriver/tree/126f4828564abb673d39e97a093fd815aab335c2):

- `Binding/WheelBindings.cs` preserves accumulated wheel motion across
  stationary reports instead of clearing both directions.
- `Interop/Input/Keyboard/MacOSVirtualKeyboard.cs` maps `Equal` to the normal
  equals key, keeping `KeypadEqual` as a separate mapping.
- `Reflection/PluginSettingStore.cs` adds JSON round-trip `Clone()` for isolated
  binding editor snapshots and cancellation.

The previous host adaptations above are retained. These source changes and
.NET 10 retargeting do not establish application, unchanged-plugin or hardware
acceptance; see [the adoption record](../../docs/parity/UPSTREAM_0.6.8_RC1.md).
