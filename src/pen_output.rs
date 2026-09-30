//! Pen output on Windows: a synthetic pen pointer device, which Windows Ink
//! applications see with pressure, tilt, eraser and hover. Other applications
//! receive the pen's mouse emulation. Windows 10 1809 or later.
//! <https://learn.microsoft.com/windows/win32/api/winuser/nf-winuser-injectsyntheticpointerinput>
//!
//! The device setup follows the Windows Pen Pointer plugin, which drives the
//! same API from OpenTabletDriver: pointer ID 1, indirect feedback, one
//! `POINTER_FLAG_NEW` injection after creation, the primary flag afterwards
//! and the foreground window as the target.
//! <https://github.com/Kuuuube/VoiDPlugins/blob/02c3ed3a54937e39157f984c42400a755b82eb9e/src/VoiDPlugins.Library/PenPointer/PointerDevice.cs>
//! Its flags change only on contact transitions (`DOWN`/`UP`), as the pointer
//! documentation describes, where the plugin keeps them set.

use std::io;

use otd_core::output::pen::{PenPacket, PenPhase, PenSink};
use windows_sys::Win32::Foundation::POINT;
use windows_sys::Win32::UI::Controls::{
    CreateSyntheticPointerDevice, DestroySyntheticPointerDevice, HSYNTHETICPOINTERDEVICE,
    POINTER_FEEDBACK_INDIRECT, POINTER_TYPE_INFO, POINTER_TYPE_INFO_0,
};
use windows_sys::Win32::UI::Input::Pointer::{
    InjectSyntheticPointerInput, POINTER_BUTTON_CHANGE_TYPE, POINTER_CHANGE_FIRSTBUTTON_DOWN,
    POINTER_CHANGE_FIRSTBUTTON_UP, POINTER_CHANGE_NONE, POINTER_FLAG_DOWN,
    POINTER_FLAG_FIRSTBUTTON, POINTER_FLAG_INCONTACT, POINTER_FLAG_INRANGE, POINTER_FLAG_NEW,
    POINTER_FLAG_PRIMARY, POINTER_FLAG_UP, POINTER_FLAG_UPDATE, POINTER_FLAGS, POINTER_INFO,
    POINTER_PEN_INFO,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, PEN_FLAG_BARREL, PEN_FLAG_ERASER, PEN_FLAG_INVERTED, PEN_FLAG_NONE,
    PEN_MASK_PRESSURE, PEN_MASK_TILT_X, PEN_MASK_TILT_Y, PT_PEN,
};

/// Windows pen pressure runs 0..=1024.
const MAX_PRESSURE: f32 = 1024.0;
const POINTER_ID: u32 = 1;

pub struct SyntheticPen {
    device: HSYNTHETICPOINTERDEVICE,
}

impl SyntheticPen {
    pub fn new() -> io::Result<Self> {
        let device = unsafe { CreateSyntheticPointerDevice(PT_PEN, 1, POINTER_FEEDBACK_INDIRECT) };
        if device.is_null() {
            let error = io::Error::last_os_error();
            return Err(io::Error::new(
                error.kind(),
                format!("cannot create a synthetic pen (Windows 10 1809 or later): {error}"),
            ));
        }
        let pen = Self { device };
        // Announces the new pointer to Windows Ink before any movement.
        let mut info = pen_info(POINTER_FLAG_NEW, POINTER_CHANGE_NONE, PEN_FLAG_NONE, None);
        pen.inject(&mut info)?;
        Ok(pen)
    }

    fn inject(&self, info: &mut POINTER_TYPE_INFO) -> io::Result<()> {
        unsafe {
            info.Anonymous.penInfo.pointerInfo.hwndTarget = GetForegroundWindow();
            if InjectSyntheticPointerInput(self.device, info, 1) == 0 {
                return Err(io::Error::last_os_error());
            }
        }
        Ok(())
    }
}

impl Drop for SyntheticPen {
    fn drop(&mut self) {
        unsafe { DestroySyntheticPointerDevice(self.device) };
    }
}

impl PenSink for SyntheticPen {
    fn send(&mut self, packet: PenPacket) -> io::Result<()> {
        self.inject(&mut pointer_info(packet))
    }
}

/// The injected pointer for one packet.
pub fn pointer_info(packet: PenPacket) -> POINTER_TYPE_INFO {
    let (flags, change) = match packet.phase {
        PenPhase::Hover => (
            POINTER_FLAG_INRANGE | POINTER_FLAG_UPDATE,
            POINTER_CHANGE_NONE,
        ),
        PenPhase::Down => (
            POINTER_FLAG_INRANGE
                | POINTER_FLAG_INCONTACT
                | POINTER_FLAG_FIRSTBUTTON
                | POINTER_FLAG_DOWN,
            POINTER_CHANGE_FIRSTBUTTON_DOWN,
        ),
        PenPhase::Contact => (
            POINTER_FLAG_INRANGE
                | POINTER_FLAG_INCONTACT
                | POINTER_FLAG_FIRSTBUTTON
                | POINTER_FLAG_UPDATE,
            POINTER_CHANGE_NONE,
        ),
        PenPhase::Up => (
            POINTER_FLAG_INRANGE | POINTER_FLAG_UP,
            POINTER_CHANGE_FIRSTBUTTON_UP,
        ),
        PenPhase::Leave => (POINTER_FLAG_UPDATE, POINTER_CHANGE_NONE),
    };
    let contact = matches!(packet.phase, PenPhase::Down | PenPhase::Contact);
    // The eraser end is "inverted"; touching with it also erases.
    let mut pen_flags = match (packet.eraser, contact) {
        (true, true) => PEN_FLAG_INVERTED | PEN_FLAG_ERASER,
        (true, false) => PEN_FLAG_INVERTED,
        (false, _) => PEN_FLAG_NONE,
    };
    // Windows pens have one barrel button. The Windows Pen Pointer plugin
    // sets its flag for each of OpenTabletDriver's three barrel buttons, and
    // so does this.
    if packet.barrel != 0 {
        pen_flags |= PEN_FLAG_BARREL;
    }
    let mut info = pen_info(
        flags | POINTER_FLAG_PRIMARY,
        change,
        pen_flags,
        Some(packet),
    );
    let pen = unsafe { &mut info.Anonymous.penInfo };
    if contact {
        // A touching pen always has some pressure.
        pen.pressure = ((packet.pressure * MAX_PRESSURE).round() as u32).clamp(1, 1024);
    }
    info
}

fn pen_info(
    flags: POINTER_FLAGS,
    change: POINTER_BUTTON_CHANGE_TYPE,
    pen_flags: u32,
    packet: Option<PenPacket>,
) -> POINTER_TYPE_INFO {
    let location = packet.map_or(POINT { x: 0, y: 0 }, |packet| POINT {
        x: packet.x.round() as i32,
        y: packet.y.round() as i32,
    });
    let tilt = packet.and_then(|packet| packet.tilt);
    POINTER_TYPE_INFO {
        r#type: PT_PEN,
        Anonymous: POINTER_TYPE_INFO_0 {
            penInfo: POINTER_PEN_INFO {
                pointerInfo: POINTER_INFO {
                    pointerType: PT_PEN,
                    pointerId: POINTER_ID,
                    pointerFlags: flags,
                    ptPixelLocation: location,
                    ptPixelLocationRaw: location,
                    ButtonChangeType: change,
                    ..Default::default()
                },
                penFlags: pen_flags,
                penMask: PEN_MASK_PRESSURE
                    | if tilt.is_some() {
                        PEN_MASK_TILT_X | PEN_MASK_TILT_Y
                    } else {
                        0
                    },
                pressure: 0,
                rotation: 0,
                tiltX: tilt.map_or(0, |tilt| tilt[0].round() as i32),
                tiltY: tilt.map_or(0, |tilt| tilt[1].round() as i32),
            },
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packet(phase: PenPhase, eraser: bool) -> PenPacket {
        PenPacket {
            phase,
            x: -1919.6,
            y: 1079.4,
            pressure: 0.5,
            tilt: Some([12.4, -90.0]),
            eraser,
            barrel: 0,
        }
    }

    fn read(info: POINTER_TYPE_INFO) -> POINTER_PEN_INFO {
        assert_eq!(info.r#type, PT_PEN);
        unsafe { info.Anonymous.penInfo }
    }

    #[test]
    fn phases_become_the_pointer_state_windows_expects() {
        use PenPhase::*;
        let base = POINTER_FLAG_PRIMARY;
        for (phase, flags, change) in [
            (
                Hover,
                base | POINTER_FLAG_INRANGE | POINTER_FLAG_UPDATE,
                POINTER_CHANGE_NONE,
            ),
            (
                Down,
                base | POINTER_FLAG_INRANGE
                    | POINTER_FLAG_INCONTACT
                    | POINTER_FLAG_FIRSTBUTTON
                    | POINTER_FLAG_DOWN,
                POINTER_CHANGE_FIRSTBUTTON_DOWN,
            ),
            (
                Contact,
                base | POINTER_FLAG_INRANGE
                    | POINTER_FLAG_INCONTACT
                    | POINTER_FLAG_FIRSTBUTTON
                    | POINTER_FLAG_UPDATE,
                POINTER_CHANGE_NONE,
            ),
            (
                Up,
                base | POINTER_FLAG_INRANGE | POINTER_FLAG_UP,
                POINTER_CHANGE_FIRSTBUTTON_UP,
            ),
            (Leave, base | POINTER_FLAG_UPDATE, POINTER_CHANGE_NONE),
        ] {
            let pen = read(pointer_info(packet(phase, false)));
            let pointer = pen.pointerInfo;
            assert_eq!(pointer.pointerType, PT_PEN);
            assert_eq!(pointer.pointerId, POINTER_ID);
            assert_eq!(pointer.pointerFlags, flags, "{phase:?}");
            assert_eq!(pointer.ButtonChangeType, change, "{phase:?}");
            assert_eq!(
                (pointer.ptPixelLocation.x, pointer.ptPixelLocation.y),
                (-1920, 1079)
            );
            assert_eq!(
                pen.penMask,
                PEN_MASK_PRESSURE | PEN_MASK_TILT_X | PEN_MASK_TILT_Y
            );
            assert_eq!((pen.tiltX, pen.tiltY), (12, -90));
            let touching = matches!(phase, Down | Contact);
            assert_eq!(pen.pressure, if touching { 512 } else { 0 }, "{phase:?}");
        }
    }

    #[test]
    fn the_eraser_is_inverted_and_erases_only_in_contact() {
        let flags = |phase| read(pointer_info(packet(phase, true))).penFlags;
        assert_eq!(flags(PenPhase::Hover), PEN_FLAG_INVERTED);
        assert_eq!(flags(PenPhase::Down), PEN_FLAG_INVERTED | PEN_FLAG_ERASER);
        assert_eq!(flags(PenPhase::Up), PEN_FLAG_INVERTED);
        assert_eq!(
            read(pointer_info(packet(PenPhase::Contact, false))).penFlags,
            PEN_FLAG_NONE
        );
    }

    #[test]
    fn any_barrel_button_sets_the_pens_barrel_flag() {
        let flags = |phase, barrel, eraser| {
            read(pointer_info(PenPacket {
                barrel,
                ..packet(phase, eraser)
            }))
            .penFlags
        };
        assert_eq!(flags(PenPhase::Hover, 0b001, false), PEN_FLAG_BARREL);
        assert_eq!(flags(PenPhase::Contact, 0b100, false), PEN_FLAG_BARREL);
        assert_eq!(flags(PenPhase::Hover, 0b110, false), PEN_FLAG_BARREL);
        assert_eq!(flags(PenPhase::Hover, 0, false), PEN_FLAG_NONE);
        assert_eq!(
            flags(PenPhase::Down, 0b001, true),
            PEN_FLAG_BARREL | PEN_FLAG_INVERTED | PEN_FLAG_ERASER
        );
    }

    #[test]
    fn contact_pressure_is_never_zero_and_tilt_is_optional() {
        let pen = read(pointer_info(PenPacket {
            pressure: 0.0,
            tilt: None,
            ..packet(PenPhase::Down, false)
        }));
        assert_eq!(pen.pressure, 1);
        assert_eq!(pen.penMask, PEN_MASK_PRESSURE);
        assert_eq!((pen.tiltX, pen.tiltY), (0, 0));
    }
}
