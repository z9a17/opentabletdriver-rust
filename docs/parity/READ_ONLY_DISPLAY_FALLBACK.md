# Read-only original display fallback

Generic Linux Wayland desktops can use the pinned Desktop display provider when
native Hyprland, Sway and X11 discovery cannot return a screen. Native discovery
success does not initialize the CLR. The Linux consumer owns the first cold
snapshot; this does not add a compositor event loop or report-thread discovery.

`dotnet::original_display_snapshot()` calls Cdecl
`OriginalDisplaySnapshot(output, capacity, refresh)`. Refresh enumerates one
fresh original display provider; capacity retries copy retained bytes on the
same thread. JSON contains `provider`, `virtual_screen` and `displays`, with
`index`, `x`, `y`, `width`, `height` for each rectangle. The aggregate object is
excluded by reference identity from child displays, preserving actual physical
children including index zero on the original macOS provider.

The internal Desktop factory uses exactly the pinned public VirtualScreen
provider selection. Only the display provider is constructed. It does not
construct a Driver, physical reader, pointer, keyboard, tablet or daemon
provider, and does not mutate the cached public VirtualScreen. Disposable
display connections are closed after serialization. The original Wayland
provider closes its read-only connection after its two registry roundtrips.

Native conversion rejects nonfinite/empty geometry, more than 256 monitors,
out-of-range i32 edges and spans which would overflow Rect width/height.
Coordinates round outward (floor origin, ceil far edge). Aggregate position
and dimensions preserve the pinned provider values, including Wayland's zero
aggregate position and independently reported child positions. Monitors are
sorted by native rectangle order; mirrored physical displays remain distinct.

Provider semantics come from the exact pinned
[DesktopInterop](https://github.com/OpenTabletDriver/OpenTabletDriver/blob/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.Desktop/Interop/DesktopInterop.cs)
and [WaylandDisplay](https://github.com/OpenTabletDriver/OpenTabletDriver/blob/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.Desktop/Interop/Display/WaylandDisplay.cs).
The internal factory modification is recorded in the vendored provenance.

Source was reviewed. Tests, check suites, compilation, compositor connections,
GUI, daemon and hardware execution were not run in this workstream. Package
compilation belongs to the release integrator; runtime acceptance is unrun.
