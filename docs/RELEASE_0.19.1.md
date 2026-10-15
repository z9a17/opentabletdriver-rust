# OpenTabletDriver Rust 0.19.1

This follow-up reduces background work, extends bounded asynchronous Internal
filter support, and protects local files during update recovery. Native driver
processing remains in Rust. The UI gains no pages, controls or settings.

- Windows device supervision wakes on control/device/lifecycle events and real
  rescan deadlines. Managed input leases use their next expiry or failed-release
  retry deadline instead of fixed empty polling. Unix retains its existing
  bounded native-cleanup retry.
- Managed observation reads native published state directly, eliminating three
  recurring status/session/log IPC requests. Unchanged session trees and logs
  reuse cached data; an observer with no snapshot consumer parks. The existing
  250ms state observation and 3-second physical-device fallback remain where
  needed for freshness.
- SDK asynchronous Internal filters and owned late/foreign reports preserve
  pressure admission through the bounded FIFO. Explicit reset retires known old
  identities, including in-flight captures. Unsupported report ownership still
  fails explicitly. Arbitrary unsynchronized plugin mutation, newly synthesized
  normalized reports and range-loss-only retirement remain outside this guarantee.
- Updates reject directory/file collisions, duplicate/overlapping journal paths,
  Windows path aliases and relative links/junctions before moving files. Failed
  rollback keeps the journal and backups for retry. An explicitly selected
  installation root may still be a junction.
- Current documentation separates RC source composition and passing scoped
  fixtures from the historical 0.6.7 acceptance ledger.

Focused offline checks passed: 25 distinct Rust cases (including three Unix
ownership-state cases compiled on Windows), seven synthetic managed modes, and
44 fresh lifecycle cycles across 22 settings cases for five unchanged filter
classes from RadialFollow, BezierInterpolator and Flip Axes. Archive/DLL hashes
and detailed limitations are recorded in the
[follow-up evidence](https://github.com/z9a17/opentabletdriver-rust/blob/v0.19.1/docs/parity/AUTONOMOUS_FOLLOWUP_2026-10-15.md)
and [real-plugin corpus](https://github.com/z9a17/opentabletdriver-rust/blob/v0.19.1/docs/parity/OFFLINE_PLUGIN_CORPUS_2026-10-15.md).
These bounded traces do not establish whole-package or ecosystem compatibility.
Scheduling work counts are not measurements of physical latency.

Windows x64, Linux x64, macOS Intel x64 and macOS Apple Silicon arm64 packages
are built from the same clean merged commit. The actual release binaries and
archives receive architecture, dependency, provenance and SHA-256 verification.
No separate broad format/Clippy/test/build-check suite or GitHub CI was run.

No driver, UI, physical discovery or real input/output was launched; saved
settings were not edited. Physical input, hotplug/sleep, Windows Ink drawing,
interactive applications, native Unix execution and end-to-end latency remain
unverified. Managed features still require the CPU-matching .NET 10 runtime.
Native Linux targets glibc 2.31+; its frontend also needs GTK3. Native macOS
targets 11.0, while packaged .NET apphosts/nethost declare a 12.0 binary minimum
and managed use requires an OS supported by .NET 10. macOS packages remain
unsigned and not notarized.
