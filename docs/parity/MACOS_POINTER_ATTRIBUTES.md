# macOS ordinary pointer attributes

The pinned Plugin AbsoluteOutputMode and RelativeOutputMode both invoke
eraser, tilt and pressure handlers before setting position and flushing a
synchronous pointer. Desktop's macOS absolute and relative pointers derive
MacOSVirtualMouse, which implements these handlers. These are ordinary output
mode behaviors, including CGEvent tablet metadata and mouse click counts.

Core exposes copy-only `output::MouseAttributes` through additive ActionSink
`pointer_attributes` and `flush_pointer` methods. Optional LocalActions setup
callbacks connect the platform's existing shared pointer state; default sinks
remain no-ops without additional report allocations. Managed binding and preset
wrappers forward both methods. `has_position` distinguishes an original position
setter (including stationary/zero motion) from an attribute-only report which
must not fabricate a pointer move on Flush. Native post-transform output supplies normalized
pressure independently of contact, honors original threshold pressure remapping,
and retains earlier property assignments when the current report lacks the
interface or the profile disables its pressure/tilt setter. Teardown attempts
pointer reset, other input releases and final flush even after a setter error.

Unchanged managed output pointer commands use the same projection and flush.
Additive flag 64 records whether the eraser setter has ever assigned a value,
separately from flag 16's eraser value; false remains an explicit assignment.
This avoids fabricating a setter on reports without the eraser interface.

Pinned MacOSVirtualMouse does not implement IHoverDistanceHandler. Its proximity
events announce tool changes immediately and refresh idle hover after 200 ms.
Its Reset releases buttons; it does not send a proximity-leave event. Pressure
is the current normalized report pressure, not a synthetic-pen pressure forced
to zero whenever the tip binding is inactive. The platform implementation owns
the retained attributes, CGEvent fields, stationary flush and double-click state.

This source hook was reviewed; no tests, suites, builds, daemon, GUI, plugin or
hardware execution were performed. Platform wiring and package compilation are
owned by the platform peer and release integrator respectively.

Native macOS wiring is in `crates/otd-macos/src/pointer.rs`. Actual system
NSEvent double-click intervals and the pinned eight-pixel queued-position
comparison determine click counts. Relative-mode queued coordinates are deltas,
as in the original; this narrow baseline quirk is retained. Button edges carry
click counts; fresh subsequent move events keep their original default count.
Mouse and tablet pressure, tilt (including inverted Y), eraser/pen device
capability/proximity fields are posted for ordinary modes. Repeated stationary
positions flush metadata; attribute-only packets do not fabricate motion.
Shared OS-code ownership suppresses duplicate held edges and preserves copied
pointer context for a pending final release after a reader has retired. Fresh
CoreGraphics events avoid subtype union contamination without Rust report-path
heap allocations. TCC/output delivery and native macOS behavior remain unrun.
