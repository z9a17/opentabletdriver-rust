//! OpenTabletDriver's key names, as its Key Binding and Multi-Key Binding
//! store them, mapped to native physical usages or original logical identities.
//!
//! The names are the `Eto.Forms.Keys` spellings of upstream's
//! `WindowsVirtualKeyboard.EtoKeysymToVK`, pinned at
//! <https://github.com/OpenTabletDriver/OpenTabletDriver/blob/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.Desktop/Interop/Input/Keyboard/WindowsVirtualKeyboard.cs>.
//! Lookup is exact and case-sensitive, as upstream's dictionary is.
//!
//! Native generic modifiers select the left physical key. Original Windows
//! generic modifiers retain their actual generic virtual keys; `Application`
//! selects VK_LWIN. `Menu` is Windows' VK_MENU, which is Alt.
//! `Equal`/`Plus` and `Slash`/`ForwardSlash` are aliases of one key.
//! Media keys retain consumer-page identities in the synthetic u16 domain
//! described by `actions::ConsumerKey`, preserving the managed command ABI.

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
    ("Clear", 0x9c),
    ("Mute", 0x10e2),
    ("VolumeDown", 0x10ea),
    ("VolumeUp", 0x10e9),
    ("PlayPause", 0x10cd),
    ("PreviousSong", 0x10b6),
    ("NextSong", 0x10b5),
    ("StopSong", 0x10b7),
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

/// The key an upstream name stands for. Returns `None` for unknown spellings
/// and upstream's `None` (which presses nothing). Platform adapters reject
/// unavailable identities.
pub fn usage_from_name(name: &str) -> Option<KeyboardUsage> {
    KEYS.iter()
        .find(|(candidate, _)| *candidate == name)
        .and_then(|(_, usage)| KeyboardUsage::new(*usage))
}

/// The canonical upstream name of a usage.
pub fn name_of(key: KeyboardUsage) -> Option<&'static str> {
    if let Some(code) = key.windows_virtual_code() {
        return windows_names().find(|(_, vk)| *vk == code).map(|(name, _)| name);
    }
    KEYS.iter()
        .find(|(_, usage)| *usage == key.usage())
        .map(|(name, _)| *name)
}

/// Exact pinned WindowsVirtualKeyboard names and VK values, including None.
/// Unlike native USB-position bindings, these retain layout-dependent semantics.
pub fn windows_names() -> impl Iterator<Item = (&'static str, u16)> {
    std::iter::once(("None", 0)).chain(KEYS.iter().filter_map(|(name, _)| windows_code(name).map(|code| (*name, code))))
}
fn windows_code(name: &str) -> Option<u16> {
    match name { "None" => return Some(0), "Shift" => return Some(0x10),
        "Control" => return Some(0x11), "Alt" | "Menu" => return Some(0x12), _ => {} }
    let usage = usage_from_name(name)?.usage();
    Some(match usage {
        0x04..=0x1d => 0x41 + usage - 0x04,
        0x1e..=0x26 => 0x31 + usage - 0x1e,
        0x27 => 0x30, 0x28 => 0x0d, 0x29 => 0x1b, 0x2a => 0x08, 0x2b => 0x09,
        0x2c => 0x20, 0x2d => 0xbd, 0x2e => 0xbb, 0x2f => 0xdb, 0x30 => 0xdd,
        0x31 => 0xdc, 0x33 => 0xba, 0x34 => 0xde, 0x35 => 0xc0, 0x36 => 0xbc,
        0x37 => 0xbe, 0x38 => 0xbf, 0x39 => 0x14,
        0x3a..=0x45 => 0x70 + usage - 0x3a,
        0x46 => 0x2c, 0x47 => 0x91, 0x48 => 0x13, 0x49 => 0x2d, 0x4a => 0x24,
        0x4b => 0x21, 0x4c => 0x2e, 0x4d => 0x23, 0x4e => 0x22, 0x4f => 0x27,
        0x50 => 0x25, 0x51 => 0x28, 0x52 => 0x26, 0x53 => 0x90, 0x54 => 0x6f,
        0x55 => 0x6a, 0x56 => 0x6d, 0x57 => 0x6b,
        0x59..=0x61 => 0x61 + usage - 0x59,
        0x62 => 0x60, 0x63 => 0x6e, 0x65 => 0x5d, 0x67 => 0x92,
        0x68..=0x73 => 0x7c + usage - 0x68,
        0x75 => 0x2f, 0x9c => 0x0c, 0xe0 => 0xa2, 0xe1 => 0xa0, 0xe2 => 0xa4,
        0xe3 => 0x5b, 0xe4 => 0xa3, 0xe5 => 0xa1, 0xe6 => 0xa5, 0xe7 => 0x5c,
        0x10e2 => 0xad, 0x10ea => 0xae, 0x10e9 => 0xaf, 0x10b5 => 0xb0,
        0x10b6 => 0xb1, 0x10b7 => 0xb2, 0x10cd => 0xb3, _ => return None,
    })
}
pub fn windows_usage_from_name(name: &str) -> Option<KeyboardUsage> {
    windows_code(name).and_then(KeyboardUsage::windows_virtual_key)
}
pub fn usage_from_original_name(name: &str) -> Option<KeyboardUsage> {
    if cfg!(windows) { windows_usage_from_name(name) } else { usage_from_name(name) }
}

/// Longest chord accepted. Upstream has no limit, but a chord has to fit the
/// fixed-size action table alongside the other held actions.
pub const MAX_CHORD: usize = 8;

/// Parses native Multi-Key syntax, key names joined by `+`
/// (`Control+Shift+Z`). Whitespace around a name is ignored. The result is
/// all-or-nothing: one unsupported name rejects the whole chord. `vk:` marks
/// an original Windows logical key without changing native physical names.
pub fn parse_chord(text: &str) -> Result<Vec<KeyboardUsage>, String> {
    parse_names(text, |name| name.strip_prefix("vk:").map_or_else(|| usage_from_name(name), windows_usage_from_name), false)
}
pub fn parse_original_chord(text: &str) -> Result<Vec<KeyboardUsage>, String> {
    parse_names(text, usage_from_original_name, true)
}
fn parse_names(text: &str, lookup: impl Fn(&str) -> Option<KeyboardUsage>, allow_none: bool) -> Result<Vec<KeyboardUsage>, String> {
    let mut keys = Vec::new();
    for name in text.split('+').map(str::trim) {
        if allow_none && name == "None" { continue; }
        let key = lookup(name).ok_or_else(|| {
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

/// Original store text, using canonical names without native domain prefixes.
pub fn chord_text(keys: &[KeyboardUsage]) -> String {
    format_chord(keys, false)
}
/// Native persistence retains whether a key is physical or logical.
pub fn native_chord_text(keys: &[KeyboardUsage]) -> String { format_chord(keys, true) }
fn format_chord(keys: &[KeyboardUsage], native: bool) -> String {
    let mut text = String::new();
    for key in keys {
        if !text.is_empty() {
            text.push('+');
        }
        if native && key.windows_virtual_code().is_some() { text.push_str("vk:"); }
        text.push_str(name_of(*key).unwrap_or("None"));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logical_windows_names_remain_distinct_and_roundtrip_native_storage() {
        let chord = parse_chord("vk:Control+vk:A+vk:VolumeUp").unwrap();
        assert_eq!(chord.iter().map(|key| key.usage()).collect::<Vec<_>>(), [0x2011, 0x2041, 0x20af]);
        assert_ne!(chord[1], usage_from_name("A").unwrap());
        assert_eq!(chord_text(&chord), "Control+A+VolumeUp");
        assert_eq!(parse_chord(&native_chord_text(&chord)).unwrap(), chord);
        assert_ne!(windows_usage_from_name("Control"), windows_usage_from_name("LeftControl"));
        assert_eq!(windows_usage_from_name("Alt"), windows_usage_from_name("Menu"));
        for name in ["Control", "Shift", "Alt", "LeftControl", "RightShift", "RightAlt", "LeftApplication", "RightApplication"] {
            assert!(windows_usage_from_name(name).unwrap().is_modifier(), "{name}");
        }
        assert!(!windows_usage_from_name("A").unwrap().is_modifier());
        assert!(windows_usage_from_name("None").is_none());
        assert!(parse_original_chord("None+None").unwrap().is_empty());
    }

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
        for name in ["None", "a", "control", ""] {
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
        assert!(parse_chord("Control+UnknownMedia").is_err());
        assert!(parse_chord("Control+").is_err());
        assert!(parse_chord("").is_err());
        assert!(parse_chord("A+B+C+D+E+F+G+H+I").is_err());
    }

    #[test]
    fn consumer_chord_roundtrips_and_retains_another_sessions_hold() {
        use crate::actions::{Action, ActionOwner, ActionState, ConsumerKey};
        let chord = parse_chord("Control+VolumeUp").unwrap();
        assert_eq!(chord_text(&chord), "LeftControl+VolumeUp");
        assert_eq!(chord[1], ConsumerKey::VolumeUp.keyboard_usage());
        assert_eq!(chord[1].consumer_key(), Some(ConsumerKey::VolumeUp));
        assert!(!chord[1].is_modifier());
        let action = Action::Key(chord[1]);
        let first = ActionOwner { device: 1, binding: 1 };
        let second = ActionOwner { device: 2, binding: 1 };
        let mut state = ActionState::<2, 2>::new();
        state.set_held(first, action, true).unwrap();
        state.next_pending().unwrap().acknowledge();
        state.set_held(second, action, true).unwrap();
        state.release_device(first.device);
        assert!(state.next_pending().is_none());
        state.release_device(second.device);
        let pending = state.next_pending().unwrap();
        assert!(!pending.transition().pressed);
        assert_eq!(pending.transition().action, action);
        pending.acknowledge();
    }
}

/// All pinned names, including aliases, for exact managed service lookup.
pub fn names() -> impl Iterator<Item = (&'static str, KeyboardUsage)> { KEYS.iter().filter_map(|(name, usage)| KeyboardUsage::new(*usage).map(|usage| (*name, usage))) }
