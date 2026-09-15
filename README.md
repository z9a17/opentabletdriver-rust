# PTH-660 Rust tablet driver

A standalone Windows 11 user-mode driver for the Wacom PTH-660 over USB. It reads the 192-byte pen HID collection and sends absolute cursor movement and tip clicks to Windows. It does not load plugins or require the OpenTabletDriver GUI. Pen side buttons are outside the first-release output scope.

This repository is licensed under LGPL-3.0-only. Its PTH-660 identification and USB report layout were researched from [OpenTabletDriver 0.6.x](https://github.com/OpenTabletDriver/OpenTabletDriver/tree/fdeaa7b0c6d6f5260f511f19fb693ed33524af4e), which is also LGPL-3.0 licensed. The [implementation plan](docs/IMPLEMENTATION_PLAN.md) records the source files and acceptance criteria.

## Build and use

Use the stable Rust MSVC toolchain on Windows 11.

    cargo build --release
    cargo test

The executable is at target/release/pth660-driver.exe. Start with diagnostics:

    pth660-driver list
    pth660-driver displays
    pth660-driver capture --seconds 10

The capture command reads a limited, short report trace **without moving the cursor**. Move, hover, tap, and release the pen during capture to check its report behavior. Paths and serial identifiers are omitted by default. The optional list --paths command shows HID device paths if a multi-device profile needs one.

Before running the cursor driver, close the original OpenTabletDriver daemon and other active tablet daemons so they do not inject duplicate movement. Then run:

    pth660-driver run
    pth660-driver run --config driver.toml

Ctrl+C stops it and releases mouse buttons on the normal shutdown path. The program waits for a disconnected PTH-660 and reconnects when it returns. It requires no driver replacement or administrator elevation. To uninstall, stop the process and remove the executable and optional profile.

By default, the full tablet area maps to the full virtual desktop and tip contact maps to left click. The example [profile](driver.example.toml) shows monitor selection, crop, rotation, and device-path selection. Profile options are parsed once on launch.

Windows mouse injection does not carry pen pressure, tilt, or eraser data to drawing applications. Those values are decoded from the tablet, but Windows Ink output is outside this first driver milestone.

The real PTH-660 on the development machine reports a readable 192-byte pen collection and a 44-byte auxiliary collection. Hover, contact, lift, and proximity-loss reports have been captured. A live Windows 11 run moved the cursor and clicked with the pen, as confirmed by the user. The driver detected an unplug and reopened the HID collection after replug; pen movement after that replug, sleep/wake, and display changes remain open hardware checks. Follow the [hardware validation guide](docs/HARDWARE_VALIDATION.md).
