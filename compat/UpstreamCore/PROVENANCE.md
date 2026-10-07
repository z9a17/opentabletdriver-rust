# OpenTabletDriver Core 0.6.7 source host

Copied from OpenTabletDriver/OpenTabletDriver revision
736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver, LGPL-3.0-or-later.
Public assembly name/version and public contracts remain original. This helper
assembly is source-modified; third-party plugin DLLs remain unchanged.

Internal host additions: Driver.PublishHostedTrees attaches original concrete
InputDevice trees to already selected native readers; IHostedDeviceEndpoint marks
streams whose physical initialization belongs to the native owner; InputDevice
skips duplicate initialization for that marker only; InputDeviceTree.OutputMode
calls an internal ownership handoff before assignment; RootHub.HostedDispose
unhooks actual hub events without enumerating disposed native scopes; internal
RootHub enumeration transform/publication routes actual custom hub endpoints
through the native sole-reader broker without changing public contracts; internal
DeviceReader callback scopes reject synchronous I/O/ownership waits on its own
report callback. Hosted tree publication keeps actual disconnect notifications. All other source is pinned.
DeviceHubsProvider has an internal constructor receiving actual native hub
objects instead of opening built-in physical hubs again.
The project references exact upstream dependency versions, and grants only
OtdCompat internal access. No physical hub or second physical handle is created.
