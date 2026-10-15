# Current source and evidence map

This is the current view for the 0.19 release series. Historical implementation
reports and the original 0.6.7 acceptance ledger remain available; neither
their old missing-feature lists nor their old passing results automatically
describe the selected 0.6.8 RC components.

## Source composition

| Area | Current source |
| --- | --- |
| Native driver | Rust owns device I/O, decoding, mapping, scheduling, output, control and Windows UI |
| Device catalog | RC commit `126f4828564abb673d39e97a093fd815aab335c2`: 359 configurations, 54 referenced parser names |
| Managed Plugin SDK | Actual RC source, assembly 0.6.8, plus documented compatibility bodies/restored older members |
| Configurations/Native assemblies | Pinned 0.6.8 NuGet packages from the same RC revision |
| Core/Desktop/UX/Console | Copied 0.6.7 sources with native-host adaptations and selected RC fixes, targeting .NET 10 |
| Historical acceptance inventory | 0.6.7 commit `736003ed72c8bbb28033b039d5a0bb76c344145c`: 339 configurations/52 parser names; unchanged |
| Historical catalog inventory | Plugin-Repository `2dfdff1cd77d274eb19c4d359b240167537c4db2`: 57 eligible versioned rows, 50 identities; eligibility is metadata only |

The [RC adoption report](UPSTREAM_0.6.8_RC1.md) and component provenance files
define the exact mixed source composition. C# provides unchanged .NET plugin
contracts and original Unix frontend compatibility. It is not the native report
processing implementation. Managed features require the matching .NET 10 runtime;
native-only input does not initialize the CLR.

## Bounded evidence

| Work | Recorded result | What it does not establish |
| --- | --- | --- |
| RC adoption in 0.19.0 | 22 focused native cases, old-SDK fixture loading, retained 532-member legacy surface | Every plugin, tablet, transport or application |
| 0.19.0 overhead review | 29 focused native tests, six managed modes and synthetic graph measurements | Physical pen-to-display latency or gameplay benefit |
| Autonomous follow-up | Focused native scheduling/recovery and seven managed fixture modes; see [execution record](AUTONOMOUS_FOLLOWUP_2026-10-15.md) | Interactive workflows, physical reconnect/sleep or native Unix runtime |
| Unchanged filter corpus | Three identities/five classes, 22 settings cases and 44 lifecycle cycles passed | Entire packages/catalog, arbitrary timing or concrete output discovery |
| Packaging | Four platforms built from one merged source revision, with release archive/provenance/hash checks | Successful execution on every target OS |

The [real-DLL corpus](OFFLINE_PLUGIN_CORPUS_2026-10-15.md) records exact release
archives/DLL hashes, source-review pins, settings and fixture limits. Its direct
reference uses the vendored upstream interfaces and settings implementation,
not a separately running upstream daemon. The [overhead report](../PERFORMANCE_IMPROVEMENTS_2026-10-15.md)
keeps CPU measurements separate from physical latency.

Asynchronous Internal support owns foreign/late report snapshots, preserves
pressure admission, and retires known identities on explicit reset. The
[contract](UP068_PLUGIN.md) documents arbitrary plugin mutation, newly cloned
pressure values and physical range-loss limitations. Ordinary graphs retain the
existing path without Internal admission tracking.

## Remaining acceptance

The historical ledger is deliberately unchanged. No configuration, parser,
capability or whole package is advanced merely because a newer source or a
bounded fixture passed. A wholesale baseline transition still requires the
[archive/diff/reset procedure](EVIDENCE_LEDGER.md#change-the-pinned-baseline).

Physical tablet behavior, real hotplug and sleep/wake, Windows Ink drawing,
interactive UI/application behavior, native Linux/macOS execution, and the full
unchanged-plugin corpus remain separate acceptance work. Further automation can
expand pure-filter traces, failure fixtures and endpoint invalidation. It cannot
substitute for evidence from the actual hardware and applications.

The follow-up adds no UI pages, controls or settings and does not edit saved
settings. Source scheduling counts indicate removed work; no new end-to-end
latency claim is made.
