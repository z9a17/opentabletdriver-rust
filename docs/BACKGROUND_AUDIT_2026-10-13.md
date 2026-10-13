# Reliability and performance review, 13 October 2026

This review continues the unmerged Windows cleanup in PR #89, starting from
`7b3f7df55ed514c424fff21df98530fe7a6e17e4`. It covers source and offline regression
evidence. It does not close the hardware, unchanged-plugin-corpus, application,
or physical-latency acceptance gates in the [0.18 implementation report](parity/IMPLEMENTATION_0.18.md).

The initial work was background-only. The owner subsequently authorized local
computer-use testing with a connected tablet; any live results are recorded
separately below. Existing settings are preserved. No release is published by
this review, and unrelated icon changes in the shared checkout are excluded.

## Fixes

| Area | Failure and correction |
| --- | --- |
| Windows input scheduling | Synchronous pen completions could indefinitely postpone auxiliary reports. Alternate ready endpoints and check already-signaled pending reads before taking the other endpoint's immediate completion. Stop retains priority. |
| Native read ownership | Retain each completed buffer until consumed once; pin `OVERLAPPED` storage across reader moves; keep pending ownership after `ERROR_IO_INCOMPLETE`. HID, WinUSB and custom-reader paths retain their cancellation/consumption contracts. |
| Worker shutdown | Close the notice receiver before joining a worker, so a lifecycle notice blocked behind a full preset/notice queue can finish instead of deadlocking shutdown. |
| Shared report delivery | Commit loss counters only through the delivered prefix of a merged event batch. Previously later gaps were charged early and could be counted again on the next poll. Acquire the nonblocking ring-write attempt before publishing its sequence, so a consumer cannot skip an in-flight successful publication as a trailing drop. |
| Mapping | Reject signed rectangle-span overflow and nonfinite derived transforms. Clamp signed desktop-relative coordinates before unsigned normalization, preventing off-desktop negative positions from jumping to the opposite edge. |
| Managed cleanup | Retire assembly/registry leases even if service disposal throws. Dispose the filter even when its event unsubscribe fails. Cover endpoint validation with the construction cleanup guard. Establish observer ownership before later tool startup can fail. |
| Unix descriptor parsing | Reject report-size arithmetic overflow instead of wrapping or saturating malformed descriptors into plausible capabilities. |
| Unix service requests | Apply expiry and retirement checks to both ordinary mailbox receives and the race recheck. Expired calls are drained iteratively. |
| Local test deployment | Check existing Windows payloads for mapped/locked files before replacing any file, avoiding partial metadata/dependency updates when the test executable is already running. This preflight is not a full multi-file transaction; use a separate destination if the old build must remain running. |

The successful native report path remains allocation-free in the exercised
fixtures. Reader storage is allocated at setup. Fairness reuses the combined
endpoint/Stop readiness probe on the synchronous fallback path; the ordinary
all-pending path gains no polling loop. These facts do not establish a measured
physical latency reduction.

## Review coverage

The review inventoried the project and traced high-risk paths across these
components; it is not a claim that every possible state or every line has been
exhaustively verified.

- Portable parsing, mapping, radial/relative filters, pipeline routing,
  keyboard/button/pen ownership and native plugin ABI.
- Windows HID/WinUSB/custom readers, paired endpoint scheduling, runtime and
  companion lifecycle, device sessions, control framing and original RPC.
- Managed graph admission, retained reports, timers, registry generations,
  endpoint construction, service/input leases, global tools and teardown.
- Windows editor/background completions, debugger recording, secondary-window
  lifetime, plugin installation, profile persistence and update recovery.
- Linux/macOS descriptors, paired readers, shared I/O, permissions/setup,
  callback cleanup, package provenance and local test deployment.
- README, setup guides, current versus historical status, and GitHub metadata.

## Automated evidence

All executable tests here are offline fixtures; they do not open a physical
tablet, start a driver, inject real input or edit existing user settings.

| Check | Result |
| --- | --- |
| `cargo test --locked -p otd-core --lib` | 235 passed, 1 manual timing test ignored |
| Windows `session::` tests | 10 passed, including 9 new scheduling/read-lifecycle fixtures |
| Windows `runtime::worker_cleanup_tests` | 1 passed |
| Windows `shared_devices::tests` | 3 passed; includes allocation checks for successful and dropped publications |
| Windows `ui::model::` tests | 22 passed |
| Original RPC protocol/apply fixtures | 3 + 1 passed |
| Update transaction/recovery fixtures | 2 passed, including mapped-image recovery without launching the image |
| Linux/macOS descriptor modules compiled and tested independently on Windows | 4 + 4 passed |
| Test-build Windows lock/replacement fixtures | 2 passed |
| GraphBench `cleanup` failure-injection fixture | Passed; filter, binding, provider parser and source parser retirement, rejecting unsubscribe and invalid early endpoint configuration |
| Linux x64, macOS x64 and macOS arm64 target checks, including tests | Passed; compilation only, no Unix runtime execution |
| Parity evidence validation and Python parity tests | Ledger valid; 21 tests passed |

The first core run exposed six stale fixtures: four original-settings examples
omitted a now-required field; a key test rejected the supported Mute key; and
the allocation fixture's own wait-recording vector grew inside the measured
region. The fixtures were corrected; the zero-allocation assertion was kept.
The managed/Unix harnesses also needed their current project references,
locked runtime graphs and platform guards repaired. Existing dependencies were
not upgraded.

Local logs are under `E:/AgentWork/tmp/otd-background-audit-20261013` and
`target/audit-managed-20261013`. Existing unused/dead-code warnings and the
upstream nullable-event warning are separate cleanup debt.

## GitHub presentation

The README now leads with downloads, platform requirements and setup, then
features and compatibility limits. Detailed Windows instructions and a
[documentation index](README.md) keep the landing page concise. The backlog
points to current 0.18 implementation status while retaining historical audits.
The repository About description and topics were updated without advertising
unverified latency or universal device/plugin compatibility.

## Remaining acceptance

Physical pen feel and input-to-display latency, held-input unplug/reconnect,
sleep/wake, broad third-party DLL compatibility, Windows Ink application
behavior, and native Linux/macOS execution still need direct evidence. The new
Unix mailbox fixtures were compiled, not executed on a Unix OS. Managed lease
release is tested; eventual collectible-assembly garbage collection is not.

Live Windows results and final build provenance will be recorded after the
authorized local review.
