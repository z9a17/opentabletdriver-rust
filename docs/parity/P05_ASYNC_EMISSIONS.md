# P05 background managed emissions

Implementation scope: unchanged `IPositionedPipelineElement<IDeviceReport>.Emit`
raised by a plugin-owned thread or after its `Consume` callback returns. Source
baseline: original `Plugin/Output/PipelineManager.cs` and
`AsyncPositionedPipelineElement.cs`, revision
`736003ed72c8bbb28033b039d5a0bb76c344145c`.

`Instance` retains the immediate same-object continuation for synchronous Emit.
Foreign/late Emit instead reserves a graph-owned FIFO slot, copies the report's
instance data, and returns without invoking any native callback. The graph's
owning native report thread drains ready entries in admission order before its
next report/timer tick. Each resumes **after** the emitting filter at its exact
pre/post-transform continuation, including original managed output-mode pipeline
subscriptions. Suppression, replacement reports and multiple outputs retain
those continuation semantics. Fields representing source time remain in the
owned report; native source/debug stamps are unchanged, while actual emission
uses the owner's current scheduler time.

The graph allocates 1,024 slots at setup and permits at most 64 pending entries
per filter. Owned payload accounting is bounded to 4 MiB across pending reports
and in-flight snapshots. A snapshot reserves its maximum 256 KiB before copying;
retirement/failure cannot reclaim that reservation while the copy still runs.
Copied arrays use their actual CLR element size, including custom value structs.
Each snapshot also limits depth to 24, object count to 2,048 and instance fields
to 256 per type. Graph bookkeeping and CLR allocation overhead are additional
to the payload budget. Data-only reports retain their original concrete CLR type
through MemberwiseClone and field-based deep copying (including nested arrays,
private fields, boxed report structs and cycles). Type metadata caches use weak
keys so they do not root a retired collectible plugin assembly. No serializers,
report getters or plugin callbacks execute during copying. Live execution or
device-handle fields, unmanaged pointers and unsupported array shapes produce
explicit ownership errors rather than borrowed mutable output.

Overflow/ownership errors identify and disable the emitting filter, discard its
pending graph batch with an explicit error, and flow through the existing native
pipeline fault/release handling. Queue failures are consumed once; they cannot
leave an immediate-error tick permanently due. A not-yet-ready copy does not
force a busy loop. Ready reports/errors are immediately due; a live managed
filter graph otherwise retains a 10 ms maximum idle poll even without an injected
timer. Its native capability query never returns the cacheable timerless `-2`.
This is bounded polling, not a jitter/latency claim or callback wake guarantee.
Native profiles without a managed graph retain their existing allocation-free
report path. Synchronous Emit makes no snapshot or queue allocation.

Range-loss reset clears the queued batch before notifying plugins, retaining
reservations for unfinished copies until they complete. Graph retirement detaches
callback sinks and invalidates all queued reservations
before releasing its registry generation. In-flight copies may finish but cannot
restore a retired slot or invoke native output. Filter disposal unsubscribes its
SDK event before disposing its plugin/services/timers. Arbitrary plugin-owned
threads still belong to the original plugin's IDisposable contract; the host does
not pretend it can terminate uncooperative third-party code.

The existing GraphNextTick/TickGraph ABI is unchanged. Rust descriptions now
cover background replay as well as injected timers. Legacy sample-only ABI callers
retain their explicit inability to represent multiple/late reports. Direct
asynchronous calls to a custom output mode's pipeline from outside its owning
Read are a distinct unsupported output lifecycle, not silently accepted as filter
Emit.

Offline source fixtures were added for background FIFO, concrete report identity,
post-Emit mutation/nested-array ownership, overflow, bounded idle scheduling and
retirement detachment. They were **not executed**. No tests, formatting, Clippy,
check suites, builds, managed/plugin execution, UI, daemon, hardware or live output
were run by this agent. Actual package compilation belongs to the integrator;
representative original interpolation binaries, sustained jitter/memory and live
retirement behavior remain for owner validation.
