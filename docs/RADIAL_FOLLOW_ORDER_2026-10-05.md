# Built-in and .NET Radial Follow, 5 October 2026

The built-in Radial Follow felt very different from the unchanged RadialFollow DLL with the same settings, in the same driver, on the same tablet. This page records what was compared. Run alone, the two produce the same cursor positions. What differed was the filter order. The built-in filter always ran before every DLL filter, while the DLL runs at its place in the profile's filter list, as in OpenTabletDriver. Release 0.16.9 runs the built-in filter in the DLL entry's place.

## The filter itself is identical

- **Source.** The port matches RadialFollow 0.3.0's `RadialFollowCore` and `RadialFollowSmoothingTabletSpace` ([tag 0.3.0](https://github.com/AbstractQbit/AbstractOTDPlugins/tree/0.3.0/RadialFollow)) in curve, derived constants, setting ranges, millimetre conversion from the digitizer specification, NaN handling and the 50 ms redetection reset.
- **Same input.** The release DLL (SHA-256 `830d61f29c07b109398f1e001df3c7fbd359b1ad2491a6d12ccc243dacf16383`, the one recorded in the [evidence ledger](parity/evidence-ledger.json)) ran through the driver's .NET bridge with the saved settings (0.7039 / 0.302 mm, 0.302, 0.603, 0.201). The built-in filter ran over the same 6,000-report trace: circles, jumps, jitter and three pen lifts longer than 50 ms. 5,845 positions were bit-identical; the largest difference was 0.001 tablet units, 5 nm.
- **Whole pipeline.** The same trace went through the report pipeline once with the built-in filter and once as the managed path, with the DLL's outputs where the bridge hands them back. On the osu! area, 5,996 of 6,000 output packets were identical. Three differed by 1 of 65,535 normalized units, the bridge's single-precision pixel position. One skipped a duplicate position as a result.
- The repository's [differential test](../tests/differential/README.md) already compares the built-in filter with OpenTabletDriver's own pipeline running the DLL over 2,000 osu!-style reports.

Settings import, panel fields (six decimals), defaults, the tablet's 224 mm × 44,800 dimensions and the managed `TabletReference` all matched. A .NET entry added in the panel saves every `DefaultPropertyValue`, so the DLL does not silently run with zeroed constructor values.

## Filter order changes the result

OpenTabletDriver runs the enabled filters of each stage in profile order (`OutputMode.Elements` and `PipelineManager.GroupElements` at the pinned [`736003e`](https://github.com/OpenTabletDriver/OpenTabletDriver/blob/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.Plugin/Output/PipelineManager.cs)). Its settings add a newly seen plugin at the end of the list. This driver's .NET entries follow the same rule: the panel appends added DLLs. Through 0.16.8 the built-in Radial Follow ran before all of them.

The order matters when another pre-transform filter is in the chain. Temporal Resampler 1.5.1, for example, declares `PipelinePosition.Raw`, which is the same value as `PreTransform` in the pinned plugin API. Add the Radial Follow DLL after Temporal Resampler to compare it with the built-in filter, and it smooths the resampler's predicted 1 kHz output. The built-in filter instead smoothed the raw reports, and the resampler then predicted from the smoothed, dead-zoned positions.

Both orders, with both unchanged DLLs through the bridge (Radial Follow first is what the built-in filter did): Temporal Resampler at its defaults, the saved Radial Follow settings, reports at 200 Hz (the PTH-660's rate was not measured), 11 seconds of synthetic aim after a one-second warm-up. Two runs per order agreed within 0.01 mm.

| Measurement | Radial Follow first | Temporal Resampler first |
| --- | --- | --- |
| Distance from the pen, mean / p95 | 0.646 / 1.49 mm | 0.541 / 1.18 mm |
| In jumps, mean | 0.81 mm | 0.67 mm |
| Holding still: outputs that moved the cursor | 37 % | 2 % |
| Step-to-step change in cursor movement, p99 | 267 µm | 196 µm |

With Radial Follow first, the cursor trails the pen further, keeps creeping after the pen stops, and moves less evenly. That is a plausible cause of the different feel. It requires such a filter in the chain, which this investigation could not see. The owner's profile was not available.

## Change in 0.16.9

- When the profile lists the tablet-space Radial Follow DLL, enabled or not, the built-in filter runs in that entry's place. Without such an entry, it still runs first, as before.
- With .NET filters, the built-in filter becomes a node in the managed graph, so timer emissions from earlier filters pass through it as they would through the DLL.
- The Filters tab lists the built-in entry where it runs.
- Each driving session logs the filters in the order they run and the settings they use, for example `Filters before mapping: TemporalResampler {...} -> built-in Radial Follow (outer 0.7039 mm, ...); after mapping: none`.

To compare the two implementations, keep the DLL entry in the list and switch between it and the built-in entry with their Enable boxes. Both then run at the same place.

## Other ways the same numbers can differ

- The DLL's **Screen coordinates** class uses pixels for its radii; the tablet-space class and the built-in filter use millimetres. On the 85 mm wide area mapped to 2,560 pixels, 1 mm is about 30 pixels.
- Builds before 0.16.6 discarded the built-in filter's settings when it was disabled. Re-enabling it then used the defaults (1.0 mm, 0.95 smoothing), which smooth much more than the saved settings. Check the values shown on the Filters tab, or the session log.
- The Linux and macOS drivers load no .NET filters, so this ordering does not apply there.

## Method and limits

Sources, harnesses and commands are in [perf/2026-10-05/radial-follow](perf/2026-10-05/radial-follow/README.md). Everything ran offline on Linux with .NET 8 and the release DLLs; Temporal Resampler's archive and DLL hashes match the [plugin archive audit](parity/plugin-archive-audit.json). No tablet, Windows session or live pen was used. The Windows build of the change was type-checked but its tests were not run. The felt difference still needs confirmation on the owner's setup.
