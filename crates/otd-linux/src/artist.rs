//! Artist Mode: the pen lifecycle as evdev frames for a virtual pressure
//! sensitive tablet, matching upstream's `EvdevVirtualTablet`
//! (<https://github.com/OpenTabletDriver/OpenTabletDriver/blob/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.Desktop/Interop/Input/Absolute/EvdevVirtualTablet.cs>):
//! positions in thousandths of a pixel, pressure 0..=65535 with `BTN_TOUCH`
//! while touching, tilt -64..=63, and the tool key held while in range.
//! Portable so its event order is unit-tested on every platform.

use otd_core::mapping::Rect;
use otd_core::output::pen::{PenPacket, PenPhase};

pub const EV_SYN: u16 = 0x00;
pub const EV_KEY: u16 = 0x01;
pub const EV_ABS: u16 = 0x03;
pub const SYN_REPORT: u16 = 0x00;
pub const ABS_X: u16 = 0x00;
pub const ABS_Y: u16 = 0x01;
pub const ABS_PRESSURE: u16 = 0x18;
pub const ABS_TILT_X: u16 = 0x1a;
pub const ABS_TILT_Y: u16 = 0x1b;
pub const BTN_TOOL_PEN: u16 = 0x140;
pub const BTN_TOOL_RUBBER: u16 = 0x141;
pub const BTN_TOUCH: u16 = 0x14a;
pub const BTN_STYLUS: u16 = 0x14b;
pub const BTN_STYLUS2: u16 = 0x14c;
pub const BTN_STYLUS3: u16 = 0x149;
pub const INPUT_PROP_POINTER: u16 = 0x00;
pub const INPUT_PROP_DIRECT: u16 = 0x01;

/// Keys the virtual tablet declares, as upstream does. Barrel buttons 1, 2
/// and 3 are `BTN_STYLUS`, `BTN_STYLUS2` and `BTN_STYLUS3`.
pub const KEYS: [u16; 6] = [
    BTN_TOUCH,
    BTN_STYLUS,
    BTN_STYLUS2,
    BTN_STYLUS3,
    BTN_TOOL_PEN,
    BTN_TOOL_RUBBER,
];
/// The stylus key of each barrel button, button 1 first.
pub const BARREL_KEYS: [u16; 3] = [BTN_STYLUS, BTN_STYLUS2, BTN_STYLUS3];
/// Subpixels per screen pixel on the position axes.
pub const RESOLUTION: i32 = 1000;
pub const MAX_PRESSURE: i32 = u16::MAX as i32;
pub const TILT_RANGE: (i32, i32) = (-64, 63);
/// Tilt units per radian: about one per degree.
pub const TILT_RESOLUTION: i32 = 57;

/// One axis: code, minimum, maximum and resolution.
pub type Axis = (u16, i32, i32, i32);

/// The absolute axes for a virtual screen.
pub fn axes(screen: Rect) -> [Axis; 5] {
    [
        (ABS_X, 0, screen.width().saturating_mul(RESOLUTION), 100_000),
        (
            ABS_Y,
            0,
            screen.height().saturating_mul(RESOLUTION),
            100_000,
        ),
        (ABS_PRESSURE, 0, MAX_PRESSURE, 0),
        (ABS_TILT_X, TILT_RANGE.0, TILT_RANGE.1, TILT_RESOLUTION),
        (ABS_TILT_Y, TILT_RANGE.0, TILT_RANGE.1, TILT_RESOLUTION),
    ]
}

/// The most events one packet needs: tool, X, Y, touch, pressure, two tilt
/// axes, three barrel buttons and the sync.
pub const FRAME_EVENTS: usize = 11;

/// A fixed-size evdev frame; one packet never needs more events.
pub struct Frame {
    pub events: [(u16, u16, i32); FRAME_EVENTS],
    pub count: usize,
}

impl Frame {
    pub fn as_slice(&self) -> &[(u16, u16, i32)] {
        &self.events[..self.count]
    }

    fn push(&mut self, kind: u16, code: u16, value: i32) {
        self.events[self.count] = (kind, code, value);
        self.count += 1;
    }
}

/// The frame for one pen packet, ending with `SYN_REPORT`.
pub fn frame(packet: PenPacket, screen: Rect) -> Frame {
    let mut frame = Frame {
        events: [(0, 0, 0); FRAME_EVENTS],
        count: 0,
    };
    let tool = if packet.eraser {
        BTN_TOOL_RUBBER
    } else {
        BTN_TOOL_PEN
    };
    let touching = matches!(packet.phase, PenPhase::Down | PenPhase::Contact);
    if packet.phase == PenPhase::Leave {
        // Upstream's Reset: release the keys and pressure; keep position and
        // tilt.
        frame.push(EV_KEY, BTN_TOUCH, 0);
        frame.push(EV_ABS, ABS_PRESSURE, 0);
        frame.push(EV_KEY, tool, 0);
        for key in BARREL_KEYS {
            frame.push(EV_KEY, key, 0);
        }
    } else {
        let scaled = |value: f64, origin: i32, span: i32| {
            ((value - f64::from(origin)) * f64::from(RESOLUTION))
                .round()
                .clamp(0.0, f64::from(span.saturating_mul(RESOLUTION))) as i32
        };
        frame.push(EV_KEY, tool, 1);
        frame.push(EV_ABS, ABS_X, scaled(packet.x, screen.left, screen.width()));
        frame.push(EV_ABS, ABS_Y, scaled(packet.y, screen.top, screen.height()));
        frame.push(EV_KEY, BTN_TOUCH, i32::from(touching));
        let pressure = if touching {
            // A touching pen always has some pressure, as BTN_TOUCH does.
            ((packet.pressure * MAX_PRESSURE as f32).round() as i32).clamp(1, MAX_PRESSURE)
        } else {
            0
        };
        frame.push(EV_ABS, ABS_PRESSURE, pressure);
        if let Some([x, y]) = packet.tilt {
            let tilt = |value: f32| (value.round() as i32).clamp(TILT_RANGE.0, TILT_RANGE.1);
            frame.push(EV_ABS, ABS_TILT_X, tilt(x));
            frame.push(EV_ABS, ABS_TILT_Y, tilt(y));
        }
        // The kernel drops a key event that repeats a key's state.
        for (index, key) in BARREL_KEYS.into_iter().enumerate() {
            frame.push(EV_KEY, key, i32::from(packet.barrel & (1 << index) != 0));
        }
    }
    frame.push(EV_SYN, SYN_REPORT, 0);
    frame
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN: Rect = Rect {
        left: 0,
        top: 0,
        right: 1920,
        bottom: 1080,
    };

    fn packet(phase: PenPhase, eraser: bool) -> PenPacket {
        PenPacket {
            phase,
            x: 100.25,
            y: 2000.0,
            pressure: 0.5,
            tilt: Some([12.4, -80.0]),
            eraser,
            barrel: 0,
        }
    }

    #[test]
    fn hover_touch_and_leave_become_evdev_frames() {
        assert_eq!(
            frame(packet(PenPhase::Hover, false), SCREEN).as_slice(),
            [
                (EV_KEY, BTN_TOOL_PEN, 1),
                (EV_ABS, ABS_X, 100_250),
                // Clamped to the axis maximum.
                (EV_ABS, ABS_Y, 1_080_000),
                (EV_KEY, BTN_TOUCH, 0),
                (EV_ABS, ABS_PRESSURE, 0),
                (EV_ABS, ABS_TILT_X, 12),
                (EV_ABS, ABS_TILT_Y, -64),
                (EV_KEY, BTN_STYLUS, 0),
                (EV_KEY, BTN_STYLUS2, 0),
                (EV_KEY, BTN_STYLUS3, 0),
                (EV_SYN, SYN_REPORT, 0),
            ]
        );
        let down = frame(packet(PenPhase::Down, false), SCREEN);
        assert_eq!(down.as_slice()[3], (EV_KEY, BTN_TOUCH, 1));
        assert_eq!(down.as_slice()[4], (EV_ABS, ABS_PRESSURE, 32768));
        let up = frame(packet(PenPhase::Up, false), SCREEN);
        assert_eq!(
            up.as_slice()[3..5],
            [(EV_KEY, BTN_TOUCH, 0), (EV_ABS, ABS_PRESSURE, 0)]
        );
        assert_eq!(
            frame(packet(PenPhase::Leave, true), SCREEN).as_slice(),
            [
                (EV_KEY, BTN_TOUCH, 0),
                (EV_ABS, ABS_PRESSURE, 0),
                (EV_KEY, BTN_TOOL_RUBBER, 0),
                (EV_KEY, BTN_STYLUS, 0),
                (EV_KEY, BTN_STYLUS2, 0),
                (EV_KEY, BTN_STYLUS3, 0),
                (EV_SYN, SYN_REPORT, 0),
            ]
        );
    }

    #[test]
    fn barrel_buttons_are_the_stylus_keys_and_a_leave_releases_them() {
        let held = frame(
            PenPacket {
                barrel: 0b101,
                ..packet(PenPhase::Hover, false)
            },
            SCREEN,
        );
        let keys: Vec<_> = held
            .as_slice()
            .iter()
            .filter(|(kind, code, _)| *kind == EV_KEY && BARREL_KEYS.contains(code))
            .collect();
        assert_eq!(
            keys,
            [
                &(EV_KEY, BTN_STYLUS, 1),
                &(EV_KEY, BTN_STYLUS2, 0),
                &(EV_KEY, BTN_STYLUS3, 1)
            ]
        );
        let leave = frame(
            PenPacket {
                barrel: 0b111,
                ..packet(PenPhase::Leave, false)
            },
            SCREEN,
        );
        assert!(
            leave
                .as_slice()
                .iter()
                .filter(|(_, code, _)| BARREL_KEYS.contains(code))
                .all(|(_, _, value)| *value == 0),
            "leaving range lets go of every barrel button"
        );
        assert!(held.count <= FRAME_EVENTS);
    }

    #[test]
    fn positions_are_relative_to_the_screen_and_tilt_is_optional() {
        let screen = Rect {
            left: -1920,
            top: -100,
            right: 1920,
            bottom: 1080,
        };
        let contact = frame(
            PenPacket {
                x: -1920.0,
                y: -200.0,
                pressure: 0.0,
                tilt: None,
                ..packet(PenPhase::Contact, true)
            },
            screen,
        );
        assert_eq!(
            contact.as_slice(),
            [
                (EV_KEY, BTN_TOOL_RUBBER, 1),
                (EV_ABS, ABS_X, 0),
                (EV_ABS, ABS_Y, 0),
                (EV_KEY, BTN_TOUCH, 1),
                (EV_ABS, ABS_PRESSURE, 1),
                (EV_KEY, BTN_STYLUS, 0),
                (EV_KEY, BTN_STYLUS2, 0),
                (EV_KEY, BTN_STYLUS3, 0),
                (EV_SYN, SYN_REPORT, 0),
            ]
        );
        assert_eq!(
            axes(screen)[0..2],
            [
                (ABS_X, 0, 3_840_000, 100_000),
                (ABS_Y, 0, 1_180_000, 100_000)
            ]
        );
    }
}
