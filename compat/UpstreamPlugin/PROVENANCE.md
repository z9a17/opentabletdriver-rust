# OpenTabletDriver.Plugin 0.6.8 source compatibility host

The SDK source is copied from OpenTabletDriver v0.6.8-rc1, commit
`126f4828564abb673d39e97a093fd815aab335c2`, under LGPL-3.0-or-later.
The upstream public assembly identity and API are retained. The local project
targets net10.0 and pins the SDK's package dependencies.

Unlike an unchanged upstream NuGet assembly, this source build supplies default
interface implementations for members added in 0.6.8. This allows an unchanged
0.6.7 plugin implementing the older interfaces to load without requiring missing
methods before it has requested any new operation. Unsupported legacy operations
must fail explicitly rather than pretend hardware support. The implementation
record in `../../docs/parity/UPSTREAM_0.6.8_RC1.md` describes the selected policy
and validation limits.

The host also restores the removed `Area()` constructor with its original zero
defaults and `PropertyAttribute.DisplayName` public setter, preserving methods
referenced by original plugin binaries. Parameterless `TabletConfiguration`
construction retains its former initialized Specifications and digitizer list.
`SetsRequiredMembers` annotations on the original parameterless data constructors
(Area, TabletConfiguration, TabletSpecifications, TabletReference, LogMessage and
SerializedDeviceEndpoint) retain source construction by the copied older host;
the original null/default values are preserved where applicable. New SDK fields
and ordinary constructor/serialization behavior otherwise remain available.
