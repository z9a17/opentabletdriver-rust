# Performance

This page describes the repeatable performance measurements (F04): what they time, how to run them, and the results on the development machine next to OpenTabletDriver 0.6.7 ([`736003e`](https://github.com/OpenTabletDriver/OpenTabletDriver/tree/736003ed72c8bbb28033b039d5a0bb76c344145c)). [Input latency](INPUT_LATENCY.md) explains the report path and its scheduling.

These are software measurements. They time the work between a report arriving and the cursor call returning. They do not include USB transfer, the HID read call, the display, or anything a camera would see.

## Results on the development machine

Ryzen 7 5800X3D (16 logical CPUs), Windows 11 Pro build 26200, Ultimate Performance power plan, 23 September 2026. This driver 0.7.5 against OpenTabletDriver 0.6.7 on .NET 8.0.31, both replaying the same trace with the development machine's osu! profile. Each number is the median of three runs unless noted; the complete output is in [perf/2026-09-23](perf/2026-09-23/summary.md).

Report processing, per report, p50 / p99.9. Both harnesses add about 25–30 ns of their own to every row:

| Work per report | This driver | OpenTabletDriver 0.6.7 |
| --- | --- | --- |
| Decode | 30 ns / 40 ns | 40 ns / 351 ns |
| Absolute mode, no filter | 70 ns / 100 ns | 110 ns / 461 ns |
| Relative mode | 60 ns / 100 ns | 130 ns / 491 ns |
| osu! profile with Radial Follow | 130 ns / 190 ns (built-in port) | 210 ns / 561 ns (RadialFollow DLL) |
| osu! profile with the unchanged RadialFollow DLL | 180 ns / 260 ns (through the .NET bridge) | 210 ns / 561 ns |
| Heap memory per report | none | 96 B; 312 B with the HID read's new buffer |
| `SendInput`, one cursor move | 86 µs / 423 µs | 86 µs / 394 µs |
| `SendInput` that leaves the cursor in place | 13 µs / 36 µs | not measured |

In this dataset, the whole session loop's p99.9 (932 ns) includes page faults in the harness's own timestamp buffer. A check after that was fixed measured p50 190 ns and p99.9 240 ns.

Paced replay at 200 reports per second with the osu! profile and `SendInput`, p50 / p99 / p99.9 over 18,000 reports:

| Stage | This driver | OpenTabletDriver 0.6.7 |
| --- | --- | --- |
| Wake after the report is signaled | 5.3 / 7.2 / 68 µs | 5.4 / 16 / 55 µs |
| Decode, filter and map | 0.95 / 1.3 / 2.5 µs | 2.5 / 3.7 / 11 µs |
| Signal to the `SendInput` call, per report | 6.2 / 8.4 / 75 µs | 7.9 / 19 / 67 µs |
| `SendInput` | 74 / 187 / 902 µs | 76 / 218 / 1305 µs |

With osu! stable open at its main menu (about 37 % of one core), in a single run of the same measurements ([osu](perf/2026-09-23/osu/summary.md)), processing times did not change. A `SendInput` cursor move took 28 µs against 32 µs, and one that left the cursor in place took 24 µs. In the paced replay, signal to the `SendInput` call took 6.4 / 8.4 / 9.6 µs against 8.2 / 28 / 74 µs, with a worst report of 10 µs against 184 µs.

Idle, with the tablet connected and the pen away, over 60 seconds after 10 seconds of warm-up. Context switches into the process's threads stand in for wakeups:

| Process | CPU (% of one core) | Context switches per second | Working set | Private memory | Threads |
| --- | --- | --- | --- | --- | --- |
| This driver, daemon | 0.002 % | 1.6 | 9.2 MB | 1.7 MB | 2 |
| This driver, panel open with the driver running | 0.004 % | 2.6 | 17.3 MB | 2.6 MB | 4 |
| OpenTabletDriver daemon | 0.11 % | 8.3 | 58.6 MB | 21.2 MB | 16 |
| OpenTabletDriver daemon, with the UX open | 0.079 % | 8.4 | 84.9 MB | 32.3 MB | 22 |
| OpenTabletDriver UX | 0.008 % | 0.2 | 230.9 MB | 164.2 MB | 18 |

What this means:

- Both drivers process a report in a tiny fraction of the 5 ms between reports. This driver takes about 60 % of upstream's time at the median and about a third at p99.9, and allocates nothing, so it never waits for garbage collection.
- `SendInput` takes most of the software time between a report and the cursor, in both drivers, because it is the same Windows call. Most of that time goes to Windows moving the cursor: about 85 µs per move on the desktop against 13 µs for a call that leaves the cursor in place. With osu! in front it drops to about 30 µs; osu! hides the Windows cursor and draws its own. Programs that watch mouse input, such as low-level hooks, raw-input listeners and overlays, add to it, so the number differs between machines.
- From the report being signaled to the `SendInput` call, this driver was faster at the median in every run: 6.1–6.2 µs against 7.9–8.0 µs. At p99 it was lower in two of three runs and tied in the third (8.0–18.6 µs against 10.2–19.4 µs). The rarest delays are not settled: at p99.9 the three runs were mixed, while the single run with osu! open favored this driver (9.6 µs against 74 µs). A handful of reports per run decide these, so they need more runs before they support a claim.

## Run the measurements

From the repository root on Windows, with the Rust toolchain, the .NET SDK, a clean OpenTabletDriver checkout at the pinned revision in `target/upstream/OpenTabletDriver`, and the unchanged RadialFollow 0.3.0 DLL (see [validation](parity/VALIDATION.md) for the checksum):

```powershell
pwsh -File scripts/bench.ps1 -Runs 3 -SendInput -ReplaySeconds 30
pwsh -File scripts/bench.ps1 -Runs 0 -Idle -UpstreamInstall <folder with OpenTabletDriver 0.6.7>
```

The script builds both harnesses, exports one workload, runs the Rust and upstream harnesses alternately, and writes `rust-N.json`, `upstream-N.json`, `environment.json`, optionally `idle.json`, and `summary.md` under `target/bench/<time>/`. `python scripts/bench-summary.py <directory>` rebuilds the summary.

- `-SendInput` and `-ReplaySeconds` move the cursor. They remove button flags, so nothing is clicked, but leave the mouse alone while they run.
- `-Idle` starts this driver, then its panel, then upstream's daemon, then its daemon with the UX, and samples each for a minute. It refuses to run while any of them is already running and stops only what it started. Each driver takes the tablet while it runs.
- `-UpstreamInstall` is a folder with upstream's released `OpenTabletDriver.Daemon.exe` and `OpenTabletDriver.UX.Wpf.exe`.

`-Runs 0` skips the harnesses, for example to sample idle processes only. `-Only TEXT` runs only cases whose names contain TEXT; the replay still runs.

Each harness also runs on its own: `cargo run --release --locked --example bench -- --help` for this driver, and `dotnet target/bench/upstream-bin/OtdUpstreamBench.dll --workload <directory>/workload.json` for upstream after `dotnet build bench/upstream/OtdUpstreamBench.csproj -c Release -o target/bench/upstream-bin`. The Rust harness's `--export-workload <directory>` writes the workload.

## What is measured

Both harnesses replay the same trace through their driver's own report path with the same profile, and write the same JSON schema.

**Trace.** `osu-synthetic-v1`: 20,000 deterministic 192-byte PTH-660 reports shaped like osu! play inside the profile's area: hovering jumps and taps, slider arcs, spinner circles and rises into the Sense band, with sensor noise and a drifting tilt. It is generated, not recorded. Its FNV-1a hash is in every result, and the upstream harness refuses a trace that does not match.

**Profile.** The development machine's osu! profile, as in `tests/golden/absolute-radial-follow.toml`: an 85 × 47.8125 mm area on the 2560 × 1440 primary monitor with clipping, the tip and eraser at 1 % pressure, and AbstractQbit's tablet-space Radial Follow (0.7039 / 0.302 mm, 0.302, 0.603, 0.201). Relative cases use 10 counts/mm with a 100 ms reset.

**Rust harness** (`examples/bench`). It compiles the driver's own modules: decoding, the report pipeline, the session loop, the DLL plugin chain, the .NET bridge, `SendInput` and the reader priority. Reports carry timestamps 5 ms apart, so time-based filters behave as on a 200 Hz device.

**Upstream harness** (`bench/upstream`). It builds against the pinned checkout's own projects and runs what upstream's daemon runs per report: `IntuosV2ReportParser.Parse`, the `DeviceReader` report event, the `InputDeviceTree` lock and `OutputMode.Read`. The pipeline is assembled as `DriverDaemon.SetSettings` does: the enabled filters, then a `BindingHandler` with the tip and eraser bound to the left button. It uses the daemon's settings: .NET 8, Release, TieredPGO and the default garbage collector. Pen buttons are left unbound; this driver has no pen-button bindings yet (B02).

| Case | Work per report |
| --- | --- |
| `empty` | The harness alone: timestamps and one indirect call. Every other row includes it. |
| `parse` | Decoding the report. |
| `absolute`, `relative` | Decoding, contact state, mapping and the output decision; the output goes to a no-op sink (Rust) or a pointer that keeps the position (upstream). |
| `absolute+radial_follow` (Rust) | The osu! profile with the built-in Radial Follow port. |
| `absolute+managed_radial_follow` | The osu! profile with the unchanged RadialFollow DLL: through the .NET bridge in Rust, in-process in upstream. |
| `absolute+managed_radial_follow+read_buffer` (upstream) | As above, plus a new 192-byte array per report, as upstream's HID read returns. Rust reads into one reused buffer. |
| `absolute+native_ema` (Rust) | The sample native filter DLL. |
| `session/absolute+radial_follow` (Rust) | The whole device-session loop over the trace, with its counters, timing and display checks, but no HID read. |
| `sendinput` | One `SendInput` cursor move per report, for the positions the osu! profile produces. |
| `…+sendinput` | The osu! profile's whole report path including `SendInput`. |

**Timing.** Each report is timed with the time-stamp counter (`lfence; rdtsc` before, `rdtscp; lfence` after), converted with a frequency measured against the performance counter. .NET has no `rdtsc` intrinsic, so the upstream harness calls two small machine-code stubs without a GC transition. The `empty` row shows each harness's own overhead. On the Ryzen 7 5800X3D the counter runs at 3.39 GHz but advances in steps of about 5 ns, so differences below 5 ns are not resolved.

**Rounds and runs.** Each case warms up for one second, pauses 250 ms so .NET can finish tiered compilation, warms up for another 2,000 reports, then times every report of the trace once. That timed pass repeats seven times in one process, with a fresh pipeline each time. The script runs each harness three times, alternating, so the summary shows noise within a run and between processes.

**Memory.** The Rust harness counts Rust heap allocations on the measuring thread during the timed pass. The upstream harness reports managed bytes allocated on that thread and garbage collections.

**Paced replay.** A device thread sets an event once per report interval, standing in for the HID class driver completing a read. The reader thread wakes, decodes, runs the osu! profile's pipeline and calls `SendInput`. Wake delay, pipeline work and output are timed separately. The Rust reader runs at the driver's time-critical priority. Upstream's runs at AboveNormal in a High priority class process, as its daemon's reader does. A timer paces the device thread, but its lateness does not count, because wake delay starts when the event is set.

**Idle.** With the tablet connected and the pen away, the script samples each process's CPU time, context switches of its threads (a proxy for wakeups), working set, private bytes, handles and threads over 60 seconds, after 10 seconds of warm-up.

## Regression thresholds

These are proposals from the first baseline; the performance budgets task (V05) decides what to adopt. Compare medians of three runs on the same machine and settings. In the baseline, the p50 of the pipeline cases moved by at most one 10 ns step between runs (up to 14 % for a 70 ns case), and by up to a third between the passes of one run. The replay's p50 moved by 2 % between runs, but its p99 by up to 2.5 times. Investigate:

- any Rust allocation in a native case, the session loop or the replay;
- a p50 of `absolute`, `relative`, `absolute+radial_follow` or `session/absolute+radial_follow` more than 20 % and at least 20 ns above the baseline;
- a p99 of the same cases more than 25 % and at least 20 ns above;
- a p50 from signal to the `SendInput` call in the paced replay more than 20 % above, or any merged report on an otherwise idle machine.

Replay p99 and p99.9, maximums, and anything that includes `SendInput` are reported but not gated: interrupts, the timer and other programs decide them. CI does not run these measurements, because shared runners are too noisy to gate on.

## Limits

- Multi-device mode waits for D02; this driver opens one tablet.
- Neither harness includes the HID read call. In upstream it also allocates a new array per report, shown in the `read_buffer` case.
- The .NET bridge runs plugins on the newest installed .NET major version (10.0 here); upstream's daemon runs them on .NET 8.
- `SendInput`'s cost depends on the system: other programs' input hooks, raw-input listeners, overlays and the monitor layout. Compare both drivers on the same machine, as these runs do.
- The trace is synthetic. Pen-to-display latency still needs a timestamped hardware measurement ([validation](parity/VALIDATION.md)).
