# Performance measurements

- Rust commit `36d584a9b0916e8ee0a5acba456ea519c3808175` (uncommitted changes); OpenTabletDriver `736003ed72c8bbb28033b039d5a0bb76c344145c`
- Microsoft Windows 11 Pro 10.0.26200 build 26200; AMD Ryzen 7 5800X3D 8-Core Processor, 16 logical CPUs; power plan Ultimate Performance
- Upstream runtime .NET 8.0.31
- Trace `osu-synthetic-v1`: 20000 reports, FNV-1a `719b88e0c722d0af`; 1 runs per harness, 3 timed passes per case

Per-report time is the median over runs of each run's median over timed passes; the range between runs follows in brackets.
Every row includes the harness's own timing overhead, shown in the first row. Upstream's memory column is managed bytes allocated per report.

| Case | Rust p50 | OTD p50 | Rust p99 | OTD p99 | Rust p99.9 | OTD p99.9 | Rust memory | OTD memory |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| Harness alone (included in every row) | 30 ns | 30 ns | 30 ns | 30 ns | 30 ns | 30 ns | 0 allocations | 0 B |
| Decode one report | 30 ns | 40 ns | 40 ns | 240 ns | 40 ns | 391 ns | 0 allocations | 96 B |
| Absolute mode, no filter | 60 ns | 110 ns | 80 ns | 341 ns | 110 ns | 521 ns | 0 allocations | 96 B |
| Relative mode | 60 ns | 130 ns | 80 ns | 341 ns | 120 ns | 541 ns | 0 allocations | 96.3 B |
| osu! profile: built-in Radial Follow / unchanged RadialFollow DLL | 130 ns | 210 ns | 170 ns | 431 ns | 190 ns | 611 ns | 0 allocations | 96 B |
| osu! profile, both with the unchanged RadialFollow DLL | 180 ns | 210 ns | 220 ns | 431 ns | 270 ns | 611 ns | 0 allocations | 96 B |
| osu! profile with a new read buffer per report | — | 220 ns | — | 491 ns | — | 751 ns | — | 312 B |
| Native EMA filter DLL | 80 ns | — | 110 ns | — | 150 ns | — | 0 allocations | — |
| Whole session loop, osu! profile, no HID read or SendInput | 190 ns | — | 220 ns | — | 250 ns | — | 0 allocations | — |
| SendInput alone | 28 µs | 32 µs | 91 µs | 89 µs | 188 µs | 160 µs | 0 allocations | 0 B |
| SendInput without cursor motion | 24 µs | — | 72 µs | — | 144 µs | — | 0 allocations | — |
| osu! profile with SendInput | 31 µs | 33 µs | 87 µs | 93 µs | 124 µs | 169 µs | 0 allocations | 96 B |

## Mean, worst report and thread CPU time

The worst report of a pass usually coincides with an interrupt or a context switch. Thread CPU time per report also covers the harness's untimed loop work.

| Case | Rust mean | OTD mean | Rust max | OTD max | Rust CPU | OTD CPU |
| --- | --- | --- | --- | --- | --- | --- |
| Harness alone (included in every row) | 26 ns | 25 ns | 341 ns | 90 ns | 54 ns | 54 ns |
| Decode one report | 30 ns | 46 ns | 351 ns | 1.2 µs | 58 ns | 73 ns |
| Absolute mode, no filter | 65 ns | 116 ns | 1.5 µs | 2.5 µs | 92 ns | 141 ns |
| Relative mode | 70 ns | 142 ns | 18 µs | 56 µs | 90 ns | 166 ns |
| osu! profile: built-in Radial Follow / unchanged RadialFollow DLL | 124 ns | 227 ns | 2.4 µs | 2.6 µs | 148 ns | 246 ns |
| osu! profile, both with the unchanged RadialFollow DLL | 179 ns | 227 ns | 2.6 µs | 2.6 µs | 202 ns | 246 ns |
| osu! profile with a new read buffer per report | — | 231 ns | — | 2.8 µs | — | 256 ns |
| Native EMA filter DLL | 83 ns | — | 2.4 µs | — | 108 ns | — |
| Whole session loop, osu! profile, no HID read or SendInput | 193 ns | — | 2.1 µs | — | 197 ns | — |
| SendInput alone | 39 µs | 41 µs | 1852 µs | 1331 µs | 12 µs | 14 µs |
| SendInput without cursor motion | 29 µs | — | 270 µs | — | 13 µs | — |
| osu! profile with SendInput | 40 µs | 42 µs | 276 µs | 1524 µs | 13 µs | 15 µs |

## Noise

How far the p50 moved: the widest range between the timed passes of one run, and the range between runs. Timestamps advance in steps of about 5 ns on this CPU, so short cases show large percentages.

| Case | Rust passes | Rust runs | OTD passes | OTD runs |
| --- | --- | --- | --- | --- |
| Harness alone (included in every row) | 0 % | — | 33 % | — |
| Decode one report | 0 % | — | 25 % | — |
| Absolute mode, no filter | 17 % | — | 9 % | — |
| Relative mode | 17 % | — | 8 % | — |
| osu! profile: built-in Radial Follow / unchanged RadialFollow DLL | 8 % | — | 5 % | — |
| osu! profile, both with the unchanged RadialFollow DLL | 6 % | — | 5 % | — |
| osu! profile with a new read buffer per report | — | — | 9 % | — |
| Native EMA filter DLL | 12 % | — | — | — |
| Whole session loop, osu! profile, no HID read or SendInput | 0 % | — | — | — |
| SendInput alone | 12 % | — | 5 % | — |
| SendInput without cursor motion | 3 % | — | — | — |
| osu! profile with SendInput | 0 % | — | 6 % | — |

## Startup

First report after creating the case (µs) and the time to create it (ms), first timed pass of the first run:

| Harness | Case | First report | Setup |
| --- | --- | --- | --- |
| Rust | empty | 0.1 µs | 0 ms |
| Rust | parse | 0.2 µs | 0 ms |
| Rust | absolute | 3.9 µs | 0 ms |
| Rust | absolute+radial_follow | 0.3 µs | 0 ms |
| Rust | relative | 0.3 µs | 0 ms |
| Rust | absolute+native_ema | 0.6 µs | 0.4 ms |
| Rust | absolute+managed_radial_follow | 1834.1 µs | 71.4 ms |
| Rust | sendinput | 60.7 µs | 0 ms |
| Rust | sendinput_still | 39.3 µs | 0 ms |
| Rust | absolute+radial_follow+sendinput | 48.2 µs | 0 ms |
| OTD | empty | 14.2 µs | 0 ms |
| OTD | parse | 466.4 µs | 0.1 ms |
| OTD | absolute | 4107 µs | 82.3 ms |
| OTD | relative | 612.8 µs | 0.8 ms |
| OTD | absolute+managed_radial_follow | 392.3 µs | 2.4 ms |
| OTD | absolute+managed_radial_follow+read_buffer | 51.8 µs | 0.4 ms |
| OTD | sendinput | 7728.9 µs | 0.5 ms |
| OTD | absolute+managed_radial_follow+sendinput | 142.5 µs | 0.8 ms |

## Paced replay

200 reports per second, SendInput on. Rust's reader runs at time-critical priority; upstream's at AboveNormal in a High priority class process, as its daemon does.

| Harness | Stage | p50 | p99 | p99.9 | max |
| --- | --- | --- | --- | --- | --- |
| Rust | Wake after the report is signaled | 5.5 µs | 7.2 µs | 8.4 µs | 9.3 µs |
| Rust | Decode, filter, map | 972 ns | 1.4 µs | 1.6 µs | 2.5 µs |
| Rust | Signal to the SendInput call, per report | 6.4 µs | 8.4 µs | 9.6 µs | 10 µs |
| Rust | SendInput | 51 µs | 129 µs | 1307 µs | 1601 µs |
| Rust | Reports processed / merged because the reader was late | 6000 / 0 | | | |
| OTD | Wake after the report is signaled | 5.5 µs | 23 µs | 68 µs | 95 µs |
| OTD | Decode, filter, map | 2.7 µs | 3.7 µs | 16 µs | 178 µs |
| OTD | Signal to the SendInput call, per report | 8.2 µs | 28 µs | 74 µs | 184 µs |
| OTD | SendInput | 55 µs | 146 µs | 1431 µs | 1988 µs |
| OTD | Reports processed / merged because the reader was late | 6000 / 0 | | | |
