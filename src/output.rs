//! Sends the core's mouse packets with `SendInput`.

use std::mem::size_of;

use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP,
    MOUSEEVENTF_MOVE, MOUSEEVENTF_VIRTUALDESK, MOUSEINPUT, SendInput,
};

use otd_core::output::{MousePacket, flags};

// The core uses Windows' own flag values, so packets pass through unchanged.
const _: () = assert!(
    flags::MOVE == MOUSEEVENTF_MOVE
        && flags::LEFTDOWN == MOUSEEVENTF_LEFTDOWN
        && flags::LEFTUP == MOUSEEVENTF_LEFTUP
        && flags::VIRTUALDESK == MOUSEEVENTF_VIRTUALDESK
        && flags::ABSOLUTE == MOUSEEVENTF_ABSOLUTE
);

pub fn send_input(packet: MousePacket) -> Result<(), std::io::Error> {
    let input = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: packet.dx,
                dy: packet.dy,
                mouseData: 0,
                dwFlags: packet.flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    if unsafe { SendInput(1, &input, size_of::<INPUT>() as i32) } != 1 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}
