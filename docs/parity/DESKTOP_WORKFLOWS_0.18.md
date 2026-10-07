# Windows desktop workflow implementation

Pinned behavior reference: original OpenTabletDriver 0.6.7 at `736003ed72c8bbb28033b039d5a0bb76c344145c` (`OpenTabletDriver.UX/Windows/Greeter`, device string reader, settings and preset menu handlers).

The native panel now exposes a six-page first-setup guide and Help > Show guide, using shared theme, high contrast, DPI/font and control painting. It never runs onboarding for a tray-only launch. These additions have source review only; no GUI, daemon, plugin or hardware was launched.

File menus explicitly load/export original JSON collections, save new original JSON presets, and apply a whole original collection through the daemon's private Console service. Normal Save/Apply still target the selected native profile. Export and preset creation use guarded create-new storage; load and async completion retain newer edits/selection. A pending collection operation blocks competing profile Save/Apply but permits Stop and panel close. Original preset selection performs actual collection SetSettings. Native preset selection applies its unsaved draft when already attached and does not start an unattached driver.

Presets no longer share command IDs with device sessions. Native TOML and original JSON choices share the existing preset directory, retain distinct labels/extensions, and are paginated rather than hiding entries beyond 64.

The device string dialog selects known or unknown enumerated devices, permits decimal VID/PID and any index 0..255, dumps 1..255, and optionally pauses for reconnect/retry on failure. Requests run in cancellable background workers through the owned native daemon, with bounded replies and output, query generations and window-instance tokens. Closing the dialog cancels every outstanding worker; it does not start or stop a driver.

Managed output/contact/button assignment also offers the actual registered classes, including shipped original classes, instead of requiring their DLL to appear as an installed package. Registry setup runs only after explicit selection on a background worker. Existing typed editors preserve missing/null settings and guard selection/revision across inspection and modal loops. Filter reorder, raw JSON and per-property reset controls remain absent as requested by the owner.

No tests, format/lint/check suites, distribution builds, UI or hardware verification were run for this source slice. The release integrator compiles the actual distributions. Managed provider/plugin execution, hardware and original-client qualification remain separate evidence.
