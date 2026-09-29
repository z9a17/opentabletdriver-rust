# Runtime reconfiguration

The daemon's `restart` command and the panel's Apply/Save now prepare a replacement while the current PTH-660 worker continues processing input. This is a bounded C03 implementation for the current synchronous pipeline. It does not complete upstream settings/reset parity or provide hardware-transaction rollback.

`restart --config FILE` supplies replacement settings; `restart` alone reuses the daemon configuration. Restart requires an existing worker in `running` state; a worker still waiting for its initial tablet is not ready for replacement. The client guards configuration retrieval and restart with the daemon instance and worker generation. A stale identity fails without stopping another client's worker. An accepted reply means preparation was requested, not that activation succeeded: inspect `status`, its generation and `last_error` afterward. A timeout does not prove rejection; inspect status before retrying.

## Replacement stages

| Stage | Current output | Candidate and publication |
| --- | --- | --- |
| Prepare | Continues under the old generation | Validate profile, display mapping and pipeline construction; discover the endpoint; construct/reset plugins and open a prepared HID handle on the eventual worker thread. No candidate input reads or hardware initialization occur. |
| Quiesce | Reader ends and pending I/O drains; held output is released | The old source is dropped and retained plugin instances receive reset/range-loss notification before acknowledgement. Only then may candidate activation start. |
| Activate | Old worker is retained at a command gate | Apply selected HID initialization, flush the prepared handle's queued input, and construct fresh mapping/output/relative state. The candidate waits at its first-read gate. |
| Commit | New worker may start reading | Sending `Run` publishes the replacement generation/configuration. The daemon retires the old worker after the replacement acknowledges running. |
| Pre-commit activation failure | Temporarily paused | Stop and join the candidate, then reactivate the retained old worker with its old profile and fresh output/relative state. The published generation stays unchanged. |

A preparation failure leaves the old worker intact. A disconnect during preparation cancels that replacement while the existing worker follows its reconnect path. If a candidate is waiting for its configured tablet, the old worker can keep running; Stop/Shutdown cancel the pending work. Additional replacement requests are rejected while a transaction or retirement is pending.

Once `Run` has been sent, failure stops the committed generation; it does not replay the previous configuration. Cleanup or rollback failure also stops the affected workers and reports failure. No candidate activation follows a failed old-output cleanup. The original driver is never terminated or relaunched by Rust.

Initial Start reserves its identity immediately, so a guarded Stop can cancel preparation using the returned generation. It can remain `starting` while waiting for the configured tablet, without acquiring the injector guard. Before activation, Rust refuses output if the original OpenTabletDriver is running. Initial ownership is acquired after candidate preparation and before activation. Replacement preparation keeps the current generation until commit. During quiesce/activation status can be `stopping`/`starting`; the configuration reply remains the published profile until commit. Use state and error fields together with generation.

## Cancellation, ownership and limits

Stop/Shutdown mark all worker roles cancelled before processing queued readiness notices. Each role has bounded command/status channels, and report logging uses nonblocking bounded sends. The daemon retains the single-injector guard through replacement, rollback and worker joins. Closing the GUI only detaches its client; it does not cancel a daemon transaction.

Cancellation wakes command gates and pending reads. Pending overlapped I/O is drained before its buffers are freed. Synchronous HID string/feature calls cannot be interrupted in flight, and trusted plugin constructors, callbacks or disposal can delay cancellation. There is no forced thread termination or deadline that falsely reports successful cleanup.

Candidate construction/reset executes trusted plugin code before quiesce; arbitrary plugin side effects are not reversible. Rollback retains old plugin instances after their reset/range-loss notification, so private state depends on each implementation. Physical reconnect reconstructs plugin instances. The resumed pipeline starts with fresh output/relative state. HID initialization is reapplied; completed hardware writes are not undone. Multiple devices, broad bindings, async plugins and unsupported output modes are outside this runtime gate.

Saving a profile and activating it are separate guarantees. Panel Save persists first and then requests apply for an attached worker. Runtime failure does not restore the previously saved file; use the explicit [backup/recovery workflow](PROFILE_STORAGE.md). Offline [named presets](NAMED_PRESETS.md) do not activate settings automatically.

The implementation is in `src/runtime.rs`, `src/daemon/state.rs` and the prepared-session first-read gate in `src/session.rs`. It was independently source-reviewed and compiled with strict workspace/all-target Clippy during 0.10.0 integration. No local tests, daemon/GUI/driver launches, plugin execution or hardware validation were performed. Runtime success/failure, cancellation races, reconnect and cleanup evidence remain open; final package results are recorded separately in the release notes.
