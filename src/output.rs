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

#[derive(Clone, Copy)]
enum Motion {
    Absolute(Option<(i32, i32)>),
    Relative(i32, i32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct MousePacket {
    dx: i32,
    dy: i32,
    flags: u32,
}

fn send_input(packet: MousePacket) -> Result<(), std::io::Error> {
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

    pub fn emit_relative(
        &mut self,
        delta: (i32, i32),
        contact: bool,
    ) -> Result<bool, std::io::Error> {
        self.emit_motion_with(Motion::Relative(delta.0, delta.1), contact, send_input)
    }

    fn emit_normalized(
        &mut self,
        position: Option<(i32, i32)>,
        desired_contact: bool,
    ) -> Result<bool, std::io::Error> {
        self.emit_motion_with(Motion::Absolute(position), desired_contact, send_input)
    }

    fn emit_motion_with(
        &mut self,
        motion: Motion,
        desired_contact: bool,
        send: impl FnOnce(MousePacket) -> Result<(), std::io::Error>,
    ) -> Result<bool, std::io::Error> {
        let mut flags = 0;
        let (dx, dy) = match motion {
            Motion::Absolute(Some(pos)) if Some(pos) != self.last_position => {
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
    use crate::config::ContactPolicy;
    use crate::protocol;
    use crate::relative::{RelativeMapper, RelativeSettings};
    use crate::state;
    use std::time::{Duration, Instant};

    // Independent real USB report prefixes already used by protocol tests.
    const CAPTURE: [[u8; 17]; 3] = [
        [
            0x10, 0x60, 0x14, 0x56, 0x00, 0xa3, 0x16, 0x00, 0x00, 0x00, 0x07, 0x04, 0, 0, 0, 0,
            0x28,
        ],
        [
            0x10, 0x61, 0x11, 0x55, 0x00, 0x8c, 0x12, 0x00, 0x0e, 0x11, 0x00, 0x07, 0, 0, 0, 0,
            0x19,
        ],
        [
            0x10, 0x40, 0xb8, 0x51, 0x00, 0x7e, 0x12, 0x00, 0x00, 0x00, 0x00, 0x04, 0, 0, 0, 0,
            0x3f,
        ],
    ];

    fn relative_mapper() -> RelativeMapper {
        RelativeMapper::new(RelativeSettings {
            sensitivity: (10.0, 10.0),
            rotation: 0.0,
            reset_delay: Duration::from_millis(100),
        })
        .unwrap()
    }

    #[test]
    fn recorded_hover_contact_high_hover_loss_and_reentry_replay_without_injection() {
        let mut mapper = relative_mapper();
        let mut output = MouseOutput::new();
        let policy = ContactPolicy {
            tip_threshold_raw: Some(82),
            ..ContactPolicy::default()
        };
        // Synthetic: neither In Range nor Sense, so the pen is not detected.
        let mut undetected = CAPTURE[2];
        undetected[1] = 0;
        let now = Instant::now();
        let mut packets = Vec::new();
        for data in [
            &CAPTURE[0],
            &CAPTURE[1],
            &CAPTURE[2],
            &undetected,
            &CAPTURE[0],
        ] {
            let frame = state::frame(protocol::parse(data).unwrap().unwrap(), policy);
            let delta = mapper.map_at(frame.position, None, now);
            output
                .emit_motion_with(
                    Motion::Relative(delta.0, delta.1),
                    frame.contact,
                    |packet| {
                        packets.push(packet);
                        Ok(())
                    },
                )
                .unwrap();
        }
        assert_eq!(
            packets,
            [
                // Recorded movement: (-259, -1047) raw units = (-12.95, -52.35) counts.
                MousePacket {
                    dx: -12,
                    dy: -52,
                    flags: MOUSEEVENTF_MOVE | MOUSEEVENTF_LEFTDOWN
                },
                // Lifted above In Range but still sensed: the cursor keeps
                // following, as in OpenTabletDriver. (-857, -14) raw units plus
                // the carried fractions = (-43.8, -1.05) counts.
                MousePacket {
                    dx: -43,
                    dy: -1,
                    flags: MOUSEEVENTF_MOVE | MOUSEEVENTF_LEFTUP
                },
                // Not detected, then detected again: a new origin, no jump.
            ]
        );
    }

    fn replay_pipeline(iterations: u32, filtered: bool) -> u64 {
        use crate::radial_follow::{RadialFollowSettings, RadialFollowSmoothingTabletSpace};
        use std::hint::black_box;

        let mut mapper = relative_mapper();
        let mut output = MouseOutput::new();
        let mut filter = RadialFollowSmoothingTabletSpace::new(RadialFollowSettings::default());
        let mut emitted = 0;
        crate::test_alloc::assert_no_allocations(|| {
            for index in 0..iterations {
                let bytes = black_box(&CAPTURE[index as usize % CAPTURE.len()]);
                let frame = state::frame(
                    protocol::parse(bytes).unwrap().unwrap(),
                    ContactPolicy::default(),
                );
                let position = if filtered {
                    frame.position.map(|(x, y)| filter.filter_raw(x, y))
                } else {
                    None
                };
                let delta = mapper.map_at(frame.position, position, Instant::now());
                output
                    .emit_motion_with(
                        Motion::Relative(delta.0, delta.1),
                        frame.contact,
                        |packet| {
                            black_box(packet);
                            emitted += 1;
                            Ok(())
                        },
                    )
                    .unwrap();
            }
        });
        emitted
    }

    #[test]
    fn relative_report_pipeline_allocates_nothing_with_or_without_filtering() {
        assert!(replay_pipeline(10_000, false) > 0);
        assert!(replay_pipeline(10_000, true) > 0);
    }

    #[test]
    #[ignore = "manual release-mode timing; does not call HID or SendInput"]
    fn benchmark_relative_pipeline() {
        const REPORTS: u32 = 1_000_000;
        for filtered in [false, true] {
            let start = Instant::now();
            let emitted = replay_pipeline(REPORTS, filtered);
            let elapsed = start.elapsed();
            println!(
                "relative replay: filtered={filtered}, reports={REPORTS}, packets={emitted}, {:.1} ns/report; simulated output, no HID or SendInput",
                elapsed.as_nanos() as f64 / f64::from(REPORTS)
            );
        }
    }

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
