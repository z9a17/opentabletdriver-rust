# OpenTabletDriver Rust 0.18.2

This release combines the Windows interface cleanup, reliability audit fixes,
and redesigned blue tablet-and-pen artwork.

- Four everyday Windows tabs: **Area, Filters, Pen and Buttons**. Tools,
  performance settings and diagnostics remain available through shorter menus
  with Back navigation. Binding menus group related actions.
- All eight PTH-660 ExpressKeys fit in the panel. Plugin-manager columns fit
  its window, empty Tools actions are disabled, and unchanged status/debugger
  views and tray tooltips avoid redundant updates.
- Windows readers alternate ready pen and auxiliary endpoints, retain completed
  buffers until consumed, keep overlapped storage stable, and prioritize Stop.
  Worker shutdown no longer retains a receiver that can block its joining worker.
- Shared report streams account for loss through the delivered batch prefix
  and no longer mistake an in-flight successful write for a dropped report.
  Mapping rejects overflowing bounds and nonfinite derived transforms.
- Managed plugin cleanup releases leases even when disposal or event unsubscribe
  throws. Startup and early validation failures retain their cleanup ownership.
- Unix descriptor arithmetic rejects overflow, and mailbox race rechecks honor
  request expiry and retirement. Windows test deployment detects locked payloads
  before replacing files.
- New artwork is embedded in both Windows apps and the original Linux/macOS
  frontends, with a simpler icon for small Windows title-bar/tray sizes. The
  README and setup guides now reflect the current interface and evidence limits.

Four packages are built from one clean merged commit: Windows x64 (MSVC),
Linux x64 (GNU), macOS Intel x64 and macOS Apple Silicon arm64. Archive contents,
architecture, embedded Windows icons, source provenance, managed dependency and
license closure, and SHA-256 checksums are verified before publication.

The preceding audit passed 235 core tests plus targeted reader, shutdown,
stream, mapping, managed-cleanup, UI, deployment and parity fixtures. Authorized
Windows review with a connected PTH-660 checked detection, filter Save selection,
menus, button layouts, light/dark plugin-manager appearance, repeated debugger
opening and orderly shutdown. Original settings were restored and hash-verified.
See the [audit and its exact validation boundaries](https://github.com/z9a17/opentabletdriver-rust/blob/v0.18.2/docs/BACKGROUND_AUDIT_2026-10-13.md).

No separate format/Clippy/test/build-check suite or CI was run during release
preparation. Required release binaries were compiled and their packages checked;
the final logo/version integration was not launched on a desktop. Earlier audit
results apply to the reviewed implementation, not an exhaustive final-release
test pass.

Physical pen feel and end-to-end latency, held-input unplug/reconnect,
sleep/wake, Windows Ink drawing, the unchanged third-party plugin corpus, and
native Linux/macOS execution remain acceptance work. Idle CPU observations did
not show a clear improvement, and this release makes no measured latency claim.
Linux requires glibc 2.31+ and GTK3/.NET 8 for its original frontend. macOS
packages require macOS 11+, are not Developer ID signed or notarized, and need
the matching .NET runtime for the original frontend. Native-only input with native display discovery or explicit `--screen` does
not require .NET; generic Wayland display discovery uses .NET. Preserve saved
settings and plugins when upgrading.
