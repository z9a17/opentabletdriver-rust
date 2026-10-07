# Remaining implementation ownership

Historical work assignment for the 0.18 implementation. Delivered source and
deferred acceptance are recorded in [the completion report](IMPLEMENTATION_0.18.md).

The owner requested continued full implementation parity after v0.17.1, with
parallel agents and runtime validation deferred. The baseline stays
OpenTabletDriver 0.6.7, revision 736003ed72c8bbb28033b039d5a0bb76c344145c.

Claims based on clean merged 632a37dd0608a92affa595c0b2233f66dfa79196:

- P07/P03/P09: parity/managed-device-completion owns original concrete Core
  services, shared stream adapters, report events, diagnostics and unchanged
  plugin API fidelity. The integration owner supplies native physical I/O and
  delegated output ownership. Original DLL callers retain their public contract.
- S04/U01-U06/C05: parity/desktop-completion owns complete original console
  commands against a persistent daemon connection and remaining Windows
  workflows, preserving the owner's removed UI controls.
- X01-X05/O04: parity/platform-completion owns portable plugin/CLR hosting,
  native Unix runtime/control services and original frontend integration.
- parity/full-implementation owns native shared readers and I/O leases,
  collection/global-tool ownership, integration, final source audit, packaging,
  the manual Windows folder, PR and four-platform publication.

Native-only report processing must remain allocation-free. Subscribed managed
device data uses bounded preallocated capture; no competing physical reader,
silent report loss or unguarded replacement target is introduced.

Implementation is not acceptance evidence. No CI, test/format/Clippy/check suite,
driver/daemon/plugin/UI/hardware/game session is authorized for this work.
Actual distribution compilation and archive/source/hash inspection produce the
packages. Native hardware, OS session and unchanged binary runtime checks remain
for the owner's later validation. The evidence ledger is not promoted by source
changes or administrative issue closure.
