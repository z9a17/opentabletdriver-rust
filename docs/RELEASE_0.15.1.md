# Release 0.15.1

This release combines the pen side-button implementation from PR #52 and the Windows startup update prompt from PR #60. Every publication continues to include Windows x64, Linux x64, Intel Mac and Apple Silicon packages from one clean merged commit. The release publisher rejects an incomplete platform set or mixed source revisions.

## Pen side buttons

The first two pen side buttons default to right-click and middle-click in mouse output. The third defaults to no mouse action. Profiles can override these with mouse buttons, keyboard chords or no action, and supported OpenTabletDriver pen bindings import into the shared Rust profile. See [the pen-button guide](PEN_BUTTONS.md) for configuration and remaining binding limits.

Windows Ink and Linux Artist Mode carry adaptive bindings as pen barrel buttons. macOS uses CoreGraphics mouse output and has no pressure-sensitive pen output. Unsupported platform key usages produce startup diagnostics. macOS bounds stale modifier-release suppression to 50 ms; physical and synthetic holds of the same modifier are not independently owned.

Mouse-left side bindings share ownership with tip contact, so releasing either hold does not release the other. Sessions release held bindings on range loss and shutdown. This is implemented behavior; physical button and drawing-app validation remains open.

## Startup update prompt

The Windows panel opens its existing update prompt when its enabled startup check finds a newer release. A minimized launch shows the panel for the prompt. Up-to-date checks remain quiet, failed background checks log to the Console, and a closing panel ignores late results. Installation and restart still require the user to choose them in the prompt.

## Release evidence and limits

Actual release compilation produces the Windows GNU-target driver, panel and rebuilt .NET bridge, the static Linux musl driver, and both macOS architectures targeting macOS 11 or newer. Package verification checks binary architecture, embedded version, executable permissions, source commit and file hashes. Publication compares GitHub asset digests with the local checksums before making the complete release public.

Per repository policy, no separate format, Clippy, test or build-check suite or CI runs. No tablet interaction or input injection is performed for this release. Native Windows and macOS execution is unavailable on this Linux host. macOS packages are unsigned and not notarized. Linux and macOS still use CLI drivers without the Windows graphical panel, persistent daemon or external plugin host. Pen-button hardware evidence and the broader parity gates remain open.
