# P07 Windows managed driver services

Owner: managed_parity, branch parity/p07-driver-services, base 4595ee5.
Claim: P07 original Plugin/Core/Desktop provider services; native backend and
daemon lifetime integration belong to the release integrator, original Settings
collection transactions to desktop_parity. Implementation increment, not a full
P07 completion claim. Windows implementation is the current authorized scope.

Baseline: upstream OpenTabletDriver 0.6.7 revision
736003ed72c8bbb28033b039d5a0bb76c344145c. Provider contracts come directly from
Plugin/IDriver.cs, Components/*, Devices/* and Desktop/Contracts/IDriverDaemon.cs.
The complete Desktop C# source is vendored unchanged in compat/UpstreamDesktop;
its local project preserves the actual OpenTabletDriver.Desktop 0.6.7 identity
and replaces sibling source-project references with exact original NuGet packages.
Real restored lockfiles accompany both projects. Existing managed/native paths
are retained; no original native RootHub, Driver or duplicate reader is created.

Implemented source contracts:

- IDriver exposes published actual opened TabletReference objects and its change
  event, invokes native detection through the owner, and constructs real original
  parsers. IReportParserProvider owns generation leases and disposes its parsers.
- IDeviceConfigurationProvider exposes the real host configuration snapshot.
  Original concrete Configurations/Desktop configuration and parser helpers are
  also available; they read the pinned resources/configuration overrides or use
  the actual installed parser registry. They never open devices.
- IDeviceHub/IDeviceHubsProvider/ICompositeDeviceHub expose real cached native
  endpoint metadata and device changes. Optional unavailable metadata raises an
  explicit error. Cached device strings avoid owner requests; other strings use
  the native service queue. Dynamic managed hub attachment remains unsupported.
- Actual Desktop IDriverDaemon reads real Settings/application/device/tablet/log
  snapshots and dispatches asynchronous mutations to a native owner. A Task
  succeeds only after the backend completes. No blocking daemon RPC is added
  to a native report path. Construction/dependency/disposal callbacks may read
  snapshots; queued operations there fail explicitly to avoid own-transaction
  deadlocks. Synchronous Detect/string owner waits are prohibited from report
  and setup callbacks.
- Shipped original Desktop classes, including unchanged PresetBinding, appear
  in registry discovery even without a separate installed Desktop DLL. Original
  JSON presets use the real AppInfo.PresetManager and full Settings collections;
  they are distinct from the native per-device TOML preset extension. An admitted
  intentional self-apply retains its completion through replacement/disposal.
  Binding source owner/tablet metadata and admission-time daemon identity let
  the native transaction reject stale applies and suppress held-button reentry.
- Original PluginManager/DesktopPluginManager identities use owned services and
  installed original types. Returned objects retain registry generations. The
  manager clears the pinned ServiceManager dictionary rather than installing
  DesktopInterop factories for a second output owner. Direct nonvirtual upstream
  plugin install/load APIs are not intercepted by this adapter.

ABI v1: InstallHostServices accepts a C layout version/size followed by Cdecl
Request(op, scope, utf8, length, out_ticket), Poll(ticket, out, capacity), and
Release(ticket). Admission 0 means accepted, never applied. Completed replies
are retained `{ok,result}` / `{ok:false,error}` JSON; capacity retries never repeat
an operation. Limits: 64 outstanding tickets/queued operations, 4 MiB per payload
or reply, 8 MiB queued payload bytes, 60-second ticket ownership. IDs never repeat
across native owner replacement. Expired/released queued operations are skipped.
Host Drop stops admission, fails pending tickets, joins the executing backend,
then removes the global owner. The owning daemon must cancel its backend and
drop Host outside any native transaction lock; Publisher clones do not own
teardown. Snapshots carry a monotonically increasing version and actual upstream
JSON fields; absent values mean unavailable. Native-only profiles do not initialize
CLR or trigger the managed observer. Event polling is at most four times/second.

Remaining concrete gaps:

- Concrete upstream Core.Driver/InputDeviceTree/InputDevice/RootHub cannot be
  supplied without a real shared-reader integration. They return no fabricated
  successful empty instance. Custom hub attachment/native dispatch remains open.
- IDeviceEndpoint.Open/read/write/features are explicitly unsupported until a
  native reader-sharing and I/O owner lease exists; no second handle is opened.
- IDriverDaemon.DeviceReport/full-rate debug source events are not attached;
  subscribing or enabling this managed event source fails explicitly. Existing
  native/RPC full-rate capture remains separate and is not advertised as this event.
- Backend operations whose original native host implementation is unavailable
  return explicit errors (including updates/input prerequisites as applicable).
- AppInfo's unchanged upstream static initializers can create its default plugin
  and preset directories before host paths are assigned. Then the bridge binds
  the real native application paths and refreshes original JSON presets. No saved
  settings are rewritten by provider bootstrap.
- Returned original objects own their IDisposable contract; callers must dispose
  them and unsubscribe handlers. The retained registry lease follows reachability.

No tests, formatting, Clippy, build-check suites, managed/plugin execution, UI,
daemon, hardware or live output were run. Parent performed dependency restore
only to generate genuine lockfiles; actual package compilation belongs to the
release integrator. Runtime behavior remains for owner validation.

Packaging must ship the complete managed runtime closure from the restored
projects (including actual Core/Native/Desktop and their transitive assemblies),
updated compat/THIRD_PARTY_NOTICES.txt and the vendored source/license provenance.
Do not reuse the older seven-file compatibility allowlist.
