# Windows upstream RPC compatibility

Start an idle Rust daemon with `daemon --upstream-rpc`, or add `--background`
for a hidden daemon. The opt-in endpoint is `OpenTabletDriverRust.Compat`.
An original fixed-name client requires
`daemon --upstream-rpc --upstream-pipe OpenTabletDriver.Daemon`.
Creation fails if that name is already owned. This command does not stop an
original daemon or change options on an existing Rust daemon.

The persistent byte pipe uses UTF-8 JSON-RPC 2.0 with
`Content-Length: <byte count>\r\n\r\n` framing. This matches the default
[StreamJsonRpc handler](https://github.com/microsoft/vs-streamjsonrpc/blob/main/src/StreamJsonRpc/JsonRpc.cs)
and [strong proxy event conventions](https://microsoft.github.io/vs-streamjsonrpc/docs/proxies.html).
The contract is pinned to
[IDriverDaemon at 736003e](https://github.com/OpenTabletDriver/OpenTabletDriver/blob/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.Desktop/Contracts/IDriverDaemon.cs):
19 methods and four events. Native protocol v2 retains its existing framing.

There are at most four simultaneous clients, 4 KiB of headers and 256 KiB per
body/response; string request IDs are limited to 4 KiB. Oversized method results
return correlated errors. Idle reads expire after 120 seconds; a header/body must finish
within five seconds after its first byte. Writes have a five-second budget.
Cancellation drains pending overlapped IO before releasing buffers. Pipe ACLs
grant the current user and SYSTEM and reject remote clients. Framing and event
delivery run on compatibility workers, outside report/input threads. RPC does
not initialize .NET. Its native calls check the actual connected server PID
before sending, so startup/shutdown races cannot dispatch into another daemon.
JSON-RPC batch messages are rejected.

| Methods | Current behavior |
| --- | --- |
| GetDevices | Actual HID/WinUSB endpoint inventory, strings, lengths and openability. |
| GetTablets | Connected device-session snapshots, actual configurations and identifiers. |
| GetSettings | Actual connected device profiles reconciled into the original OTD document. Representable standalone explicit-area/relative profiles receive canonical known stores. Resolved managed DLL paths require matching original identities/order. Different tools across devices, differing profiles for same-model physical tablets, native pixel-span crops/hardware-tip contact and ambiguous stores return errors. |
| SetSettings | Preflights all detected named profiles, preserves inactive rows and generates pinned defaults for missing detected names. Applies through guarded per-device receipts, waits committed generations, and rolls back only accepted generations it still owns on failure. Newer/pending peer operations are preserved and reported as partial/uncertain failure with Resynchronize. Group atomicity is not claimed. Unsupported active stores and idle collection storage remain explicit errors. |
| GetCurrentLog, WriteMessage | Bounded native log snapshot and native log append. |
| InstallPlugin, UninstallPlugin, DownloadPlugin | Existing catalog installation/removal/download transactions; removal must uniquely identify an installed name or folder. Network traffic uses `src/download.rs`. Installation does not imply live DLL reload. |
| RequestDeviceString | Explicit VID/PID/index HID string request. |
| GetApplicationInfo | Actual native data/settings/plugin/preset/temp paths and configured external tablet directory. Cache/backup/trash directory fields are null because Rust has no corresponding provider. LogDirectory is the data folder containing crash records; normal recent logs remain in memory. |
| GetDiagnosticInfo | Actual version, Windows version API result, environment, endpoint inventory, native log snapshot and current profile/state. Build Date is null because no build timestamp is recorded. |
| ForceResynchronize | Broadcasts the resynchronization event to compatibility clients. |
| DetectTablets | Owned D05 refresh waits for an actual discovery pass and returns its snapshot. Without an existing supervisor, returns an error; new sessions may still be preparing/starting. |
| LoadPlugins | Explicit unsupported error pending live plugin-manager reload. |
| ResetSettings | True pinned OTD defaults across connected running/stopped sessions, with actual digitizer/button/wheel specifications and guarded per-device apply/recovery. Uses upstream 1% contact thresholds, no filters, clipping, default adaptive bindings and 100 ms relative reset. Stopped devices remain stopped. |
| SetTabletDebug | Explicit unsupported error; latest native samples are not full-rate multi-tablet DeviceReport events. |
| CheckForUpdates | Actual Rust release service/version comparison, returning the pinned Version-string DTO or null when current. The checked release is retained in bounded daemon state. |
| InstallUpdate | Requires a checked update and a daemon reservation that drains all devices/tools and blocks competing starts/applies. Uses the existing checksum-verified update transaction, writes the RPC result before requesting owned shutdown, and recovers failed replacement before releasing the reservation. Successful replacement or uncertain recovery exits the old daemon. |

All methods accept positional parameters. Single parameters also accept named
parameters; aliases cover pinned interface/implementation name differences for
UninstallPlugin and SetTabletDebug. RequestDeviceString accepts vendorID/productID
or vid/pid. Notifications execute without replies, including on errors. Missing
providers return code -32004; invalid parameters -32602; unknown methods -32601;
operation failures -32000.

Message and TabletsChanged events poll actual native snapshots every 250 ms.
Message timestamps describe observation time, Group is `RustDaemon (observed)`
and Level is Info: native recent strings do not preserve upstream typed metadata.
Only retained logs are available, and a lagging client can lose older records.
The initial snapshot does not generate historical events. Resynchronize is sent
as one EventArgs argument. DeviceReport events are not emitted in this slice.
These event/provider differences mean full S03 parity is not complete.

Framing, malformed input, parameter handling, persistent fragmented messages and
managed store reconciliation have regression fixtures. They were written but
not executed. Source review is the current evidence; no test/check suites,
original-client integration, live daemon, hardware or UI validation was run.
Package compilation is performed separately by the release integrator.
