# Current tablet catalog and range audit: 0.14.5

This release embeds all **357** configurations from OpenTabletDriver's current `0.6.x` branch at [a126f7b241e417399be6c6a760c0a9d4b987ecfd](https://github.com/OpenTabletDriver/OpenTabletDriver/tree/a126f7b241e417399be6c6a760c0a9d4b987ecfd). It adds 18 configurations and adopts 45 changed definitions compared with stable 0.6.7. Every one of the **53** referenced parser type names has a native registry entry. Source metadata supplies positive representable coordinate, physical-size and pressure ranges for all 357 entries.

## Provenance and evidence

[device-catalog.json](parity/device-catalog.json) records each path, model, Git blob, endpoint metadata, parser names and unverified hardware state. `scripts/update-tablet-database.py <upstream-checkout>` authenticates all blobs before replacing files, copies exact Git bytes and writes `tablets/SOURCE`. The independent source review found zero blob mismatches. This is source/data evidence, not executed Rust deserialization or a physical compatibility result.

The stable [upstream-inventory.json](parity/upstream-inventory.json) and evidence ledger remain pinned to 0.6.7. The managed plugin API remains 0.6.7. The device catalog advances independently; old automated and hardware results do not certify the newer catalog or parser changes. The legacy differential corpus is retained for unchanged parser families and excludes 16 changed parser names. A smaller set of literal cases covers key new contracts; a full current-upstream differential corpus still needs generation and execution.

## Input corrections

- Added Huion KamvasOffset's 17-bit X, offset Y/pressure and inverted signed tilt layout, with Giano's auxiliary dispatch.
- XP-Pen Gen2 pressure clears status bits using upstream's current `0x1fff` mask, then supplies its high pressure bit. Deco03 keeps its six buttons together with wheel state and uses pen decoding for other non-range packets.
- UCLogic V1 distinguishes auxiliary, relative-wheel and pen subtypes. Veikk Tilt and Huion Giano recognize their current relative-wheel packets, including signed deltas.
- Wacom Bamboo exposes the current nullable absolute wheel; pen and auxiliary packets need nine bytes, while mouse packets remain eight. IntuosV1-derived families retain rotation between reports, and IntuosV3 extended reports expose signed rotation.
- IntuosV2 retains the existing checked compact pen decoder. Other report IDs use a stateful decoder allocated once at endpoint setup; successful native decoding adds no per-report heap allocation. No timing improvement was measured.

Source: [current parser tree](https://github.com/OpenTabletDriver/OpenTabletDriver/tree/a126f7b241e417399be6c6a760c0a9d4b987ecfd/OpenTabletDriver.Configurations/Parsers).

## Tablet identity and settings

Anonymous profiles no longer reject high-pressure thresholds or large raw crops against PTH-660 ranges during file loading. Nonzero/overflow and finite-value validation remains at the file boundary. Device-dependent bounds are checked after selection; invalid settings still fail before session output. UI Save/Apply retains detected geometry through its validation round trip. CLI and daemon preparation defer geometry checks until a tablet supplies its actual specification.

Selecting Any connected tablet explicitly stores `tablet = "*"`. It overrides an imported profile's old target while preserving the original imported JSON. Older profiles with no `tablet` field keep their existing imported-name fallback. Choosing a named target stays exact. OTD export requires a named tablet and writes that selected identity instead of silently exporting the old imported target.

Endpoint matching checks current HidReports predicates where the backend provides HID_REPORTS and requires metadata for Interface constraints. Auxiliary pairing uses each auxiliary identifier's own VID/PID and the same physical device, fixing devices whose pen and auxiliary IDs differ.

## Remaining compatibility work

Catalog and parser registration alone do not establish that every upstream tablet works. Only PTH-660 has prior hardware confirmation; this release performed no live validation. Separate auxiliary input collections are paired but are not read or initialized by the current Windows session. Buttons, wheels, strips and touch are not bound. Native rotation fields do not add the newer IRotationReport interface to unchanged managed plugins. WinUSB-only endpoint support, current descriptor metadata on every transport, Bluetooth variants and full Linux/macOS output need separate implementation or validation. Initialization failures intentionally abort the Rust session, unlike upstream's warning-and-continue policy.

Keep D02/D04/D06-D09 and platform/hardware work open until their actual acceptance evidence exists. The next priority is device identification/initialization traces from testers on PTK-470 and the newly added families, followed by transport and separate auxiliary input work.

## Release validation

Windows release binaries and the managed compatibility bridge are compiled for packaging. Regression cases were added but not executed. Per owner instructions, no fmt/clippy/test/build-check suite or CI ran. Package integrity is checked independently. No UI, active driver, HID reads, plugins, live input or user settings were touched.
