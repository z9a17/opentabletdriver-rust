# Performance measurements

- Rust commit `36ad033eb0e0f25810496994e987260ff3577e97` (uncommitted changes); OpenTabletDriver `736003ed72c8bbb28033b039d5a0bb76c344145c`
- Microsoft Windows 11 Pro 10.0.26200 build 26200; AMD Ryzen 7 5800X3D 8-Core Processor, 16 logical CPUs; power plan Ultimate Performance
- Upstream runtime .NET 8.0.31
- Trace `osu-synthetic-v1`: 20000 reports, FNV-1a `719b88e0c722d0af`; 3 runs per harness, 7 timed passes per case

Per-report time is the median over runs of each run's median over timed passes; the range between runs follows in brackets.
Every row includes the harness's own timing overhead, shown in the first row. Upstream's memory column is managed bytes allocated per report.

| Case | Rust p50 | OTD p50 | Rust p99 | OTD p99 | Rust p99.9 | OTD p99.9 | Rust memory | OTD memory |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| Harness alone (included in every row) | 30 ns | 30 ns | 30 ns | 30 ns (30 ns–40 ns) | 30 ns | 40 ns (30 ns–40 ns) | 0 allocations | 0 B |
| Decode one report | 30 ns | 40 ns | 40 ns | 351 ns (321 ns–371 ns) | 40 ns | 521 ns (501 ns–521 ns) | 0 allocations | 96 B |
| Absolute mode, no filter | 100 ns (100 ns–110 ns) | 110 ns (110 ns–120 ns) | 160 ns (160 ns–170 ns) | 431 ns (411 ns–441 ns) | 210 ns (210 ns–220 ns) | 621 ns (581 ns–641 ns) | 0 allocations | 96.2 B |
| Relative mode | 90 ns | 140 ns | 150 ns (130 ns–150 ns) | 451 ns (431 ns–481 ns) | 180 ns (170 ns–200 ns) | 711 ns (701 ns–751 ns) | 0 allocations | 96 B |
| osu! profile: built-in Radial Follow / unchanged RadialFollow DLL | 160 ns | 210 ns (210 ns–220 ns) | 230 ns | 531 ns (511 ns–571 ns) | 270 ns (270 ns–290 ns) | 761 ns (741 ns–822 ns) | 0 allocations | 96 B |
| osu! profile, both with the unchanged RadialFollow DLL | 381 ns (371 ns–391 ns) | 210 ns (210 ns–220 ns) | 802 ns (792 ns–832 ns) | 531 ns (511 ns–571 ns) | 1.3 µs (1.1 µs–1.3 µs) | 761 ns (741 ns–822 ns) | 0 allocations | 96 B |
| osu! profile with a new read buffer per report | — | 230 ns (220 ns–240 ns) | — | 641 ns (631 ns–651 ns) | — | 902 ns (892 ns–962 ns) | — | 312 B |
| Native EMA filter DLL | 130 ns | — | 210 ns (200 ns–210 ns) | — | 270 ns (250 ns–280 ns) | — | 0 allocations | — |
| Whole session loop, osu! profile, no HID read or SendInput | 250 ns | — | 301 ns (290 ns–321 ns) | — | 361 ns (341 ns–391 ns) | — | 0 allocations | — |

## Mean, worst report and thread CPU time

The worst report of a pass usually coincides with an interrupt or a context switch. Thread CPU time per report also covers the harness's untimed loop work.

| Case | Rust mean | OTD mean | Rust max | OTD max | Rust CPU | OTD CPU |
| --- | --- | --- | --- | --- | --- | --- |
| Harness alone (included in every row) | 25 ns (25 ns–26 ns) | 29 ns (28 ns–29 ns) | 70 ns (40 ns–90 ns) | 280 ns (70 ns–361 ns) | 53 ns (52 ns–55 ns) | 56 ns (55 ns–57 ns) |
| Decode one report | 32 ns (31 ns–32 ns) | 45 ns (45 ns–46 ns) | 361 ns (301 ns–361 ns) | 1.6 µs (952 ns–2.1 µs) | 58 ns (55 ns–58 ns) | 71 ns (70 ns–72 ns) |
| Absolute mode, no filter | 113 ns (112 ns–114 ns) | 126 ns (124 ns–130 ns) | 3.1 µs (2.7 µs–4.2 µs) | 4.5 µs (3.2 µs–9.1 µs) | 137 ns (137 ns–139 ns) | 152 ns (149 ns–157 ns) |
| Relative mode | 93 ns (91 ns–94 ns) | 155 ns (146 ns–159 ns) | 3.5 µs (2.6 µs–7.2 µs) | 2.8 µs (2.5 µs–8.9 µs) | 118 ns (116 ns–118 ns) | 180 ns (170 ns–183 ns) |
| osu! profile: built-in Radial Follow / unchanged RadialFollow DLL | 165 ns (164 ns–168 ns) | 220 ns (217 ns–231 ns) | 3.0 µs (2.6 µs–8.8 µs) | 3.2 µs (2.9 µs–5.8 µs) | 189 ns (188 ns–193 ns) | 243 ns (241 ns–256 ns) |
| osu! profile, both with the unchanged RadialFollow DLL | 399 ns (394 ns–411 ns) | 220 ns (217 ns–231 ns) | 10 µs (3.2 µs–10 µs) | 3.2 µs (2.9 µs–5.8 µs) | 423 ns (418 ns–436 ns) | 243 ns (241 ns–256 ns) |
| osu! profile with a new read buffer per report | — | 247 ns (238 ns–257 ns) | — | 6.0 µs (3.1 µs–8.9 µs) | — | 272 ns (262 ns–281 ns) |
| Native EMA filter DLL | 138 ns (136 ns–142 ns) | — | 3.0 µs (2.4 µs–9.4 µs) | — | 163 ns (161 ns–167 ns) | — |
| Whole session loop, osu! profile, no HID read or SendInput | 246 ns (246 ns–248 ns) | — | 2.3 µs (2.2 µs–2.5 µs) | — | 251 ns (251 ns–252 ns) | — |

## Noise

How far the p50 moved: the widest range between the timed passes of one run, and the range between runs. Timestamps advance in steps of about 5 ns on this CPU, so short cases show large percentages.

| Case | Rust passes | Rust runs | OTD passes | OTD runs |
| --- | --- | --- | --- | --- |
| Harness alone (included in every row) | 33 % | 0 % | 33 % | 0 % |
| Decode one report | 0 % | 0 % | 25 % | 0 % |
| Absolute mode, no filter | 46 % | 10 % | 73 % | 9 % |
| Relative mode | 22 % | 0 % | 57 % | 0 % |
| osu! profile: built-in Radial Follow / unchanged RadialFollow DLL | 6 % | 0 % | 9 % | 5 % |
| osu! profile, both with the unchanged RadialFollow DLL | 54 % | 5 % | 9 % | 5 % |
| osu! profile with a new read buffer per report | — | — | 41 % | 9 % |
| Native EMA filter DLL | 31 % | 0 % | — | — |
| Whole session loop, osu! profile, no HID read or SendInput | 4 % | 0 % | — | — |

## Startup

First report after creating the case (µs) and the time to create it (ms), first timed pass of the first run:

| Harness | Case | First report | Setup |
| --- | --- | --- | --- |
| Rust | empty | 0 µs | 0 ms |
| Rust | parse | 2.2 µs | 0 ms |
| Rust | absolute | 5.1 µs | 0 ms |
| Rust | absolute+radial_follow | 0.5 µs | 0 ms |
| Rust | relative | 0.4 µs | 0 ms |
| Rust | absolute+native_ema | 3 µs | 1.9 ms |
| Rust | absolute+managed_radial_follow | 6413.1 µs | 140.7 ms |
| OTD | empty | 14 µs | 0 ms |
| OTD | parse | 536.7 µs | 0.1 ms |
| OTD | absolute | 3691.9 µs | 79.3 ms |
| OTD | relative | 587.9 µs | 1 ms |
| OTD | absolute+managed_radial_follow | 552.1 µs | 2.7 ms |
| OTD | absolute+managed_radial_follow+read_buffer | 46.3 µs | 0.3 ms |

## Paced replay

200 reports per second, SendInput off. Rust's reader runs at time-critical priority; upstream's at AboveNormal in a High priority class process, as its daemon does.

| Harness | Stage | p50 | p99 | p99.9 | max |
| --- | --- | --- | --- | --- | --- |
| Rust | Wake after the report is signaled | 5.4 µs (5.3 µs–5.4 µs) | 7.8 µs (7.6 µs–18 µs) | 24 µs (18 µs–27 µs) | 80 µs (30 µs–97 µs) |
| Rust | Decode, filter, map | 1.1 µs (1.1 µs–1.2 µs) | 1.7 µs (1.7 µs–3.3 µs) | 5.1 µs (4.5 µs–13 µs) | 14 µs (8.4 µs–22 µs) |
| Rust | Signal to the SendInput call, per report | 6.5 µs (6.4 µs–6.6 µs) | 9.2 µs (9.0 µs–19 µs) | 26 µs (22 µs–29 µs) | 81 µs (33 µs–99 µs) |
| Rust | SendInput | 30 ns | 40 ns | 40 ns (40 ns–70 ns) | 301 ns (190 ns–411 ns) |
| Rust | Reports processed / merged because the reader was late | 18000 / 0 | | | |
| OTD | Wake after the report is signaled | 5.6 µs (5.6 µs–5.8 µs) | 8.9 µs (8.7 µs–13 µs) | 51 µs (44 µs–69 µs) | 190 µs (154 µs–244 µs) |
| OTD | Decode, filter, map | 2.2 µs (2.1 µs–2.2 µs) | 4.0 µs (3.9 µs–4.5 µs) | 5.5 µs (5.1 µs–12 µs) | 149 µs (114 µs–194 µs) |
| OTD | Signal to the SendInput call, per report | 7.8 µs (7.7 µs–8.1 µs) | 12 µs (11 µs–16 µs) | 64 µs (53 µs–76 µs) | 192 µs (168 µs–246 µs) |
| OTD | SendInput | 0 ns | 0 ns | 0 ns | 0 ns |
| OTD | Reports processed / merged because the reader was late | 18000 / 0 | | | |

## Other files in this folder

Measured for the [0.15.3 performance audit](../../AUDIT_FIXES_0.15.3.md). The files above come from the 0.15.3 source before its commit, with the corrected .NET bench case, against upstream's harness; SendInput was off.

- `versions/`: each release's own harness, run alternately three times (5 passes, 0.5 s warm-up, 15 s paced replay, no SendInput). `0.7.6` is commit `6d470b2`, the first with the harness; `0.15.0` is commit `6b0fbea`. Before 0.15.3 the `absolute+managed_radial_follow` case passed no raw packet and timed the bridge's rejection; its numbers there do not run the plugin.
- `dotnet-bridge/`: the 0.15.3 driver with the unchanged RadialFollow DLL through the 0.15.2 bridge (`three-crossings-*`, the fallback path) and through the 0.15.3 bridge (`fused-*`), three alternating runs each.

