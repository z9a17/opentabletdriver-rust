# Release 0.16 startup and CPU affinity

Open `opentabletdriver-rust-ui.exe`. It opens the panel and starts the sibling
`opentabletdriver-rust.exe daemon` as a separate hidden process. If a daemon
already exists, the panel connects to it and keeps its active profile.
Concurrent launches still use the existing per-user daemon endpoint and
panel instance guard.

The panel launches the daemon even when Tablets > Start driver when the panel
opens is disabled or a profile failed to load. That daemon stays idle until
Start driver is requested. With the default launch preference and valid
settings, tablet input starts automatically. A daemon already observed
running or stopping is not revived by a queued autostart request.

Closing the panel still waits for tablet input to stop and release held
actions. Minimizing retains input. The independent daemon can remain idle
after panel close and is reused on reopening; CLI `shutdown` explicitly
terminates it after cleanup.

Use the updater's restart action when upgrading from an older release. It
waits for the old daemon to exit before opening the new panel. With a manual
upgrade, shut down the old daemon with its matching CLI first; an older
already-running daemon does not understand the new CPU settings request.

## Choose CPUs

Open View > Experimental settings. The window has separate GUI CPUs and
Driver CPUs fields. These numbers match Windows logical processor numbers,
starting at zero. A physical core may have more than one logical processor.

- `All` or a blank field lets Windows schedule the process on available CPUs.
- `2` pins that process to logical CPU 2.
- `0,2,4-7` allows logical CPUs 0, 2, 4, 5, 6 and 7.
- All CPUs fills both fields with `All`. Save and apply commits the change.

Save and apply validates both selections, asks the attached daemon to apply
its choice and save the file, then applies the GUI choice. It does not restart
the tablet worker or rewrite its profile. The Console reports completion or
failure. If saving fails, the daemon restores its previous affinity. A lost
IPC reply can follow an accepted save, so reopen the window to check persisted
choices before retrying. If GUI affinity fails after the daemon accepted the
save, the error says so; reopen the panel to retry the saved GUI choice.

Choices are stored in `%LOCALAPPDATA%\OpenTabletDriverRust\experimental.toml`,
or under `OTD_RUST_PORTABLE_DIR` in portable mode. The previous file is retained
as `experimental.toml.bak`. UI preferences remain in `ui.toml`, and tablet
settings remain in `driver.toml`. Corrupt scheduling settings produce a
Console warning rather than preventing driver control; the settings window
offers valid defaults that can replace them while retaining a backup.

Defaults do not pin the GUI. A newly launched daemon clears affinity inherited
from the GUI before applying its own saved choice. Explicit All resets a prior
selection. Third-party launchers and Windows job restrictions can still prevent
an affinity change; Windows failures are reported.

Pinning currently supports one Windows processor group, up to 64 logical CPUs.
On systems with multiple groups, keep both fields at All; the driver leaves
Windows group scheduling in place. Invalid or duplicate CPU numbers and
reversed ranges are rejected. This uses Windows
[process affinity](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-setprocessaffinitymask).
Affinity is experimental; this release makes no latency improvement claim.
Linux/macOS downloads retain their CLI behavior and have no settings window.

## Blue icon and validation

Both Windows apps embed the earlier blue rounded-tablet icon, with its white
active-area outline and center dot. Explorer, shortcuts, taskbar, tray and
the plugin manager share the resource. The icon generator and provenance are
documented in [resources/README.md](../resources/README.md).

Actual release builds and archive/architecture/source/icon/checksum inspection
are required for all four packages. The owner's pre-release
format/Clippy/test/build-check suite and CI were not run. Focused checks after
publication use an isolated test process for affinity and a fake, test-only
daemon endpoint for panel attachment. Interactive UI, real driver scheduling,
physical tablet input and native Linux/macOS execution remain unverified.
