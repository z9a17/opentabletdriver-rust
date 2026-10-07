# S03 idle settings collection work

Claim: S03, bounded C03-C05 original settings collection round-trip.

Owner: desktop_parity. Branch: `parity/p07-desktop-providers`.
Base: `4595ee5` (origin/main). Scope: `src/upstream_rpc/*`, collection accessors in `src/upstream_rpc.rs`; shared startup/provider glue coordinated with the integration owner. Core configuration and bindings remain owned by control_bindings_parity.

The pinned baseline is [OpenTabletDriver 0.6.7, 736003e](https://github.com/OpenTabletDriver/OpenTabletDriver/tree/736003ed72c8bbb28033b039d5a0bb76c344145c). `DriverDaemon.SetSettings` retains a complete collection even with no input tablets, while `Settings.Serialize` is an explicit client-side write. `ProfileCollection.GetProfile` retains disconnected profiles and adds defaults only when a named detected tablet needs them.

The deliverable is a bounded retained original collection, truthful offline Get/Set/Reset, guarded explicit load/save, reuse of existing active-device apply and rollback, and a cold startup accessor for applicable retained profiles. Settings not representable by original JSON must continue to fail explicitly. Enabled global tool lifetime and managed provider callbacks require coordinated ownership rather than successful placeholders.

Implemented source behavior:

- The process owner retains original JSON across RPC connections and managed provider calls. It reads `data/upstream-rpc-settings.json` once on cold initialization, falling back to the original OpenTabletDriver settings file or an empty pinned collection. Unknown JSON fields, duplicate rows and disconnected tablet profiles remain present; matching follows the pinned first-name row semantics.
- `SetSettings` changes memory only. Idle empty/disabled Tools collections do not construct plugins or require tablet geometry. `ResetSettings` yields empty pinned defaults while idle and creates profile defaults from actual configuration metadata when devices are available. Active replacement still preflights every applicable profile and uses guarded per-device receipts plus owned-generation rollback; this is not an atomic group transaction.
- `GetSettings` overlays registry-owned authored profiles, including retained explicit native disconnected/stopped profiles, onto the collection. It rejects native settings lacking an exact original representation and differing same-model physical profiles. Active native ABI stores and unknown native extensions cannot silently be discarded by original `SetSettings`.
- A connection that has read settings records collection revision and daemon/device generations. A stale subsequent Set fails before mutation. Concurrent/nested collection operations return Busy; setup readers never acquire the mutation reservation while candidate preparation waits. Cached provider reads are nonblocking and never initialize state or invoke IPC/CLR.
- `GetApplicationInfo.SettingsFile` identifies the actual original-format file. Rust RPC extensions `LoadSettings` and `SaveSettings` are explicit; the pinned 19-method interface remains unchanged. Save uses the core exact-byte file snapshot, backup and sidecar writer lock, and returns file conflicts. This cannot provide an atomic compare-and-swap against external writers that ignore the sidecar lock.

Integration contracts in `src/upstream_rpc.rs`:

```rust
get_original_settings() -> Result<Value, String>
initial_original_settings() -> Result<Value, String>
cached_original_settings() -> Option<Arc<Value>>
settings_collection_revision() -> Result<u64, String>
cached_settings_collection_revision() -> Option<u64>
publish_idle_original_settings(Value, expected_revision: u64) -> Result<u64, String>
set_original_settings(Value) -> Result<(), String>
set_original_settings_expected(Value, WorkerIdentity) -> Result<(), String>
set_original_settings_expected_with_source(Value, WorkerIdentity, Option<(String,u32)>) -> Result<(), String>
set_original_settings_expected_with_source_generation(Value, WorkerIdentity, Option<(String,u32)>, Option<(String,u64)>) -> Result<(), String>
reset_original_settings() -> Result<(), String>
load_original_settings() -> Result<(), String>
save_original_settings() -> Result<(), String>
original_settings_path() -> Result<PathBuf, String>
original_application_info() -> Result<Value, String>
profile_for_tablet(&TabletConfiguration) -> Result<Option<Profile>, String>
invoke_original(&str, &Value) -> Result<Value, String>
original_resynchronize_epoch() -> u64
```

These are background/control setup operations. The cached accessor alone is suitable for a nonblocking provider snapshot read. Managed service mutations must be dispatched away from report callbacks and pass admission-captured native identity where available. Root integration owns the actual original Desktop provider binding and startup lookup call sites: explicit per-physical native persisted/Apply settings outrank retained model rows; retained RPC/saved collection rows outrank original imported/default model settings. Without those call sites, collection storage alone does not change a future tablet pipeline.

Idle publication uses the additive native command `SetIdleOriginalSettings { expected: WorkerIdentity, expected_revision: u64, settings_json: String }`, expecting `OriginalSettingsCommitted { revision: u64 }`. Root owns this protocol and daemon handler dependency. The daemon owner must atomically reject changed identity, active/pending preparation and connected sessions before calling the short publication helper; the helper never reacquires the RPC mutation reservation. Default/RPC-origin workers read one retained document snapshot before each physical reconnect; the initial peer lookup uses that same accessor. This cold path does not acquire the mutation owner or perform control IPC. Explicit native Apply and saved physical profiles retain precedence. Initial incoming RPC candidates retain their proposed document until the collection transaction publishes; they do not reload the previous retained document. Inner quiesce/rollback activation retains its owned settings. The transient use_settings_collection marker is excluded from authored TOML and exposed in session metadata; stopped Start and rollback preserve its origin. Parent primary lifecycle hooks and the additive original-profile command are integration dependencies.

Original managed PresetBinding may supply source tablet name and binding owner. The setter resolves exactly one running physical source or fails before mutation. Admission-captured physical ID and device generation must still identify that unique running source before any mutation; a newer same-name tablet or generation cannot inherit an old queued request. Original replacement uses additive `ApplyOriginalDeviceProfile { expected, id, device_generation, profile_toml, binding_inhibit: Option<u32> }`; the native owner sets the transient original-collection marker and optional source inhibition after parsing. This preserves held-preset suppression without serializing it into settings. Rollback restores the prior original/native origin explicitly. The root protocol/handler and B04 core field are integration dependencies.

GetSettings skips stale disconnected collection-origin profiles, including primary global Tools, so observation cannot overwrite a newer idle Set before reconnect. Explicit native disconnected profiles remain projected. Peer reconnect publication occurs only after Run dispatch and updates device ownership even when authored TOML remains unchanged.

Source fixtures cover the transient origin/TOML boundary, offline unknown-store/extension preservation, empty reset, invalid collection shapes, stale revision rejection, idempotent Set, and the pinned omitted-Enable=false JSON behavior. They are unrun. Existing active-device receipt/rollback fixtures remain unrun too.

Remaining limits: enabled global Tools cannot be applied while no running primary owns their lifetime; Set/Load return an explicit unsupported error and retain the prior collection. Unknown disconnected plugin stores are retained, but are validated/resolved only when their tablet is explicitly prepared. Collections and individual imported profiles are bounded to 128 KiB; transport frames remain bounded to 256 KiB. New default rows use actual tablet configuration and virtual-screen metadata; no connected tablet is inferred from a saved profile. Rust native profile files are not rewritten by original collection Save.

Validation: no tests, formatting, Clippy, check suites, package builds, plugins, UI, drivers, daemons, hardware or live input are executed. Source fixtures may be authored but remain unrun. Package compilation and release integration belong to the parent agent; original-client and hardware behavior remain unverified.
