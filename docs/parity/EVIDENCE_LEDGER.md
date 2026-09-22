# Parity evidence ledger

The [source inventory](upstream-inventory.json) lists what the pinned OpenTabletDriver 0.6.7 baseline contains. The [evidence ledger](evidence-ledger.json) records what this port has actually implemented and checked. They are separate files: regenerating the inventory never changes evidence.

The first ledger has one row for each of the 52 capability IDs, 339 device configurations, 52 referenced parser types and 98 catalog records. Of those catalog records, metadata marks 57 eligible for the baseline. Eligible rows start `open`; the other 41 start `not_applicable` because their catalog metadata excludes this baseline. Neither state proves that a DLL runs. Plugin class records remain empty until P01 enumerates the classes in real packages.

## Record evidence

Each row has `state`, `owner`, `pull_request`, `release`, `evidence` and `blockers`. The owner and links are null until known. Use a stable task ID from [WORK_ITEMS.md](WORK_ITEMS.md). `implemented` requires a merged PR and a passing automated test; `verified` needs the extra evidence appropriate to the row. `blocked` requires a reason. A catalog record may be `not_applicable` only when its pinned metadata excludes the baseline. All other untested records stay `open` or `blocked`.

An evidence entry records its kind, task, pinned source revision, full Rust commit, OS and architecture, date with timezone, tester, upstream reference, result link, and `pass` or `fail`. Automated tests and manual hardware procedures need a `command`; use `fixture_sha256` when a fixture is involved. Hardware results also need exact `device_model`, `firmware` and `transport`. An unchanged .NET plugin integration result needs the tested DLL's `dll_sha256`. Put the PR and shipped release on the row once they exist. Link test logs or a durable CI run rather than treating a test command alone as proof that it passed.

Configuration `verified` requires a passing automated test and a physical result naming that configuration's model. Parser `verified` requires both differential and hardware evidence. Capability `verified` requires integration or hardware evidence. A plugin class `verified` needs an integration run with the unchanged DLL and its hash. A package `verified` needs a complete, nonempty class inventory and verification for every declared class. These claims apply only to the OS, architecture, firmware, transport, DLL version and other conditions recorded in their evidence. A generated fixture, imported device configuration or successful DLL load cannot establish hardware or full plugin compatibility.

Catalog package keys are the exact inventory paths, which include the published version. Class keys are `catalog:<package-path>::<fully-qualified-type-name>` and declare `plugin_key` and `type_name`. For a manually installed plugin, use a stable `manual:<publisher>/<identity>/<version>` key with `identity`, `version`, `source_url`, `classes` and `class_inventory_complete`; its class keys use the same `::<type-name>` suffix. This preserves version and class distinctions without inventing support for catalog entries that have no available binary.

From the repository root, run:

```powershell
python scripts/parity-evidence.py validate
python -m unittest discover -s scripts/tests -p 'test_parity_*.py'
```

CI runs both commands. The validator rejects missing or extra IDs, changed inventory contents, stale source revisions, invalid task IDs, malformed evidence and completion states without their required tests. It checks record structure; reviewers must still judge whether linked results substantiate the claim. The Git history and [workstream issues](GITHUB_TRACKING.md) remain the record of review and ownership.

## Change the pinned baseline

1. Generate a candidate inventory at a **new path** with `pwsh -File scripts/parity-inventory.ps1 -UpstreamRoot <clean-upstream-checkout> -CatalogRoot <clean-catalog-checkout> -OutputPath docs/parity/upstream-inventory.next.json`. Keep the current inventory and ledger intact. Use clean checkouts of the proposed upstream and catalog commits.
2. Run `python scripts/parity-evidence.py diff --old docs/parity/upstream-inventory.json --new docs/parity/upstream-inventory.next.json`. Review added, removed and changed configuration, parser, catalog and plugin-contract records. Read the source diff too: the inventory does not encode every line of the upstream contracts.
3. Run `python scripts/parity-evidence.py advance --old-inventory docs/parity/upstream-inventory.json --new-inventory docs/parity/upstream-inventory.next.json --old-ledger docs/parity/evidence-ledger.json --archive docs/parity/history/baseline-<old-revision>.json --output docs/parity/evidence-ledger.next.json`. Pass `--old-matrix` and `--new-matrix` if capability IDs changed. The command validates the old ledger, refuses to overwrite any input or output, archives the old inventory, matrix, evidence and source diff together, then creates a fresh ledger with all eligible records open. Old results remain available but do not silently certify a new upstream revision.
4. Review the archive, new ledger and changed requirements. Replace the two current JSON files with the reviewed `.next.json` files in one PR, update capability/task scope and validation evidence, then run `validate` and the unit tests. Keep the archive in Git. Re-run tests against the new baseline before restoring any `implemented` or `verified` state.

`bootstrap` also creates a ledger at a new path, exclusively. It cannot replace an existing ledger. A baseline update is a review of behavior and evidence, not a count-only inventory refresh.
