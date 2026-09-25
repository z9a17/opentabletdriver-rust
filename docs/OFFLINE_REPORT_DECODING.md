# Offline report decoding

`decode` prints checked parser results from explicitly supplied hex bytes. It does not open HID endpoints, initialize hardware, load plugins, run the output pipeline or inject input.

```text
opentabletdriver-rust.exe decode --parser intuos-v3 --hex "11 00 00 00 00 00"
opentabletdriver-rust.exe decode --parser intuos-v2-touch --input captured-hex.txt
```

Input files contain one hex packet per line. ASCII whitespace and colons can separate byte pairs; blank lines and lines beginning with `#` are ignored. Limits are 4 MiB of UTF-8 input, 4,096 packets and 192 bytes per packet. Each valid packet produces one JSON line. Malformed input reports its line number and exits unsuccessfully; earlier output remains available.

| Parser name | Decoded scope |
| --- | --- |
| `pth660-pen` | Existing PTH-660-specific 0x10/0x1E decoder and its coordinate/pressure limits |
| `intuos-v2-aux` | IntuosV2 0x11 auxiliary buttons and absolute ring |
| `intuos-v2-touch` | Stateful 0x21/0xD2 touch snapshots |
| `wacom-driver-intuos-v2-touch` | Touch snapshots after the Wacom-driver prefix |
| `intuos-v3` | Full pinned IntuosV3 dispatch: extended 0x1E pen, 0x1F subtype 1 pen, 0x11 auxiliary |
| `bamboo` | Pinned Bamboo 0x02 pen/mouse/auxiliary dispatch |
| `bamboo-pad` | Pinned BambooPad 0x10 pen and auxiliary subtypes |
| `bamboo-v2-aux` | Pinned BambooV2 auxiliary 0x02 layout |
| `uc-logic` | Base UCLogic pen/auxiliary and explicit range loss |
| `uc-logic-tilt` | UCLogic tilt pen and auxiliary dispatch |
| `uc-logic-v1` | UCLogic V1 pen/auxiliary and explicit range loss |
| `uc-logic-v2` | UCLogic V2 pen/auxiliary; undecoded wheel bytes stay raw-only |
| `huion-tilt` | Huion tilt, auxiliary buttons and nullable absolute wheel |
| `huion-giano` | Huion Giano 17-bit pen, auxiliary and relative wheels |
| `huion-inspiroy` | Huion Inspiroy pen, auxiliary, relative wheel and explicit range loss |
| `xp-pen` | Base XP-Pen length-dependent pen/tilt and auxiliary layouts |
| `xp-pen-gen2` | Extended Gen2 pressure and position, auxiliary and range loss |
| `xp-pen-offset-pressure` | XP-Pen offset-pressure variants, including the unmasked short layout |
| `xp-pen-offset-aux` | Offset auxiliary buttons and ordinary pen/tilt |
| `xp-pen-dedicated-aux` | Dedicated auxiliary buttons and relative wheels |
| `tablet` / `auxiliary` | Pinned generic tablet/auxiliary report layouts |
| `skip-byte-tablet` | Generic pen after removing one transport prefix byte |
| `veikk` / `veikk-a15` / `veikk-tilt` | Distinct Veikk pen, auxiliary and wheel dispatch |
| `veikk-v1` | Veikk V1 flags, variable auxiliary IDs and prefixed pen |

All fields that a report does not provide are `null`, distinct from zero, false or an explicitly empty array. `has_capabilities: false` identifies unrecognized/raw-only results. Raw bytes remain present as `raw_hex`; `report_raw_hex` shows the parser's view, including removal of the Wacom-driver prefix where applicable. These exports are deliberately unredacted bytes supplied by the caller and can contain device-specific identifiers.

`kind` distinguishes ordinary `data` (including raw-only packets) from explicit `out_of_range` notifications, which also have no capabilities. UCLogic and Huion dispatch by flags rather than a report-ID whitelist, matching the pinned source. The PTH-660 decoder shows its transport fields; it does not infer the later output pipeline's range-loss events.

Touch packets update the retained snapshot in line order for this invocation only. Files must contain one endpoint/session; start another command to reset state. No timestamps, device identity, gestures or output events are inferred from a hex file.

SkipByte always removes one byte from the parser's raw view; Veikk V1 does so only for pen reports. Both keep the complete transport bytes separately. Their `raw_hex` and `report_raw_hex` outputs make the distinction explicit.

The newly ported parsers follow [the pinned OTD 0.6.7 Wacom source](https://github.com/OpenTabletDriver/OpenTabletDriver/tree/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.Configurations/Parsers/Wacom), including the IntuosV3 short-report tilt subtraction and BambooPad button equality checks. Unknown IDs/subtypes retain raw bytes without falling back to another pen layout. Known short layouts fail before field reads.

The [UCLogic](https://github.com/OpenTabletDriver/OpenTabletDriver/tree/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.Configurations/Parsers/UCLogic), [Huion](https://github.com/OpenTabletDriver/OpenTabletDriver/tree/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.Configurations/Parsers/Huion) and [XP-Pen](https://github.com/OpenTabletDriver/OpenTabletDriver/tree/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.Configurations/Parsers/XP_Pen) variants retain their distinct flag dispatch, axis inversions, pressure masks and absent-wheel values. XP-Pen's actual packet length selects the layout: do not pad or trim captured bytes. Stateful Deco03 decoding is not included.

Version 0.10.0 adds 23 stateless pinned dispatchers: IntuosV3 (1), Bamboo variants (3), UCLogic/Huion tilt (5), other Huion/XP-Pen variants (7), and generic/Veikk variants (7). The CLI exposes 27 decoder names including the four earlier PTH-660/auxiliary/touch paths. The registry marks 25 parser type names Partial, including IntuosV2 and its Wacom-driver variant; Partial records available decoding, not full parser/device acceptance.

Decoder availability does not enable a tablet in the live driver. Live runtime remains restricted to the supported PTH-660 configuration. New parser differential, captured-report and hardware evidence remains open; Rust formatting and strict workspace/all-target Clippy passed during integration, but no local tests or decoder execution were performed. Final package results belong to the release notes.
