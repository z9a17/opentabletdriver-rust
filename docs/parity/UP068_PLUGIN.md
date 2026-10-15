# 0.6.8-rc1 managed pipeline adoption

Source reference: OpenTabletDriver `v0.6.8-rc1`, commit
`126f4828564abb673d39e97a093fd815aab335c2`. This records source implementation;
it does not establish unchanged-DLL, application, timing or hardware acceptance.

The managed graph now orders synchronous Internal filters, pressure admission,
ordinary PreTransform filters, mapping, PostTransform filters and binding/output.
Managed output modes receive equivalent actual SDK pipeline elements. Admission
uses the active eraser value, rewrites pressure before ordinary filters can retain
it, and does not remap timer/late emissions at bindings. The graph keeps weak
report identities so a synchronous Internal filter emitting the same object
twice cannot progressively reduce its pressure. A newly received source report
resets its own admission identity, permitting parsers that reuse source objects.
Built-in ordinary filters still consume each Internal emission.

Native-only profiles keep their existing raw pressure/contact and hardware-tip
semantics. Managed graph profiles without a configured pressure threshold retain
hardware-tip behavior. Imported/configured thresholds in a managed graph use
rewritten pressure greater than zero for binding contact, including upstream's
integer truncation and full-pressure special case. Non-pen reports do not clear
an earlier contact merely because they lack a pressure interface.

Asynchronous Internal filters are explicitly unsupported. SDK async-derived
Internal types and Internal timer acquisition fail construction. Any foreign or
late Internal emission produces the existing graph fault/held-output cleanup;
custom output-mode late entry through Internal elements requests a mode restart. Ordinary
PreTransform/PostTransform background emissions retain bounded owned snapshots,
generation disposal, reset and continuation behavior. Custom modes without
Internal elements retain their owned background entry, including admission
identity across snapshots of reports that were already rewritten. This limit addresses the
pressure re-entry problem described in upstream
[issue 5151](https://github.com/OpenTabletDriver/OpenTabletDriver/issues/5151);
it is not advertised as full arbitrary Internal-filter compatibility.

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
ordering, duplicate and retained pressure, async Internal rejection, touch
passthrough/FilterState overrides, frequency bootstrap, overridden Resolved
members and full-width rotation. A core fixture covers admission, repeated
binding reads, eraser-false selection, disabled contact, maximum thresholds and
hardware-tip preservation. The focused core admission regression passed. After explicit
integration authorization, the managed GraphBench and legacy fixture projects
built successfully, and the isolated `upstream068` probe passed. The probe also
checks the managed-output retained-report path and capability-specific normalized
rotation commands without native output. No driver, GUI, physical device or live
output was started; only the synthetic managed fixture code was executed.

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
