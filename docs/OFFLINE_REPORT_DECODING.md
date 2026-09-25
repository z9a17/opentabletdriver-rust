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

All fields that a report does not provide are `null`, distinct from zero, false or an explicitly empty array. `has_capabilities: false` identifies unrecognized/raw-only results. Raw bytes remain present as `raw_hex`; `report_raw_hex` shows the parser's view, including removal of the Wacom-driver prefix where applicable. These exports are deliberately unredacted bytes supplied by the caller and can contain device-specific identifiers.

Touch packets update the retained snapshot in line order for this invocation only. Files must contain one endpoint/session; start another command to reset state. No timestamps, device identity, gestures or output events are inferred from a hex file.

The newly ported parsers follow [the pinned OTD 0.6.7 Wacom source](https://github.com/OpenTabletDriver/OpenTabletDriver/tree/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.Configurations/Parsers/Wacom), including the IntuosV3 short-report tilt subtraction and BambooPad button equality checks. Unknown IDs/subtypes retain raw bytes without falling back to another pen layout. Known short layouts fail before field reads.

Decoder availability does not enable a tablet in the live driver. Live runtime remains restricted to the supported PTH-660 configuration. New parser differential, captured-report and hardware evidence remains open; no local decoder execution was performed during this implementation.
