# Independent review decisions

Intent: improve source-confirmed performance, crash resilience and control-panel usability while preserving the Rust report loop and pinned OpenTabletDriver 0.6.7 interfaces. No active driver, game, device, plugin or UI was exercised. Source review and release compilation are distinct from runtime validation.

Reviewers: a same-family independent Codex reviewer, a second Codex reviewer covering native/managed reports and remaining components, and a read-only Claude CLI reviewer. Claude returned 15 numbered findings; the same-family final review identified the restart/discard ordering and confirmed a custom-configuration discovery correction. The report reviewer independently identified stale puck contact, font lifetime, a slider panic, excessive debugger formatting, stale capability text and synchronous diagnostics. All review outputs were read; they are source evidence, not executed tests. A focused final Claude pass identified panel lockout and a lost stop wake; the integration owner corrected both.

| Finding | Reviewer | Decision and rationale |
| --- | --- | --- |
| Benign companion-supervisor completion stops primary | Claude 1 | Act: optional companion discovery failure must not terminate ordinary single-tablet use. |
| Unconditional plugin recovery blocks every command | Claude 2 | Act: recovery belongs at plugin-loading/mutation boundaries, and unrelated bad backups must be isolated. |
| Inventory mutates files and clears the list on errors | Claude 3 | Act: use read-only inventory with explicit errors; keep the last UI snapshot on failure. |
| Reconnected primary briefly runs as a companion | Claude 4 | Act: reserve its configuration during primary absence before a changed endpoint can drive output. |
| Unsupported device breaks default-profile loading | Claude 5; Codex | Act, resolved before final review: shared configured discovery ignores unsupported candidates and propagates database/enumeration failures. |
| Invalid mapping rebuilt/logged at 1 Hz during report flow | Claude 6 | Act: retry snapshot failures separately from unchanged invalid layouts. |
| Generic puck packet gets IntuosV2 pen buttons | Claude 7 | Act: compact decoder supplies its own buttons; generic graph preserves its actual interfaces. |
| Selected DLL path replaced with verbatim canonical path | Claude 8 | Act on path preservation: compare canonical identities but save the chosen path. Claim of duplicate identities is partly superseded by existing alias deduplication. |
| Linux transient HID errors become fatal | Claude 9 | Act: classify recoverable HID setup/read failures separately from permanent profile/output/plugin failures. |
| Configured-database failure silently supplies PTH ranges | Claude 10 | Act: propagate configuration/specification errors for named profiles. Reuse the effective database at setup boundaries where practical. |
| Modern lock markers prevent old-version saving | Claude 11 | Noted and documented: an intentional fail-closed migration. Reject the proposed alternate lock name because it permits concurrent old/new writers. Act on Windows delete sharing to protect the held marker. |
| Panel inspection keeps managed DLLs mapped | Claude 12 | Act: inspection loads main and resolved managed dependency assemblies from streams. Runtime plugin loading remains separate; plugin-native dependencies or arbitrary static provider code have independent lifetime limits. |
| Deferred refresh sequence scattered among callers | Claude 13 | Act on repeated refresh/removal sequences. Consider tooltip-module extraction later; moving code alone does not establish a behavior or performance improvement. |
| Tooltip stale list index may panic | Claude 14 | Act: bounds-check the snapshot index and verify the notification source. |
| Small reset/timer/parse/tick redundancies | Claude 15 | Noted: repeated setup parsing is outside the report loop. Dismiss removing timer interval validation: zero/nonfinite intervals are invalid scheduling state. Raw JSON preserves explicit null/omission, so the reset menu need not duplicate that mode. No wrapper is added solely to deduplicate the small tick match. |
| Recovery I/O error prevents opening the panel to Stop | final Claude 1 | Act: recovery failure is logged and the panel remains available; output/plugin mutation paths retain their stricter failure checks. |
| Stop wake erased by supervisor ResetEvent | final Claude 2 | Act: recheck stop immediately after resetting the event, before discovery and wait. |
| Installation appends disabled entries to the editor | final Claude 3 | Noted, intentional: document that installation changes the editor and still needs Save/Apply; the existing 32-entry bound remains. |
| Restart stops/spawns before discard consent | independent Codex | Act: resolve dirty settings first, keep editing stable, and close the completed restart without a second prompt. |
| Puck position inherits held pen contact | report Codex | Act: release stale native contact without fabricating tablet pressure or managed capabilities. |
| GDI font lifetime, inverted slider, large debugger preview | report Codex | Act: restore selected fonts, guard invalid channel bounds, and bound presentation formatting. |
| Diagnostics request/storage stalls UI | report Codex | Act: worker owns bundle generation and file publication; clipboard completion stays on the UI thread. |

Agreement: report preservation and capability dispatch were traced independently through the actual parser producers. No currently implemented parser mask was found missing from the managed snapshot factory. Reviewers agreed that source inspection does not prove generated-code cost, allocation counts, physical latency or broad unchanged-DLL compatibility. The restart/discard problem is a confirmed pre-existing ordering defect in a touched path. Claude's higher-severity lifecycle findings were independently traced by the integration owner and assigned for correction rather than treated as an automatic verdict.

Retained limits: shared-output acknowledgement stays serialized; independently owned managed arrays remain because plugins may retain reports; integer mapping is retained to preserve rounding; broad binding/auxiliary/touch actions and arbitrary asynchronous .NET compatibility remain roadmap work. The original-driver coexistence check runs at startup and cannot detect an original driver launched later. No numerical latency improvement is claimed.

Final review outcome: concrete blocking findings from the focused pass were traced and corrected. That reviewer did not rerun after the two final changes, and the release does not claim independent approval of them. The integration owner inspected both narrow source changes and recompiled the downloadable Windows artifacts.
