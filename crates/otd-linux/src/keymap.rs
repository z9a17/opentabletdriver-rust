//! USB keyboard usages to Linux evdev key codes, and mouse buttons to evdev
//! button codes. Portable, so the tables are unit-tested on every platform.
//!
//! The codes are `linux/input-event-codes.h`; each row names its constant.
//! Keypad keys are the keypad's own codes; upstream's Linux table maps a few of
//! them (Divide, Add, Multiply, Decimal) to main-block keys instead.

use otd_core::actions::{KeyboardUsage, MouseButton};

pub const BTN_LEFT: u16 = 0x110;
pub const BTN_RIGHT: u16 = 0x111;
pub const BTN_MIDDLE: u16 = 0x112;
pub const BTN_SIDE: u16 = 0x113;
pub const BTN_EXTRA: u16 = 0x114;

/// The evdev button for a mouse button, as upstream's `EvdevVirtualMouse`
/// chooses: Backward is `BTN_SIDE` and Forward is `BTN_EXTRA`.
pub fn mouse_code(button: MouseButton) -> u16 {
    match button {
        MouseButton::Left => BTN_LEFT,
        MouseButton::Right => BTN_RIGHT,
        MouseButton::Middle => BTN_MIDDLE,
        MouseButton::Backward => BTN_SIDE,
        MouseButton::Forward => BTN_EXTRA,
    }
}

pub const MOUSE_BUTTONS: [u16; 5] = [BTN_LEFT, BTN_RIGHT, BTN_MIDDLE, BTN_SIDE, BTN_EXTRA];

/// USB page 0x07 usage, evdev key code.
const KEYS: &[(u16, u16)] = &[
    (0x04, 30),  // KEY_A
    (0x05, 48),  // KEY_B
    (0x06, 46),  // KEY_C
    (0x07, 32),  // KEY_D
    (0x08, 18),  // KEY_E
    (0x09, 33),  // KEY_F
    (0x0a, 34),  // KEY_G
    (0x0b, 35),  // KEY_H
    (0x0c, 23),  // KEY_I
    (0x0d, 36),  // KEY_J
    (0x0e, 37),  // KEY_K
    (0x0f, 38),  // KEY_L
    (0x10, 50),  // KEY_M
    (0x11, 49),  // KEY_N
    (0x12, 24),  // KEY_O
    (0x13, 25),  // KEY_P
    (0x14, 16),  // KEY_Q
    (0x15, 19),  // KEY_R
    (0x16, 31),  // KEY_S
    (0x17, 20),  // KEY_T
    (0x18, 22),  // KEY_U
    (0x19, 47),  // KEY_V
    (0x1a, 17),  // KEY_W
    (0x1b, 45),  // KEY_X
    (0x1c, 21),  // KEY_Y
    (0x1d, 44),  // KEY_Z
    (0x1e, 2),   // KEY_1
    (0x1f, 3),   // KEY_2
    (0x20, 4),   // KEY_3
    (0x21, 5),   // KEY_4
    (0x22, 6),   // KEY_5
    (0x23, 7),   // KEY_6
    (0x24, 8),   // KEY_7
    (0x25, 9),   // KEY_8
    (0x26, 10),  // KEY_9
    (0x27, 11),  // KEY_0
    (0x28, 28),  // KEY_ENTER
    (0x29, 1),   // KEY_ESC
    (0x2a, 14),  // KEY_BACKSPACE
    (0x2b, 15),  // KEY_TAB
    (0x2c, 57),  // KEY_SPACE
    (0x2d, 12),  // KEY_MINUS
    (0x2e, 13),  // KEY_EQUAL
    (0x2f, 26),  // KEY_LEFTBRACE
    (0x30, 27),  // KEY_RIGHTBRACE
    (0x31, 43),  // KEY_BACKSLASH
    (0x33, 39),  // KEY_SEMICOLON
    (0x34, 40),  // KEY_APOSTROPHE
    (0x35, 41),  // KEY_GRAVE
    (0x36, 51),  // KEY_COMMA
    (0x37, 52),  // KEY_DOT
    (0x38, 53),  // KEY_SLASH
    (0x39, 58),  // KEY_CAPSLOCK
    (0x3a, 59),  // KEY_F1
    (0x3b, 60),  // KEY_F2
    (0x3c, 61),  // KEY_F3
    (0x3d, 62),  // KEY_F4
    (0x3e, 63),  // KEY_F5
    (0x3f, 64),  // KEY_F6
    (0x40, 65),  // KEY_F7
    (0x41, 66),  // KEY_F8
    (0x42, 67),  // KEY_F9
    (0x43, 68),  // KEY_F10
    (0x44, 87),  // KEY_F11
    (0x45, 88),  // KEY_F12
    (0x46, 99),  // KEY_SYSRQ
    (0x47, 70),  // KEY_SCROLLLOCK
    (0x48, 119), // KEY_PAUSE
    (0x49, 110), // KEY_INSERT
    (0x4a, 102), // KEY_HOME
    (0x4b, 104), // KEY_PAGEUP
    (0x4c, 111), // KEY_DELETE
    (0x4d, 107), // KEY_END
    (0x4e, 109), // KEY_PAGEDOWN
    (0x4f, 106), // KEY_RIGHT
    (0x50, 105), // KEY_LEFT
    (0x51, 108), // KEY_DOWN
    (0x52, 103), // KEY_UP
    (0x53, 69),  // KEY_NUMLOCK
    (0x54, 98),  // KEY_KPSLASH
    (0x55, 55),  // KEY_KPASTERISK
    (0x56, 74),  // KEY_KPMINUS
    (0x57, 78),  // KEY_KPPLUS
    (0x58, 96),  // KEY_KPENTER
    (0x59, 79),  // KEY_KP1
    (0x5a, 80),  // KEY_KP2
    (0x5b, 81),  // KEY_KP3
    (0x5c, 75),  // KEY_KP4
    (0x5d, 76),  // KEY_KP5
    (0x5e, 77),  // KEY_KP6
    (0x5f, 71),  // KEY_KP7
    (0x60, 72),  // KEY_KP8
    (0x61, 73),  // KEY_KP9
    (0x62, 82),  // KEY_KP0
    (0x63, 83),  // KEY_KPDOT
    (0x64, 86),  // KEY_102ND
    (0x65, 127), // KEY_COMPOSE
    (0x67, 117), // KEY_KPEQUAL
    (0x68, 183), // KEY_F13
    (0x69, 184), // KEY_F14
    (0x6a, 185), // KEY_F15
    (0x6b, 186), // KEY_F16
    (0x6c, 187), // KEY_F17
    (0x6d, 188), // KEY_F18
    (0x6e, 189), // KEY_F19
    (0x6f, 190), // KEY_F20
    (0x70, 191), // KEY_F21
    (0x71, 192), // KEY_F22
    (0x72, 193), // KEY_F23
    (0x73, 194), // KEY_F24
    (0x75, 138), // KEY_HELP
    (0x9c,355), // KEY_CLEAR, original EvdevVirtualKeyboard.
    (0xe0, 29),  // KEY_LEFTCTRL
    (0xe1, 42),  // KEY_LEFTSHIFT
    (0xe2, 56),  // KEY_LEFTALT
    (0xe3, 125), // KEY_LEFTMETA
    (0xe4, 97),  // KEY_RIGHTCTRL
    (0xe5, 54),  // KEY_RIGHTSHIFT
    (0xe6, 100), // KEY_RIGHTALT
    (0xe7, 126), // KEY_RIGHTMETA
    (0x10b5,163), // KEY_NEXTSONG
    (0x10b6,165), // KEY_PREVIOUSSONG
    (0x10b7,166), // KEY_STOPCD
    (0x10cd,164), // KEY_PLAYPAUSE
    (0x10e2,113), // KEY_MUTE
    (0x10e9,115), // KEY_VOLUMEUP
    (0x10ea,114), // KEY_VOLUMEDOWN
];

/// The evdev key code for a usage, if this backend can press it.
pub fn key_code(key: KeyboardUsage) -> Option<u16> {
    if let Some(code)=key.linux_event_code(){return Some(code);}
    KEYS.binary_search_by_key(&key.usage(), |(usage, _)| *usage)
        .ok()
        .map(|index| KEYS[index].1)
}

/// Every key code the virtual keyboard declares.
pub fn key_codes() -> impl Iterator<Item = u16> {
    KEYS.iter().map(|(_, code)| *code).chain(otd_core::keys::linux_names().filter_map(|(_,code)|(code!=0).then_some(code)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(usage: u16) -> Option<u16> {
        KeyboardUsage::new(usage).and_then(key_code)
    }

    #[test]
    fn usages_map_to_the_codes_of_a_physical_keyboard() {
        assert_eq!(key(0x04), Some(30), "A");
        assert_eq!(key(0x1d), Some(44), "Z");
        assert_eq!(key(0x27), Some(11), "0");
        assert_eq!(key(0x29), Some(1), "Escape");
        assert_eq!(key(0xe0), Some(29), "Left Control");
        assert_eq!(key(0xe7), Some(126), "Right GUI");
        assert_eq!(key(0x52), Some(103), "Up");
        assert_eq!(key(0x54), Some(98), "keypad divide is not the main slash");
        assert_eq!(key(0x73), Some(194), "F24");
    }

    #[test]
    fn the_table_is_sorted_and_free_of_duplicates() {
        assert!(KEYS.windows(2).all(|pair| pair[0].0 < pair[1].0));
        let mut codes: Vec<_> = KEYS.iter().map(|(_,code)|*code).collect();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), KEYS.len());
    }

    #[test]
    fn unknown_usages_are_not_pressed() {
        for usage in [0x32, 0x66, 0xe8, 0x100] {
            assert_eq!(key(usage), None, "{usage:#x}");
        }
    }

    #[test]
    fn every_upstream_key_name_this_driver_accepts_has_a_key() {
        for name in [
            "A",
            "D0",
            "Enter",
            "Escape",
            "Space",
            "F12",
            "F24",
            "Left",
            "Keypad5",
            "Control",
            "Shift",
            "Alt",
            "Menu",
            "Application",
            "RightAlt",
            "Decimal",
            "Divide",
        ] {
            let usage = otd_core::keys::usage_from_name(name).expect(name);
            assert!(key_code(usage).is_some(), "{name}");
        }
    }

    #[test]
    fn mouse_buttons_follow_upstream() {
        assert_eq!(mouse_code(MouseButton::Backward), 0x113);
        assert_eq!(mouse_code(MouseButton::Forward), 0x114);
        assert_eq!(MOUSE_BUTTONS.len(), 5);
    }
}
