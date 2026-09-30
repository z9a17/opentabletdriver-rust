# Tablet identification delay

Release 0.14.6 addresses delayed tablet identification in Windows discovery and the panel. The report-processing path is unchanged.

## Source diagnosis and changes

The panel treated a generic device-tree change as a chance to enumerate, but did not register for HID interface arrival/removal messages. A USB tablet can create its pen collection after that first broadcast. Further events received during enumeration were dropped, so an early snapshot could leave the model missing until another scan.

The panel now registers for the HID interface class before its initial scan. Events received during a scan coalesce into one follow-up scan. Completed scans publish identified models immediately; the follow-up includes collections that appeared later. A manual Detect request retains its announcement when coalesced. Registration is released when the panel exits, and registration failures report a warning with the manual Detect fallback. Named targets and unsaved editor values keep their existing behavior.

Every discovery pass also opened all HID interfaces and walked their device ancestry before checking whether their vendor/product IDs existed in the tablet catalog. Discovery now reads the local device instance ID first. Recognized USB HID IDs outside the catalog are skipped before opening their descriptors. Unknown IDs and metadata failures retain descriptor inspection. Physical ancestry is resolved only after descriptor IDs and capabilities identify a catalog candidate. The actual HID descriptors still supply identity and report sizes; the prefilter never selects a tablet by itself.

Indexed USB strings previously came from every identifier sharing the device's vendor/product IDs, including identifiers whose input, output or feature report sizes could not match the collection. Discovery now shares the matcher's report-size predicate and only requests strings required by possible identifiers. String indices remain deduplicated; omitted report sizes remain wildcards, and failed reads remain missing-string rejections. This removes unnecessary synchronous device requests without weakening model disambiguation.

References: [Windows USB identifiers](https://learn.microsoft.com/windows-hardware/drivers/install/standard-usb-identifiers), [HID device identifiers](https://learn.microsoft.com/windows-hardware/drivers/hid/hidclass-hardware-ids-for-top-level-collections), [device notification registration](https://learn.microsoft.com/windows/win32/devio/registering-for-device-notification), [indexed HID strings](https://learn.microsoft.com/windows-hardware/drivers/ddi/hidsdi/nf-hidsdi-hidd_getindexedstring).

## Evidence and remaining validation

These are reachable source defects and removed blocking operations, not a measured hardware speedup. Focused regression cases cover irrelevant hardware IDs, unknown-ID fallback, unnecessary string requests for each report-size constraint, string deduplication/failure, late pen arrivals and coalesced manual detection. They were added but not executed.

Per repository instructions, the format/Clippy/test/build-check suite and CI were not run. Windows binaries and the managed bridge are compiled only to create the release assets. Archive contents and hosted asset hashes are checked separately. No active driver, UI, daemon, plugin session, HID reads or user settings were changed during this work.

Manual validation remains required: launch with a tablet already connected, reconnect while the panel is visible and minimized, and check the model label and dimensions without moving the pen. Repeat with PTK-470 and PTH-660, and with a string-disambiguated tablet if available. Confirm exact named targets, dirty edits, manual Detect and multiple tablets remain correct. Full D02/U03 parity and hardware gates stay open. Necessary synchronous device requests and third-party initialization can still take time.
