# Autonomous compatibility and driver hardening follow-up

## Claim and scope

The owner authorized this follow-up on 2026-10-15 after v0.19.0. Branch
`parity/autonomous-hardening` starts at
`e18386a0a8839db2afb5d6a93fe9027f94b4e86d`. No open GitHub workstream issues or
competing pull requests existed at claim time. The root integrator owns
F03/F04/F05/P05/V02/V03/V06/R02 integration and the release.

Bounded workstreams:

- Compatibility: expand offline unchanged-DLL and lifecycle coverage and
  investigate safe asynchronous Internal-filter behavior. Preserve retained
  pressure, concrete report identity, ordering, bounded queues and retirement.
- Background work: reduce redundant status/metadata work and unnecessary
  maintenance wakeups without changing input fairness or cancellation.
- Failure recovery: exercise plugin, queue, cancellation and interrupted-update
  failures using fake services and temporary files; fix concrete defects.
- Evidence: reconcile the current release/status view while preserving the
  historical 0.6.7 inventory and its unclosed hardware acceptance gates.

The UI is outside this scope. Native report processing stays in Rust; original
.NET plugin contracts and Unix frontend support remain compatibility code.
Only GPT 6.1 Sol at high reasoning is authorized for delegated work.

## Execution boundaries

Do not launch or stop a driver, daemon, UI, game, real device discovery or OS
input/output. Do not edit saved settings. Validation uses targeted offline
fixtures, fake clocks/endpoints, discarded outputs and temporary install roots.
The parent coordinates compilation to avoid concurrent shared build outputs.
No broad release check suite or GitHub Actions is used. Physical input, native
Unix runtime and actual application behavior remain separate acceptance work.

## Results

The implementation targets v0.19.1. No UI files, pages, controls or settings were
added. Native processing remains in Rust; managed changes are the compatibility
graph and its offline fixtures.

- Managed Internal timers and foreign/late emissions now use owned snapshots
  with pressure-admission identity. Explicit reset invalidates known old and
  in-flight work. Ordinary-only graphs retain their admission-free path. See
  [the exact contract](UP068_PLUGIN.md), including physical range-loss and
  arbitrary-plugin mutation limits.
- Windows supervisor work wakes for requests, lifecycle notices, completion,
  PnP events and rescan deadlines, replacing the 20ms polling loop. Managed input
  maintenance uses real expiry/retry deadlines and sleeps without scopes.
  Unix retains a 250ms retry ceiling because retired native input can leave
  pending cleanup independently of managed leases.
- The Windows managed observer no longer sends recurring Status,
  ListDeviceSessions and GetUpstreamLog requests to its own daemon. Native state
  publication supplies identity, session handle and logs. Complete same-generation
  state comparison reuses unchanged Arc trees and avoids repeated derived JSON.
  Log history is cloned only when its sequence changes. Before a snapshot
  consumer exists the observer parks; after that it retains the 250ms observation
  interval. The 3-second physical-device fallback remains because managed/custom
  endpoint changes are not all covered by tablet PnP notifications.
- Update preparation and recovery preflight all affected paths. Directory/file
  collisions, duplicate/overlapping recovery entries, Windows trailing-dot/space
  or stream aliases, and relative links/junctions fail while preserving files.
  The caller-selected installation root may itself be a junction. This is not
  an atomic defense against unrelated processes changing paths during update.
- The [current evidence map](CURRENT_STATUS.md) supersedes stale current-status
  claims without rewriting the historical 0.6.7 inventory/ledger. No acceptance
  row was promoted or carried across the mixed RC source composition.

The removed intervals correspond to up to 50 supervisor polls/second, four
empty Windows input-maintenance polls/second and 12 read-only self-IPC requests
per second while managed snapshots were active. These are source scheduling
counts, not a CPU or physical-latency benchmark. Actual wakes depend on work;
fallback refreshes and requests still wake the relevant owners.

## Focused execution

On Windows x64, the integrator built the real managed bridge/GraphBench and
native test executable, then ran only the bounded fixtures for this work:

- 25 distinct Rust cases passed: six managed-service snapshot/lease cases,
  three supervisor replacement/fairness cases, two lifecycle/completion wake
  cases, three native publication cases, one session-tree cache case, seven
  update recovery cases, and three Unix ownership-state cases compiled on the
  Windows host with fake writers. The latter do not establish native Unix runtime.
- Seven managed modes passed: `async-internal`, `upstream068`, `admission`,
  `background`, `wake`, `cleanup` and `snapshots`. They cover retained pressure,
  capture/reset races, multiple Internal nodes, custom-mode entry, explicit
  ownership failure, overflow, cleanup failure and retirement.
- [Three unchanged plugin identities/five classes](OFFLINE_PLUGIN_CORPUS_2026-10-15.md)
  passed 22 settings cases and 44 fresh lifecycle cycles against direct upstream
  interfaces/settings plus the Flip Axes analytic oracle. The recorded clock,
  output-model and package-coverage limits apply.
- The existing ledger validator passed. Its historical inventories and evidence
  remain intact; this structural check does not promote their acceptance state.

[Recorded transcripts and code hashes](autonomous-followup-evidence-2026-10-15.json)
identify the tested implementation. The corpus has its own archive/DLL/source
hash manifest and complete cycle transcript. Initial fixture compile/reflection
errors and a mixed-path-separator junction fixture failure were corrected before
the passing runs; none involved a real driver or device.

Reproduction uses `cargo test --locked -p opentabletdriver-rust --bin
opentabletdriver-rust <recorded-filter> -- --nocapture`, the GraphBench modes
above with the recorded legacy/tablet fixtures, and the corpus runner documented
in its report. Release packaging is a separate four-platform compile and archive
verification, not a broad pre-release test suite.

Independent GPT 6.1 Sol high reviews covered managed admission/lock ordering,
Windows wake/publication lifetime and recovery data safety. Integration review
also caught the shared service code's Unix wake/deadline dependency; the final
implementation preserves Unix native-cleanup retries.

No physical pen, real device disconnect, OS input delivery, interactive UI,
application or native Unix runtime validation was performed. Those acceptance
gates remain open; temporary recovery directories and fake endpoints do not
replace them.
