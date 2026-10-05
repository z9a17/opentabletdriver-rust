# Real-time report scheduling, 5 October 2026

The driver's own work per report takes microseconds ([performance audit](PERFORMANCE_AUDIT_2026-10-05.md)). Reports reach the cursor late when the report thread waits for a CPU. Release 0.16.10 runs the Linux and macOS report loop at real-time priority, as audio programs run their processing threads. Windows keeps its time-critical report thread, and its scheduling benchmark gains rows for heavier load and for MMCSS.

## What other drivers and low-latency programs do

| Program | Scheduling of the input or processing thread | Source |
| --- | --- | --- |
| OpenTabletDriver 0.6.7 | Process at the High priority class, device reader at AboveNormal: priority 14 on Windows. | [`DriverDaemon.cs`](https://github.com/OpenTabletDriver/OpenTabletDriver/blob/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.Daemon/DriverDaemon.cs#L719-L745), [`DeviceReader.cs`](https://github.com/OpenTabletDriver/OpenTabletDriver/blob/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver/Devices/DeviceReader.cs#L15-L21) |
| OpenTabletDriver 0.6.1 | "Set process priority to high by default and fix timers on Windows 11". The pull request's two commits set the High class and disallow power throttling. | [v0.6.1 release](https://github.com/OpenTabletDriver/OpenTabletDriver/releases/tag/v0.6.1), [#2746](https://github.com/OpenTabletDriver/OpenTabletDriver/pull/2746) |
| OpenTabletDriver pull request 5071 (open) | macOS: Mach time-constraint policy on the device reader and HidSharp's reader thread, 1 ms of computation within a 2 ms constraint, preemptible. Its timer thread uses the timer interval as the period. With 20 busy processes on an M1 Max, p99 report latency fell from 72 ms to 8.1 ms and missed timer ticks from 87 % to 1.0 %. | [#5071](https://github.com/OpenTabletDriver/OpenTabletDriver/pull/5071) |
| Hawku TabletDriver | No thread priority change. Raises the system timer resolution with `NtSetTimerResolution` for its smoothing timer. | [`TabletHandler.cpp`](https://github.com/hawku/TabletDriver/blob/master/TabletDriverService/TabletHandler.cpp) |
| PipeWire | Linux `SCHED_FIFO` through `RLIMIT_RTPRIO`, falling back to the Realtime portal or RTKit over D-Bus. Defaults: 88 for the server, 83 for clients, and a packaged rtprio limit of 95 for the `pipewire` group. | [`module-rt.c`](https://github.com/PipeWire/pipewire/blob/master/src/modules/module-rt.c), [`meson_options.txt`](https://github.com/PipeWire/pipewire/blob/master/meson_options.txt) |
| Windows audio | MMCSS: registered threads of the Pro Audio task run at 23–26 while their category stays within its CPU share. Chromium-based browsers and Apex Legends register Pro Audio threads; no input thread was found registered. | [MMCSS documentation](https://github.com/MicrosoftDocs/win32/blob/docs/desktop-src/ProcThread/multimedia-class-scheduler-service.md), [GamingPCSetup research](https://github.com/djdallmann/GamingPCSetup/blob/master/CONTENT/RESEARCH/WINSERVICES/README.md) |

This driver already ran its Windows report thread at time-critical priority, 15, with EcoQoS disabled ([input latency](INPUT_LATENCY.md#scheduling)). Linux and macOS ran their report loop at normal priority.

## Changes

**Linux.** While a session drives output, the report loop runs under `SCHED_FIFO` at priority 40. That is above every normal thread and below threaded interrupt handlers (50) and audio servers. `SCHED_RESET_ON_FORK` keeps child processes at normal priority. Without `CAP_SYS_NICE`, the loop uses the highest priority up to 40 that the user's `RLIMIT_RTPRIO` allows. If the limit is zero, it keeps normal priority and logs once how to grant a limit. The previous policy and priority return when the session ends, so the two-second rescan between sessions normally runs at normal priority. Restoring them first tries the original flags; if permission is denied, it retries while retaining `SCHED_RESET_ON_FORK`, because [clearing that flag requires `CAP_SYS_NICE`](https://github.com/torvalds/linux/blob/v6.18/kernel/sched/syscalls.c#L500). A failed restoration is logged. The kernel's real-time throttling still reserves 5 % of each second for normal threads by default. Capture mode is unchanged.

**macOS.** While a session drives output, the report thread uses the time-constraint policy with the reader values from pull request 5071: no period, 1 ms of computation within a 2 ms constraint, preemptible. The kernel returns a thread that keeps overrunning its budget to normal scheduling. The standard policy returns when the session ends.

**Both.** `OTD_RUST_REALTIME=0` keeps normal scheduling. The log states which scheduling the report loop uses.

**Windows.** The report thread is unchanged. `benchmark_reader_wake_latency_under_load` adds time-critical busy threads, and a reader registered with MMCSS's Pro Audio task, so MMCSS can be measured on the development machine before any decision. Each reader reports its initialization result before CPU load starts. A failed MMCSS registration skips that row with its error, even if the earlier availability probe succeeded. The startup wait has a ten-second timeout, and a disconnected coordinator cancels a late initialization.

## Measurements

`benchmark_report_loop_under_load` (in `crates/otd-linux/src/realtime.rs`) runs the driver's hidraw reader and session loop with the generic tablet parser and absolute output. A FIFO stands in for the hidraw node. A writer thread sends one 65-byte report every millisecond; each report moves the cursor and carries its send time. The benchmark measures from the send to the output callback, so it includes the reader's wakeup, the read, decoding, filtering and mapping. It excludes USB, `uinput` and the display. Rows with busy threads run one spinning normal-priority thread per logical CPU, standing in for a game or a build. The writer stands in for hardware that is always on time, at `SCHED_FIFO` 80, so run the benchmark as root or with an rtprio limit of 80. Otherwise the writer is delayed too, and a row the driver could not raise is labeled `real-time denied`.

    cargo test -p otd-linux --release --locked benchmark_report_loop_under_load -- --ignored --nocapture

Three runs, 3,000 reports per row, on the shared container described in the [audit's raw results](perf/2026-10-05/summary.md#environment) (4 logical CPUs, Linux 6.18):

| Busy threads | Report loop | p50 | p99 | Max | Over 1 ms |
| --- | --- | --- | --- | --- | --- |
| None | Normal | 54 / 56 / 52 µs | 166 / 131 / 143 µs | 0.19 / 1.14 / 1.99 ms | 0 / 1 / 2 |
| None | `SCHED_FIFO` | 45 / 57 / 60 µs | 125 / 137 / 180 µs | 0.17 / 0.79 / 0.51 ms | 0 / 0 / 0 |
| 4 | Normal | 0.02 / 0.02 / 14.9 ms | 329 / 95 / 121 ms | 329 / 120 / 151 ms | 1,331 / 1,312 / 1,951 |
| 4 | `SCHED_FIFO` | 27 / 20 / 18 µs | 603 / 304 / 42 µs | 0.61 / 0.32 / 0.12 ms | 0 / 0 / 0 |

At normal priority with every CPU busy, 44–65 % of reports came out more than 1 ms late, and some waited a third of a second. At real-time priority none did. With idle CPUs the two match. The roughly 50 µs median there is this virtual machine waking a halted CPU; with busy CPUs nothing halts, so the median falls. [Raw output](perf/2026-10-05/realtime.txt) also includes a smaller harness: a bare thread blocked in `poll` on a socket, without the session loop. It gave the same picture: p99 100–170 ms at normal priority and 36–56 µs under `SCHED_FIFO`.

Neither the macOS nor the Windows change was measured here. Pull request 5071's numbers above come from upstream's macOS reader, not this driver's.

## Decisions

- **MMCSS stays off on Windows.** On the development machine the time-critical thread already woke with p99 134 µs behind a busy above-normal thread on every CPU ([input latency](INPUT_LATENCY.md#scheduling)). MMCSS would also lift it above time-critical threads. But MMCSS drops a category's threads to priorities 1–7, below normal threads, once they use up the category's CPU share. That share is shared with every other Pro Audio thread, such as a browser's or a DAW's. MMCSS also caps registered threads per system: Steinberg [documents a default of 32](https://helpcenter.steinberg.de/hc/en-us/articles/13338094735762-Error-message-on-Windows-MMCSS-priority-cannot-be-set), after which audio applications fail to register. The extended benchmark shows whether the gain is worth that on real hardware.
- **No coalescing of queued cursor moves.** When the loop falls behind, it could send only the newest of several queued positions. That saves one output call per skipped report, about 30 µs of CPU time for `SendInput`, and only while the loop is already behind. Upstream outputs every report, and drawing programs and raw-input games use the intermediate points. The session summary's `reads found a report already waiting` shows how often the loop falls behind.
- **No CPU idle-state limit.** Linux's `/dev/cpu_dma_latency` and Windows power plans can keep CPUs out of deep idle states. That shortens waking from idle on real hardware, but every CPU then uses more power while idle, and the Linux interface needs root. This virtual machine has no cpuidle driver, so the effect could not be measured.
- **No RTKit or portal requests.** PipeWire falls back to these D-Bus services when `RLIMIT_RTPRIO` is zero. The driver would need a D-Bus client; granting the limit is the documented route instead.
- **The Linux installer does not grant real-time limits.** A limits rule is a system-wide policy, which the user or distribution decides. Many distributions grant one with their audio packages.
- **Earlier rejections stand.** Busy-polling the HID read costs a CPU core. Fewer HID input buffers risk losing button reports.

## Granting real-time priority on Linux

Check the current limit with `ulimit -r`. If it is 0, add a file such as `/etc/security/limits.d/99-opentabletdriver-rust.conf` with the line `<user> - rtprio 40`, replacing `<user>` with your user name, then sign out and in. The driver logs `Report loop runs at real-time priority 40 (SCHED_FIFO).` when it applies.

## Limits

- No tablet, hidraw node, `uinput` device, game or display was involved. The container runs as root, so `SCHED_FIFO` came from `CAP_SYS_NICE`. A user without a limit was tested as `nobody`: the loop kept normal priority and logged the hint. The container cannot raise `RLIMIT_RTPRIO`, so the path that uses a limit below 40 did not run.
- Restoration regression tests cover retaining reset-on-fork without privilege, exact restoration with privilege, and reporting failures. On Windows, an offline permission model also exercised the actual Linux guard with limits of 0, 20, 40 and 95, with `CAP_SYS_NICE`, with the opt-out, and across repeated sessions. All cases restored the saved policy and priority. The updated Linux code and tests cross-check successfully; these checks do not execute the Linux kernel permission path.
- The macOS change was type-checked and linted for Intel and Apple Silicon. Its test was not run; no Mac was available.
- Four targeted Windows tests pass: the existing priority guard test and startup regressions for failed registration, successful initialization, and a disconnected coordinator. The extended CPU-saturating Windows benchmark was not run.
- Real-time priority shortens waiting for a CPU. It does not change USB polling, compositor or display latency.
