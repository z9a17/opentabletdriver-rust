# Performance audit: 0.15.3

This audit asked whether anything became slower, heavier or broken between the first measured release (0.7.5) and 0.15.2, measured against the project's goal: OpenTabletDriver's feel with less latency and less overhead. The source baseline was 0.15.2, `36ad033eb0e0f25810496994e987260ff3577e97`. Upstream comparisons use OpenTabletDriver 0.6.7, [`736003e`](https://github.com/OpenTabletDriver/OpenTabletDriver/tree/736003ed72c8bbb28033b039d5a0bb76c344145c).

Development machine: Ryzen 7 5800X3D (16 logical CPUs), Windows 11 Pro build 26200, Ultimate Performance power plan, 1 October 2026. Nothing in the audit moved the cursor, injected input, opened a window or used a tablet. A released 0.14.2 daemon kept running on the machine throughout; it was only sampled with Windows performance counters. The raw results are in [perf/2026-10-01](perf/2026-10-01/summary.md).

## Method

- Each version's own benchmark harness (`examples/bench`, see [performance](PERFORMANCE.md)) was built from 0.7.6 (`6d470b2`, the first commit with the harness), 0.12.0, 0.13.1, 0.14.2, 0.14.6 and 0.15.0. The six ran alternately, three times each: five timed passes per case over the 20,000-report osu! trace, and a 15-second paced replay at 200 Hz without `SendInput`.
- A step in the results was bisected through the merges between 0.7.6 and 0.12.0.
- Single system calls and pipeline stages were timed in a separate harness linked against the driver's core.
- Upstream's harness (`bench/upstream`) ran alternately with this driver's on the same day.
- Idle wakeups of the running 0.14.2 daemon were read from the `Thread\Context Switches/sec` counter.

## What changed

| Finding | Change | Evidence |
| --- | --- | --- |
| Since 0.8.0 the daemon's control thread woke every 50 ms to poll its workers, even with no client and the driver stopped. The running 0.14.2 daemon measured 19.8 to 20.0 context switches a second. 0.7.5 measured 1.6 and OpenTabletDriver's daemon 8.3. | The control thread sleeps until a client connects or something needs it. Workers wake it after each notice, log line and exit; Ctrl+C wakes it. A worker marks itself finished before its thread exits, so a wakeup never misses a finished worker. | A new test holds the control wait idle for 500 ms with no poll, then sees exactly one poll per wake. The whole daemon was not re-measured, because the running 0.14.2 daemon owned the per-user control pipe. |
| Since 0.9.0 the HID reader checked the stop event twice per report, and since the first release it reset the read event before each read. Each of these system calls costs about 0.2 µs on the development machine: `WaitForSingleObject` with no wait 217 ns, `ResetEvent` 206 ns. Together they took longer than decoding, filtering and mapping a report. | Stop is checked only when a read completes at once, because the read wait already gives Stop precedence. `ReadFile` resets the event itself when it starts the read ([Microsoft](https://learn.microsoft.com/windows/win32/api/fileapi/nf-fileapi-readfile)). | About 0.64 µs less work per report, from the timed calls. Not measured with a tablet. |
| Since 0.9.0, .NET filters cross between .NET and Rust three times per report: built-in filters, transform, then output. Each crossing exports the report, enters Rust and decodes it again. With the unchanged RadialFollow 0.3.0 DLL a report took 531 ns at the median and 1.02 µs at p99. In 0.7.5 it took 180 ns and 240 ns, and OpenTabletDriver takes 210 to 230 ns and 0.57 to 0.72 µs on the same day. | The host runs its built-in filters before entering .NET, since they always come first. Without post-transform .NET filters, transform and output share one crossing. New bridge entry points `DispatchGraph2` and `TickGraph2` carry this. The old entry points still work, so a mismatched bridge and driver fall back to three crossings. | 381 ns at the median and 832 ns at p99, three alternating runs each, with no Rust allocation. Still slower than OpenTabletDriver; the remaining cost is copying each report between the two runtimes. All .NET bridge integration tests pass with the new bridge. |
| The benchmark's .NET case passed no raw packet. Since 0.13 the bridge rejects such a pen report, so the case timed the rejection: 150 ns and three allocations, without running the plugin. | Every pipeline case passes the raw packet, as the driver does. | The .NET numbers above come from the corrected case. |
| Companion-tablet discovery woke every 2 seconds only to notice finished sessions. | A companion session wakes discovery when it ends. Discovery otherwise sleeps until a device change, with the 60-second fallback rescan. | By construction; no second tablet was connected. |
| The Linux reader woke every 100 ms to check its stop flag, and waited between device scans in 100 ms steps. | The driver has one thread, and `poll` is never restarted after a signal handler, so SIGINT and SIGTERM end the wait directly. The wait is capped at the session's own one second, and the scan pause is one interruptible two-second wait. | Compiles for Linux; not run on Linux. |
| Since 0.15.1 the startup update check restored a panel minimized to the tray, brought it to the front and showed a dialog. When the check finished during a full-screen game, it could take focus from the game. | The prompt opens only if the panel or one of its windows is in front. Otherwise the Console notes the release, the tray icon shows a notification, and the prompt opens when the panel is next activated. Windows holds the notification back during quiet time and full-screen games; clicking it opens the panel. | Not run in a live panel. |
| Five unit tests and one .NET integration test failed on 0.15.2. The tablet discovery test from 0.14.6 used a fixture that the database rejects, so it never checked anything. | Each test now matches the intended behavior: thresholds checked against the tablet that runs the profile; non-pen reports passed through like upstream; field warnings before duplicate-name errors; one more parser type; unknown saved plugin settings ignored, as upstream does. The discovery fixture is valid, and the test asserts that it loaded. | All 244 regular tests pass. |

## Measured results

Per-report time at the median in each version's own harness, median of three runs. Every row includes about 20 ns of harness time.

| Case | 0.7.6 | 0.12.0 | 0.13.1 | 0.14.2 | 0.14.6 | 0.15.0 |
| --- | --- | --- | --- | --- | --- | --- |
| Absolute mode, no filter | 60 ns | 100 ns | 100 ns | 100 ns | 100 ns | 100 ns |
| osu! profile, built-in Radial Follow | 130 ns | 150 ns | 150 ns | 150 ns | 160 ns | 160 ns |
| Relative mode | 60 ns | 80 ns | 80 ns | 80 ns | 80 ns | 80 ns |
| Native EMA filter DLL | 80 ns | 110 ns | 110 ns | 120 ns | 120 ns | 120 ns |
| Whole session loop | 210 ns | 200 ns | 230 ns | 240 ns | 250 ns | 240 ns |
| Paced 200 Hz, decode to output decision | 0.90 µs | 0.88 µs | 0.91 µs | 1.05 µs | 1.04 µs | 1.04 µs |
| Paced 200 Hz, signal to output call | 6.29 µs | 6.21 µs | 6.28 µs | 6.50 µs | 6.35 µs | 6.38 µs |

No case allocated in any version. The step from 0.7.6 to 0.12.0 comes from `cf55323`, which replaced the direct pipeline with the synchronous report graph that .NET filters need. In a harness that calls both versions' cores the same way, the cost is 10 ns (39 against 49 ns); the larger difference in the table above comes from how each harness build lays out its code. Two likely causes were tested and ruled out: shrinking the 1,040-byte report value struct and replacing the graph's dynamic calls with direct ones changed nothing. This was left as is. The wake-up delay after a report is signaled stayed at 5.3 µs in every version.

Against OpenTabletDriver 0.6.7 on the same day, with this release's code and three alternating runs ([details](perf/2026-10-01/summary.md)):

| Per report | This driver, p50 / p99 / p99.9 | OpenTabletDriver, p50 / p99 / p99.9 |
| --- | --- | --- |
| Absolute mode, no filter | 100 / 160 / 210 ns | 110 / 431 / 621 ns |
| osu! profile, built-in Radial Follow port / RadialFollow DLL | 160 / 230 / 270 ns | 210 / 531 / 761 ns |
| osu! profile, both with the unchanged RadialFollow DLL | 381 / 802 / 1,300 ns | 210 / 531 / 761 ns |
| Paced 200 Hz, signal to the output call | 6.5 / 9.2 / 26 µs | 7.8 / 12 / 64 µs |

The native path stays ahead of upstream at every percentile, and allocates nothing where upstream allocates 96 bytes per report. The .NET path is faster than before this release but still slower than upstream running the same DLL.

## Left as is

- The 10 ns pipeline cost above, until a change that keeps report-graph behavior can be measured.
- An absolute position that has not changed is not sent again, while upstream sends every report. A physical mouse can therefore move the cursor while the pen hovers still (unchanged since 0.7).
- PTH-660 reports with a position or pressure beyond the tablet's range are dropped, where upstream passes them on (BC-06, unchanged).
- Pen-to-display latency still needs a hardware measurement. The Linux changes were not run on Linux, and the macOS driver is unchanged.

## Checks run for this release

- `cargo test --workspace --exclude otd-macos`: 244 passed.
- Ignored integration tests with the new bridge, the unchanged RadialFollow 0.3.0 DLL and the fixture DLLs: all nine .NET bridge tests, the native plugin round trip, and the live updater install (into a temporary folder) passed. The debugger preview render was not run.
- `cargo check` for `x86_64-unknown-linux-gnu` (`otd-linux`) and `aarch64-apple-darwin` (`otd-macos`).
- Clippy: no new warnings; nine existing style warnings remain.
