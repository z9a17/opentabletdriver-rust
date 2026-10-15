# Offline unchanged-plugin corpus, 2026-10-15

This bounded follow-up adds repeatable real release-DLL cases to F05/P05/V03.
It does not replace the historical 57-release/50-identity inventory, close
hardware gates, or establish whole-package or catalog compatibility.

The machine-readable input is
[plugin-offline-corpus-2026-10-15.json](plugin-offline-corpus-2026-10-15.json).
It pins archive and extracted DLL SHA-256 independently, source revisions and
reviewed source-file hashes, exact class names and settings cases. No plugin
binary or third-party source is committed in this pass.

| Catalog identity | Release | Reviewed classes | Settings cases |
| --- | --- | --- | --- |
| AbstractQbit's Radial Follow Smoothing | 0.3.0 | TabletSpace, ScreenSpace | 4 per class |
| BezierInterpolator | 0.3.1 | BezierInterp | 4 |
| Flip Axes | 0.0.4 | Flip_Tablet_Axes, Flip_Area_Axes | 5 per class |

There are five classes, 22 settings cases and two fresh instance/graph cycles
per case (44 lifecycle cycles). These counts describe fixture scope; the
execution evidence record determines which cases actually passed.

The coordinated Windows x64 run passed all 44 lifecycle cycles across the
22 settings cases and five classes, with exit status 0 and empty stderr.
[The scoped execution record](plugin-offline-evidence-2026-10-15.json) retains
the complete successful transcript, per-class results, archive/DLL identity,
source revisions and manifest/fixture/runner/tablet hashes. Its local command
paths are normalized to documented placeholders; the original full result
JSON remains unchanged in the integrator's scratch evidence directory.

## Source and artifact review

Archives came from the pinned catalog URLs. RadialFollow was reused from a
previously cached archive and rehashed; BezierInterpolator and Flip Axes were
retrieved through the unchanged repository `src/download.rs` implementation,
compiled into a scratch-only console downloader with the existing Windows
dependency. The downloader never starts the app, daemon or a device reader.

Sources inspected before execution:

- RadialFollow [tablet class](https://github.com/AbstractQbit/AbstractOTDPlugins/blob/58846450e9a5f6f3cfada9dd1b4d4909a5ca25bb/RadialFollow/RadialFollowSmoothingTabletSpace.cs),
  [screen class](https://github.com/AbstractQbit/AbstractOTDPlugins/blob/58846450e9a5f6f3cfada9dd1b4d4909a5ca25bb/RadialFollow/RadialFollowSmoothingScreenSpace.cs)
  and [core](https://github.com/AbstractQbit/AbstractOTDPlugins/blob/58846450e9a5f6f3cfada9dd1b4d4909a5ca25bb/RadialFollow/RadialFollowCore.cs):
  numerical position filters, tablet-specification scaling and HPET elapsed
  reads. They do not open endpoints or produce native input.
- BezierInterpolator [release class](https://github.com/AbstractQbit/AbstractOTDPlugins/blob/da220ed3c7d962f8fa2fd1fd34f7a9ddee8a5b7d/BezierInterpolator/BezierInterp.cs)
  and its project: numerical interpolation, injected SDK timer, HPET elapsed
  reads and report events. Release 0.3.1 forwards auxiliary/non-tablet state;
  the earlier cached source did not have that fix. This pass uses the reviewed
  release revision, not the earlier cache, for its provenance.
- Flip Axes [base](https://github.com/Kuuuube/Flip_Axes/blob/11d49af1af13f59e232a98126cae719558bc97f5/Flip_Axes/Flip_Axes_Base.cs),
  [tablet class](https://github.com/Kuuuube/Flip_Axes/blob/11d49af1af13f59e232a98126cae719558bc97f5/Flip_Axes/Flip_Tablet_Axes.cs),
  [area class](https://github.com/Kuuuube/Flip_Axes/blob/11d49af1af13f59e232a98126cae719558bc97f5/Flip_Axes/Flip_Area_Axes.cs)
  and project: pure coordinate math after finding a concrete Driver output
  mode. The fixture supplies the reviewed private output-mode cache with an
  `AbsoluteOutputMode` model whose pointer is null. It never constructs a
  Driver, reads devices, invokes the mode or discovers an output. Automatic
  concrete Driver/output-mode resolution remains unverified.

The archive hashes identify catalog release bytes. Source review and matching
API dependencies support the bounded execution decision; this is not a
reproducible-build proof that every release DLL came from those sources.

## Fixture behavior

[CorpusFixtures.cs](../../compat/GraphBench/CorpusFixtures.cs) runs the actual
bridge and unchanged DLL alongside a separately constructed unchanged DLL
using the shared upstream interfaces. Reference settings call the actual
vendored Desktop `PluginSettingStore.ApplySettings` method, independently of
the bridge's settings adapter. That method matches the pinned historical
0.6.7 source; the later vendored file adds an unrelated `Clone` method.

The cases cover absent settings, explicit null defaults, configured values,
clamping and unknown saved properties. Flip Axes also covers independent X,
Y and combined flips. Synchronous traces compare position, pressure, tilt,
raw bytes, button values and concrete report type for 24 reports, then deliver
range loss and compare resumed input. Reset emissions are discarded on both
sides at the host's lifecycle boundary. Graph and instance disposal are
repeated, retired ticks must reject, and each case is recreated from scratch.

RadialFollow's elapsed stopwatch is reset before each paired consume, fixing
its observed delta at zero. This makes the numerical comparison independent
of JIT and scheduling delays. Its 50 ms re-entry branch is not covered here.
Flip Axes additionally compares to the analytic `extent - coordinate` mirror
oracle; its 0.01 raw-unit/pixel tolerance permits float cancellation through
normalized coordinates. Direct reference/bridge tolerance is 0.001.

Bezier's constant zero-position/pressure/tilt trace removes interpolation's
wall-clock dependence. Test-owned session timer deadlines are forced due;
no OS timer is created. Auxiliary reports must pass synchronously, tablet
reports wait for a timer emission, and range loss must stop stale tablet
output. A worker invokes the reviewed SDK emission method on the actual
Bezier instance to test retained-state snapshots, source mutation, queued
delivery, pending-queue retirement and late-callback detachment. This is
controlled event injection, not evidence of external timer timing behavior.

The corpus process requires native services to be unavailable. All graph
callbacks capture/discard outputs. There is no display query, UI, live
device discovery, OS input/output or saved-settings access. The bridge is
not a security sandbox; these are an intentionally source-reviewed allowlist.

## Repeat

Build GraphBench through the integrator's coordinated build first. Put the
three original ZIP files named by the manifest into a temporary artifact
directory. All downloads must use the repository download path. Then run:

```text
python scripts/offline-plugin-corpus.py --artifacts <temporary-artifact-directory> --graphbench <built-GraphBench.dll> --tablet <fixture-tablet.json> --output <temporary-results.json>
```

The runner verifies each ZIP, extracts only its named bounded DLL entry,
verifies DLL bytes and launches only the offline GraphBench corpus mode.
It writes command, manifest hash, archive/DLL hashes, exit status, stdout and
stderr into the result JSON. GraphBench independently rechecks archive and
DLL hashes before loading any class. The runner does not compile or retrieve
anything from the network.

## Remaining acceptance boundaries

Physical input, real reconnect/sleep/wake, native Windows output, native
Linux/macOS runtime, mixed real device endpoints and real external plugin
prerequisites remain separate acceptance work. Nonzero Bezier interpolation
under matched clocks, timer jitter/frequency accuracy, RadialFollow timed
re-entry and Flip Axes automatic concrete Driver resolution remain outside
these fixtures. Other exported classes, settings sequences and versions are
unverified unless named in an execution result. Native output plugins such
as Windows Ink/VMulti require their external runtimes and controlled output
validation; these pure-filter fixtures cannot establish their support.
