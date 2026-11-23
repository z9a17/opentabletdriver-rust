# OpenTabletDriver changes after 0.6.8.1

Reference: [upstream `0.6.x`, commit `3345dc73a2d607d013a4762e0fa2672118ef132e`](https://github.com/OpenTabletDriver/OpenTabletDriver/tree/3345dc73a2d607d013a4762e0fa2672118ef132e), compared with the
[adopted 0.6.8.1](UPSTREAM_0.6.8.1.md). Automated checks only; nothing here was
tried on hardware. The three fixes marked open came from pull requests that were
still open upstream at the earlier 0.20.0 revision, so they may change before release.

| Upstream change | Here |
| --- | --- |
| Tablet updates ([#5083](https://github.com/OpenTabletDriver/OpenTabletDriver/pull/5083), [#5127](https://github.com/OpenTabletDriver/OpenTabletDriver/pull/5127), [#5200](https://github.com/OpenTabletDriver/OpenTabletDriver/pull/5200), [#5204](https://github.com/OpenTabletDriver/OpenTabletDriver/pull/5204), [#5208](https://github.com/OpenTabletDriver/OpenTabletDriver/pull/5208), [#5308](https://github.com/OpenTabletDriver/OpenTabletDriver/pull/5308)) | Exact configuration blobs: Gaomon GM156HD (16K) and Huion H950P (16K) are added, Wacom DTH-2700 and Huion New 1060 Plus (2048) gain identifiers, PTH-460 and PTH-660 declare touch sizes, and Bosto BT-12HD declares 8 auxiliary buttons. The catalog is 363 configurations with the same 54 parsers. |
| Bosto BT-12HD aux & pen bit check ([#5308](https://github.com/OpenTabletDriver/OpenTabletDriver/pull/5308)) | Adopted in the native `BostoReportParser`: `0x01` with bit 4 of byte 1 set decodes as pen, `0x04` decodes as `UCLogicAuxReport`, and other packets decode as `OutOfRange`. |
| Bamboo 8-byte report length check ([#5311](https://github.com/OpenTabletDriver/OpenTabletDriver/pull/5311)) | Adopted in the native `BambooReportParser`: 8-byte Bamboo tablet and auxiliary reports emit a null wheel position instead of failing with a short-report error. |
| Linux `hid_digimend_uclogic` driver detection ([#5279](https://github.com/OpenTabletDriver/OpenTabletDriver/pull/5279)) | Adopted in `packaging/linux/install.sh` alongside `wacom` and `hid_uclogic` (already handled in `crates/otd-linux`). |
| IntuosV3 tilt is signed ([#5215](https://github.com/OpenTabletDriver/OpenTabletDriver/pull/5215), open) | Adopted in the native parser: `0xFF` is -1 rather than 0, and negative tilt is no longer off by one. |
| Giano eraser ([#5232](https://github.com/OpenTabletDriver/OpenTabletDriver/pull/5232), open) | Adopted in the native parser for flag bit 5. Upstream's author verified it on a Kamvas Pro 19 (4K). |
| One-step thresholds for wheels added to existing profiles ([#5237](https://github.com/OpenTabletDriver/OpenTabletDriver/pull/5237), open) | Adopted in the managed `BindingSettings`. The native path already defaults a missing wheel to one step. |
| Artist `resolution` set to 1000 ([#5175](https://github.com/OpenTabletDriver/OpenTabletDriver/pull/5175), open) | Not adopted. The value of 100000 came from [#1671](https://github.com/OpenTabletDriver/OpenTabletDriver/pull/1671) to stop cursor jitter on Sway, and the pull request itself questions whether that problem is gone. [#5174](https://github.com/OpenTabletDriver/OpenTabletDriver/issues/5174), wrong proportions on KDE, therefore also remains here. |
