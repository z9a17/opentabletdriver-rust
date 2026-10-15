# OpenTabletDriver Core: 0.6.7 baseline with selected 0.6.8-rc1 changes

Copied from OpenTabletDriver/OpenTabletDriver revision
736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver, LGPL-3.0-or-later.
The original assembly name and namespaces remain; the local assembly version is
now 0.6.8 and the target framework is net10.0. This is not a wholesale RC checkout.
This helper assembly is source-modified; third-party plugin DLLs remain unchanged.

The retained host adaptations to that baseline are:

- `Driver.cs`: internal `PublishHostedTrees` publishes original concrete trees
  over selected native readers, hooks newly attached tree disconnections,
  disposes removed devices and raises actual TabletsChanged notifications.
  `HostedDisconnected` removes the exact disconnected tree and republishes.
- `HostedDeviceEndpoint.cs`: added internal `IHostedDeviceEndpoint` marker
  identifies tee endpoints whose physical initialization belongs to native code.
  `InputDevice.cs` skips duplicate hardware initialization for that marker only.
- `InputDeviceTree.cs`: OutputMode assignment uses a synchronized backing field
  and internal `HostedOutputOwnership` admission before assignment. Reassigning
  the same object is a no-op. `HostedRetire` marks the tree retired before
  serialized output cleanup; `HostedShouldDisposeOutput` distinguishes output
  instances whose disposal belongs to another retained owner, preventing double
  disposal. Retirement clears the output, rejects later nonnull assignment and
  suppresses `HandleReport` delivery into the retired output pipeline.
- `Devices/DeviceReader.cs`: internal `HostedReportScope` surrounds parsing and
  both normal/raw report callbacks, rejecting synchronous I/O/ownership waits
  on the reader's own callback. Internal `HostedCompletion` records worker exit
  for cold scoped retirement (an unstarted worker is already complete).
  `Main` signals completion in a nested finally after attempting Connected=false,
  including when a disconnect subscriber throws. Native tool/update completion
  can therefore wait for managed parser/report callbacks to finish.
- `Devices/RootHub.cs`: internal endpoint transformation/publication routes actual
  custom hub endpoints through the native sole-reader broker. `HostedDispose`
  marks the hub retired, unhooks notifications, clears host callbacks and cached
  endpoints without enumerating disposed scopes; late notifications are ignored.
- `ComponentProviders/DeviceHubsProvider.cs`: an internal constructor accepts the
  actual host-supplied hub objects instead of opening built-in physical hubs again.
- `Instance.cs`: internal `HostedDispose` releases its named mutex on the original
  owner thread and removes the exact retained instance; shared owned-instance
  list accesses are synchronized. Hosted RPC retains the actual Instance until
  listener/client/provider retirement completes.

## Selective RC adoption

Source reference: [OpenTabletDriver v0.6.8-rc1, `126f4828564abb673d39e97a093fd815aab335c2`](https://github.com/OpenTabletDriver/OpenTabletDriver/tree/126f4828564abb673d39e97a093fd815aab335c2).

- `Driver.cs` caches each endpoint's string results, including null, for a
  detection attempt, materializes matching results once, and checks the
  `HID_REPORTS` predicate. A new detection attempt clears the string cache.
- `Devices/DeviceReader.cs` parses once, delivers raw observers before ordinary
  filtered observers, recovers from individual packet index failures and logs
  raw report bytes, while retaining hosted callback/completion scopes.
- `Devices/WinUSB/WinUSBInterface.cs` rejects Google fastboot devices as well as
  the existing Quest exclusion before querying interfaces.
- `Devices/HidSharpBackend/HidSharpEndpoint.cs` supplies explicit feature/output
  initialization helpers with owned stream disposal for the new SDK members.

The local project references the vendored RC `../UpstreamPlugin` SDK and exact
Configurations/Native NuGet 0.6.8 packages. Both packages identify the same RC
source commit above. The Plugin SDK has documented legacy-interface defaults;
see [its provenance](../UpstreamPlugin/PROVENANCE.md). Other dependency versions
are fixed in the project/lock files, rather than asserted unchanged from 0.6.7.
The host retains internal access for OtdCompat and native-owned tee streams;
these adaptations do not add a second physical reader. Existing copyrights and
LGPL-3.0-or-later `LICENSE` are retained. Compilation, unchanged-plugin loading
and live input are separate validation steps, not established by this record.
