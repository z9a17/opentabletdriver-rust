# Tablet identity correction: 0.14.4

The owner reported that a connected PTK-470 appeared as PTH-660. An unnamed profile's panel label fell back to `Wacom PTH-660`, and the editor retained that default tablet's dimensions. This was a UI defect; Windows device selection uses the matched configuration's identity, parser and specifications.

## Correction

An unnamed profile now shows the sole detected model, or an explicit no-device/multiple-device/unknown label. Named profiles retain their saved target label. The editor resolves transient dimensions from the sole detected model. Discovery preserves dirty/invalid edits and controls being edited; loading or explicitly selecting a profile synchronizes its dimensions. Selecting any tablet preserves the last known geometry until detection resolves it.

No tablet target is silently saved. No extra daemon/debug polling is added. Report parsing, output mapping mathematics and smoothing are unchanged. The manager theming, streamlined filter controls and dropdown click-to-close fixes from 0.14.3 remain included.

## PTK-470 and other tablets

The pinned configuration contains `Wacom PTK-470`, USB VID/PID `056a:03f5`, a 192-byte input report and `IntuosV3ReportParser`. Its area is 187 by 105 mm, with 37400 by 21000 raw coordinates and maximum pressure 8191. It has its own initialization report. Windows matching uses device identifiers, report constraints and strings; runtime parsing and managed tablet references use the selected configuration.

The database contains 339 configurations and Rust ports for the 52 referenced parser names. This is configuration/parser coverage, not complete hardware, initialization, auxiliary, transport or application qualification. The owner's report establishes a labeling symptom; no PTK-470 pen/pressure/reconnect trace was collected in this task. With multiple detected models, select a named tablet for its settings rather than treating the aggregate label as one selected model.

## Evidence and limits

Source review identified the fixed-name fallback and the editor synchronization path. Windows release compilation produces the executables and managed bridge. Package contents/checksums are compared with those outputs before publication.

The owner disables CI and the pre-publish format, Clippy, test and build-check suite. Those checks are not run. No panel, daemon, live driver, HID session, third-party plugin or input injection is launched. Active user settings are unchanged. Device labels, dimensions and physical pen behavior remain for manual release validation. No latency improvement is claimed.
