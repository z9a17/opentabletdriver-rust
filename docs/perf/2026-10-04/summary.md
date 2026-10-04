# Offline audit evidence

Results for the [4 October input audit](../../INPUT_AUDIT_2026-10-04.md). All JSON files are original harness output. `environment.json` records the clean baseline revision, host, unchanged plugin hash and the two extended benchmark executable hashes. The harness extensions were uncommitted during measurement; the clock comparison differs only in the production session's shared timestamp. The final formatting, error cleanup and explicit endpoint metadata were added afterwards.

| Files | Conditions |
| --- | --- |
| `baseline-rust-{1,2,3}.json`, `baseline-upstream-{1,2,3}.json` | Alternating harnesses; 20,000 reports, 500 Hz, 3 rounds, 250 ms warm-up, 8-second replay, discard output. Rust's paced path here is native; upstream's is managed. These totals use the older endpoint excluding output time. |
| `extended-before-load0-{1,2,3}.json` | Extended harness; native and managed Rust replay, 500 Hz, 8 seconds each, no CPU workers. |
| `extended-before-load16-{1,2,3}.json` | Same extended harness with 16 normal-priority CPU workers scoped to each replay. The session cases run before the workers start. |
| `clock-before-{1,2,3}.json`, `clock-after-{1,2,3}.json` | Alternating native/managed full sessions; 20,000 reports, 500 Hz, 5 rounds, 500 ms warm-up and tiered-compilation pause. No paced replay or CPU workers. |

The extended replay totals already include output time, although these interim JSON files do not yet have the final `timing_endpoint` metadata. Their sink is a discard callback. The early extended session warm-up did not include the tiered-compilation pause; only the matched clock comparison uses the corrected warm-up. The timing change is in `crates/otd-core/src/session.rs`; the paced model calls the pipeline directly and does not exercise that session optimization.

The game was absent at the start and began at 14:15, during the third clock-comparison pair; the user's .NET driver began at 14:17, after the pair completed. The agent did not start, stop or measure these processes. The baseline and CPU-stress runs finished before either appeared. Treat the third pair and the later script smoke as potentially affected by background game activity.

Reproduce an offline comparison with both compiled harnesses and the pinned upstream checkout:

```powershell
pwsh -File scripts/bench.ps1 -UpstreamRoot <pinned-upstream-checkout> -RadialFollow <unchanged-DLL> -RateHz 500 -WarmupMs 500 -Runs 3 -Rounds 5 -ReplaySeconds 8
```

For Rust-only CPU contention, run the compiled harness with `--rate 500 --replay-seconds 8 --load-threads 16 --radialfollow <unchanged-DLL> --compat <compiled-bridge> --out <result.json>`. Use `--load-threads 0` for the reference. Neither command injects input or opens a live tablet unless `-SendInput` / `--send-input` or `-Idle` is explicitly added. The stress model measures event scheduling, not OBS, GPU load, real HID queues or hardware latency.
