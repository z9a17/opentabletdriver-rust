# OpenTabletDriver Core 0.6.7 source host

Copied from OpenTabletDriver/OpenTabletDriver revision
736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver, LGPL-3.0-or-later.
Public assembly name/version and public type/member signatures remain original.
This helper assembly is source-modified; third-party plugin DLLs remain unchanged.

The tracked upstream C# source differs from the pin in these host adaptations:

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

Apart from those listed adaptations, tracked upstream C# source remains pinned.
The local project retains the original net8.0/0.6.7 assembly identity, references
exact upstream dependency versions and grants OtdCompat internal access. Hosted
physical endpoint readers use native-owned tee streams; these host adaptations
do not create a second physical handle or duplicate built-in physical hub.
