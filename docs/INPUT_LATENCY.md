# Input latency

This page describes how a pen report becomes cursor input, what 0.7.0 changed after comparing that path with OpenTabletDriver 0.6.7 ([`736003e`](https://github.com/OpenTabletDriver/OpenTabletDriver/tree/736003ed72c8bbb28033b039d5a0bb76c344145c)), and how to measure it. Tracking issue: [#16](https://github.com/z9a17/opentabletdriver-rust/issues/16).

## Report path

One thread handles a device session. It waits on an overlapped `ReadFile` for the 192-byte pen collection, then decodes the report, runs the built-in Radial Follow port and any DLL filters, maps the position and calls `SendInput` once. A cursor move and a button change travel in the same `SendInput` event, as in upstream's `WindowsVirtualMouse`. There is no queue between these steps; while the thread works, the Windows HID class driver buffers new reports.

## Scheduling

Upstream's daemon sets its process to the High priority class and its device reader thread to AboveNormal ([DriverDaemon.cs](https://github.com/OpenTabletDriver/OpenTabletDriver/blob/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.Daemon/DriverDaemon.cs#L719-L745), [DeviceReader.cs](https://github.com/OpenTabletDriver/OpenTabletDriver/blob/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver/Devices/DeviceReader.cs#L15-L21)), an effective priority of 14. Until 0.7.0 the Rust report thread ran at normal priority, 8.

The report thread now runs at time-critical priority, 15, for the whole device session. That is the highest priority available without real-time rights. The rest of the process, including the panel's window, keeps normal priority. The thread also opts out of EcoQoS, which Windows 11 can apply to background processes such as a panel minimized to the tray. The thread waits on the HID read between reports and works for microseconds per report, so running it ahead of other threads costs them almost nothing.

The effect shows when every CPU is busy. The benchmark below measures how long a thread blocked on an event, like the reader waiting for a report, takes to run after the event is signaled, while one busy thread per logical CPU keeps the processor full. Ryzen 7 5800X3D (16 logical CPUs), Windows 11 build 26200, Ultimate Performance power plan, 500 wakeups per row:

| Busy threads on every CPU | Reader at normal priority | Reader at time-critical priority |
| --- | --- | --- |
| Normal priority | p50 3.0 µs, p99 4.1 µs, max 7.1 µs | p50 3.2 µs, p99 4.3 µs, max 12.1 µs |
| Above-normal priority | p50 2.5 ms, p99 14.3 ms, max 15.3 ms; 369 of 500 over 1 ms | p50 3.4 µs, p99 134 µs, max 202 µs |

Results vary between runs. In an earlier prototype of the same measurement, single normal-priority wakeups behind normal-priority load took up to 7.6 ms. To reproduce, run the following; it keeps every CPU busy for about ten seconds:

    cargo test --release --locked benchmark_reader_wake_latency_under_load -- --ignored --nocapture --test-threads 1

This measures Windows scheduling alone. It includes no USB transfer, no report processing and no display latency.

## Hover tracking

The PTH-660's report descriptor defines byte 1 of report `0x10` as follows (vendor page `0xFF0D` mirrors the digitizer usages):

| Bit | Usage | Meaning |
| --- | --- | --- |
| `0x40` | `0x36` Sense | the pen is detected at any hover height the tablet reports |
| `0x20` | `0x32` In Range | a lower hover band; upstream calls it `NearProximity` |
| `0x10` | `0x3C` Invert | the eraser end faces the tablet |
| `0x08` | `0x45` Eraser | eraser switch |
| `0x04`, `0x02` | `0x5A`, `0x44` | secondary and primary barrel buttons |
| `0x01` | `0x42` Tip Switch | tip contact |

Upstream's `IntuosV2Report` always carries a position, and neither its output modes nor its bindings check `NearProximity`, so the cursor follows the pen through the whole Sense band. Through 0.6.0 this driver ignored reports without In Range, so the cursor started following later as the pen approached and stopped at a lower hover height than in OpenTabletDriver. Since 0.7.0 the cursor follows while In Range or Sense is set.

A report with neither bit set is treated as "not detected" and moves nothing. Upstream would use its coordinates. Such a report has not been captured on this tablet yet; if the tablet sends one with zeroed coordinates when the pen leaves, upstream would move the cursor to the area's corner.

## Tip threshold

Upstream presses the tip or eraser binding when `pressure / MaxPressure * 100 > threshold`, computed in single precision; at a 100 % threshold, full pressure counts. For the common 1 % threshold the first pressing raw value is 82. Through 0.6.0 this driver converted 1 % to 83. Profiles saved by those versions keep 83, which **Pen Settings** now shows as 1.01 %. Enter 1 % again, or import the OpenTabletDriver settings again, to use 82.

## Other work on the report thread

- While reports flow, the thread compares a display fingerprint (the virtual screen and the monitor count) once a second. It re-reads the monitor layout only when the fingerprint changes, and always after a second without reports. Reading the fingerprint costs 0.36 µs; the full layout read costs 2.46 µs, allocates, and enumerates monitors (`cargo test --release --locked benchmark_display_checks -- --ignored --nocapture`).
- Any HID device arriving or leaving, such as a keyboard or headset, used to make the thread enumerate every HID device in the system before checking whether the tablet was still there; reports waited while it ran. It now opens the tablet's own device path without access rights.

## Session summary

When a device session ends (**Stop driver**, unplugging the tablet, or Ctrl+C in the console), the driver logs one line, which the panel shows in the Console:

    Processed <reports> reports; read to output p50 <µs> us, p99 <µs> us, max <µs> us; <count> reads found a report already waiting

The times run from a completed read to the return of `SendInput`, in whole microseconds. The last number counts reads whose report was already queued, meaning the thread fell at least one report behind the tablet. The line does not include USB transfer time, the time the thread takes to wake up, or display latency.

## Not yet measured

End-to-end pen-to-cursor latency, and a comparison with OpenTabletDriver on the same machine, need a timestamped hardware measurement ([validation plan](parity/VALIDATION.md)). Hover-height tracking still needs a side-by-side check on the tablet ([hardware validation](HARDWARE_VALIDATION.md)).
