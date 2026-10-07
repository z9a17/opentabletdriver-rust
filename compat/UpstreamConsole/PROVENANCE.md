# Original console source

`Program.cs`, `Program.Commands.cs`, `Program.IPC.cs`, `Program.Misc.cs`,
`Extensions.cs`, and `CommandTools.cs` are copied unchanged from official
OpenTabletDriver commit `736003ed72c8bbb28033b039d5a0bb76c344145c` (0.6.7).
`LICENSE` is the exact license file at that revision.

The project targets net8.0 and references the retained original Desktop project
and its existing exact dependency versions. These project adaptations do not
replace the command implementation. The original client uses the original
`OpenTabletDriver.Daemon` instance and pipe; start the native daemon with that
explicit pipe or let the original UX launch the packaged native daemon launcher.

On Linux and macOS, the native `original-console` and `otd` aliases invoke the
packaged original `OpenTabletDriver.Console.dll` with `dotnet`; the .NET runtime
is required. Windows translates the original command surface through the native
console/daemon bridge and does not package or invoke this Console.dll. This source
project's original executable client is therefore a Linux/macOS package component.
Source integration is separate from package compilation and runtime validation.
