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
body/response. Idle reads expire after 120 seconds; a header/body must finish
within five seconds after its first byte. Writes have a five-second budget.
Cancellation drains pending overlapped IO before releasing buffers. Pipe ACLs
grant the current user and SYSTEM and reject remote clients. Framing and event
delivery run on compatibility workers, outside report/input threads. RPC does
not initialize .NET. JSON-RPC batch messages are rejected.

| Methods | Current behavior |
| --- | --- |
| GetDevices | Actual HID/WinUSB endpoint inventory, strings, lengths and openability. |
| GetTablets | Connected device-session snapshots, actual configurations and identifiers. |
| GetSettings | Strict original OTD export from the active configuration. Resolved managed DLL paths are reconciled only with matching original store identities/order; missing or ambiguous source returns an error. |
| SetSettings | One running tablet only. Rejects unsupported active import settings, validates before guarded restart, and waits for a committed generation and matching configuration. Timeout/failure does not pretend completion or retry a mutation. |
| GetCurrentLog, WriteMessage | Bounded native log snapshot and native log append. |
| InstallPlugin, UninstallPlugin, DownloadPlugin | Existing catalog installation/removal/download transactions; removal must uniquely identify an installed name or folder. Network traffic uses `src/download.rs`. Installation does not imply live DLL reload. |
| RequestDeviceString | Explicit VID/PID/index HID string request. |
| GetApplicationInfo | Actual native data/settings/plugin/preset/temp paths and configured external tablet directory. Cache/backup/trash directory fields are null because Rust has no corresponding provider. LogDirectory is the data folder containing crash records; normal recent logs remain in memory. |
| GetDiagnosticInfo | Actual version, Windows version API result, environment, endpoint inventory, native log snapshot and current profile/state. Build Date is null because no build timestamp is recorded. |
| ForceResynchronize | Broadcasts the resynchronization event to compatibility clients. |
| DetectTablets | Explicit unsupported error pending an owned rediscovery transaction. |
| LoadPlugins | Explicit unsupported error pending live plugin-manager reload. |
| ResetSettings | Explicit unsupported error pending actual OTD defaults across detected tablets. |
| SetTabletDebug | Explicit unsupported error; latest native samples are not full-rate multi-tablet DeviceReport events. |
| CheckForUpdates, InstallUpdate | Explicit unsupported error pending updater DTO/daemon exit ownership integration. Existing native update commands remain available. |

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
