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
    Action, ActionOwner, ActionState, ActionTransition, KeyboardUsage, MouseButton,
};
use otd_core::output::buttons::ActionSink;
use windows_sys::Win32::Foundation::SetLastError;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYEVENTF_EXTENDEDKEY,
    KEYEVENTF_KEYUP, KEYEVENTF_SCANCODE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP,
    MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP,
    MOUSEEVENTF_XDOWN, MOUSEEVENTF_XUP, MOUSEINPUT, SendInput,
};

/// Check support before adding a configured action to the ownership state.
/// Unknown usages are rejected, not silently replaced with a different key.
pub fn supports(action: Action) -> bool {
    match action {
        Action::Mouse(_) => true,
        Action::Key(key) => keyboard_scan_code(key).is_some(),
    }
}

/// Translate without injecting input. Keyboard usages denote physical key
/// positions; the active Windows layout determines the resulting characters.
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
/// This bounded subset covers ordinary alphanumeric/punctuation positions,
/// F1-F12, navigation, keypad and all eight modifiers. Pause (E1 sequence),
/// consumer/media usages, power, and uncommon international keys are explicit
/// unsupported cases. Usage 0x32 is omitted because Windows aliases it to 0x31;
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

/// Every session's held actions.
static HELD: Mutex<ActionState> = Mutex::new(ActionState::new());
// Created during session setup, so registration never allocates in a report.
// One owner represents the combined left holds in HELD across every session.
static LEFT_ACTION_OUTPUT: Mutex<Option<crate::output::SessionOutput>> = Mutex::new(None);
static NEXT_DEVICE: AtomicU64 = AtomicU64::new(1);

fn held() -> io::Result<std::sync::MutexGuard<'static, ActionState>> {
    HELD.lock()
        .map_err(|_| io::Error::other("held action lock poisoned"))
}

/// One tablet session's share of the held actions. Dropping it lets go of
/// what it still holds.
pub struct SessionActions {
    device: u64,
}

impl SessionActions {
    pub fn new() -> io::Result<Self> {
        let mut output = LEFT_ACTION_OUTPUT.lock()
            .map_err(|_| io::Error::other("left action output lock poisoned"))?;
        if output.is_none() {
            *output = Some(crate::output::SessionOutput::new()?);
        }
        Ok(Self {
            device: NEXT_DEVICE.fetch_add(1, Ordering::Relaxed),
        })
    }
}

impl ActionSink for SessionActions {
    fn supports(&self, action: Action) -> bool {
        supports(action)
    }

    fn hold(&mut self, binding: u32, action: Action, down: bool) -> io::Result<()> {
        held()?
            .set_held(
                ActionOwner {
                    device: self.device,
                    binding,
                },
                action,
                down,
            )
            .map(|_| ())
            .map_err(io::Error::other)
    }

    fn flush(&mut self) -> io::Result<usize> {
        let mut state = held()?;
        flush_pending(&mut state)
    }

    fn release_all(&mut self) -> io::Result<usize> {
        let mut state = held()?;
        state.release_device(self.device);
        flush_pending(&mut state)
    }
}

impl Drop for SessionActions {
    fn drop(&mut self) {
        // Best effort: a release that fails stays pending, and the next flush
        // by any session retries it.
        if let Ok(mut state) = HELD.lock() {
            state.release_device(self.device);
            let _ = flush_pending(&mut state);
        }
    }
}
