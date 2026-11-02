# OpenTabletDriver 0.6.8.1 adoption

Reference: [upstream v0.6.8.1, commit `f3927ac11146cb7dd5c3b43277d5d1e33e36f6d6`](https://github.com/OpenTabletDriver/OpenTabletDriver/tree/f3927ac11146cb7dd5c3b43277d5d1e33e36f6d6), compared with the
[adopted 0.6.8-rc1](UPSTREAM_0.6.8_RC1.md). Automated checks only; nothing here
was tried on hardware, and both new tablets remain unverified.

| Upstream change | Here |
| --- | --- |
| CTK-4120 and XP-Pen Artist Ultra 14 ([#5161](https://github.com/OpenTabletDriver/OpenTabletDriver/pull/5161), [#5157](https://github.com/OpenTabletDriver/OpenTabletDriver/pull/5157)) | Exact configuration blobs. The catalog is 361 configurations; the 54 parsers are unchanged, so the differential parser fixtures still apply. |
| Aspect lock confined only with "Lock to usable area" ([#5193](https://github.com/OpenTabletDriver/OpenTabletDriver/pull/5193)) | Adopted in the original UX and the native panel. |
| Skip Microsoft devices on WinUSB ([#5178](https://github.com/OpenTabletDriver/OpenTabletDriver/pull/5178)) | Adopted in native and managed enumeration. **Deviation:** upstream's edit compares `0x05AC` (Apple) under the Quest message, which drops its `0x2833` check. We keep `0x2833` and do not skip Apple. |
| Scheduler setter no longer sets the interval and rejects replacement ([#5219](https://github.com/OpenTabletDriver/OpenTabletDriver/pull/5219), [#5220](https://github.com/OpenTabletDriver/OpenTabletDriver/pull/5220)) | Adopted in the SDK. **Host change:** `OtdCompat` used to set `Frequency` before injecting the timer, so it now does so afterwards; otherwise async filters would run at the timer's 1 ms default. |
| Linux F13 to F24 in the managed keyboard ([#5197](https://github.com/OpenTabletDriver/OpenTabletDriver/pull/5197)) | Adopted. The native Linux key map already had them. |
| StreamJsonRpc 2.26.150 ([#5194](https://github.com/OpenTabletDriver/OpenTabletDriver/pull/5194)) | Adopted; the lock files move MessagePack from 2.5.302 to 2.5.307. |
| Unit label wrapping ([#5209](https://github.com/OpenTabletDriver/OpenTabletDriver/pull/5209)) | Adopted in the original UX. |
| Daemon SIGTERM handling ([#5158](https://github.com/OpenTabletDriver/OpenTabletDriver/pull/5158)) | Not applicable: the native daemons handle SIGTERM themselves and the upstream daemon is not shipped. |
| macOS and Windows packaging scripts, local-dependency scripts | Not applicable: this project has its own packaging. |
