# Relative mouse output

The PTH-660 can now drive relative Windows mouse motion. Select enabled `OpenTabletDriver.Desktop.Output.RelativeMode` in its OpenTabletDriver profile, or use [driver.relative.example.toml](../driver.relative.example.toml). The `settings` command previews either profile without starting a device session or injecting input.

The importer reads `XSensitivity`, `YSensitivity`, `RelativeRotation`, and `RelativeResetDelay` from `RelativeModeSettings`. The delay uses Newtonsoft's nonnegative TimeSpan representation, `[days.]hh:mm:ss[.fffffff]`; fractional milliseconds are preserved. Absolute areas are not required for relative mode. Existing tip/eraser thresholds and tablet-space Radial Follow filtering remain active. An independent TOML profile selects relative mode by including a `[relative]` table; mixing it with absolute monitor/crop/top-level rotation options is rejected.

## Mapping and performance

The PTH-660 has 200 raw units per millimetre on both axes. At startup, the mapper combines negative tablet rotation, physical-unit conversion, and independent axis sensitivities into four coefficients. It applies those coefficients to each position difference. A sensitivity of 10 produces 10 mouse counts per mm before Windows pointer adjustments. Zero disables an axis; negative sensitivity reverses it. Nonfinite settings and sensitivities that could overflow a Windows `LONG` on a full-tablet movement are rejected.

The mapper retains fractional counts, truncating each emitted delta toward zero and carrying the signed remainder forward. Small movements therefore accumulate even when one report is less than one count. Consecutive identical nonzero deltas are all emitted; only zero movement without a button transition is suppressed. Movement and contact transitions share one stack-allocated `INPUT` and at most one `SendInput` call per report. There is no per-report heap allocation, queue, trigonometry, or formatting. Relative sessions do not enumerate displays.

Relative input is subject to [Windows pointer speed and acceleration](https://learn.microsoft.com/en-us/windows/win32/api/winuser/ns-winuser-mouseinput#remarks), just like upstream's Windows relative pointer. The driver does not modify those system preferences.

## Reset and error behavior

- The first positional report establishes an origin without moving the pointer. Contact changes on that report still work.
- An input gap strictly greater than the reset delay clears the origin. Repeated stale raw positions are ignored for movement until the position changes; that changed position becomes the new origin. The delay should exceed the normal report interval; zero can reset on every report.
- Explicit proximity loss clears the origin and fractional carry immediately and releases a held button. Reconnection constructs fresh mapping and output state.
- Fractional motion is also cleared on timeout, preventing leftover movement across separate pen interactions.
- Failed mouse injection consumes that report's relative movement instead of replaying it later as a burst. Button state is only committed after successful injection, so a subsequent report or session cleanup retries a failed transition.

## Upstream reference

The behavioral reference remains the repository's pinned OpenTabletDriver revision `fdeaa7b0c6d6f5260f511f19fb693ed33524af4e`:

- [RelativeOutputMode.cs](https://github.com/OpenTabletDriver/OpenTabletDriver/blob/fdeaa7b0c6d6f5260f511f19fb693ed33524af4e/OpenTabletDriver.Plugin/Output/RelativeOutputMode.cs): transform order, strict reset-delay comparison, stale-position suppression.
- [WindowsRelativePointer.cs](https://github.com/OpenTabletDriver/OpenTabletDriver/blob/fdeaa7b0c6d6f5260f511f19fb693ed33524af4e/OpenTabletDriver.Desktop/Interop/Input/Relative/WindowsRelativePointer.cs): signed fractional carry and integer deltas.
- [RelativeModeSettings.cs](https://github.com/OpenTabletDriver/OpenTabletDriver/blob/fdeaa7b0c6d6f5260f511f19fb693ed33524af4e/OpenTabletDriver.Desktop/Profiles/RelativeModeSettings.cs): JSON properties and 10/10 sensitivity, 100 ms default values.

The relative output implementation was also checked against current `0.6.x` revision `cadb51af69e8a69db1ab8d0a9c960176db1ba65c`; its `RelativeOutputMode.cs` matched the pinned source. No newer protocol layouts were incorporated.

Intentional differences: Rust immediately rebases on explicit proximity loss, clears fractional carry on reset, still processes contact changes on stale duplicate positions, and avoids zero-motion injections. It transforms differences in `f64` instead of subtracting two transformed `f32` positions. This avoids cancellation at low sensitivities but is not a bit-for-bit floating-point emulation of the C# implementation.

## Verification

`cargo test --locked` covers import and invalid settings, TimeSpan boundaries, independent axes, rotation, signed fractional carry, filtering, strict timeout boundaries, stale reports, loss/reentry, and injection failure recovery. A recorded USB trace is replayed through parsing, contact state, relative mapping, and a simulated output sink. A thread-local test allocator verifies zero allocations in 10,000 report iterations, both with and without Radial Follow. Tests do not call `SendInput`.

Run the optional CPU microbenchmark with:

```text
cargo test --locked --release benchmark_relative_pipeline -- --ignored --nocapture
```

It replays one million reports per filter setting, including clock reads and output packet preparation, while checking allocations. It excludes HID I/O and the Windows injection call; its timing is not end-to-end pen latency or a comparison against the C# driver. Relative cursor feel, Windows pointer acceleration, and lift/reconnect behavior still require the [hardware checks](HARDWARE_VALIDATION.md).

A local Windows release run measured 34.7 ns/report without filtering and 62.8 ns/report with Radial Follow (one million reports each, zero measured allocations). These are a single development-machine CPU sample, not a portable performance guarantee. The release executable was 634,880 bytes.
