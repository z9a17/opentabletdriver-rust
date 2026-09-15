use std::mem::size_of;

use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP,
    MOUSEEVENTF_MOVE, MOUSEEVENTF_VIRTUALDESK, MOUSEINPUT, SendInput,
};

use crate::mapping::Mapper;
use crate::state::Frame;

pub struct MouseOutput {
    emitted_contact: bool,
    last_position: Option<(i32, i32)>,
}

fn contact_flag(desired: bool, emitted: bool) -> u32 {
    if desired == emitted {
        0
    } else if desired {
        MOUSEEVENTF_LEFTDOWN
    } else {
        MOUSEEVENTF_LEFTUP
    }
}

impl MouseOutput {
    pub fn new() -> Self {
        Self {
            emitted_contact: false,
            last_position: None,
        }
    }

    pub fn emit(&mut self, frame: Frame, mapper: Mapper) -> Result<bool, std::io::Error> {
        self.emit_filtered(frame, mapper, None)
    }

    pub fn emit_filtered(
        &mut self,
        frame: Frame,
        mapper: Mapper,
        filtered_position: Option<(f32, f32)>,
    ) -> Result<bool, std::io::Error> {
        let position = if let Some((x, y)) = frame.position {
            let mapped = filtered_position
                .map_or_else(|| mapper.map(x, y), |(fx, fy)| mapper.map_filtered(fx, fy));
            let Some(position) = mapped else {
                // Area limiting ignores a report outside the configured area.
                return Ok(false);
            };
            Some(position)
        } else {
            None
        };
        let result = self.emit_normalized(position, frame.contact);
        if position.is_none() {
            self.last_position = None;
        }
        result
    }

    pub fn release_all(&mut self) -> Result<bool, std::io::Error> {
        let result = self.emit_normalized(None, false);
        self.last_position = None;
        result
    }

    fn emit_normalized(
        &mut self,
        position: Option<(i32, i32)>,
        desired_contact: bool,
    ) -> Result<bool, std::io::Error> {
        let mut flags = 0;
        let (dx, dy) = if let Some(pos) = position.filter(|p| Some(*p) != self.last_position) {
            flags |= MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK;
            pos
        } else {
            (0, 0)
        };
        flags |= contact_flag(desired_contact, self.emitted_contact);
        if flags == 0 {
            return Ok(false);
        }
        let input = INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx,
                    dy,
                    mouseData: 0,
                    dwFlags: flags,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };
        if unsafe { SendInput(1, &input, size_of::<INPUT>() as i32) } != 1 {
            return Err(std::io::Error::last_os_error());
        }
        self.emitted_contact = desired_contact;
        if let Some(pos) = position {
            self.last_position = Some(pos);
        }
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contact_emits_once_and_releases_after_proximity_loss() {
        assert_eq!(contact_flag(true, false), MOUSEEVENTF_LEFTDOWN);
        assert_eq!(contact_flag(true, true), 0);
        assert_eq!(contact_flag(false, true), MOUSEEVENTF_LEFTUP);
    }
}
