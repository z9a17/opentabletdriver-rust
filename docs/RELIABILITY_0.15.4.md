# Update and packaging reliability, 0.15.4

This release fixes Windows panel update lifecycle problems, clipboard preparation failures and Unix package permissions. It does not change tablet decoding, mapping, bindings or smoothing.

## Windows updates

Startup and manual update checks share one background request. A manual check while the startup request is running receives the result as a manual check, including errors. Further checks and installations are blocked while an update prompt or installation is active.

A resolved update offer clears the deferred startup offer. Bringing the panel forward later cannot show that stale offer again. Deferred offers run through a posted window message so they do not open a dialog directly inside activation handling. A release already installed during this panel session offers restart instead of another download.

Closing the panel during installation waits for download, replacement or rollback to finish, then follows normal driver cleanup and unsaved-edit handling. Closing during an update prompt queues the close and cancels any installation or restart choice from that prompt. Installation completion behind another window or in the tray defers the restart prompt until the panel is brought forward.

The update transaction's existing installation lock and crash recovery remain in use. Forced process termination, Windows session termination and updater calls from a separate CLI are outside these panel lifecycle guards.

## Clipboard and Unix packages

Console and diagnostic copy operations prepare their UTF-16 transfer before opening and emptying the clipboard. Allocation, locking and clipboard-open failures leave the existing clipboard intact. A failure after Windows accepts the replacement can still leave the clipboard empty.

The release packager writes Unix permissions explicitly into tar archives, including when packaging on Windows. Directories, drivers and Linux setup executables use 755; other files use 644. The archive verifier requires executable permission for all users on the drivers and both Linux setup scripts. A separate archive permission repair is no longer needed.

## Evidence and remaining work

Source review and actual release compilation/package inspection are separate from behavioral validation. The owner-disabled pre-release format, Clippy, test and build-check suite and CI were not run. Three updater state regression tests were added; they do not establish live dialog or driver behavior.

Manual validation remains open for repeated manual/startup checks, declining an update, deferring restart, close during download/replacement/rollback, unsaved edits during close and clipboard failures. Linux and macOS need native installation/runtime validation. No new hardware compatibility or physical latency result is claimed.
