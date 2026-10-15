# 0.6.8-rc1 managed pipeline adoption

Source reference: OpenTabletDriver `v0.6.8-rc1`, commit
`126f4828564abb673d39e97a093fd815aab335c2`. This records source implementation;
it does not establish unchanged-DLL, application, timing or hardware acceptance.

The managed graph now orders Internal filters, pressure admission,
ordinary PreTransform filters, mapping, PostTransform filters and binding/output.
Managed output modes receive equivalent actual SDK pipeline elements. Admission
uses the active eraser value, rewrites pressure before ordinary filters can retain
it, and does not remap timer/late emissions at bindings. The graph keeps weak
report identities so an Internal filter emitting the same object
twice cannot progressively reduce its pressure. A newly received source report
resets its own admission identity, permitting parsers that reuse source objects.
Built-in ordinary filters still consume each Internal emission.

Native-only profiles keep their existing raw pressure/contact and hardware-tip
semantics. Managed graph profiles without a configured pressure threshold retain
hardware-tip behavior. Imported/configured thresholds in a managed graph use
rewritten pressure greater than zero for binding contact, including upstream's
integer truncation and full-pressure special case. Non-pen reports do not clear
an earlier contact merely because they lack a pressure interface.

The autonomous follow-up adds SDK async-derived Internal filters, injected
Internal timers and owned foreign/late emissions. Internal timers run on the
graph's owner thread and continue after their emitting element. Foreign
emissions and custom output-mode entry through Internal elements reserve the
existing bounded FIFO and copy the concrete report and its nested owned data;
they never call native output on a producer thread. A snapshot carries its
source's pressure-admission identity, so an already rewritten report remains
rewritten, while each snapshot of a still-raw report is admitted once. The
pressure write and identity update synchronize with snapshot capture; the gate
does not span the native continuation or filter Consume. A suppressed ordinary
filter result also retains its completed pressure admission.

Reset discards queued reservations and retires weak identities from the earlier
epoch, including known Internal source reports that had not emitted yet. Later
emissions of these known reports are discarded. Receiving a new source dispatch
gives that original object a fresh identity, preserving parser-object reuse.
Retirement detaches wake/continuation scopes and keeps its generation leased
until in-flight snapshots finish. Queue limits, explicit snapshot-ownership
failures and graph fault/held-output cleanup remain in force. Ordinary-only
filter graphs keep their existing path without pressure-identity tracking or
the additional admission gate.

This is a bounded extension over upstream's retained-pressure hazard in
[issue 5151](https://github.com/OpenTabletDriver/OpenTabletDriver/issues/5151)
and the boxed-report mutation explained in
[PR 4944](https://github.com/OpenTabletDriver/OpenTabletDriver/pull/4944).
The pinned SDK's `AsyncPositionedPipelineElement` retains a report reference
under its synchronization lock and `PipelineManager` directly links Emit to
downstream Consume. The host retains those synchronous identity semantics and
uses concrete owned copies only for asynchronous handoff. Newly synthesized
or plugin-cloned Internal report objects are treated as raw reports; the host
cannot infer that an unrelated object already contains normalized pressure.
It also cannot infer the age of unpublished plugin objects or make arbitrary
unsynchronized plugin/parser mutations atomic. These cases are not a claim of
full arbitrary Internal-filter compatibility. Epoch retirement follows an
explicit transport/graph reset; a physical OutOfRange report alone retains
upstream pipeline semantics and does not retire every report identity. Arbitrary
plugin replay of a retained pre-loss report after physical re-entry is outside
this stale-emission guarantee.

The actual updated SDK supplies `FilterState`, frequency clamping/warnings,
`PipelinePosition.Internal`, `IRotationReport` and non-inherited `Resolved`.
The host initializes only the SDK async Frequency property before timer injection:
the new Scheduler setter immediately derives its interval from Frequency, whose
untouched backing value is zero. Remaining plugin settings keep their existing
dependency-injection order. Nonfinite frequencies remain explicit errors.
Injection walks retain supported private/base members, while a most-derived
virtual declaration controls whether its slot is marked for injection.

Rotation crosses the private managed graph as a full signed 32-bit SDK value;
the older position-only native plugin ABI remains unchanged. Native ranges are
normalized only from declared pen bounds and `DisableRotation` is passed through
to managed modes. Native managed pointer services advertise `IRotationHandler`
only for a prepared pen backend or the macOS tablet-pointer channel. Native pen
packets carry the normalized axis; platform conversion and limitations are
recorded in the main adoption document. Output-mode plugins keep their own real pointer
providers and external-driver prerequisites. Unvalidated string placeholders are
optional metadata and never become saved/default property values.

Authored offline fixtures include `GraphBench ... upstream068` for Internal
ordering, duplicate and retained pressure, async Internal construction, touch
passthrough/FilterState overrides, frequency bootstrap, overridden Resolved
members and full-width rotation. A core fixture covers admission, repeated
binding reads, eraser-false selection, disabled contact, maximum thresholds and
hardware-tip preservation. The focused core admission regression passed. After explicit
integration authorization, the managed GraphBench and legacy fixture projects
built successfully, and the isolated `upstream068` probe passed. The probe also
checks the managed-output retained-report path and capability-specific normalized
rotation commands without native output. No driver, GUI, physical device or live
output was started; only the synthetic managed fixture code was executed.

The follow-up adds `GraphBench ... async-internal`: SDK Internal timer repeats,
raw/prepared owned snapshots, nested concrete ownership, multiple Internal
elements, managed-mode late entry, suppression admission, parser-object reuse,
capture during the pressure-write/identity window, reset and retirement during
in-flight reservation, stale reports followed by fresh FIFO work, overflow and
unownable report rejection. The expanded mode passed in the integrator's
targeted offline build/run, alongside upstream068, admission, background, wake,
cleanup and snapshots. These synthetic fixtures do not qualify real third-party
plugins, runtime timing or hardware behavior; separate real-DLL evidence is
recorded in the [bounded corpus](OFFLINE_PLUGIN_CORPUS_2026-10-15.md).

The RC adds abstract `IDeviceEndpoint` initialization members and
`IOutputMode.DisableRotation`; an unchanged direct 0.6.7 implementation would
otherwise fail type loading. The vendored Plugin SDK retains its 0.6.8 assembly
identity and adds default interface bodies only at those new members. Legacy
endpoints report unsupported initialization when explicitly requested, without
assuming ownership of an `Open()` stream. Legacy direct modes accept the default
false rotation option and report unsupported if asked to disable rotation.
Concrete new SDK output modes/endpoints retain their implementations. The old
SDK 0.6.7 SettingsFixture project includes direct implementors whose DLL is loaded
unchanged against the updated SDK; it remains separate from the new SDK fixtures.
The targeted probe passed its loading/default-operation checks. Reflection over
the official 0.6.7 SDK and the source-built replacement found all 532 original
public/protected methods, constructors and fields present. This focused surface
check and synthetic fixture do not qualify the existing third-party plugin
corpus, runtime timing, or hardware behavior.
