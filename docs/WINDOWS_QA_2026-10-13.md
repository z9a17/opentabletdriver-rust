# Windows desktop review, 12–13 October 2026

Follow-up: the [13 October reliability review](BACKGROUND_AUDIT_2026-10-13.md)
addresses the reader-starvation issue described below and records later live
checks of the navigation cleanup and debugger. This document preserves the
earlier observations and their limits.

Baseline: published Windows x64 **v0.18.0**, source
`ef331b9e2bbac3abd825d363ffd0e80e2b9a58d3`. The local release archive SHA-256
matched the published asset:
`1b8e6bbbc41accb4e76cbefb37cf9f1fe870a111bae07502dc285f5d32447456`.
Review used native computer-use interaction on the owner's Windows 11 PC
(Ryzen 7 5800X3D). No other operating system was tested.

## Findings addressed

| Finding | Reproduction and cause | Change |
| --- | --- | --- |
| Missing ExpressKeys | With the PTH-660 detected, Auxiliary Settings showed six of eight keys at the saved window size and seven when maximized. Group height used a different column calculation from the row renderer. | Use the same grid calculation for sizing and placement, including the adjacent pen/wheel groups. |
| Unnecessary idle rendering | Every 500 ms, a repeated driver state invalidated the status strip, but painting allocated and rasterized the whole window. | Skip invalidation for unchanged state and limit the backing canvas to the Win32 update rectangle; preserve window-coordinate drawing. |
| Stale empty-page actions | Select a filter, then switch to the empty Tools page: Defaults and Remove could remain enabled. | Disable selection-dependent actions when the selected target is absent. |
| Stray plugin-manager horizontal scrollbar | Loading the catalog introduced a vertical scrollbar after the name column had already filled the earlier client width. | Refit columns after populating the list. |
| Redundant debugger redraws | A 100 ms completion timer repainted without a recorder, and unchanged 33 ms snapshots repainted the entire debugger. | Repaint for changed report content, report rate, errors or recording progress/completion; preserve polling, statistics and shutdown behavior. |

The missing-controls and stale-actions regressions were reproduced with hidden
native controls before the fixes. Four existing test-fixture compile errors also needed repair:
the shared UI fixture's module path/visibility and missing fields in one device
candidate and two plugin chains. These repairs only affect test configuration.

## Baseline live coverage

- Startup detected the actual Wacom PTH-660 on 12 October and started its daemon.
- Tablet Debugger opened and closed without crashing or stopping the driver.
  Idle/no-report display was inspected; no physical pen movement was available.
- The plugin catalog loaded 51 eligible entries. The installed Radial Follow
  plugin was recognized. Dark/light list headers, details, selection and buttons
  were inspected. No third-party plugin was installed or removed.
- Filter metadata populated friendly names, units and defaults. Saving with the
  third filter selected retained that filter and its values.
- Clicking the open output dropdown again closed it. Maximize/restore and theme
  changes worked. Pen, mouse, auxiliary, experimental and console pages were
  inspected.
- A negative tablet width showed validation feedback; applying did not send
  invalid geometry to the driver. Shortcut capture cancellation preserved the
  existing binding.
- Minimize-to-tray and relaunch restored the existing panel. Closing the panel
  stopped its daemon, with no tablet processes remaining in the verified close.
- On 13 October the tablet was no longer detected. The tablet preview correctly
  stayed empty and saved dimensions were retained. This limits post-fix hardware
  validation; a synthetic device fixture is not a live connected-tablet check.

## Focused automated coverage

`cargo test --locked -p opentabletdriver-rust --bin opentabletdriver-rust ui::app::area_preview_tests:: -- --test-threads=1`

Nine tests passed, including all PTH-660 key/ring controls visible and
non-overlapping at 962×756, 2560×1360 and 820×600 client sizes; empty Tools action
state; and pixel equality of partial/full rendering for five pages in dark,
light and high contrast. Existing discovery, settings and accent tests in this
module also passed. This was a focused run, not the full workspace test suite.
The build still emits pre-existing unused-import/dead-code warnings.

Five `ui::debugger::tests::` tests and the managed-settings modal guard regression
also passed, bringing the focused total to 15. The new hidden debugger test
covers unchanged snapshots, changed data with the same sequence, rate settling,
cleared reports and repeated/changed errors. Three GPT-6.1 Sol reviewers at high reasoning
reviewed the changes, remaining debugger work and the report path. The diff
review found no actionable regression; the report-path limitation below remains
open.

## Idle performance evidence

Measurements use process CPU-time deltas divided by wall time, expressed as a
percentage of **one logical CPU** (not total CPU across the 16 logical CPUs).
The same saved-size Light-theme Output page was visible, with no tablet detected.
The computer-use connection was reset before each usable 30-second sample.

| Build | CPU seconds / 30 seconds | One-core utilization |
| --- | --- | --- |
| Published v0.18.0 | 0.140625 | 0.469% |
| UI fixes | 0.078125 | 0.260% |

Earlier attached-observer samples ranged around 3–10% of one core. Disconnecting
the review connection sharply reduced that overhead; those samples cannot be
used to attribute normal idle cost to the driver. The small observer-disconnected
difference is a single-run observation, not a robust latency benchmark. It does
not establish lower USB, pen-processing or input-to-display latency. No sustained
private-memory or handle growth was demonstrated by these bounded observations.

Live retests confirmed the manager's horizontal scrollbar is gone, empty Tools
actions are disabled, console-log export writes the requested file, and malformed
TOML loading reports its parse error without replacing the current profile.

The build with the recording-timer fix also opened its debugger without a crash. Its later 30-second idle
sample used 6.234375 CPU seconds (20.767% of one core), compared with 1.71875
seconds (5.725%) for the published debugger in an earlier session. These samples
were separated by hours and desktop activity/session conditions changed. This
unresolved discrepancy prevents claiming a measured debugger CPU improvement.
Subsequent source review identified and removed the unchanged-snapshot redraw
path as well. That additional change has no live CPU sample: desktop interaction
stopped when requested by the owner, with no further live checks. A controlled
repeat remains necessary before making a debugger performance claim.

## Highest-priority remaining report-path issue

**Conditional auxiliary-input starvation** remains open in
`src/session.rs`, `HidSource::next`. A synchronous pen completion returns before
the auxiliary reader is checked. If processing remains slower than incoming pen
reports, a continuously nonempty pen queue can postpone auxiliary press/release
reports. A held ExpressKey may therefore remain held until that backlog drains.
This is supported by source control flow, not reproduced on the owner's device.

A simple reversal of the two readers is unsafe: it can reverse starvation, and
`Reader::start` returns false for an already pending read even if its completion
event is signaled. The next change should introduce deterministic Windows-reader
fairness tests before changing scheduling: continuous streams on both endpoints,
pending/signaled auxiliary completion alongside queued pen input, exactly-once
delivery, per-endpoint order, Stop priority and output-ownership transitions.
The portable fake report source starts after reader selection and cannot test
this defect. Report timestamps are taken when completions are consumed; timing
statistics exclude earlier HID queue residence and do not certify physical
latency.

## Validation limits and follow-up

- Physical pen movement, ExpressKey presses, hotplug, Windows Ink drawing,
  subjective filter feel and input-to-display latency remain unverified.
- The configured unchanged Radial Follow DLL was recognized and replaced by its
  native Rust implementation. This is not evidence for arbitrary managed DLL
  report processing or the full plugin ecosystem.
- Injected shortcut chords were rejected by the scan-code-based capture dialog.
  Computer-use injection may omit the physical scan code; physical keyboard
  capture needs a separate check before labeling that a product failure.
- Some accessibility names refer to unrelated hidden labels (for example the
  filter list and console). Screen-reader naming deserves a dedicated fix.
- The debugger uses a generic title-bar icon instead of the main app icon.
- Original-settings import/export, preset round trips, plugin installation and
  live report recording are not certified by this review.

Local evidence includes timestamped accessibility trees, computer-use screenshots
in the review transcript, CPU/memory samples, build output and the full original
settings backup. Personal paths/settings are intentionally excluded here.

## Handoff and restoration

The corrected Windows native apps and managed dependencies were packaged into
the dedicated local test folder. This remains an unmerged v0.18.0 test build,
not a new release; its `data/TEST-BUILD.json` records the exact branch and commit.
All 11 original application settings/plugin files were restored and verified by
SHA-256. The complete review after-state was archived separately. The final test
UI was terminated during background cleanup, its daemon accepted the CLI
shutdown command and exited, and no tablet app/daemon remained. That cleanup is
separate from the ordinary UI-close behavior verified earlier.
