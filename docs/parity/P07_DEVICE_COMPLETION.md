# P07 concrete driver and shared-device completion

Owned by managed_parity on parity/managed-device-completion from 632a37d.
Pinned source: 736003ed72c8bbb28033b039d5a0bb76c344145c.

The Core source helper preserves OpenTabletDriver 0.6.7 public assembly identity.
Internal source patches are listed in compat/UpstreamCore/PROVENANCE.md. Original
Driver/InputDeviceTree/InputDevice/RootHub services use actual host-owned endpoints,
matched configuration and identifiers. A source-session reader generation selects
physical ownership; ambiguous tree injection fails. Original managed parser threads
consume bounded shared tees; overflow/gaps terminate their stream explicitly.
Native owns hardware initialization and physical handles. Direct tree output
assignment requires completed Op11 ownership handoff; retired trees drain their
managed callback before requesting native output restoration.

ABI ops 4..9: Open {path,session_id?,device_generation?,reader_generation?,
lease_ms:15000,capacity_reports:256} returns {stream,report_length,last_sequence};
Read {stream,after_sequence,limit:1} returns {reports:[{sequence,data:hex}],
next_sequence,lost_reports,closed}. Write/SetFeature {stream,data:hex} complete
only after real native I/O. GetFeature returns equal-size hex. Close is idempotent.
Endpoint snapshot requires Identifier/Configuration and exact reader-generation
fields for concrete attachment. Operations execute on a separate native I/O lane.
Original constructor stores and plugin-type metadata are exposed via original
PluginSettingStore object constructors, not attribute-derived guessed defaults.

No tests, builds, plugin execution or hardware validation were run. Parent owns
native backend and package compilation/dependency-lock generation. Implementation
and runtime acceptance remain separate. Further commits supply report events,
custom hub native forwarding, hosted RPC and typed diagnostics.
