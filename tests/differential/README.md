# Differential fixtures

These files hold report sequences and the outputs OpenTabletDriver 0.6.7 ([`736003e`](https://github.com/OpenTabletDriver/OpenTabletDriver/tree/736003ed72c8bbb28033b039d5a0bb76c344145c)) produced for them. `crates/otd-core/src/differential.rs` runs the same reports through this driver and compares every report. See [behavior contracts](../../docs/parity/BEHAVIOR_CONTRACTS.md#differential-tests) for what is compared and why each tolerance exists.

| File | Reports | Cases |
| --- | --- | --- |
| `osu-trace.json` | the first 2,000 reports of the benchmark's `osu-synthetic-v1` trace | osu! profile with and without Radial Follow, a 30° rotated area, area limiting, relative mode at 10 counts/mm, and at 12 × 8 counts/mm rotated 15° |
| `edges.json` | 35 handwritten reports | tip pressure 81/82/83 and 8191, the eraser, tablet corners, exact area edges, Sense or In Range alone, report IDs without a position, a pressed crossing of the area edge |
| `thresholds.json` | none | the first raw pressure that presses the tip for 16 activation thresholds |

## Origin and licenses

The fixtures are `upstream-generated`: `bench/upstream --reference` builds OpenTabletDriver's own parser, output modes and binding handler from the pinned checkout, assembles them as its daemon does, and records what its pointer receives. The Radial Follow case runs the unchanged RadialFollow 0.3.0 DLL; its SHA-256 is in the fixture's `provenance`. The files contain report bytes, settings and numeric outputs, not upstream source code. OpenTabletDriver is LGPL-3.0; RadialFollow is GPL-3.0-only.

The report sequences are synthetic. They are not recordings of a player or of a physical tablet.

## Format

- `reports`: the first 17 bytes of each report in hex. Both parsers read no further; tests pad them to the pen collection's 192 bytes with zeros.
- `interval_ms`: the time between reports this driver sees. Upstream's filters time reports themselves and see them back to back, so neither side reaches a reset timeout.
- `desktop`: the virtual screen and monitors, as `[left, top, right, bottom]`.
- Each case has `profile`, this driver's TOML; `upstream`, the same settings in OpenTabletDriver's terms; and `expected`, one entry per report:
  - `"x,y"`: absolute position in desktop pixels, or a relative delta in counts;
  - an optional third field of button events, `D` for press and `U` for release;
  - `""` when upstream produced nothing.

## Regenerate

From the repository root, with a clean OpenTabletDriver checkout at the pinned revision in `target/upstream/OpenTabletDriver` and the unchanged RadialFollow DLL:

```powershell
pwsh -File scripts/differential-fixtures.ps1
cargo test --locked -p otd-core differential
```

The script rebuilds both harnesses, rewrites all three files, and fills them from upstream. Review the diff. A changed expectation means upstream's behavior or the workload changed, so this driver has to be compared again.
