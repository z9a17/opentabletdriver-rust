//! Windows adapter for the portable shared action state (B01 foundation),
//! and the key and button output of the pen side buttons.
//!
//! Left-button actions share ownership with the combined mouse motion/tip path.
//! All sessions share one `ActionState` for their synthetic held actions, so
//! two tablets holding the same key press it once. Physical user input and
//! other injectors are outside that ownership model.
//!
//! Each call sends exactly one INPUT and acknowledges only a return count of 1.
//! See https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-sendinput
//! and https://learn.microsoft.com/en-us/windows/win32/api/winuser/ns-winuser-keybdinput.

use std::io;
use std::mem::size_of;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use otd_core::actions::{
    Action, ActionOwner, ActionState, ActionTransition, ConsumerKey, KeyboardUsage, MouseButton,
};
use otd_core::output::buttons::{ActionSink, ScrollAxis, ScrollPulse};
use windows_sys::Win32::Foundation::SetLastError;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYEVENTF_EXTENDEDKEY,
    KEYEVENTF_KEYUP, KEYEVENTF_SCANCODE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP,
    MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP,
    MOUSEEVENTF_XDOWN, MOUSEEVENTF_XUP, MOUSEEVENTF_WHEEL, MOUSEEVENTF_HWHEEL, MOUSEINPUT, SendInput,
    GetKeyboardLayout, MapVirtualKeyExW, MAPVK_VK_TO_VSC_EX,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

/// Check support before adding a configured action to the ownership state.
/// Unknown usages are rejected, not silently replaced with a different key.
pub fn supports(action: Action) -> bool {
    match action {
        Action::Mouse(_) => true,
        Action::Key(key) => keyboard_scan_code(key).is_some() || keyboard_virtual_code(key).is_some(),
    }
}

/// Translate without injecting input. Native usages denote physical positions;
/// the original Windows synthetic domain denotes logical virtual keys.
pub fn encode_transition(transition: ActionTransition) -> io::Result<INPUT> {
    match transition.action {
        Action::Mouse(button) => {
            let (down, up, data) = match button {
                MouseButton::Left => (MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, 0),
                MouseButton::Right => (MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, 0),
                MouseButton::Middle => (MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP, 0),
                // MOUSEINPUT mouseData uses XBUTTON1=1 and XBUTTON2=2.
                MouseButton::Backward => (MOUSEEVENTF_XDOWN, MOUSEEVENTF_XUP, 1),
                MouseButton::Forward => (MOUSEEVENTF_XDOWN, MOUSEEVENTF_XUP, 2),
            };
            Ok(INPUT {
                r#type: INPUT_MOUSE,
                Anonymous: INPUT_0 {
                    mi: MOUSEINPUT {
                        dx: 0,
                        dy: 0,
                        mouseData: data,
                        dwFlags: if transition.pressed { down } else { up },
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            })
        }
        Action::Key(key) => {
            if let Some(vk) = keyboard_virtual_code(key) {
                return Ok(INPUT { r#type: INPUT_KEYBOARD, Anonymous: INPUT_0 {
                    ki: KEYBDINPUT { wVk: vk, wScan: 0,
                        dwFlags: if transition.pressed { 0 } else { KEYEVENTF_KEYUP },
                        time: 0, dwExtraInfo: 0 },
                } });
            }
            let (scan, extended) = keyboard_scan_code(key).ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::Unsupported,
                    format!("unsupported USB keyboard usage 0x{:04x}", key.usage()),
                )
            })?;
            let mut flags = KEYEVENTF_SCANCODE;
            if extended {
                flags |= KEYEVENTF_EXTENDEDKEY;
            }
            if !transition.pressed {
                flags |= KEYEVENTF_KEYUP;
            }
            Ok(INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: 0,
                        wScan: scan,
                        dwFlags: flags,
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            })
        }
    }
}

/// Encode one upstream pointer scroll call without injecting input.
pub fn encode_scroll(pulse: ScrollPulse) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 { mi: MOUSEINPUT {
            dx: 0, dy: 0, mouseData: pulse.delta as u32,
            dwFlags: match pulse.axis { ScrollAxis::Vertical => MOUSEEVENTF_WHEEL, ScrollAxis::Horizontal => MOUSEEVENTF_HWHEEL },
            time: 0, dwExtraInfo: 0,
        } },
    }
}

fn send_scroll(pulse: ScrollPulse) -> io::Result<()> {
    let input = encode_scroll(pulse);
    unsafe { SetLastError(0) };
    if unsafe { SendInput(1, &input, size_of::<INPUT>() as i32) } != 1 {
        let error = io::Error::last_os_error();
        return Err(if error.raw_os_error().is_some_and(|code| code != 0) { error }
            else { io::Error::other("SendInput accepted no scroll event; input may be blocked") });
    }
    Ok(())
}

/// Emit exactly one transition. A successful return means SendInput accepted
/// the event, not that any particular foreground application processed it.
pub fn send_transition(transition: ActionTransition) -> io::Result<()> {
    if transition.action == Action::Mouse(MouseButton::Left) {
        // Share the tip's acknowledged left-button ownership. A side-button
        // release must not lift another session's tip or left binding.
        let output = LEFT_ACTION_OUTPUT.lock()
            .map_err(|_| io::Error::other("left action output lock poisoned"))?;
        let output = output.as_ref()
            .ok_or_else(|| io::Error::other("left action output was not prepared"))?;
        return output.send(otd_core::output::MousePacket {
            dx: 0,
            dy: 0,
            flags: if transition.pressed {
                otd_core::output::flags::LEFTDOWN
            } else {
                otd_core::output::flags::LEFTUP
            },
        });
    }
    let input = encode_transition(transition)?;
    // SendInput may report zero without setting an error (including UIPI cases).
    // Clear stale thread error state and provide an honest fallback diagnostic.
    unsafe { SetLastError(0) };
    let accepted = unsafe { SendInput(1, &input, size_of::<INPUT>() as i32) };
    if accepted != 1 {
        let error = io::Error::last_os_error();
        return Err(if error.raw_os_error().is_some_and(|code| code != 0) {
            error
        } else {
            io::Error::other("SendInput accepted no action event; input may be blocked")
        });
    }
    Ok(())
}

/// Reconcile held actions until synchronized or the first output failure.
/// Already accepted events remain acknowledged when a later event fails;
/// calling again retries only remaining differences. Call `release_owner`,
/// `release_device` or `release_all` first for the corresponding cleanup.
pub fn flush_pending<const ACTIONS: usize, const HOLDS: usize>(
    state: &mut ActionState<ACTIONS, HOLDS>,
) -> io::Result<usize> {
    let mut accepted = 0;
    while let Some(pending) = state.next_pending() {
        send_transition(pending.transition())?;
        pending.acknowledge();
        accepted += 1;
    }
    Ok(accepted)
}

/// USB page 0x07 to PC Set 1 scan code, plus the E0 extended flag.
///
/// Ordinary alphanumeric/punctuation positions, F1-F12, navigation, keypad
/// and all eight modifiers use scan codes. The other pinned names use their
/// original virtual keys below. Usage 0x32 is omitted because Windows aliases it to 0x31;
/// accepting both as different held actions would break shared ownership.
/// Scan-code facts cross-referenced against Chromium's platform code table:
/// https://github.com/chromium/chromium/blob/main/ui/events/keycodes/dom/dom_code_data.inc
fn keyboard_scan_code(key: KeyboardUsage) -> Option<(u16, bool)> {
    let code = match key.usage() {
        // A-Z positions, independent of the active keyboard layout.
        0x04..=0x1d => {
            const LETTERS: [u16; 26] = [
                0x1e, 0x30, 0x2e, 0x20, 0x12, 0x21, 0x22, 0x23, 0x17, 0x24, 0x25, 0x26, 0x32, 0x31,
                0x18, 0x19, 0x10, 0x13, 0x1f, 0x14, 0x16, 0x2f, 0x11, 0x2d, 0x15, 0x2c,
            ];
            LETTERS[(key.usage() - 0x04) as usize]
        }
        0x1e..=0x27 => key.usage() - 0x1e + 0x02, // 1-9, 0
        0x28 => 0x1c,                             // Enter
        0x29 => 0x01,                             // Escape
        0x2a => 0x0e,                             // Backspace
        0x2b => 0x0f,                             // Tab
        0x2c => 0x39,                             // Space
        0x2d => 0x0c,                             // Minus
        0x2e => 0x0d,                             // Equal
        0x2f => 0x1a,                             // Left bracket
        0x30 => 0x1b,                             // Right bracket
        0x31 => 0x2b,                             // Backslash
        0x33 => 0x27,                             // Semicolon
        0x34 => 0x28,                             // Quote
        0x35 => 0x29,                             // Grave
        0x36 => 0x33,                             // Comma
        0x37 => 0x34,                             // Period
        0x38 => 0x35,                             // Slash
        0x39 => 0x3a,                             // Caps Lock
        0x3a..=0x43 => key.usage() - 0x3a + 0x3b, // F1-F10
        0x44 => 0x57,                             // F11
        0x45 => 0x58,                             // F12
        0x46 => 0xe037,                           // Print Screen
        0x47 => 0x46,                             // Scroll Lock
        0x49 => 0xe052,                           // Insert
        0x4a => 0xe047,                           // Home
        0x4b => 0xe049,                           // Page Up
        0x4c => 0xe053,                           // Delete
        0x4d => 0xe04f,                           // End
        0x4e => 0xe051,                           // Page Down
        0x4f => 0xe04d,                           // Right
        0x50 => 0xe04b,                           // Left
        0x51 => 0xe050,                           // Down
        0x52 => 0xe048,                           // Up
        0x53 => 0xe045,                           // Num Lock
        0x54 => 0xe035,                           // Keypad divide
        0x55 => 0x37,                             // Keypad multiply
        0x56 => 0x4a,                             // Keypad subtract
        0x57 => 0x4e,                             // Keypad add
        0x58 => 0xe01c,                           // Keypad Enter
        0x59..=0x63 => {
            const KEYPAD: [u16; 11] = [
                0x4f, 0x50, 0x51, 0x4b, 0x4c, 0x4d, 0x47, 0x48, 0x49, 0x52, 0x53,
            ];
            KEYPAD[(key.usage() - 0x59) as usize]
        }
        0x64 => 0x56,   // ISO extra key near left Shift
        0x65 => 0xe05d, // Context menu
        0xe0 => 0x1d,   // Left Control
        0xe1 => 0x2a,   // Left Shift
        0xe2 => 0x38,   // Left Alt
        0xe3 => 0xe05b, // Left GUI
        0xe4 => 0xe01d, // Right Control
        0xe5 => 0x36,   // Right Shift
        0xe6 => 0xe038, // Right Alt
        0xe7 => 0xe05c, // Right GUI
        _ => return None,
    };
    Some((code & 0xff, code & 0xff00 == 0xe000))
}

/// Exact pinned WindowsVirtualKeyboard virtual keys for names whose keyboard
/// scan encoding is absent, and consumer controls from the synthetic domain.
fn keyboard_virtual_code(key: KeyboardUsage) -> Option<u16> {
    if let Some(code) = key.windows_virtual_code() { return Some(code); }
    if let Some(consumer) = key.consumer_key() {
        return Some(match consumer {
            ConsumerKey::Mute => 0xad, ConsumerKey::VolumeDown => 0xae,
            ConsumerKey::VolumeUp => 0xaf, ConsumerKey::NextSong => 0xb0,
            ConsumerKey::PreviousSong => 0xb1, ConsumerKey::StopSong => 0xb2,
            ConsumerKey::PlayPause => 0xb3,
        });
    }
    Some(match key.usage() {
        0x48 => 0x13, // VK_PAUSE (the E1 sequence is not one scan INPUT)
        0x67 => 0x92, // VK_OEM_NEC_EQUAL
        0x68..=0x73 => 0x7c + key.usage() - 0x68, // VK_F13..VK_F24
        0x75 => 0x2f, // VK_HELP
        0x9c => 0x0c, // VK_CLEAR
        _ => return None,
    })
}

/// Canonical ownership for original VK-only keys, including media. Call before
/// scan-code conversion so original and native holders share the same action.
pub fn usage_for_virtual_key(code: u32) -> Option<KeyboardUsage> {
    otd_core::keys::names().map(|(_, key)| key)
        .find(|key| keyboard_virtual_code(*key).is_some_and(|vk| u32::from(vk) == code))
}

/// The key at a physical position, from a keyboard message's scan code and
/// extended-key flag: the inverse of the injection table, so a captured key
/// is sent back as the same position.
pub fn usage_for_scan_code(scan: u16, extended: bool) -> Option<KeyboardUsage> {
    (0x04..=0xe7u16)
        .filter_map(KeyboardUsage::new)
        .find(|key| keyboard_scan_code(*key) == Some((scan, extended)))
}

/// Every session's held actions.
#[derive(Clone, Copy)]
struct Encoder { canonical: Action, source: Action, owner: ActionOwner }
#[derive(Clone, Copy)]
struct SourceHold { owner: ActionOwner, canonical: Action, source: Action }
struct SharedActions { actions: ActionState, encoders: [Option<Encoder>; 64], sources: [Option<SourceHold>; 128] }
impl SharedActions {
    const fn new() -> Self { Self { actions: ActionState::new(), encoders: [None; 64], sources: [None; 128] } }
    fn reap_encoders(&mut self) {
        for entry in &mut self.encoders {
            if entry.is_some_and(|entry| !self.actions.is_desired(entry.canonical) && !self.actions.is_emitted(entry.canonical)) { *entry = None; }
        }
    }
    fn set_held(&mut self, owner: ActionOwner, canonical: Action, source: Action, down: bool) -> io::Result<()> {
        self.reap_encoders();
        let existing = self.encoders.iter().position(|entry| entry.is_some_and(|entry| entry.canonical == canonical));
        let slot = if down && existing.is_none() {
            Some(self.encoders.iter().position(Option::is_none).ok_or_else(|| io::Error::other("Held encoder capacity exceeded"))?)
        } else { None };
        let existing_source = self.sources.iter().position(|entry| entry.is_some_and(|entry| entry.owner == owner && entry.canonical == canonical));
        let source_slot = if down && existing_source.is_none() {
            Some(self.sources.iter().position(Option::is_none).ok_or_else(|| io::Error::other("Held source capacity exceeded"))?)
        } else { existing_source };
        self.actions.set_held(owner, canonical, down).map_err(io::Error::other)?;
        if let Some(slot) = source_slot { self.sources[slot] = down.then_some(SourceHold { owner, canonical, source }); }
        if let Some(slot) = slot { self.encoders[slot] = Some(Encoder { canonical, source, owner }); }
        Ok(())
    }
    fn release_device(&mut self, device: u64) {
        self.actions.release_device(device);
        for source in &mut self.sources { if source.is_some_and(|entry| entry.owner.device == device) { *source = None; } }
    }
    fn flush(&mut self) -> io::Result<usize> { self.flush_with(send_transition) }
    fn flush_with(&mut self, mut send: impl FnMut(ActionTransition) -> io::Result<()>) -> io::Result<usize> {
        let mut count = 0;
        while let Some(pending) = self.actions.next_pending() {
            let transition = pending.transition();
            let encoder = self.encoders.iter_mut().flatten().find(|entry| entry.canonical == transition.action)
                .ok_or_else(|| io::Error::other("Held action has no retained output encoder"))?;
            // A press not yet accepted must belong to a still-desired source.
            // Once accepted, keep that exact encoder until its release succeeds.
            if transition.pressed && !self.sources.iter().flatten().any(|source| source.owner == encoder.owner && source.canonical == encoder.canonical && source.source == encoder.source) {
                let source = self.sources.iter().flatten().find(|source| source.canonical == encoder.canonical)
                    .ok_or_else(|| io::Error::other("Desired action has no retained source"))?;
                encoder.source = source.source; encoder.owner = source.owner;
            }
            send(ActionTransition { action: encoder.source, pressed: transition.pressed })?;
            pending.acknowledge(); count += 1;
        }
        self.reap_encoders();
        Ok(count)
    }
}
static HELD: Mutex<SharedActions> = Mutex::new(SharedActions::new());
// Created during session setup, so registration never allocates in a report.
// One owner represents the combined left holds in HELD across every session.
static LEFT_ACTION_OUTPUT: Mutex<Option<crate::output::SessionOutput>> = Mutex::new(None);
static NEXT_DEVICE: AtomicU64 = AtomicU64::new(1);

fn held() -> io::Result<std::sync::MutexGuard<'static, SharedActions>> {
    HELD.lock()
        .map_err(|_| io::Error::other("held action lock poisoned"))
}

/// One tablet session's share of the held actions. Dropping it lets go of
/// what it still holds.
pub struct SessionActions {
    device: u64,
    aliases: [Option<HeldAlias>; 128],
}
#[derive(Clone, Copy)]
struct HeldAlias { binding: u32, source: Action, canonical: Action }

fn canonical_action(source: Action) -> Action {
    let Action::Key(key) = source else { return source; };
    let Some(code) = key.windows_virtual_code() else { return source; };
    // VK-only baseline keys/media already have a shared portable identity.
    if let Some(key) = usage_for_virtual_key(u32::from(code)) { return Action::Key(key); }
    // A named original key follows the foreground input layout. A physical
    // native key stays at its scan position. This lookup occurs only when a
    // logical hold starts; release uses its retained identity below.
    let foreground = unsafe { GetForegroundWindow() };
    let thread = if foreground.is_null() { 0 } else { unsafe { GetWindowThreadProcessId(foreground, std::ptr::null_mut()) } };
    let layout = unsafe { GetKeyboardLayout(thread) };
    let scan = unsafe { MapVirtualKeyExW(u32::from(code), MAPVK_VK_TO_VSC_EX, layout) };
    usage_for_scan_code((scan & 0xff) as u16, scan & 0xff00 == 0xe000).map(Action::Key).unwrap_or(source)
}

impl SessionActions {
    fn hold_in_state(&mut self, state: &mut SharedActions, binding: u32, action: Action, down: bool,
        canonicalize: impl FnOnce(Action) -> Action) -> io::Result<()> {
        let existing = self.aliases.iter().position(|alias| alias.is_some_and(|alias| alias.binding == binding && alias.source == action));
        let slot = match existing {
            Some(slot) => slot,
            None if !down => return Ok(()),
            None => self.aliases.iter().position(Option::is_none).ok_or_else(|| io::Error::other("Session hold capacity exceeded"))?,
        };
        let canonical = self.aliases[slot].map_or_else(|| canonicalize(action), |alias| alias.canonical);
        state.set_held(ActionOwner { device: self.device, binding: slot as u32 }, canonical, action, down)?;
        self.aliases[slot] = down.then_some(HeldAlias { binding, source: action, canonical });
        Ok(())
    }
    pub fn new() -> io::Result<Self> {
        let mut output = LEFT_ACTION_OUTPUT.lock()
            .map_err(|_| io::Error::other("left action output lock poisoned"))?;
        if output.is_none() {
            *output = Some(crate::output::SessionOutput::new()?);
        }
        Ok(Self {
            device: NEXT_DEVICE.fetch_add(1, Ordering::Relaxed),
            aliases: [None; 128],
        })
    }
}

impl ActionSink for SessionActions {
    fn supports_scroll(&self) -> bool { true }
    fn scroll(&mut self, pulse: ScrollPulse) -> io::Result<()> {
        // Serialize with this application's held events. Scroll is a pulse;
        // one owner must never suppress another owner's independent wheel tick.
        let mut state = held()?;
        state.flush()?;
        send_scroll(pulse)
    }

    fn supports(&self, action: Action) -> bool {
        supports(action)
    }

    fn hold(&mut self, binding: u32, action: Action, down: bool) -> io::Result<()> {
        let mut state = held()?;
        self.hold_in_state(&mut state, binding, action, down, canonical_action)
    }

    fn flush(&mut self) -> io::Result<usize> {
        let mut state = held()?;
        state.flush()
    }

    fn release_all(&mut self) -> io::Result<usize> {
        let mut state = held()?;
        state.release_device(self.device);
        self.aliases.fill(None);
        state.flush()
    }
}

impl Drop for SessionActions {
    fn drop(&mut self) {
        // Best effort: a release that fails stays pending, and the next flush
        // by any session retries it.
        if let Ok(mut state) = HELD.lock() {
            state.release_device(self.device);
            let _ = state.flush();
        }
    }
}

#[cfg(test)]
mod scroll_tests {
    use super::*;
    #[test]
    fn logical_and_physical_holders_share_edges_and_retain_release_encoding() {
        // A fake layout maps original A to the physical Q position. No OS calls.
        let logical = Action::Key(KeyboardUsage::windows_virtual_key(0x41).unwrap());
        let physical = Action::Key(KeyboardUsage::new(0x14).unwrap());
        let mut session = std::mem::ManuallyDrop::new(SessionActions { device: 1, aliases: [None; 128] });
        let mut state = SharedActions::new();
        let mut sent = Vec::new();
        session.hold_in_state(&mut state, 7, logical, true, |_| physical).unwrap();
        state.flush_with(|event| { sent.push(event); Ok(()) }).unwrap();
        // A second chord member has the same canonical key and its own subowner.
        session.hold_in_state(&mut state, 7, physical, true, |key| key).unwrap();
        session.hold_in_state(&mut state, 7, logical, false, |_| panic!("release must not resolve a new layout")).unwrap();
        assert_eq!(state.flush_with(|event| { sent.push(event); Ok(()) }).unwrap(), 0);
        session.hold_in_state(&mut state, 7, physical, false, |_| panic!("release must retain its identity")).unwrap();
        state.flush_with(|event| { sent.push(event); Ok(()) }).unwrap();
        assert_eq!(sent, [ActionTransition { action: logical, pressed: true }, ActionTransition { action: logical, pressed: false }]);
    }
    #[test]
    fn rejected_press_selects_a_remaining_source_before_retry() {
        let logical = Action::Key(KeyboardUsage::windows_virtual_key(0x41).unwrap());
        let physical = Action::Key(KeyboardUsage::new(0x14).unwrap());
        let first = ActionOwner { device: 1, binding: 0 };
        let second = ActionOwner { device: 2, binding: 0 };
        let mut state = SharedActions::new();
        state.set_held(first, physical, logical, true).unwrap();
        assert!(state.flush_with(|_| Err(io::Error::other("blocked"))).is_err());
        state.set_held(second, physical, physical, true).unwrap();
        state.release_device(first.device);
        let mut sent = Vec::new();
        state.flush_with(|event| { sent.push(event); Ok(()) }).unwrap();
        state.release_device(second.device);
        state.flush_with(|event| { sent.push(event); Ok(()) }).unwrap();
        assert_eq!(sent, [ActionTransition { action: physical, pressed: true }, ActionTransition { action: physical, pressed: false }]);
    }
    #[test]
    fn pinned_windows_names_and_consumer_vk_encoding_are_available() {
        for (name, key) in otd_core::keys::names() {
            assert!(supports(Action::Key(key)), "{name}");
        }
        // Numeric constants from the pinned original VirtualKey dictionary.
        for (name, vk) in [("Mute",0xad), ("VolumeDown",0xae), ("VolumeUp",0xaf),
            ("NextSong",0xb0), ("PreviousSong",0xb1), ("StopSong",0xb2), ("PlayPause",0xb3),
            ("Clear",0x0c), ("Pause",0x13), ("F24",0x87)] {
            let key = otd_core::keys::usage_from_name(name).unwrap();
            assert_eq!(usage_for_virtual_key(vk as u32), Some(key));
            for pressed in [true, false] {
                let encoded = encode_transition(ActionTransition { action: Action::Key(key), pressed }).unwrap();
                let keyboard = unsafe { encoded.Anonymous.ki };
                assert_eq!(keyboard.wVk, vk);
                assert_eq!(keyboard.wScan, 0);
                assert_eq!(keyboard.dwFlags, if pressed { 0 } else { KEYEVENTF_KEYUP });
            }
        }
    }
    #[test]
    fn wheel_encoding_preserves_signed_amount_and_axis_without_injection() {
        for (axis, delta, flags) in [(ScrollAxis::Vertical, 120, MOUSEEVENTF_WHEEL), (ScrollAxis::Vertical, -120, MOUSEEVENTF_WHEEL), (ScrollAxis::Horizontal, -240, MOUSEEVENTF_HWHEEL)] {
            let input = encode_scroll(ScrollPulse { axis, delta });
            assert_eq!(input.r#type, INPUT_MOUSE);
            let mouse = unsafe { input.Anonymous.mi };
            assert_eq!(mouse.mouseData as i32, delta);
            assert_eq!(mouse.dwFlags, flags);
            assert_eq!((mouse.dx, mouse.dy), (0, 0));
        }
    }
}
