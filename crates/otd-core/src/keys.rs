//! OpenTabletDriver's key names, as its Key Binding and Multi-Key Binding
//! store them, mapped to portable USB keyboard usages.
//!
//! The names are the `Eto.Forms.Keys` spellings of upstream's
//! `WindowsVirtualKeyboard.EtoKeysymToVK`, pinned at
//! <https://github.com/OpenTabletDriver/OpenTabletDriver/blob/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.Desktop/Interop/Input/Keyboard/WindowsVirtualKeyboard.cs>.
//! Lookup is exact and case-sensitive, as upstream's dictionary is.
//!
//! Upstream's generic modifiers (`Shift`, `Control`, `Alt`, `Application`)
//! press the left key. `Menu` is Windows' `VK_MENU`, which is Alt.
//! `Equal`/`Plus` and `Slash`/`ForwardSlash` are aliases of one key.
//! Media keys are on another HID usage page and are not supported.

use crate::actions::KeyboardUsage;

/// Name, usage. The first name listed for a usage is its canonical name,
/// used when a profile is written back.
const KEYS: &[(&str, u16)] = &[
    ("A", 0x04),
    ("B", 0x05),
    ("C", 0x06),
    ("D", 0x07),
    ("E", 0x08),
    ("F", 0x09),
    ("G", 0x0a),
    ("H", 0x0b),
    ("I", 0x0c),
    ("J", 0x0d),
    ("K", 0x0e),
    ("L", 0x0f),
    ("M", 0x10),
    ("N", 0x11),
    ("O", 0x12),
    ("P", 0x13),
    ("Q", 0x14),
    ("R", 0x15),
    ("S", 0x16),
    ("T", 0x17),
    ("U", 0x18),
    ("V", 0x19),
    ("W", 0x1a),
    ("X", 0x1b),
    ("Y", 0x1c),
    ("Z", 0x1d),
    ("D1", 0x1e),
    ("D2", 0x1f),
    ("D3", 0x20),
    ("D4", 0x21),
    ("D5", 0x22),
    ("D6", 0x23),
    ("D7", 0x24),
    ("D8", 0x25),
    ("D9", 0x26),
    ("D0", 0x27),
    ("Enter", 0x28),
    ("Escape", 0x29),
    ("Backspace", 0x2a),
    ("Tab", 0x2b),
    ("Space", 0x2c),
    ("Minus", 0x2d),
    ("Equal", 0x2e),
    ("Plus", 0x2e),
    ("LeftBracket", 0x2f),
    ("RightBracket", 0x30),
    ("Backslash", 0x31),
    ("Semicolon", 0x33),
    ("Quote", 0x34),
    ("Grave", 0x35),
    ("Comma", 0x36),
    ("Period", 0x37),
    ("Slash", 0x38),
    ("ForwardSlash", 0x38),
    ("CapsLock", 0x39),
    ("F1", 0x3a),
    ("F2", 0x3b),
    ("F3", 0x3c),
    ("F4", 0x3d),
    ("F5", 0x3e),
    ("F6", 0x3f),
    ("F7", 0x40),
    ("F8", 0x41),
    ("F9", 0x42),
    ("F10", 0x43),
    ("F11", 0x44),
    ("F12", 0x45),
    ("PrintScreen", 0x46),
    ("ScrollLock", 0x47),
    ("Pause", 0x48),
    ("Insert", 0x49),
    ("Home", 0x4a),
    ("PageUp", 0x4b),
    ("Delete", 0x4c),
    ("End", 0x4d),
    ("PageDown", 0x4e),
    ("Right", 0x4f),
    ("Left", 0x50),
    ("Down", 0x51),
    ("Up", 0x52),
    ("NumberLock", 0x53),
    ("Divide", 0x54),
    ("Multiply", 0x55),
    ("Subtract", 0x56),
    ("Add", 0x57),
    ("Keypad1", 0x59),
    ("Keypad2", 0x5a),
    ("Keypad3", 0x5b),
    ("Keypad4", 0x5c),
    ("Keypad5", 0x5d),
    ("Keypad6", 0x5e),
    ("Keypad7", 0x5f),
    ("Keypad8", 0x60),
    ("Keypad9", 0x61),
    ("Keypad0", 0x62),
    ("Decimal", 0x63),
    ("ContextMenu", 0x65),
    ("KeypadEqual", 0x67),
    ("F13", 0x68),
    ("F14", 0x69),
    ("F15", 0x6a),
    ("F16", 0x6b),
    ("F17", 0x6c),
    ("F18", 0x6d),
    ("F19", 0x6e),
    ("F20", 0x6f),
    ("F21", 0x70),
    ("F22", 0x71),
    ("F23", 0x72),
    ("F24", 0x73),
    ("Help", 0x75),
    ("LeftControl", 0xe0),
    ("Control", 0xe0),
    ("LeftShift", 0xe1),
    ("Shift", 0xe1),
    ("LeftAlt", 0xe2),
    ("Alt", 0xe2),
    ("Menu", 0xe2),
    ("LeftApplication", 0xe3),
    ("Application", 0xe3),
    ("RightControl", 0xe4),
    ("RightShift", 0xe5),
    ("RightAlt", 0xe6),
    ("RightApplication", 0xe7),
];

/// The key an upstream name stands for. `None` for names this driver does not
/// implement, including upstream's `None` (which presses nothing) and the
/// media keys, and for unknown spellings.
pub fn usage_from_name(name: &str) -> Option<KeyboardUsage> {
    KEYS.iter()
        .find(|(candidate, _)| *candidate == name)
        .and_then(|(_, usage)| KeyboardUsage::new(*usage))
}

/// The canonical upstream name of a usage.
pub fn name_of(key: KeyboardUsage) -> Option<&'static str> {
    KEYS.iter()
        .find(|(_, usage)| *usage == key.usage())
        .map(|(name, _)| *name)
}

/// Longest chord accepted. Upstream has no limit, but a chord has to fit the
/// fixed-size action table alongside the other held actions.
pub const MAX_CHORD: usize = 8;

/// Parses upstream's Multi-Key syntax, key names joined by `+`
/// (`Control+Shift+Z`). Whitespace around a name is ignored. The result is
/// all-or-nothing: as upstream, one unsupported name rejects the whole chord.
pub fn parse_chord(text: &str) -> Result<Vec<KeyboardUsage>, String> {
    let mut keys = Vec::new();
    for name in text.split('+').map(str::trim) {
        let key = usage_from_name(name).ok_or_else(|| {
            if name.is_empty() {
                "empty key name in the key list".to_owned()
            } else {
                format!("unsupported key name {name:?}")
            }
        })?;
        if !keys.contains(&key) {
            keys.push(key);
        }
    }
    if keys.len() > MAX_CHORD {
        return Err(format!("at most {MAX_CHORD} keys can be held together"));
    }
    Ok(keys)
}

/// The inverse of `parse_chord`, using canonical names.
pub fn chord_text(keys: &[KeyboardUsage]) -> String {
    let mut text = String::new();
    for key in keys {
        if !text.is_empty() {
            text.push('+');
        }
        text.push_str(name_of(*key).unwrap_or("None"));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_map_to_usb_usages() {
        assert_eq!(usage_from_name("A").unwrap().usage(), 0x04);
        assert_eq!(usage_from_name("D0").unwrap().usage(), 0x27);
        assert_eq!(usage_from_name("Enter").unwrap().usage(), 0x28);
        assert_eq!(usage_from_name("LeftControl").unwrap().usage(), 0xe0);
        assert_eq!(usage_from_name("RightApplication").unwrap().usage(), 0xe7);
        assert_eq!(usage_from_name("F24").unwrap().usage(), 0x73);
    }

    #[test]
    fn generic_modifiers_and_aliases_press_one_key() {
        for (generic, specific) in [
            ("Control", "LeftControl"),
            ("Shift", "LeftShift"),
            ("Alt", "LeftAlt"),
            ("Menu", "LeftAlt"),
            ("Application", "LeftApplication"),
            ("Plus", "Equal"),
            ("ForwardSlash", "Slash"),
        ] {
            assert_eq!(usage_from_name(generic), usage_from_name(specific));
            assert!(usage_from_name(generic).is_some());
        }
    }

    #[test]
    fn unsupported_and_unknown_names_are_rejected() {
        for name in ["None", "Mute", "VolumeUp", "PlayPause", "a", "control", ""] {
            assert!(usage_from_name(name).is_none(), "{name}");
        }
    }

    #[test]
    fn every_name_has_a_distinct_valid_usage_and_a_canonical_name() {
        for (name, usage) in KEYS {
            let key = usage_from_name(name).unwrap_or_else(|| panic!("{name}"));
            assert_eq!(key.usage(), *usage);
            let canonical = name_of(key).unwrap();
            assert_eq!(usage_from_name(canonical), Some(key));
        }
        let mut names: Vec<_> = KEYS.iter().map(|(name, _)| *name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), KEYS.len(), "duplicate name");
    }

    #[test]
    fn chords_parse_trim_and_dedupe() {
        let keys = parse_chord("Control + Shift+Z").unwrap();
        assert_eq!(
            keys.iter().map(|key| key.usage()).collect::<Vec<_>>(),
            [0xe0, 0xe1, 0x1d]
        );
        assert_eq!(parse_chord("A+A").unwrap().len(), 1);
        assert_eq!(chord_text(&keys), "LeftControl+LeftShift+Z");
        assert_eq!(parse_chord(&chord_text(&keys)).unwrap(), keys);
    }

    #[test]
    fn a_bad_chord_is_rejected_whole() {
        assert!(parse_chord("Control+Mute").is_err());
        assert!(parse_chord("Control+").is_err());
        assert!(parse_chord("").is_err());
        assert!(parse_chord("A+B+C+D+E+F+G+H+I").is_err());
    }
}
