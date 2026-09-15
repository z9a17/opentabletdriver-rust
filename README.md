# PTH-660 Rust tablet driver

Planning repository for a performance-focused, user-mode Rust tablet driver. The first target is the Wacom PTH-660 over USB on Windows 11.

No driver code exists yet. Start with the [implementation plan](docs/IMPLEMENTATION_PLAN.md).

The first deliverable is a standalone daemon that moves the cursor and handles pen contact and side buttons. It has no plugins, OpenTabletDriver GUI compatibility, Bluetooth support, or Windows Ink output.
