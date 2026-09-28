# Project instructions

- Start new parity work with [docs/FULL_PARITY_PLAN.md](docs/FULL_PARITY_PLAN.md), [the task backlog](docs/parity/WORK_ITEMS.md), and [the agent handoff](docs/parity/AGENT_HANDOFF.md). Claim a stable task ID in its linked GitHub workstream issue; check dependencies and current claims before editing shared files. The older implementation plan is historical.
- Full parity includes unchanged existing OpenTabletDriver .NET plugins, the pinned upstream device database, UI/daemon/CLI workflows, and Windows/Linux/macOS behavior. A native plugin replacement does not establish unchanged-DLL compatibility. Keep implementation status separate from automated and hardware evidence.
- Windows 11, Rust stable MSVC. Preserve allocation-free successful report processing.
- User preference: always push completed work to GitHub and publish a GitHub release with the Windows binaries. Include honest validation and compatibility limits in the release notes.
- No GitHub Actions/CI: the owner does not pay for runners. Do not run the format/clippy/test/build checks before publishing; the owner tests every release by hand afterwards. Say in release notes that the checks were not run.
- Build the workspace before testing DLL integration. Set `OTD_TEST_PLUGIN` to the absolute path of `target/release/otd_ema_filter.dll` and run `cargo test --locked native_plugin_round_trip -- --ignored`.
- Run `gitleaks` on staged changes before committing configuration changes. Use the pinned source links in docs for upstream behavior. Do not claim live pen validation from replay or UI tests.
- Run `actionlint` for workflow edits and PowerShell ScriptAnalyzer for changed PowerShell scripts.
- Source inventory reproduction: `pwsh -File scripts/parity-inventory.ps1 -UpstreamRoot <clean-OTD-checkout> -CatalogRoot <clean-catalog-checkout>`. Use the revisions recorded in `docs/parity/upstream-inventory.json`; review baseline changes explicitly.
- Evidence checks: `python scripts/parity-evidence.py validate` and `python -m unittest discover -s scripts/tests -p 'test_parity_*.py'`. Follow [the ledger process](docs/parity/EVIDENCE_LEDGER.md) when the pinned baseline changes.
- Coordinate version/tag creation through one release integrator. Never rewrite a published tag. Documentation-only releases must state that driver functionality is unchanged.
