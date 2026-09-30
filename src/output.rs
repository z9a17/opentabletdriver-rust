//! Sends the core's mouse packets with `SendInput`.

use std::mem::size_of;
use windows_sys::Win32::Foundation::SetLastError;

use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP,
    MOUSEEVENTF_MOVE, MOUSEEVENTF_VIRTUALDESK, MOUSEINPUT, SendInput,
};

use otd_core::output::owners::{OutputOwners, Owner};
use otd_core::output::{MousePacket, flags};
use std::{io, sync::Mutex};

static OWNERS: Mutex<OutputOwners> = Mutex::new(OutputOwners::new());

pub struct SessionOutput {
    owner: Option<Owner>,
}

impl SessionOutput {
    pub fn new() -> io::Result<Self> {
        let mut owners = OWNERS
            .lock()
            .map_err(|_| io::Error::other("output ownership lock poisoned"))?;
        Ok(Self {
            owner: Some(owners.register()),
        })
    }
    pub fn send(&self, packet: MousePacket) -> io::Result<()> {
        let owner = self
            .owner
            .ok_or_else(|| io::Error::other("output session ended"))?;
        let mut owners = OWNERS
            .lock()
            .map_err(|_| io::Error::other("output ownership lock poisoned"))?;
        owners.recover(send_input)?;
        owners.send(owner, packet, send_input)
    }
    pub fn finish(&mut self) -> io::Result<()> {
        if let Some(owner) = self.owner {
            OWNERS
                .lock()
                .map_err(|_| io::Error::other("output ownership lock poisoned"))?
                .release(owner, send_input)?;
            self.owner = None;
        }
        Ok(())
    }
}
impl Drop for SessionOutput {
    fn drop(&mut self) {
        if let Some(owner) = self.owner
            && let Ok(mut owners) = OWNERS.lock()
        {
            owners.abandon(owner);
        }
    }
}

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
    unsafe { SetLastError(0) };
    if unsafe { SendInput(1, &input, size_of::<INPUT>() as i32) } != 1 {
        let error = std::io::Error::last_os_error();
        return Err(if error.raw_os_error().is_some_and(|code| code != 0) {
            error
        } else {
            std::io::Error::other("SendInput accepted no mouse event; input may be blocked")
        });
    }
    Ok(())
}
