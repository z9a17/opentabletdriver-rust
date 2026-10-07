# Pre-landing source review, 0.17.1

Status: INCOMPLETE for behavioral acceptance. No confirmed source defects remain
in the reviewed integration paths; required runtime probes were deferred by the
owner. This is not a clean full-parity or plugin-runtime review result.

The gstack review checklist guided the integration pass. Parallel native agents
reviewed bindings, retained settings and the managed provider/queue contracts.
The integration owner reviewed cross-slice control commands, physical generation
guards, profile origin, cold setup, cleanup and the release dependency closure.
The unchanged vendored Desktop source remains pinned to upstream 0.6.7.

## Fixed source findings

- IDriver.Detect consumed an array as bool; native detection now returns a typed
  result from the actual opened tablet collection.
- Managed Message delivery stopped when bounded logs rolled over; actual native
  log sequences now identify the available new tail.
- Event baselines were sampled after subscription; cold subscription now captures
  the existing cached state before its monitor starts.
- Third-party parser disposal could escape a CLR finalizer; finalizer cleanup
  contains errors while explicit deterministic cleanup retains its error contract.
- Synchronous service waits escaped binding-only guards; stack-only nested scopes
  now cover report/output/parser/timer callbacks and setup.
- Detached self-apply admission could race scope retirement; admission and Dispose
  share the provider gate. Native queue admission captures the exact physical
  source ID/generation and the collection setter checks it before mutation.
- Idle settings publication could race discovery; the daemon holds the registry
  admission lock across the short collection publication.
- Reconnect and stopped Start could lose original collection ownership; explicit
  transient origin and guarded Run publication preserve it. Disconnected stale
  RPC profiles/tools cannot overwrite a newer idle collection during GetSettings.
- Original Desktop cleanup received the system temporary root; host metadata now
  uses dedicated native data folders.
- Managed metadata could open extra input readers merely to probe CanOpen;
  background metadata leaves unproven availability and descriptors unavailable.
- Native-only operation would wake for managed snapshots; the observer now parks
  until needed and avoids periodic projection for unused providers.

## Exploratory QA and verification results

No behavioral probes, tests, format/Clippy suites, CI, plugin execution, GUI,
daemon, driver, hardware, game or OBS sessions ran. Fixtures remain unrun.
Actual package compilation, architecture/layout/hash/provenance inspection and
the guarded copy into E:/OTD RUST TEST produce distributables, without runtime
acceptance. Native Linux/macOS checks are deferred.

Concrete upstream readers/hubs/shared endpoint I/O, managed DeviceReport,
native-hosted original DiagnosticInfo and the unchanged binary corpus remain
explicit gaps in [the provider contract](P07_SERVICES_0.17.1.md) and
[the acceptance backlog](WORK_ITEMS.md).
