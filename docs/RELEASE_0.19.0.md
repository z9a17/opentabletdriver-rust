# OpenTabletDriver Rust 0.19.0

This release adopts selected OpenTabletDriver 0.6.8-rc1 fixes and reduces driver
overhead. Native device handling, mapping, scheduling and output remain in Rust;
C# supports existing .NET plugins and the original Unix frontends. The
optimization pass adds no pages, settings or controls to the driver UI.

**Upgrade requirement:** managed plugins, original frontends/Console and other
managed services now require the architecture-matching **.NET 10 runtime**.
Native-only input retains its existing runtime independence. Preserve saved
settings and plugins when upgrading.

- Update the tablet catalog to 359 configurations and 54 referenced parsers,
  including Parblo A610 Plus and Silvercrest SGT 10.6 A2. Correct Wacom auxiliary
  matching, ignore XP-Pen status packets as button input, and preserve stationary
  wheel accumulation. Exclude unrelated Google fastboot/Quest WinUSB devices.
- Correct aspect-locked area resizing and Full/Quarter sizing at tablet bounds.
  Carry pen rotation through capable output backends and support disabling it.
  Protect bulk diagnostic string dumps, reload configuration overrides on Detect,
  and warn when the Windows panel and daemon builds differ.
- Adopt the 0.6.8 Plugin SDK and synchronous Internal-filter pressure ordering.
  Preserve retained report identities and provide compatibility defaults for
  older SDK implementations. Asynchronous Internal filters remain explicitly
  unsupported; ordinary asynchronous filters retain their supported continuation.
- Skip admission bookkeeping where the graph does not need it, reuse unchanged
  managed service snapshots, and wake Windows sessions for queued managed work.
  Timer/provider-capable graphs and endpoints keep bounded polling where needed.
  Wake handling preserves shutdown, ownership changes and pen/auxiliary fairness.
- Seek directly to unread debugger packets; caught-up reads allocate no memory.

Three alternating synthetic managed-graph runs reduced the median of process
medians from 542.7 to 215.7 ns/report and managed allocation from 410.2 to 384.0
bytes/report. The fixture has an ordinary filter, no Internal nodes and no managed
output. These are bridge CPU measurements, not physical pen or gameplay latency.
See the [optimization evidence and reproduction data](https://github.com/z9a17/opentabletdriver-rust/blob/v0.19.0/docs/PERFORMANCE_IMPROVEMENTS_2026-10-15.md).

The preceding implementation work passed 22 focused RC-adoption native cases,
29 optimization regression tests and six synthetic managed fixture modes. The
old-SDK fixture loaded unchanged, and a focused surface comparison retained all
532 original public/protected SDK members. Those checks do not qualify every
third-party plugin. [RC adoption scope and limits](https://github.com/z9a17/opentabletdriver-rust/blob/v0.19.0/docs/parity/UPSTREAM_0.6.8_RC1.md)
record the exact source composition and remaining acceptance work.

Windows x64 (MSVC), Linux x64 (GNU), macOS Intel x64 and macOS Apple Silicon arm64
packages are built from one clean merged commit. Publication verifies archive
contents, architecture, version, source provenance, managed dependency/license
closure, embedded Windows icons and SHA-256 hashes. No separate broad
format/Clippy/test/build-check suite or CI was run during release preparation;
the actual release binaries were compiled and their packages checked.

No driver, UI, hardware discovery or real input/output was launched during this
work, and saved settings were not edited. Physical tablet behavior, hotplug,
sleep/wake, Windows Ink drawing, native Linux/macOS execution, the full unchanged
plugin corpus and pen-to-display latency remain unverified for this release.
Linux requires glibc 2.31+ and GTK3/.NET 10 for the original frontend. macOS
packages require macOS 11+ and the matching .NET 10 runtime for the frontend;
they are unsigned and not notarized. Generic Wayland display discovery uses
.NET; native display discovery or explicit `--screen` remains available without it.
