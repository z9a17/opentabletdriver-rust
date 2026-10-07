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
