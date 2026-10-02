# Disconnected tablet area

Release 0.16.3 keeps the tablet canvas empty when no selected tablet is detected. No gray full-tablet rectangle, blue mapping, dimensions, ratio, center dot or error text appears there. Connection information remains in the existing status bar. Numeric area settings remain stored and editable; disconnecting does not reset a profile or discard pending/invalid values.

The preview uses the detected device's catalog dimensions. It does not use the profile's default PTH-660 spec or the last connected device as evidence of present hardware. An automatic profile requires one identified model. A named profile requires that model among the detected devices; another connected tablet does not stand in for it. Unresolved/ambiguous selection and failed discovery leave the canvas empty. A successful scan after reconnection restores the matching device's bounds and the saved mapping.

Every discovery completion refreshes the layout, including empty results and failures, independently of whether profile values can be synchronized. A missing preview cannot begin a drag or supply context-menu area actions. Removal cancels a pending tablet drag; a menu command selected after removal cannot change the saved area. Known-device invalid mappings still show their validation message. Display rendering is unchanged.

## Original OpenTabletDriver reference

The reference is stable v0.6.7 at commit `736003ed72c8bbb28033b039d5a0bb76c344145c`:

- [OutputModeEditor.SetTabletSize](https://github.com/OpenTabletDriver/OpenTabletDriver/blob/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.UX/Controls/Output/OutputModeEditor.cs) sets usable bounds from the selected digitizer and clears them when no digitizer is available.
- [AreaDisplay.OnPaint](https://github.com/OpenTabletDriver/OpenTabletDriver/blob/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.UX/Controls/Output/Area/AreaDisplay.cs) draws tablet geometry only with valid usable bounds. Unavailable bounds and invalid configured area are separate states.
- [AbsoluteModeEditor](https://github.com/OpenTabletDriver/OpenTabletDriver/blob/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.UX/Controls/Output/AbsoluteModeEditor.cs) supplies unavailable/invalid messages. [ControlPanel](https://github.com/OpenTabletDriver/OpenTabletDriver/blob/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.UX/Controls/ControlPanel.cs) hides device-specific editors without a selected tablet.

The owner explicitly requested an empty tablet canvas in this port. Retaining the output page and numeric settings while putting connection status outside the canvas is the presentation difference; fabricating connected geometry is not part of the upstream behavior.

## Verification and remaining evidence

Focused regressions cover the reported 82.6 by 48.2 mm mapping, default/named/automatic profiles, detected PTK-470 dimensions with a stale PTH-660 profile spec, removal/reconnect, profile reload while absent, failed discovery, ambiguity and invalid pending settings. The UI regression drives actual discovery completions, layout, paint, hit testing and context actions through an isolated hidden owner. It checks every tablet-canvas pixel against the theme background in dark/light/high contrast and confirms known-device geometry/errors actually render. It has no daemon client, app startup, live discovery or preferences writes. Run after publication with `cargo test --locked --bin opentabletdriver-rust preview -- --test-threads=1`; `OTD_AREA_TEST_OUTPUT` optionally captures BMP renders.

Per repository policy, the pre-release format/Clippy/test/build-check suite and CI are not run. Actual release compilation and artifact verification produce the four platform downloads. Post-publication execution/results belong in the linked PR, workstream issue and release notes. Hidden synthetic checks do not prove live HID removal/reconnect, physical input, interactive desktop behavior or native Linux/macOS execution. U02 and broader parity gates remain open.
