# OpenTabletDriver 0.6.7 implementation completion

Version 0.18 completes the remaining source work against OpenTabletDriver 0.6.7
at `736003ed72c8bbb28033b039d5a0bb76c344145c`. This is an implementation report.
It does not certify the unrun plugin corpus, application, platform or hardware
acceptance gates. The owner explicitly deferred those checks.

| Area | Delivered implementation | Acceptance still to run |
| --- | --- | --- |
| Devices and parsers | Independent primary/auxiliary readers and per-device generations; HID, Windows WinUSB and original custom-hub endpoints; native and original parser routing | Hardware families/transports, hotplug, sleep, malformed reports and simultaneous tablets |
| Original services | Actual pinned Driver, InputDeviceTree, InputDevice, RootHub, DeviceHubsProvider, DesktopPluginManager and platform services; owned raw tees and existing-handle writes/features/strings | Unchanged third-party constructor, reader, device-hub and provider binaries |
| Managed pipeline | Concrete report types and synchronous stage traversal; bounded owned background emissions, timer scheduling and generation-scoped cleanup | Async ordering, retained reports, overflow, disposal and the exact unchanged DLL corpus |
| Input ownership | One keyboard/button domain per platform; native and original holders cannot release each other; acknowledged releases survive errors and have idle retries | Held-input failures, native/managed overlap and OS application delivery |
| Tools and plugin manager | A global tool owner independent of tablet attachment; actual registry contexts, constructor defaults, install/update/remove and exact retirement receipts | Reload, dependency isolation, failed constructors and external-driver prerequisites |
| Settings and presets | Original full collections, disconnected rows, defaults, import/export and native/original presets; guarded runtime apply and foreground preset actions | Original-client round trips, same-model selections, storage recovery and user workflows |
| Windows panel | Original collection commands, first-setup guide, device-string reader, original diagnostic file/clipboard output, typed built-in/plugin selectors and paginated tray presets | UI, focus, accessibility, DPI, theme/high contrast and asynchronous completion |
| Console and daemon | Original command names and parameters, persistent clients, original RpcHost/Instance plus the native control channel | Unchanged original clients, connection/event ordering and process cleanup |
| Linux and macOS | Native multi-device daemons, Bluetooth HID, generic Wayland fallback, macOS pointer attributes/click counts, shared managed services, custom/auxiliary sources and original Gtk/Eto frontends, original Console and launcher | Native OS permissions, desktop behavior, hardware, runtime dependencies and macOS signing |
| Updates and packages | Reader/tool retirement before replacement, matching update receipts and final response flush; four packages from one merged commit with dependency/license/hash provenance | Installed update/recovery behavior and native launch checks |

The original public assembly identities are retained. Internal hosting patches
are documented in [Core provenance](../../compat/UpstreamCore/PROVENANCE.md),
[Desktop provenance](../../compat/UpstreamDesktop/PROVENANCE.md), and
[UX provenance](../../compat/UpstreamUX/PROVENANCE.md). Rust continues to own
physical readers and native output. The original daemon is not run alongside it.
Original managed readers consume owned tees from the existing physical reader.

## Using the completed workflows

On Windows, normal foreground `run` now owns the same services as the panel's
daemon. Original clients can use the explicitly enabled original endpoint:

```text
opentabletdriver-rust.exe daemon --upstream-rpc
opentabletdriver-rust.exe daemon --upstream-rpc --upstream-pipe OpenTabletDriverRust.Compat
```

The default explicit compatibility endpoint is `OpenTabletDriver.Daemon`.
The private native Console endpoint remains separate. An ordinary native-only
profile does not initialize CoreCLR unless a managed feature is requested.

Linux and macOS packages include the original graphical frontend and Console.
From the extracted directory:

```sh
./opentabletdriver-rust-linux ui
./opentabletdriver-rust-linux original-console --help
./opentabletdriver-rust-macos ui
./opentabletdriver-rust-macos original-console --help
```

These are commands for the owner to run later; they were not executed during
this implementation. The frontends and unchanged managed plugins need the
CPU-matching .NET 8 runtime. Linux Gtk additionally needs GTK3; explicit network
operations need system curl. Generic Wayland discovery uses the original
read-only display provider and needs .NET; native Hyprland/Sway/X11 discovery
and an explicit `--screen` remain CLR-free. The Linux distribution uses GNU dynamic linking,
with a glibc 2.31 build baseline. macOS packages target 11.0+, are unsigned and
not notarized, and need Input Monitoring/Accessibility permissions. The pinned
original macOS driver has mouse/keyboard output, not an Artist Mode or Windows
Ink drawing contract. Plugins keep their original platform/driver prerequisites.

## Deliberate project differences and boundaries

The owner requested the native Windows panel, its accent/theme choices, an
Experimental tab, removal of filter reorder/raw/per-property reset controls,
and panel-close shutdown rather than the original detach behavior. Those
presentation choices remain. Original JSON collections retain the complete
saved store data even when the native editor cannot project it.

Native TOML adds physical-device profiles and physical key bindings. The
original format groups profiles by tablet name, so differing profiles for two
same-model devices cannot be projected into a single faithful original
collection. Nonrepresentable native extensions report an explicit projection
error. Original bindings retain their platform's original logical key semantics.
Imported original stores and native physical key bindings are separate concepts.
Rust's explicit `None` binding remains a no-op. The pinned macOS dictionary's
literal `None` aliases the A key; that existing Rust difference is documented
in the Unix implementation report.

Report queues and snapshots have documented capacity, generation, overflow and
retirement rules. Overflow is an explicit pipeline failure followed by input
cleanup. Arbitrary live execution/device handles inside a plugin's private
report fields cannot be copied into an owned asynchronous snapshot. See
[async ownership](P05_ASYNC_EMISSIONS.md), [device services](P07_DEVICE_COMPLETION.md),
[desktop workflows](DESKTOP_WORKFLOWS_0.18.md) and
[Unix implementation](UNIX_RUNTIME_COMPLETION.md).

No format/Clippy/test/build-check suites, CI, driver/daemon/UI/plugin/game/OBS
launches, live tablet runs or native Linux/macOS checks were run for this release.
Actual distribution compilation, source review and archive/provenance
verification are separate evidence. The manual Windows package is copied to
`E:/OTD RUST TEST` before merge without launching it or changing saved settings.

The [evidence ledger](EVIDENCE_LEDGER.md) retains its existing acceptance states
and historical results. No new passing integration or hardware evidence is
invented. G6/full verified parity remains open until the owner performs the
deferred acceptance work. No input-to-photon or osu!lazer/OBS improvement is
claimed from compilation or source inspection.
