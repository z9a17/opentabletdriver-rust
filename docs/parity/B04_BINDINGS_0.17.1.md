# B04 native toggle and preset bindings

Owner: control bindings agent, branch `parity/b04-toggle-presets`, base `4595ee5`.
Status: source implementation delivered; meaningful fixtures written and unrun. The owner requested
no validation execution. Full parity and live input behavior remain unverified.

Reference: OpenTabletDriver 0.6.7, commit `736003ed72c8bbb28033b039d5a0bb76c344145c`:
[PresetBinding](https://github.com/OpenTabletDriver/OpenTabletDriver/blob/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.Desktop/Binding/PresetBinding.cs),
[BindingState](https://github.com/OpenTabletDriver/OpenTabletDriver/blob/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.Desktop/Binding/BindingState.cs),
[BindingHandler](https://github.com/OpenTabletDriver/OpenTabletDriver/blob/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.Desktop/Binding/BindingHandler.cs),
[PresetManager](https://github.com/OpenTabletDriver/OpenTabletDriver/blob/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.Desktop/PresetManager.cs).

Native held-input toggle is a Rust addition; pinned core contains no generic
ToggleBinding. Its rising edge changes the retained key/mouse/barrel hold;
physical release does not release that hold. Session cleanup must release it.
Preset switching is deferred to the existing control transaction owner, with
old-worker ownership checks and output cleanup before replacement activation.

## Native interface and behavior

All existing pen, auxiliary, mouse, mouse-scroll and wheel action slots accept
`toggle:keys:Control+Z`, `toggle:mouse:left`, `toggle:barrel:1`, and `preset:NAME`.
The typed editor toggles a selected native key/mouse/barrel action and lists
valid native presets. CLI profile setters use the same parser. Toggle supports
held native actions only; nested toggles, scroll, managed actions and presets
cannot be wrapped. Invalid forms return an error.

Toggle physical state and retained output state are separate. Repeated held
reports do not flip a toggle. Two owners holding the same OS action still
share the existing ownership/refcount state. Endpoint loss, Stop, replacement,
range-loss cleanup and failed output reconciliation release retained holds.
Wheel rotation toggles once per actual threshold crossing.

Native preset requests copy a filename stem and source owner into a fixed
request through the existing eight-entry worker notice channel. No profile
read, plugin construction or driver replacement occurs on the report callback.
The active control owner rejects requests while a transaction/update is pending;
retired and candidate workers cannot replace a newer client generation. Primary
requests use the existing daemon replacement; peers use guarded per-device Apply.
Missing, invalid, wrong-tablet or stale presets are diagnostic failures.

`Profile.binding_inhibit` is a transient execution handoff, never serialized to
TOML/OTD. Runtime moves it out of the authored profile before publishing settings,
then carries it in the execution copy. Only the source held slot is suppressed
until a real false reading; absent reports do not arm it. Rotation impulses are
not suppressed. Original OS holds are released by the existing quiesce/drain
transaction. Rollback retains the old worker and its own input state.
Source readings are observed before filtering. Tip/eraser source owners remain
inhibited until actual raw pressure reaches zero, so a new activation threshold
or a filter-modified report cannot fabricate a physical release.

Native presets have a per-worker 50 ms total-time press debounce. Pinned original
PresetBinding instead has a process-wide stopwatch and checks the TimeSpan
millisecond component; that class is not replaced by this native extension.

## Original settings and limits

Original PresetBinding must resolve as the actual unchanged managed class with
its IDriverDaemon provider, retaining whole original Settings semantics. Pure
core import preserves its source store and unsupported diagnostic until the
verified managed resolver supplies that class. It is never mapped to a single
native physical profile. Native toggle and per-device preset export explicitly
reject an OTD JSON representation. Native TOML import/export round-trips them.

Native preset names now allow safe Unicode/internal periods up to 240 UTF-8
bytes, with path/control characters, reserved Windows names and unsafe suffixes
rejected. No name is silently rewritten. Preset menu shows up to 256 valid
entries; CLI can select any valid saved name. Standalone foreground native
preset switching remains unavailable and reports that limitation.

## Evidence

Unrun fixtures cover shared ownership, toggle cleanup failure/retry, pen barrel
latch state, source release inhibition with unrelated held input, malformed
toggle forms, transient settings equality, native export rejection, and a native
allocation assertion. No tests, suites, format, Clippy, build checks, GUI,
plugins, driver, hardware or input execution occurred. Actual package compilation
and release integration belong to the parent agent; live validation belongs to
the owner. Original managed preset integration is supplied by the parallel P06/
S03 provider work, not established by these native fixtures.
