# Agent handoff and contribution procedure

Read the root [AGENTS.md](../../AGENTS.md), [full parity plan](../FULL_PARITY_PLAN.md), [current status](../PORTING_STATUS.md), [capability matrix](CAPABILITY_MATRIX.md) and the selected [work item](WORK_ITEMS.md). The original narrow implementation plan is historical. Existing .NET plugins running unchanged is a user requirement; a native-only plugin solution is insufficient.

## Choose and claim a task

1. Check `git status`, applicable local instructions, current branch and remote history. Preserve unrelated work. Fetch before choosing a base.
2. Read the task's dependencies and [GitHub workstream issue](GITHUB_TRACKING.md), including recent claims/PRs. Initial independent tasks are F01, F03, P01 and U01. F06 is ready for a hardware tester. F02 follows F01; F04 and F05 follow F02.
3. Claim one stable task ID in the workstream issue, identifying your branch, expected files, deliverable and dependency assumptions. Check for conflicting claims first. A workstream issue is shared; claiming F01 does not reserve every F task.
4. Use `parity/<task-id>-<short-description>` as a branch convention. Use an isolated worktree when another agent is editing the same checkout. Shared core/configuration/ABI changes need a named integration owner and a reviewed contract before parallel consumers change them.
5. If the task is too large for one PR, create bounded child tasks with the original acceptance criteria and explicit prerequisites. Do not silently omit difficult categories. Put blockers in the issue, then select independent useful work when possible.

Suggested claim record:

```text
Claim: P03
Branch: parity/p03-managed-reports
Base commit: <sha>
Dependencies: <merged PRs or open prerequisite IDs>
Files/interfaces: <owned paths and proposed shared contract>
Deliverable: <bounded result>
Validation: <planned commands and required hardware>
```

## Implement and review

Inspect pinned upstream source before deciding semantics. Link exact source paths/commits and sanitized fixture provenance in the PR. Preserve current usable behavior while extending it. Avoid broad refactors, framework migrations or dependency upgrades unrelated to the selected task.

Use replay/fake endpoints for automated work; do not start input injection merely to inspect configuration or build a UI. Respect the current user's application/session when hardware tests are needed. Ordinary third-party plugins execute code with the host's permissions; execute only the known test corpus in intentional checks. Never present an in-process bridge as a sandbox.

Keep setup/reflection/configuration work away from report callbacks. Retained managed reports require safe ownership even if that means a slower compatibility path. Record performance differences rather than deleting plugin semantics to achieve zero allocations.

Run checks appropriate to the changes using [VALIDATION.md](VALIDATION.md). Always distinguish executed checks, ignored checks, unavailable environments and pending physical tests. Update the [evidence ledger](EVIDENCE_LEDGER.md) and current status only for behavior actually delivered. Keep future plans separate from README compatibility claims.

## Definition of done

Each completed work item has:

- A bounded implementation with its dependency contracts satisfied and no known behavioral regression.
- Source/fixture references, meaningful success/failure tests, and performance evidence for report-path changes.
- Required platform/hardware evidence, or a still-open validation task that prevents closing the relevant gate.
- Updated user/developer documentation, configuration migration and accurate compatibility errors.
- A reviewed diff, appropriate lint/build checks, and secret scanning when config/dependency material changes.
- Pushed commits, a linked PR/merge, tracker/evidence updates and accurate release notes/artifacts when shipping.

The user has authorized pushing completed work and publishing releases. Do not ask for that permission again. Coordinate releases: one integrator assigns the next version/tag and packages the merged source, avoiding competing agents creating or overwriting the same tag. Never force-update a published release tag to include later work. Documentation-only releases must say driver functionality is unchanged.

Closing an issue is not proof of parity. The source inventory records expected scope, the [evidence ledger](EVIDENCE_LEDGER.md) records validation, the workstream issue tracks work, and [PORTING_STATUS.md](../PORTING_STATUS.md) records shipped behavior. Update the relevant records together.

## PR and handoff format

```text
Task/CAP IDs:
Problem and resulting behavior:
Upstream source/contract and intentional differences:
Dependencies and shared interface changes:
Tests executed and results:
Performance impact (when relevant):
Hardware/platform evidence and remaining blockers:
Migration, documentation and release impact:
Next action / handoff branch and commit:
```

If interrupted, leave the branch/commit, dirty files, commands/results, exact blocker and next action in the issue. Do not label the task complete to avoid an unfinished handoff. Keep descriptions factual so another agent can continue without this chat.
