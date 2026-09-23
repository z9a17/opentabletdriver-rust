# Performance measurements

- Rust commit `36d584a9b0916e8ee0a5acba456ea519c3808175` (uncommitted changes); OpenTabletDriver `736003ed72c8bbb28033b039d5a0bb76c344145c`
- Microsoft Windows 11 Pro 10.0.26200 build 26200; AMD Ryzen 7 5800X3D 8-Core Processor, 16 logical CPUs; power plan Ultimate Performance
- Upstream runtime .NET 8.0.31
- Trace `osu-synthetic-v1`: 20000 reports, FNV-1a `719b88e0c722d0af`; 3 runs per harness, 7 timed passes per case

Per-report time is the median over runs of each run's median over timed passes; the range between runs follows in brackets.
Every row includes the harness's own timing overhead, shown in the first row. Upstream's memory column is managed bytes allocated per report.

| Case | Rust p50 | OTD p50 | Rust p99 | OTD p99 | Rust p99.9 | OTD p99.9 | Rust memory | OTD memory |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| Harness alone (included in every row) | 20 ns (20 ns–30 ns) | 30 ns | 30 ns | 30 ns | 30 ns | 30 ns | 0 allocations | 0 B |
| Decode one report | 30 ns | 40 ns (40 ns–50 ns) | 40 ns | 220 ns (210 ns–220 ns) | 40 ns | 351 ns (351 ns–401 ns) | 0 allocations | 96 B |
| Absolute mode, no filter | 70 ns (60 ns–70 ns) | 110 ns | 80 ns | 301 ns (290 ns–311 ns) | 100 ns (100 ns–110 ns) | 461 ns (421 ns–471 ns) | 0 allocations | 96 B |
| Relative mode | 60 ns | 130 ns (130 ns–140 ns) | 80 ns | 341 ns (311 ns–351 ns) | 100 ns (100 ns–110 ns) | 491 ns (451 ns–521 ns) | 0 allocations | 96 B |
| osu! profile: built-in Radial Follow / unchanged RadialFollow DLL | 130 ns | 210 ns | 170 ns | 391 ns (391 ns–411 ns) | 190 ns | 561 ns (561 ns–591 ns) | 0 allocations | 96 B |
| osu! profile, both with the unchanged RadialFollow DLL | 180 ns | 210 ns | 240 ns | 391 ns (391 ns–411 ns) | 260 ns | 561 ns (561 ns–591 ns) | 0 allocations | 96 B |
| osu! profile with a new read buffer per report | — | 220 ns | — | 511 ns (471 ns–511 ns) | — | 711 ns (691 ns–751 ns) | — | 312 B |
| Native EMA filter DLL | 80 ns | — | 100 ns (100 ns–110 ns) | — | 130 ns (120 ns–150 ns) | — | 0 allocations | — |
| Whole session loop, osu! profile, no HID read or SendInput | 190 ns | — | 220 ns | — | 932 ns (371 ns–942 ns) | — | 0 allocations | — |
| SendInput alone | 86 µs (85 µs–86 µs) | 86 µs (84 µs–87 µs) | 192 µs (177 µs–248 µs) | 199 µs (183 µs–204 µs) | 423 µs (383 µs–445 µs) | 394 µs (385 µs–404 µs) | 0 allocations | 0 B |
| SendInput without cursor motion | 13 µs (12 µs–13 µs) | — | 21 µs (20 µs–64 µs) | — | 36 µs (32 µs–111 µs) | — | 0 allocations | — |
| osu! profile with SendInput | 83 µs (83 µs–84 µs) | 89 µs (88 µs–89 µs) | 181 µs (175 µs–208 µs) | 181 µs (178 µs–188 µs) | 393 µs (390 µs–396 µs) | 395 µs (386 µs–403 µs) | 0 allocations | 96 B |

## Mean, worst report and thread CPU time

The worst report of a pass usually coincides with an interrupt or a context switch. Thread CPU time per report also covers the harness's untimed loop work.

| Case | Rust mean | OTD mean | Rust max | OTD max | Rust CPU | OTD CPU |
| --- | --- | --- | --- | --- | --- | --- |
| Harness alone (included in every row) | 25 ns (24 ns–26 ns) | 27 ns (26 ns–29 ns) | 40 ns (40 ns–50 ns) | 100 ns (60 ns–110 ns) | 52 ns (50 ns–54 ns) | 56 ns (53 ns–57 ns) |
| Decode one report | 29 ns (28 ns–30 ns) | 50 ns (49 ns–50 ns) | 80 ns (70 ns–200 ns) | 1.0 µs (651 ns–2.9 µs) | 56 ns (54 ns–59 ns) | 75 ns (74 ns–76 ns) |
| Absolute mode, no filter | 66 ns (65 ns–66 ns) | 118 ns (116 ns–118 ns) | 2.4 µs (2.4 µs–6.5 µs) | 2.6 µs (2.5 µs–3.8 µs) | 92 ns (91 ns–92 ns) | 142 ns (141 ns–143 ns) |
| Relative mode | 62 ns (62 ns–63 ns) | 145 ns (144 ns–147 ns) | 2.5 µs (1.5 µs–7.0 µs) | 3.1 µs (2.4 µs–5.8 µs) | 88 ns (87 ns–89 ns) | 170 ns (169 ns–172 ns) |
| osu! profile: built-in Radial Follow / unchanged RadialFollow DLL | 126 ns (125 ns–126 ns) | 213 ns (212 ns–214 ns) | 2.8 µs (2.1 µs–4.5 µs) | 6.6 µs (2.5 µs–8.5 µs) | 150 ns (150 ns–151 ns) | 237 ns (235 ns–238 ns) |
| osu! profile, both with the unchanged RadialFollow DLL | 183 ns (182 ns–183 ns) | 213 ns (212 ns–214 ns) | 2.8 µs (2.5 µs–12 µs) | 6.6 µs (2.5 µs–8.5 µs) | 206 ns (206 ns–206 ns) | 237 ns (235 ns–238 ns) |
| osu! profile with a new read buffer per report | — | 229 ns (228 ns–231 ns) | — | 6.7 µs (2.6 µs–12 µs) | — | 252 ns (251 ns–255 ns) |
| Native EMA filter DLL | 88 ns (83 ns–88 ns) | — | 2.3 µs (2.2 µs–8.0 µs) | — | 114 ns (108 ns–114 ns) | — |
| Whole session loop, osu! profile, no HID read or SendInput | 194 ns (194 ns–194 ns) | — | 3.1 µs (2.5 µs–5.0 µs) | — | 196 ns (196 ns–196 ns) | — |
| SendInput alone | 91 µs (90 µs–92 µs) | 90 µs (89 µs–91 µs) | 1612 µs (1520 µs–1749 µs) | 1530 µs (1523 µs–1669 µs) | 31 µs (31 µs–31 µs) | 31 µs (31 µs–31 µs) |
| SendInput without cursor motion | 14 µs (13 µs–15 µs) | — | 468 µs (191 µs–693 µs) | — | 7.1 µs (6.6 µs–7.8 µs) | — |
| osu! profile with SendInput | 86 µs (86 µs–86 µs) | 91 µs (90 µs–92 µs) | 1544 µs (1540 µs–1583 µs) | 1663 µs (1520 µs–1666 µs) | 30 µs (30 µs–30 µs) | 33 µs (33 µs–33 µs) |

## Noise

How far the p50 moved: the widest range between the timed passes of one run, and the range between runs. Timestamps advance in steps of about 5 ns on this CPU, so short cases show large percentages.

| Case | Rust passes | Rust runs | OTD passes | OTD runs |
| --- | --- | --- | --- | --- |
| Harness alone (included in every row) | 50 % | 51 % | 0 % | 0 % |
| Decode one report | 33 % | 0 % | 25 % | 25 % |
| Absolute mode, no filter | 17 % | 14 % | 18 % | 0 % |
| Relative mode | 33 % | 0 % | 21 % | 8 % |
| osu! profile: built-in Radial Follow / unchanged RadialFollow DLL | 8 % | 0 % | 0 % | 0 % |
| osu! profile, both with the unchanged RadialFollow DLL | 11 % | 0 % | 0 % | 0 % |
| osu! profile with a new read buffer per report | — | — | 9 % | 0 % |
| Native EMA filter DLL | 25 % | 0 % | — | — |
| Whole session loop, osu! profile, no HID read or SendInput | 5 % | 0 % | — | — |
| SendInput alone | 6 % | 1 % | 6 % | 3 % |
| SendInput without cursor motion | 16 % | 11 % | — | — |
| osu! profile with SendInput | 8 % | 0 % | 4 % | 2 % |

## Startup

First report after creating the case (µs) and the time to create it (ms), first timed pass of the first run:

| Harness | Case | First report | Setup |
| --- | --- | --- | --- |
| Rust | empty | 0 µs | 0 ms |
| Rust | parse | 0.2 µs | 0 ms |
| Rust | absolute | 4.1 µs | 0 ms |
| Rust | absolute+radial_follow | 0.4 µs | 0 ms |
| Rust | relative | 0.2 µs | 0 ms |
| Rust | absolute+native_ema | 0.6 µs | 0.4 ms |
| Rust | absolute+managed_radial_follow | 1829 µs | 71.8 ms |
| Rust | sendinput | 107.6 µs | 0 ms |
| Rust | sendinput_still | 66.8 µs | 0 ms |
| Rust | absolute+radial_follow+sendinput | 18.8 µs | 0 ms |
| OTD | empty | 14.5 µs | 0 ms |
| OTD | parse | 474.3 µs | 0.1 ms |
| OTD | absolute | 3662.1 µs | 74.5 ms |
| OTD | relative | 578.7 µs | 0.8 ms |
| OTD | absolute+managed_radial_follow | 386.2 µs | 2.3 ms |
| OTD | absolute+managed_radial_follow+read_buffer | 48.4 µs | 0.3 ms |
| OTD | sendinput | 7797.9 µs | 0.5 ms |
| OTD | absolute+managed_radial_follow+sendinput | 166.2 µs | 1 ms |

## Paced replay

200 reports per second, SendInput on. Rust's reader runs at time-critical priority; upstream's at AboveNormal in a High priority class process, as its daemon does.

| Harness | Stage | p50 | p99 | p99.9 | max |
| --- | --- | --- | --- | --- | --- |
| Rust | Wake after the report is signaled | 5.3 µs (5.2 µs–5.3 µs) | 7.2 µs (7.0 µs–18 µs) | 68 µs (40 µs–95 µs) | 146 µs (90 µs–1037 µs) |
| Rust | Decode, filter, map | 952 ns (942 ns–962 ns) | 1.3 µs (1.3 µs–1.4 µs) | 2.5 µs (1.6 µs–4.8 µs) | 128 µs (26 µs–362 µs) |
| Rust | Signal to the SendInput call, per report | 6.2 µs (6.1 µs–6.2 µs) | 8.4 µs (8.0 µs–19 µs) | 75 µs (54 µs–96 µs) | 367 µs (147 µs–1038 µs) |
| Rust | SendInput | 74 µs (74 µs–75 µs) | 187 µs (126 µs–231 µs) | 902 µs (812 µs–1135 µs) | 1652 µs (1395 µs–1674 µs) |
| Rust | Reports processed / merged because the reader was late | 18000 / 0 | | | |
| OTD | Wake after the report is signaled | 5.4 µs (5.4 µs–5.5 µs) | 16 µs (7.2 µs–17 µs) | 55 µs (17 µs–61 µs) | 159 µs (25 µs–259 µs) |
| OTD | Decode, filter, map | 2.5 µs (2.5 µs–2.5 µs) | 3.7 µs (3.6 µs–4.0 µs) | 11 µs (4.5 µs–14 µs) | 54 µs (9.5 µs–114 µs) |
| OTD | Signal to the SendInput call, per report | 7.9 µs (7.9 µs–8.0 µs) | 19 µs (10 µs–19 µs) | 67 µs (20 µs–73 µs) | 161 µs (27 µs–261 µs) |
| OTD | SendInput | 76 µs (76 µs–77 µs) | 218 µs (125 µs–232 µs) | 1305 µs (1298 µs–1461 µs) | 1844 µs (1711 µs–1902 µs) |
| OTD | Reports processed / merged because the reader was late | 18000 / 0 | | | |

## Idle processes

Tablet connected, pen away, after 10 s of warm-up.

| Scenario | Process | CPU (% of one core) | Context switches/s | Working set | Private | Handles | Threads |
| --- | --- | --- | --- | --- | --- | --- | --- |
| rust-daemon | opentabletdriver-rust | 0.002 | 1.6 | 9.2 MB | 1.7 MB | 141 | 2 |
| rust-panel | opentabletdriver-rust-ui | 0.0035 | 2.6 | 17.3 MB | 2.6 MB | 220 | 4 |
| upstream-daemon | OpenTabletDriver.Daemon | 0.1066 | 8.3 | 58.6 MB | 21.2 MB | 396 | 16 |
| upstream-daemon+ux | OpenTabletDriver.Daemon | 0.0791 | 8.4 | 84.9 MB | 32.3 MB | 577 | 22 |
| upstream-daemon+ux | OpenTabletDriver.UX.Wpf | 0.0078 | 0.2 | 230.9 MB | 164.2 MB | 725 | 18 |
