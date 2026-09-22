# GitHub parity tracking

The [roadmap](../FULL_PARITY_PLAN.md) and [work items](WORK_ITEMS.md) are the scope reference. The [evidence ledger](EVIDENCE_LEDGER.md) tracks tested claims. GitHub issues track ownership, PRs and current progress. Check current issue state before taking a task; tracker checkboxes can lag a merged PR.

Start at [parent tracker #13](https://github.com/z9a17/opentabletdriver-rust/issues/13). The [seven GitHub milestones](https://github.com/z9a17/opentabletdriver-rust/milestones) correspond to gates G0-G6. A workstream's issue milestone identifies its primary delivery gate; individual tasks can contribute to other gates.

| Workstream | Task IDs | GitHub issue |
| --- | --- | --- |
| Foundations and existing behavior | F01-F06 | [#2](https://github.com/z9a17/opentabletdriver-rust/issues/2) |
| Device configuration, reports and transports | D01-D09 | [#3](https://github.com/z9a17/opentabletdriver-rust/issues/3) |
| Bindings and action ownership | B01-B04 | [#4](https://github.com/z9a17/opentabletdriver-rust/issues/4) |
| Mapping and output | O01-O05 | [#5](https://github.com/z9a17/opentabletdriver-rust/issues/5) |
| Settings, migration and profiles | C01-C05 | [#6](https://github.com/z9a17/opentabletdriver-rust/issues/6) |
| Unchanged .NET plugins and native API | P01-P10 | [#7](https://github.com/z9a17/opentabletdriver-rust/issues/7) |
| Daemon, IPC, CLI and diagnostics | S01-S05 | [#8](https://github.com/z9a17/opentabletdriver-rust/issues/8) |
| Desktop user interface | U01-U06 | [#9](https://github.com/z9a17/opentabletdriver-rust/issues/9) |
| Linux and macOS platform support | X01-X05 | [#10](https://github.com/z9a17/opentabletdriver-rust/issues/10) |
| Validation and final audit | V01-V06 | [#11](https://github.com/z9a17/opentabletdriver-rust/issues/11) |
| CI, packaging, licensing and release | R01-R04 | [#12](https://github.com/z9a17/opentabletdriver-rust/issues/12) |

The existing [hardware validation issue #1](https://github.com/z9a17/opentabletdriver-rust/issues/1) remains the evidence thread for F06. Do not close it because a new planning issue exists.

Consult current issue comments and the [agent handoff procedure](AGENT_HANDOFF.md) before claiming a task. F01 was delivered in v0.7.1; F03 is the evidence-ledger work. F06 still needs physical tablet testing.
