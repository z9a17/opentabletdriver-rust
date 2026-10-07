# Unix runtime implementation workstream

Owner: platform completion agent; branch `parity/platform-completion`, baseline
`632a37d` (0.17.1). Stable scope X01–X05/O04. The historical GitHub platform
tracker is administratively retired; this file records the continuation claim.
Pinned upstream is OpenTabletDriver 0.6.7 at
`736003ed72c8bbb28033b039d5a0bb76c344145c`.

The portable runtime now shares the shipped native ABI, managed graph, original
parser instances, registry generations, and scoped service contracts. Unix
hostfxr uses byte `char_t` and `.so`/`.dylib` loading. Linux distributions use the
GNU target so real CoreCLR/native dynamic loading is available; historical
static-musl CLI binaries do not establish unchanged-DLL support. Supported native
profiles initialize no CLR unless an explicitly requested managed service needs
it. Original plugin constructors retain their actual loaded registry generation.

Both platforms have real multi-device workers, independent digitizer/auxiliary
parsers, prepared-candidate activation, old-owner cleanup, generation-fenced
replacement and rollback. Original custom hubs enter discovery only after their
actual saved global tool connects them, as in upstream; registry metadata does
not construct arbitrary hubs. One actual original stream reader owns each custom
endpoint. Native/custom auxiliary combinations share the bounded physical owner
broker. Its service paths provide real writes/features/explicit indexed strings,
raw reports and exclusive output ownership without opening another input reader.
Custom packet timestamps record actual managed read completion. Producers copy
into preallocated report storage and never allocate successful native reports.

Linux retains uinput Artist Mode. Pinned macOS supplies mouse/keyboard, without a
Windows Ink or Linux Artist Mode pen-output contract. All native keys, mouse
buttons, contacts and original scoped input share one OS-code hold ledger across
readers; OS aliases retain independent action owners. Pending releases retry on
an independent idle service lane even after the last native worker retires.

Original imported Key/MultiKey bindings and unchanged managed SessionKeyboard
use exact pinned platform dictionaries. Their synthetic identities are Windows
`0x2000 | VK`, macOS `0x4000 | CGKeyCode`, and Linux `0x8000 | evdev`. Native TOML
physical HID bindings keep their prior meanings; `vk:`, `cg:` and `evdev:` prefixes
persist explicit original identities. macOS aliases include Equal/KeypadEqual,
Plus/Add, Slash/Backslash and Clear/NumberLock. Linux original Multiply is
KEY_NUMERIC_STAR and StopSong is KEY_STOP, distinct from native physical/media
extensions. Original export selects exact representable names and rejects native
positions absent from the original dictionary. The existing Rust None binding
remains a no-op: pinned macOS's literal None entry is CG0, which its original
KeyBinding would send as A. That narrow baseline quirk is deliberately not copied;
real A retains encoded CG0. No exact equivalence is claimed for that quirk.

A private current-user AF_UNIX native endpoint supplies multi-device control,
persistent JSON console and foreground service ownership. Optional original
StreamJsonRpc uses the actual RpcHost/IDriverDaemon and original named Instance.
The unchanged original Eto GTK/macOS UX and Console source are included in their
managed projects; distributions stage their real dependency graphs and launchers.
The frontend and native owner never become independent competing input readers.

Original settings collections, global tools, plugin manager, current logs,
application/diagnostic information, device strings and report subscriptions now
route real services. Tools start independently of tablet presence and dispose
before readers; callback lanes remain alive through their actual retirement.
Updater check/install uses real release lookup, HTTPS/SHA256, shared journaled
staging and cancellable downloads. A guarded reservation drains tools, readers
and managed retirement before replacement. Successful original/native replies
must flush before FinishUpdate shuts down; cleanup/recovery failures retain the
reservation and report errors. Cold native updates acquire the same ownership
lease without starting tablets. Startup recovery occurs under that lease before
loading installed managed code. System curl/tar and .NET/Eto runtime prerequisites
are explicit distribution requirements, separate from input permissions.

The source-only topology audit found that pinned DesktopInterop caches its
VirtualScreen. WaylandDisplay performs two constructor Roundtrip calls and then
disposes its connection; XScreen snapshots its monitor list/position and contains
no XRandR event-dispatch refresh path. Linux native uinput axes and display layout
therefore retain startup geometry. macOS NativeDisplays additionally checks a
fixed 32-monitor layout during native report refresh. Generic Wayland discovery
fallback to the actual read-only original display provider is a coordinated
follow-up; Hyprland/Sway/X11 native backends remain CLR-free.

No tests, format/Clippy/check suites, driver, daemon, UI, managed/native plugin,
hardware, game or OBS execution occurred in this workstream. Fixtures were
written but not run. The parent owns actual package compilation/distribution.
Native permissions, TCC, signing, desktop integration, output acceptance and
hardware behavior remain deferred to owner validation. Source implementation
and cross-compilation do not establish executed unchanged-plugin or live-device
compatibility, and this document does not declare full parity verified.
