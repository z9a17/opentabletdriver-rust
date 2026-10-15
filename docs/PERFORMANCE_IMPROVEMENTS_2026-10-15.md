# Driver overhead follow-up, 2026-10-15

## Scope and claim

Branch `perf/managed-overhead` follows the OpenTabletDriver 0.6.8 RC adoption
in [PR #91](https://github.com/z9a17/opentabletdriver-rust/pull/91), starting at
`274427a4e9809f5c698a4eb82a87a871dc439846`. The root integrator owns F04 with
bounded P03/P05/S05 continuation. The historical workstream issues are archived;
the claim lives in the checked-in acceptance backlog. No other active issue
claim existed when this branch started.

The owner requested implementation of useful optimization findings while
keeping the UI small. This work changes driver internals and offline fixtures;
it adds no UI, setting, dependency or plugin replacement. Native parsing,
mapping, scheduling and output remain Rust-owned. The C# bridge still supports
unchanged .NET plugins and their retained report identities. Unix frontend
compatibility code remains necessary and is outside this optimization scope.

## Implemented changes

- Skip managed admission-table work only when the graph has neither Internal
  nodes nor a managed output mode. Evaluate that condition after output
  attachment too. Retained and repeated reports in the other graphs keep the
  existing pressure-admission rules.
- Iterate enabled endpoint timers directly without a LINQ iterator.
- Avoid serializing and parsing unchanged service snapshots. Native publication
  compares observable fields, serializes a borrowed snapshot once, and shares
  immutable reply bytes with retained tickets. The managed monitor supplies a
  process-unique cache token; unchanged reads allocate no ticket or JSON reply.
  Explicit refresh after commit still requests a complete snapshot. Logical
  daemon/device generations retain their original meaning. Provider event
  comparisons also observe the token so a replacement owner at the same
  logical version still reports changed devices and tablets.
- Seek directly to unread debugger packets and allocate a result vector only
  when packets are available. Report/byte limits, evictions, lost counts and
  independent subscriber cursors retain their existing behavior.
- Windows managed queues signal the existing native session event. A separate
  flag distinguishes managed work from custom-reader notifications. Queue-only
  graphs/endpoints can stop idle polling; timers and provider-capable endpoints
  retain their bounded fallback because timer changes have no notification.
  Older bridges and Unix retain polling. Callback detachment synchronizes with
  producers before releasing the native context. Wake handling preserves Stop,
  ownership transitions and alternating pen/auxiliary input before managed Idle,
  including already-signaled pending reads.

Independent reviews found and resolved expired snapshot tickets retaining old
reply bytes during unchanged reads, negative endpoint deadline interpretation,
dynamic timer capability, and repeated managed signals bypassing input fairness.
Expired tickets are pruned even when the snapshot itself is unchanged.

## Review boundaries

Skipping the binding fallback projector is not useful in normal configured
managed profiles: their managed parser/graph already supplies the original
report. Report pooling would violate plugin retention semantics. Self-IPC
status polling, HID metadata discovery, Unix timer resolution and broader
service-lane scheduling need separate lifecycle work and are not changed here.

The original independent review's standalone ConditionalWeakTable program was
a synthetic allocation model. Its GC outliers are not driver latency evidence.
The release matrix's GNU default also does not establish which toolchain built
every Windows release; the local test package uses MSVC explicitly.

## Offline measurements

The existing GraphBench synthetic managed graph runs against a fixture compiled
for the original 0.6.7 SDK, with discard callbacks. Three fresh processes per
revision alternated baseline/current; each reports the median of 15 batches of
200,000 reports. The median of process medians fell from **542.7 ns to 215.7 ns**.
Reported managed allocation fell from **410.2 to 384.0 bytes/report**. This case
has an ordinary filter, no Internal filter and no managed output. It measures
bridge CPU work and allocation, not USB, scheduling, pen latency or gameplay.
Other graph shapes intentionally retain admission tracking.

The standalone debugger probe retains the same corpus digest
`8de207a46c7a85e8` as the baseline. Caught-up reads with 4096 retained packets
measured about **5684 ns before, 15 ns after**, and **3584 allocated bytes before,
zero after**. Baseline and current debugger timings were collected separately;
allocation and corpus equivalence are stronger evidence than a timing ratio.
The debugger is an optional consumer; this is not a native report-path latency
claim. [Raw data and compiler/fixture metadata](perf/2026-10-15/summary.md) are
checked in with a small reproducible Rust probe.

The working-tree inventory is about 72% Rust by combined Rust/C# source bytes,
including vendored compatibility code and test fixtures. That metric does not
describe runtime execution: unchanged plugins still execute their .NET code,
and profiles needing managed reports retain their original parser semantics.

## Validation and limits

The targeted Rust checks passed **29 tests**: snapshot caching/expiry (7), native
wake reset interleavings (1), endpoint deadline sentinels (1), debugger rings (8)
and input selection/fairness (12). GraphBench `snapshots`, `admission`,
`background`, `endpoints`, `upstream068` and `wake` all passed with fake endpoints
and discard callbacks. The managed fixture build succeeded. The shared Unix
library type-checked for `x86_64-unknown-linux-gnu` using the installed rustup
compiler explicitly; the separate PATH compiler lacks that target's stdlib.

The endpoint fixture initially failed identically on the base revision because
the updated configuration SDK emits decimal `JRaw` tokens during `FromObject`.
It now round-trips synthetic configuration through JSON, matching production's
wire parsing boundary, before retaining the same behavior assertions.

No driver, GUI, device discovery, real pen input or OS output was started, and
no saved settings were edited. Whole-workspace test/format/clippy/check suites
were not run. Physical input, hotplug, sleep/wake, third-party plugin behavior,
native Unix runtime and pen-to-display latency still require manual validation.

The branch is intended for an unmerged Windows test package through
`scripts/update-test-build.py`, which verifies hashes, preserves destination
files/settings and records exact branch/commit provenance in
`E:/OTD RUST TEST/data/TEST-BUILD.json`. PR handoff records the package outcome;
this document does not claim a merged release.
