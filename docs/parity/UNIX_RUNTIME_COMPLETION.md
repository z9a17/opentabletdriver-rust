# Unix runtime implementation workstream

Owner: platform completion agent; branch `parity/platform-completion`, baseline
`632a37d` (0.17.1). Stable scope X01–X05/O04. The historical GitHub platform
tracker is administratively retired; this file records the continuation claim.

The source baseline is OpenTabletDriver 0.6.7 at
`736003ed72c8bbb28033b039d5a0bb76c344145c`. The existing Linux uinput Artist Mode
is retained. Pinned macOS supplies mouse/keyboard output; it has no Windows Ink
or Linux Artist Mode pen-output contract.

Implementation is in progress. A portable runtime reuses the shipped native
filter ABI, managed graph, original parser instances and managed service ABI.
Unix hostfxr uses byte `char_t` and `.so`/`.dylib` libraries; Windows retains its
wide strings and restricted LoadLibraryEx flags. Native profiles never need CLR.

The chosen frontend is the pinned original Eto Gtk/macOS UX, connected through
the real IDriverDaemon service contract. The native reader/output owner remains
Rust; an upstream Driver/RootHub must not create a second reader. UI source,
runtime prerequisites and native distribution packaging are tracked separately.

Linux dynamic plugins require a dynamically linked distribution. A static musl
executable cannot host .so/CoreCLR. The integrator is preparing the actual GNU
runtime distribution rather than claiming parity from the historical static CLI.

No tests, format/Clippy/check suites, driver, daemon, UI, managed/native plugin or
hardware execution is authorized in this workstream. The parent compiles actual
packages; native permissions, displays, output, signing and hardware validation
remain deferred to the owner. Implementation progress is not execution evidence.

The next increment adds a native multi-device owner with per-device generations,
prepare/retire/activate ordering, rollback, a private current-user Unix control
socket, a persistent JSON console, and opt-in original StreamJsonRpc hosting.
Global original tools have one cold owner independent of tablet readers; receipts
wait outside native dispatch. Native profile startup remains free of CLR.

Linux explicit stream writes/features/strings use the existing hidraw reader's
bounded mailbox. macOS stream writes/features use asynchronous IOKit callbacks
on its existing CFRunLoop, retaining buffers and callback length storage through
completion or close. The shared output authority releases native held inputs
before acknowledging original output ownership. Activation is acknowledged only
after core mapping/binding/output construction succeeds.

Unix plugin installation reuses the same catalog identity, hash verification,
staging, journal and recovery code. Explicit ZIP installation uses the packaged
OtdArchiveTools helper with traversal/link/alias/expanded-size rejection before
writes; ordinary native profiles do not launch it. System curl is required for
explicit network catalog/update operations. GNU/CoreCLR and original Eto runtime
prerequisites are distinct from native input/output permissions.

Remaining work in this branch includes auxiliary multiplexing, custom managed
hub candidates, foreground CLI service ownership, full updater wiring and cold
CLI aliases. These are implementation gaps, not deferred hardware-only checks.
The first increment intentionally returns explicit errors for native original
commands whose portable service has not yet been connected. The real hosted
managed provider already routes debug subscriptions through the endpoint broker.
