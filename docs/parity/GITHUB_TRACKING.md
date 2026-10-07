# GitHub parity tracking

The [roadmap](../FULL_PARITY_PLAN.md) and [work items](WORK_ITEMS.md) remain the scope reference. The [evidence ledger](EVIDENCE_LEDGER.md) tracks tested claims. At the owner's request, release 0.17.0 consolidates the broad issues #2-#13 into the checked-in [current audit](PARITY_AUDIT_2026-10-07.md) and stable acceptance backlog. Their closure archives the tracking structure; it does not certify full parity or completed hardware/plugin/platform validation. Historical comments, claims and PR links remain available below.

Start new continuation work at the current audit and [agent handoff](AGENT_HANDOFF.md). The archived [parent tracker #13](https://github.com/z9a17/opentabletdriver-rust/issues/13) retains the original roadmap history. The [seven milestones](https://github.com/z9a17/opentabletdriver-rust/milestones) correspond to gates G0-G6; administrative issue closure does not close those gates.

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

The historical [hardware validation issue #1](https://github.com/z9a17/opentabletdriver-rust/issues/1) retains earlier observations. Its issue state does not establish current F06 hardware acceptance.

Consult the checked-in backlog, historical issue comments and the [agent handoff procedure](AGENT_HANDOFF.md) before claiming a task. F01 was delivered in v0.7.1; F03 is the evidence-ledger work. F06 still needs physical tablet testing.
