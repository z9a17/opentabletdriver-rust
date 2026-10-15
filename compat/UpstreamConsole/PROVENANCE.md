# Original console source with selective 0.6.8-rc1 adoption

`Program.cs`, `Program.Commands.cs`, `Program.IPC.cs`, `Program.Misc.cs`,
`Extensions.cs`, and `CommandTools.cs` originate from official
OpenTabletDriver commit `736003ed72c8bbb28033b039d5a0bb76c344145c` (0.6.7).
`LICENSE` is the exact license file at that revision.

The project now targets net10.0 and uses assembly version 0.6.8 with the original
`OpenTabletDriver.Console` name. It references the modified Desktop/Core source
hosts, vendored Plugin SDK and exact Configurations/Native NuGet 0.6.8 packages,
whose source commit is RC `126f4828564abb673d39e97a093fd815aab335c2`. Project
and lock files record the dependency closure. This is still a 0.6.7 source
baseline with selective changes, not an unchanged Console assembly or wholesale
RC source copy.

`Program.Commands.cs` and `Extensions.cs` adopt the RC wheel-binding output:
wheel button bindings, clockwise/counterclockwise assignments and activation
thresholds are included by `getbindings`. Other commands retain the existing
source implementation. The original client uses the original
`OpenTabletDriver.Daemon` instance and pipe; start the native daemon with that
explicit pipe or let the original UX launch the packaged native daemon launcher.

On Linux and macOS, the native `original-console` and `otd` aliases invoke the
packaged original `OpenTabletDriver.Console.dll` with `dotnet`; the .NET runtime
is required. Windows translates the original command surface through the native
console/daemon bridge and does not package or invoke this Console.dll. This source
project's original executable client is therefore a Linux/macOS package component.
Source integration is separate from package compilation and runtime validation.
