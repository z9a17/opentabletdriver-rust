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
| Idle tray updates | Skip identical tooltip updates instead of converting text and calling `Shell_NotifyIconW` on every 500 ms status snapshot. Cache successful updates only; icon creation and Explorer recovery always reapply the icon and text. |

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

## Authorized live Windows review

The owner subsequently authorized computer use with the connected Wacom PTH-660.
The first live package came from clean commit
`1eda08ef1e790c1dcb562d8601ee9f300390845e`; it excluded unrelated working-tree icon
edits. It was exercised at the owner's saved window size and violet accent.

- Startup detected the tablet and its auxiliary endpoint. Status remained
  `running`, with no reported driver error through repeated Save/Apply cycles.
- Saving the third filter kept it selected. Friendly plugin names, units and
  saved values populated after metadata loading.
- All eight ExpressKeys and ring controls fit in the Buttons page. The Mouse
  tool selection survived visiting Pen and returning to Buttons. A binding
  dropdown closed on a second click.
- Plugin manager headers, rows, details and buttons were inspected in dark and
  light themes. Columns fit without a horizontal scrollbar. Selecting the
  installed Radial Follow plugin populated its details and enabled its actions.
  Installation/removal was not performed.
- Performance settings opened from Settings and its Back navigation worked.
  Invalid area text displayed a validation message; leaving the field restored
  its last valid value. Save subsequently applied that valid profile.
- The debugger opened three times without terminating the driver; its
  statistics panel and dark layout rendered correctly. It showed zero reports
  while the pen was idle/out of range. This does not certify active capture,
  physical report timing, pen movement or ExpressKey delivery.
- Clicking the visible main window's close button exited both owned processes.
  An earlier `.NET Process.CloseMainWindow` attempt while the UI was hidden in
  the tray did not close them; that helper attempt is recorded separately and
  is not used as evidence for the actual close-button behavior.
- No matching Application Error/Windows Error Reporting event was found for
  the test executables during this first session.

All 11 original settings/plugin files were backed up and verified by SHA-256.
The three device-profile files created by testing Save were archived with the
after-state before removal. Original settings were restored before the final
package retest.

### Idle observations

These bounded CPU observations used the connected tablet without pen motion;
they are not throughput or input-latency benchmarks. The computer-use observer
was disconnected for sampling. Percentages below are fractions of one logical
CPU, not the whole machine.

| First live package scenario | 30-second UI CPU time | One-core utilization |
| --- | --- | --- |
| Visible Area, first sample | 1.296875 s | 4.32% |
| Visible Area, settled repeat | 1.437500 s | 4.79% |
| Minimized to tray | 1.250000 s | 4.17% |

The daemon used 0 and 0.03125 CPU seconds in the two visible-window samples.
In the settled repeat, UI handles, threads and working set stayed constant;
daemon handles, threads and working set also stayed constant. This brief
observation does not exclude long-running leaks.

Source review found the redundant tray-tooltip shell update independently of
these samples. The samples do not establish how much of their CPU time came
from that update.

The final package's two 30-second visible-Area samples used 0.953125 and
1.421875 UI CPU seconds (3.18% and 4.74% of one core), and 0.0625 and 0 daemon
CPU seconds. The settled second sample had unchanged handles, threads and
working set for both processes. This does not demonstrate a clear CPU
improvement from the tray cache. Further idle-UI optimization should start with
sampled call stacks in a controlled session rather than attributing this cost
to rendering or tablet latency from these totals.

### Final package and restoration

The native Windows release-mode apps and managed dependencies were rebuilt
from clean commit `2ca106377ee179d77407ff1321865219b19b02e1` and copied to
`E:/OTD RUST TEST`. The audit checkpoint that followed changed documentation
only. Subsequent release preparation adds the logo integration and version bump;
the release notes distinguish those changes from this live-tested checkpoint.

| Artifact | SHA-256 |
| --- | --- |
| Windows test ZIP | `1a72a514f36331a7a97d67579091f74e2dd09901242112b73ce8aeb2e39a2a8f` |
| UI executable | `516fa2d8548495aba74cf67bb035a6408c3e889012781e2e53b785864de32bcd` |
| Driver executable | `d4fe6395e9cdce5411801dfc16aee965b7d23b2cf266fd0bd0bb90da6138e83c` |

With original settings restored, the final build detected the PTH-660, opened
its debugger, and reported no driver error. Clicking the main window's close
button while the debugger remained open exited the debugger, UI and daemon.
All 11 original settings/plugin files were restored and SHA-256 verified again;
no test app or daemon was left running. Local after-state archives, screenshots,
status snapshots, process samples and build logs remain in the audit folder.

This is an unmerged Windows test build, not a release. Physical input acceptance
and the other platforms' runtime checks remain open as described above.
