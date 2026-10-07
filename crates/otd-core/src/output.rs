//! Turns mapped positions and contact state into mouse packets: at most one
//! per report, carrying the move and any left-button change together.
//! Unchanged absolute positions and zero relative motion send nothing;
//! button state is committed only after the sink accepts a packet.

use crate::mapping::Mapper;
use crate::state::Frame;
pub mod buttons;
pub mod owners;
pub mod pen;

/// `MOUSEINPUT` flag values. They equal Windows' `MOUSEEVENTF_*` constants,
/// so the Windows adapter passes them through; other platforms translate.
pub mod flags {
    pub const MOVE: u32 = 0x0001;
    pub const LEFTDOWN: u32 = 0x0002;
    pub const LEFTUP: u32 = 0x0004;
    pub const VIRTUALDESK: u32 = 0x4000;
    pub const ABSOLUTE: u32 = 0x8000;
}

use flags::{
    ABSOLUTE as MOUSEEVENTF_ABSOLUTE, LEFTDOWN as MOUSEEVENTF_LEFTDOWN,
    LEFTUP as MOUSEEVENTF_LEFTUP, MOVE as MOUSEEVENTF_MOVE, VIRTUALDESK as MOUSEEVENTF_VIRTUALDESK,
};

#[derive(Default)]
pub struct MouseOutput {
    emitted_contact: bool,
    last_position: Option<(i32, i32)>,
    shared_position: bool,
}

#[derive(Clone, Copy)]
enum Motion {
    Absolute(Option<(i32, i32)>),
    Relative(i32, i32),
}

/// Optional pointer properties mirror the original composable handlers.
/// None retains a previously assigned value; pressure is normalized independently
/// of contact. Reset invokes the synchronous pointer's lifecycle cleanup.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MouseAttributes {
    pub pressure: Option<f32>,
    pub tilt: Option<[f32; 2]>,
    pub eraser: Option<bool>,
    pub reset: bool,
}

/// One `MOUSEINPUT`: a move and a button change travel together.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MousePacket {
    pub dx: i32,
    pub dy: i32,
    pub flags: u32,
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
        Self::default()
    }

    /// A shared sink deduplicates absolute positions across all device owners.
    pub fn share_position(&mut self) {
        self.shared_position = true;
    }

    pub fn emit_filtered(
        &mut self,
        frame: Frame,
        mapper: Mapper,
        filtered_position: Option<(f32, f32)>,
        send: impl FnOnce(MousePacket) -> Result<(), std::io::Error>,
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
        self.emit_mapped(position, frame.contact, send)
    }

    pub fn emit_mapped(
        &mut self,
        position: Option<(i32, i32)>,
        contact: bool,
        send: impl FnOnce(MousePacket) -> Result<(), std::io::Error>,
    ) -> Result<bool, std::io::Error> {
        let result = self.emit_motion_with(Motion::Absolute(position), contact, send);
        if position.is_none() {
            self.last_position = None;
        }
        result
    }

    pub fn release_all(
        &mut self,
        send: impl FnOnce(MousePacket) -> Result<(), std::io::Error>,
    ) -> Result<bool, std::io::Error> {
        let result = self.emit_motion_with(Motion::Absolute(None), false, send);
        self.last_position = None;
        result
    }

    /// An ordinary non-positional report does not invalidate the last known
    /// pointer position. Explicit range loss uses release_all/emit_mapped(None).
    pub fn emit_contact(
        &mut self,
        contact: bool,
        send: impl FnOnce(MousePacket) -> Result<(), std::io::Error>,
    ) -> Result<bool, std::io::Error> {
        self.emit_motion_with(Motion::Absolute(self.last_position), contact, send)
    }

    pub fn emit_relative(
        &mut self,
        delta: (i32, i32),
        contact: bool,
        send: impl FnOnce(MousePacket) -> Result<(), std::io::Error>,
    ) -> Result<bool, std::io::Error> {
        self.emit_motion_with(Motion::Relative(delta.0, delta.1), contact, send)
    }

    fn emit_motion_with(
        &mut self,
        motion: Motion,
        desired_contact: bool,
        send: impl FnOnce(MousePacket) -> Result<(), std::io::Error>,
    ) -> Result<bool, std::io::Error> {
        let mut flags = 0;
        let (dx, dy) = match motion {
            Motion::Absolute(Some(pos))
                if self.shared_position || Some(pos) != self.last_position =>
            {
                flags |= MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK;
                pos
            }
            Motion::Relative(dx, dy) => {
                if dx != 0 || dy != 0 {
                    flags |= MOUSEEVENTF_MOVE;
                }
                self.last_position = None;
                (dx, dy)
            }
            Motion::Absolute(None) => {
                self.last_position = None;
                (0, 0)
            }
            Motion::Absolute(Some(_)) => (0, 0),
        };
        flags |= contact_flag(desired_contact, self.emitted_contact);
        if flags == 0 {
            return Ok(false);
        }
        send(MousePacket { dx, dy, flags })?;
        self.emitted_contact = desired_contact;
        if let Motion::Absolute(Some(pos)) = motion {
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

    #[test]
    fn relative_motion_is_not_absolute_and_repeated_deltas_are_not_deduplicated() {
        let mut output = MouseOutput::new();
        for _ in 0..3 {
            assert!(
                output
                    .emit_motion_with(Motion::Relative(5, -2), false, |packet| {
                        assert_eq!(
                            packet,
                            MousePacket {
                                dx: 5,
                                dy: -2,
                                flags: MOUSEEVENTF_MOVE
                            }
                        );
                        Ok(())
                    })
                    .unwrap()
            );
        }
    }

    #[test]
    fn stationary_relative_motion_suppresses_calls_but_keeps_contact_transitions() {
        let mut output = MouseOutput::new();
        assert!(
            !output
                .emit_motion_with(Motion::Relative(0, 0), false, |_| panic!(
                    "no event expected"
                ))
                .unwrap()
        );
        output
            .emit_motion_with(Motion::Relative(0, 0), true, |packet| {
                assert_eq!(packet.flags, MOUSEEVENTF_LEFTDOWN);
                Ok(())
            })
            .unwrap();
        assert!(
            !output
                .emit_motion_with(Motion::Relative(0, 0), true, |_| panic!(
                    "held contact must not repeat"
                ))
                .unwrap()
        );
        output
            .emit_motion_with(Motion::Absolute(None), false, |packet| {
                assert_eq!(packet.flags, MOUSEEVENTF_LEFTUP);
                Ok(())
            })
            .unwrap();
        assert!(
            !output
                .emit_motion_with(Motion::Absolute(None), false, |_| panic!(
                    "already released"
                ))
                .unwrap()
        );
    }

    #[test]
    fn failed_relative_input_retries_contact_without_replaying_old_motion() {
        let mut output = MouseOutput::new();
        assert!(
            output
                .emit_motion_with(Motion::Relative(5, 0), true, |_| Err(
                    std::io::Error::other("injected failure")
                ))
                .is_err()
        );
        output
            .emit_motion_with(Motion::Relative(2, 0), true, |packet| {
                assert_eq!(
                    packet,
                    MousePacket {
                        dx: 2,
                        dy: 0,
                        flags: MOUSEEVENTF_MOVE | MOUSEEVENTF_LEFTDOWN
                    }
                );
                Ok(())
            })
            .unwrap();
        assert!(
            output
                .emit_motion_with(Motion::Absolute(None), false, |_| Err(
                    std::io::Error::other("failed release")
                ))
                .is_err()
        );
        output
            .emit_motion_with(Motion::Absolute(None), false, |packet| {
                assert_eq!(packet.flags, MOUSEEVENTF_LEFTUP);
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn absolute_position_is_deduplicated_only_after_success() {
        let mut output = MouseOutput::new();
        let motion = Motion::Absolute(Some((12_000, 8_000)));
        assert!(
            output
                .emit_motion_with(motion, true, |_| Err(std::io::Error::other("failed input")))
                .is_err()
        );
        output
            .emit_motion_with(motion, true, |packet| {
                assert_eq!(
                    packet.flags,
                    MOUSEEVENTF_MOVE
                        | MOUSEEVENTF_ABSOLUTE
                        | MOUSEEVENTF_VIRTUALDESK
                        | MOUSEEVENTF_LEFTDOWN
                );
                Ok(())
            })
            .unwrap();
        assert!(
            !output
                .emit_motion_with(motion, true, |_| panic!("duplicate absolute event"))
                .unwrap()
        );
    }
}
