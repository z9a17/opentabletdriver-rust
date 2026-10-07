# Windows compatibility workflows in 0.17.0

This release extends Windows device, plugin and daemon workflows against
OpenTabletDriver 0.6.7 (`736003ed72c8bbb28033b039d5a0bb76c344145c`).
The [current audit](parity/PARITY_AUDIT_2026-10-07.md) retains unfinished
acceptance work. Full OpenTabletDriver parity is not certified by this release.

## Independent physical tablets

**Tablets > Device sessions** lists physical session IDs, models and states.
Choose the session whose settings you want to edit. Unsaved or invalid edits
block selection until handled. Save persists that physical tablet's profile and
also requests a guarded replacement when its worker is active. Apply requests
a replacement without persisting it. Per-device Start/Stop
controls leave other sessions running. Global Stop driver stops all sessions.

When another client applies changed settings that are not saved, the panel
adopts them as a runtime draft requiring Save. Recoverable startup defaults
stay clean. An unchanged runtime draft can become clean only after an owned
saved match; intervening local edits remain intact.

Physical profiles live under the Rust data directory's `devices` folder. Paths
use a hash of the normalized device key. Saved revision and file digest guards
prevent overwriting an external edit or applying to a replacement session.
Path-based identity can change if a device has no persistent identifier.
Two same-model tablets can have separate Rust profiles; upstream JSON profiles
keyed only by tablet name cannot represent different profiles for that model.

The CLI uses IDs returned by the current daemon:

```text
opentabletdriver-rust.exe devices list
opentabletdriver-rust.exe devices profile device-1
opentabletdriver-rust.exe devices save device-1 new-profile.toml
opentabletdriver-rust.exe devices apply device-1 edited-profile.toml
opentabletdriver-rust.exe devices persist device-1 edited-profile.toml
opentabletdriver-rust.exe devices stop device-1
opentabletdriver-rust.exe devices start device-1
```

`save` exports a runtime snapshot to a new file (`--replace` is explicit).
`persist` saves to the physical tablet's guarded profile without activating it.
`apply` changes the runtime; `start` reloads that tablet's saved profile.
An accepted operation can still be pending: inspect session state and generation.

## Managed plugin settings

Output mode, tip/eraser and other binding editors expose a managed-plugin choice.
Select a trusted installed DLL and an eligible class, then edit its declared
properties in the themed dialog. Boolean, enum and scalar fields use typed
controls. Omitted, null, unknown and structured settings remain distinct;
unsupported structured values are retained without pretending to be editable.
Async results are discarded if the profile, edits or window lifetime changed.

The bridge uses the actual pinned `OpenTabletDriver.Plugin.dll` and
`OpenTabletDriver.Configurations.dll`. Supported unchanged IOutputMode,
IBinding and IStateBinding classes receive configured tablet properties,
actual opened identifiers and available pointer/keyboard services.
Managed output/binding faults request held-output cleanup. Trusted plugins
execute in process; constructors and private side effects are outside
native transaction rollback.

Explicit original-settings import and runtime startup resolve eligible installed
filter, tool, output and binding stores through a retained registry generation.
Disabled stores stay disabled; a missing Enable field means false. Enabled
managed stores with missing/null Settings are rejected as upstream does.
Source JSON and store extensions are preserved. Startup loads CLR only when
managed consumers or matched custom-parser candidates need it. Passive
discovery and settings-only inspection use cached metadata without starting CLR.
Original Desktop/core/device-provider dependencies still need adapters and
per-binary qualification. A native port does not prove unchanged-DLL support.

Installed unchanged `IReportParser` classes can supply a missing configured
parser at explicit startup. Managed report consumers receive the parser's
original concrete report and its original `Raw` data through a checked,
single-use identity. Each opened endpoint owns its parser state. Native-only
profiles retain the native decoder; passive discovery uses cached metadata.

## Transport and diagnostics

Windows supports HID and WinUSB discovery, initialization and input reads.
WinUSB cancellation drains outstanding IO before buffers and handles are freed.
Configured strings/report lengths are matched; ambiguous input pipe selections
fail. Actual tablet/firmware validation remains required.

The debugger can record raw reports at observed read-completion rate separately
from the sampled visualizer. This covers selected opened sessions; losses before
the read tap are not measured. Bounded storage reports missing packets explicitly. Decoded
continuity begins at capture start and resets across gaps. Independent upstream
RPC subscriptions do not steal the panel's capture lease.

See [upstream RPC compatibility](UPSTREAM_RPC.md) for the opt-in original-client
endpoint, 19 method mappings, four events and explicit differences. Typed logs
retain original timestamps, severity, stack traces and notification flags.
Settings exports reconcile actual managed stores with original identities/order.
Update ownership drains device/tool resources before checksum-verified
replacement and owned daemon exit.

## Validation

The release procedure compiles Windows manual packages and all four archives,
then inspects them through the repository release scripts. The manual package is copied
to `E:/OTD RUST TEST` with exact source metadata; extras and settings are preserved.
No app, daemon, plugin, tablet input, game or OBS session is launched.

Fixtures were authored but not run. Owner-disabled CI, format/Clippy/test/build
check suites remain unrun. Native Linux/macOS checks are deferred; cross-builds
do not establish native operation. Live Windows UI, hardware, original clients,
unchanged plugin corpus and osu!lazer/OBS feel still require separate evidence.
