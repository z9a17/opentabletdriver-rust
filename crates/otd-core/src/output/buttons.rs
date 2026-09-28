//! Pen side buttons: what each button does, and the output that carries it out.
//!
//! OpenTabletDriver keeps one binding per pen button (`Bindings.PenButtons`)
//! and its `BindingHandler` presses or releases it whenever the button's state
//! in a report changes. A pen leaving range releases every pen button
//! (`HandleOutOfRangeReport`). This follows that behavior; the sources are
//! <https://github.com/OpenTabletDriver/OpenTabletDriver/blob/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.Desktop/Binding/BindingHandler.cs>
//! and the `AdaptiveBinding`, `MouseBinding`, `KeyBinding` and
//! `MultiKeyBinding` next to it.
//!
//! Held keys and buttons go through the shared [`ActionState`], so two
//! bindings (or two tablets) holding the same key press it once and release it
//! once, and an output failure never marks an action as sent.

use std::fmt;
use std::io;
use std::str::FromStr;

use crate::actions::{
    Action, ActionOwner, ActionState, ActionTransition, KeyboardUsage, MouseButton,
};
use crate::keys;
use crate::reports::Buttons;

/// What one pen button does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ButtonAction {
    /// Nothing.
    None,
    /// Upstream's Adaptive Binding `Button 1`..`Button 3`: whatever the output
    /// does for that barrel button. Mouse output clicks right for 1 and middle
    /// for 2 and does nothing for 3; pen output presses the pen's barrel
    /// button of that number.
    Barrel(u8),
    /// A mouse button, held while the pen button is.
    Mouse(MouseButton),
    /// One key or a chord, held while the pen button is. Modifiers go down
    /// first and come up last.
    Keys(Vec<KeyboardUsage>),
}

/// OpenTabletDriver's defaults: barrel buttons 1, 2 and 3.
pub fn default_pen_buttons() -> Vec<ButtonAction> {
    (1..=3).map(ButtonAction::Barrel).collect()
}

/// The barrel buttons a pen device can report.
pub const MAX_BARREL: u8 = 3;

fn mouse_name(button: MouseButton) -> &'static str {
    match button {
        MouseButton::Left => "left",
        MouseButton::Right => "right",
        MouseButton::Middle => "middle",
        MouseButton::Backward => "backward",
        MouseButton::Forward => "forward",
    }
}

impl FromStr for ButtonAction {
    type Err = String;

    /// `none`, `barrel:1`..`barrel:3`, `mouse:left|right|middle|backward|forward`
    /// or `keys:Control+Shift+Z` with OpenTabletDriver's key names.
    fn from_str(text: &str) -> Result<Self, String> {
        let text = text.trim();
        if text.eq_ignore_ascii_case("none") {
            return Ok(Self::None);
        }
        let (kind, value) = text
            .split_once(':')
            .ok_or_else(|| format!("unknown button action {text:?}"))?;
        let value = value.trim();
        match kind.trim().to_ascii_lowercase().as_str() {
            "barrel" => match value.parse::<u8>() {
                Ok(number @ 1..=MAX_BARREL) => Ok(Self::Barrel(number)),
                _ => Err(format!(
                    "barrel button must be 1..={MAX_BARREL}, not {value:?}"
                )),
            },
            "mouse" => [
                MouseButton::Left,
                MouseButton::Right,
                MouseButton::Middle,
                MouseButton::Backward,
                MouseButton::Forward,
            ]
            .into_iter()
            .find(|button| mouse_name(*button).eq_ignore_ascii_case(value))
            .map(Self::Mouse)
            .ok_or_else(|| {
                format!(
                    "unknown mouse button {value:?}; use left, right, middle, backward or forward"
                )
            }),
            "keys" | "key" => keys::parse_chord(value).map(Self::Keys),
            other => Err(format!("unknown button action kind {other:?}")),
        }
    }
}

impl fmt::Display for ButtonAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::None => f.write_str("none"),
            Self::Barrel(number) => write!(f, "barrel:{number}"),
            Self::Mouse(button) => write!(f, "mouse:{}", mouse_name(*button)),
            Self::Keys(keys) => write!(f, "keys:{}", keys::chord_text(keys)),
        }
    }
}

/// Where held actions become operating-system input. One sink serves one
/// tablet session; the platform decides whether sessions share ownership.
pub trait ActionSink {
    /// Whether the platform can inject this action.
    fn supports(&self, action: Action) -> bool;
    /// Records that `binding` (a pen button index) wants `action` held or not.
    /// Nothing is sent until `flush`.
    fn hold(&mut self, binding: u32, action: Action, held: bool) -> io::Result<()>;
    /// Sends what differs from the last accepted output, stopping at the
    /// first failure. A later call retries only what remains.
    fn flush(&mut self) -> io::Result<usize>;
    /// Drops every hold of this session and sends the releases.
    fn release_all(&mut self) -> io::Result<usize>;
}

/// An `ActionSink` with its own ownership state, for one tablet on a platform
/// whose output is not shared between sessions.
pub struct LocalActions<F> {
    state: Box<ActionState>,
    send: F,
    supports: fn(Action) -> bool,
}

impl<F: FnMut(ActionTransition) -> io::Result<()>> LocalActions<F> {
    pub fn new(send: F, supports: fn(Action) -> bool) -> Self {
        Self {
            state: Box::default(),
            send,
            supports,
        }
    }
}

impl<F: FnMut(ActionTransition) -> io::Result<()>> ActionSink for LocalActions<F> {
    fn supports(&self, action: Action) -> bool {
        (self.supports)(action)
    }

    fn hold(&mut self, binding: u32, action: Action, held: bool) -> io::Result<()> {
        self.state
            .set_held(ActionOwner { device: 0, binding }, action, held)
            .map(|_| ())
            .map_err(io::Error::other)
    }

    fn flush(&mut self) -> io::Result<usize> {
        let mut sent = 0;
        while let Some(pending) = self.state.next_pending() {
            (self.send)(pending.transition())?;
            pending.acknowledge();
            sent += 1;
        }
        Ok(sent)
    }

    fn release_all(&mut self) -> io::Result<usize> {
        self.state.release_device(0);
        self.flush()
    }
}

/// One configured pen button, resolved for the output in use.
struct Slot {
    /// Held through the sink.
    actions: Box<[Action]>,
    /// The pen's barrel button number (bit 0 is button 1), for pen output.
    barrel: u8,
}

/// Follows the pen buttons of a session's reports and presses their actions.
pub struct ButtonOutput {
    slots: Vec<Slot>,
    sink: Box<dyn ActionSink>,
    /// Pen buttons currently wanted down, by button index.
    held: u64,
    /// A flush failed, so the sink still differs from the wanted state.
    unsettled: bool,
}

impl ButtonOutput {
    /// Resolves `bindings` (index = pen button) for mouse output, or for pen
    /// output when `pen` is set. Returns a message for each binding the
    /// platform cannot carry out; those buttons do nothing.
    pub fn new(
        bindings: &[ButtonAction],
        pen: bool,
        sink: Box<dyn ActionSink>,
    ) -> (Self, Vec<String>) {
        let mut rejected = Vec::new();
        let mut slots = Vec::with_capacity(bindings.len().min(64));
        for (index, binding) in bindings.iter().take(64).enumerate() {
            let mut slot = Slot {
                actions: Box::default(),
                barrel: 0,
            };
            let actions: Vec<Action> = match binding {
                ButtonAction::None => Vec::new(),
                ButtonAction::Barrel(number @ 1..=MAX_BARREL) if pen => {
                    slot.barrel = 1 << (number - 1);
                    Vec::new()
                }
                ButtonAction::Barrel(1) => vec![Action::Mouse(MouseButton::Right)],
                ButtonAction::Barrel(2) => vec![Action::Mouse(MouseButton::Middle)],
                ButtonAction::Barrel(_) => Vec::new(),
                ButtonAction::Mouse(button) => vec![Action::Mouse(*button)],
                ButtonAction::Keys(keys) => keys.iter().copied().map(Action::Key).collect(),
            };
            match actions.iter().find(|action| !sink.supports(**action)) {
                Some(action) => rejected.push(format!(
                    "pen button {} ({binding}): {} cannot be sent on this platform",
                    index + 1,
                    describe(*action)
                )),
                None => slot.actions = actions.into_boxed_slice(),
            }
            slots.push(slot);
        }
        (
            Self {
                slots,
                sink,
                held: 0,
                unsettled: false,
            },
            rejected,
        )
    }

    /// The buttons wanted down after a report. A report without a button
    /// reading leaves them as they are; a pen out of range holds none.
    pub fn wanted(&self, buttons: Option<Buttons>, present: bool) -> u64 {
        if !present {
            return 0;
        }
        match buttons {
            Some(buttons) => {
                let mut wanted = 0;
                for index in 0..self.slots.len().min(buttons.len()) {
                    if buttons.get(index) == Some(true) {
                        wanted |= 1 << index;
                    }
                }
                wanted
            }
            None => self.held,
        }
    }

    /// The barrel buttons of a pen device that `wanted` presses.
    pub fn barrel(&self, wanted: u64) -> u8 {
        self.slots
            .iter()
            .enumerate()
            .filter(|(index, _)| wanted & (1 << index) != 0)
            .fold(0, |barrel, (_, slot)| barrel | slot.barrel)
    }

    /// Presses and releases to reach `wanted`. Allocation-free, and does no
    /// work when nothing changed.
    pub fn apply(&mut self, wanted: u64) -> io::Result<()> {
        let changed = wanted ^ self.held;
        if changed == 0 && !self.unsettled {
            return Ok(());
        }
        self.unsettled = true;
        for index in 0..self.slots.len() {
            let bit = 1u64 << index;
            if changed & bit == 0 {
                continue;
            }
            let down = wanted & bit != 0;
            for action in self.slots[index].actions.iter() {
                self.sink.hold(index as u32, *action, down)?;
            }
            self.held = (self.held & !bit) | (wanted & bit);
        }
        self.sink.flush()?;
        self.unsettled = false;
        Ok(())
    }

    /// Releases everything, including what an earlier failure left down.
    /// Returns whether any release was sent.
    pub fn release_all(&mut self) -> io::Result<bool> {
        if self.held == 0 && !self.unsettled {
            return Ok(false);
        }
        self.held = 0;
        self.unsettled = true;
        let sent = self.sink.release_all()?;
        self.unsettled = false;
        Ok(sent != 0)
    }
}

fn describe(action: Action) -> String {
    match action {
        Action::Mouse(button) => format!("mouse {}", mouse_name(button)),
        Action::Key(key) => format!(
            "key {}",
            keys::name_of(key).unwrap_or("with an unnamed usage")
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    type Log = Rc<RefCell<Vec<(Action, bool)>>>;

    fn sink(log: &Log, fail: &Rc<RefCell<bool>>) -> Box<dyn ActionSink> {
        let (log, fail) = (log.clone(), fail.clone());
        Box::new(LocalActions::new(
            move |transition: ActionTransition| {
                if *fail.borrow() {
                    return Err(io::Error::other("injected failure"));
                }
                log.borrow_mut()
                    .push((transition.action, transition.pressed));
                Ok(())
            },
            |action| !matches!(action, Action::Key(key) if key.usage() == 0x75),
        ))
    }

    fn buttons(bits: u64) -> Option<Buttons> {
        Some(Buttons::from_bits(bits, 3).unwrap())
    }

    fn output(bindings: &[ButtonAction], pen: bool) -> (ButtonOutput, Log, Rc<RefCell<bool>>) {
        let log = Log::default();
        let fail = Rc::new(RefCell::new(false));
        let (output, rejected) = ButtonOutput::new(bindings, pen, sink(&log, &fail));
        assert!(rejected.is_empty(), "{rejected:?}");
        (output, log, fail)
    }

    fn right() -> Action {
        Action::Mouse(MouseButton::Right)
    }

    fn key(usage: u16) -> Action {
        Action::Key(KeyboardUsage::new(usage).unwrap())
    }

    #[test]
    fn actions_round_trip_through_text() {
        for text in [
            "none",
            "barrel:2",
            "mouse:backward",
            "keys:LeftControl+LeftShift+Z",
            "keys:Escape",
        ] {
            let action: ButtonAction = text.parse().unwrap();
            assert_eq!(action.to_string(), text);
        }
        assert_eq!(
            "Mouse: Right".parse::<ButtonAction>().unwrap(),
            ButtonAction::Mouse(MouseButton::Right)
        );
        assert_eq!(
            "key:Control".parse::<ButtonAction>().unwrap().to_string(),
            "keys:LeftControl"
        );
        for bad in [
            "",
            "barrel:0",
            "barrel:4",
            "mouse:none",
            "keys:Mute",
            "wheel:1",
        ] {
            assert!(bad.parse::<ButtonAction>().is_err(), "{bad:?}");
        }
    }

    #[test]
    fn default_barrel_buttons_click_right_then_middle() {
        let (mut out, log, _) = output(&default_pen_buttons(), false);
        for bits in [0b001, 0b000, 0b010, 0b000, 0b100, 0b000] {
            let wanted = out.wanted(buttons(bits), true);
            out.apply(wanted).unwrap();
        }
        assert_eq!(
            *log.borrow(),
            [
                (right(), true),
                (right(), false),
                (Action::Mouse(MouseButton::Middle), true),
                (Action::Mouse(MouseButton::Middle), false),
            ],
            "barrel 3 does nothing on a mouse"
        );
    }

    #[test]
    fn a_held_button_presses_once_however_many_reports_repeat_it() {
        let (mut out, log, _) = output(&[ButtonAction::Mouse(MouseButton::Forward)], false);
        for _ in 0..5 {
            let wanted = out.wanted(buttons(1), true);
            out.apply(wanted).unwrap();
        }
        assert_eq!(log.borrow().len(), 1);
        out.apply(out.wanted(buttons(0), true)).unwrap();
        assert_eq!(log.borrow().len(), 2);
    }

    #[test]
    fn a_report_without_a_button_reading_keeps_the_state() {
        let (mut out, log, _) = output(&[ButtonAction::Mouse(MouseButton::Left)], false);
        out.apply(out.wanted(buttons(1), true)).unwrap();
        out.apply(out.wanted(None, true)).unwrap();
        assert_eq!(log.borrow().len(), 1, "no release from an absent reading");
        assert_eq!(out.wanted(None, true), 1);
    }

    #[test]
    fn leaving_range_releases_the_buttons() {
        let (mut out, log, _) = output(&default_pen_buttons(), false);
        out.apply(out.wanted(buttons(0b011), true)).unwrap();
        out.apply(out.wanted(buttons(0b011), false)).unwrap();
        let log = log.borrow();
        assert_eq!(log.iter().filter(|(_, pressed)| *pressed).count(), 2);
        assert_eq!(log.iter().filter(|(_, pressed)| !*pressed).count(), 2);
    }

    #[test]
    fn a_chord_presses_modifiers_first_and_releases_them_last() {
        let chord = keys::parse_chord("Control+Shift+Z").unwrap();
        let (mut out, log, _) = output(&[ButtonAction::Keys(chord)], false);
        out.apply(out.wanted(buttons(1), true)).unwrap();
        out.apply(out.wanted(buttons(0), true)).unwrap();
        assert_eq!(
            *log.borrow(),
            [
                (key(0xe0), true),
                (key(0xe1), true),
                (key(0x1d), true),
                (key(0x1d), false),
                (key(0xe0), false),
                (key(0xe1), false),
            ]
        );
    }

    #[test]
    fn two_buttons_sharing_a_key_press_and_release_it_once() {
        let escape = ButtonAction::Keys(keys::parse_chord("Escape").unwrap());
        let (mut out, log, _) = output(&[escape.clone(), escape], false);
        out.apply(out.wanted(buttons(0b01), true)).unwrap();
        out.apply(out.wanted(buttons(0b11), true)).unwrap();
        out.apply(out.wanted(buttons(0b10), true)).unwrap();
        assert_eq!(*log.borrow(), [(key(0x29), true)]);
        out.apply(out.wanted(buttons(0b00), true)).unwrap();
        assert_eq!(*log.borrow(), [(key(0x29), true), (key(0x29), false)]);
    }

    #[test]
    fn a_failed_press_is_retried_and_a_failed_release_is_not_forgotten() {
        let (mut out, log, fail) = output(&[ButtonAction::Mouse(MouseButton::Right)], false);
        *fail.borrow_mut() = true;
        let wanted = out.wanted(buttons(1), true);
        assert!(out.apply(wanted).is_err());
        assert!(log.borrow().is_empty());
        *fail.borrow_mut() = false;
        out.apply(wanted).unwrap();
        assert_eq!(*log.borrow(), [(right(), true)]);

        *fail.borrow_mut() = true;
        assert!(out.release_all().is_err());
        *fail.borrow_mut() = false;
        assert!(out.release_all().unwrap());
        assert_eq!(*log.borrow(), [(right(), true), (right(), false)]);
        assert!(!out.release_all().unwrap(), "nothing left to release");
    }

    #[test]
    fn release_all_lets_go_of_everything() {
        let (mut out, log, _) = output(&default_pen_buttons(), false);
        out.apply(out.wanted(buttons(0b011), true)).unwrap();
        assert!(out.release_all().unwrap());
        assert_eq!(log.borrow().iter().filter(|(_, p)| !*p).count(), 2);
        // The next report starts from a clean slate.
        out.apply(out.wanted(buttons(0b001), true)).unwrap();
        assert_eq!(log.borrow().last(), Some(&(right(), true)));
    }

    #[test]
    fn pen_output_turns_barrel_buttons_into_pen_bits() {
        let (mut out, log, _) = output(
            &[
                ButtonAction::Barrel(1),
                ButtonAction::Barrel(3),
                ButtonAction::Mouse(MouseButton::Middle),
            ],
            true,
        );
        let wanted = out.wanted(buttons(0b111), true);
        assert_eq!(out.barrel(wanted), 0b101);
        out.apply(wanted).unwrap();
        assert_eq!(
            *log.borrow(),
            [(Action::Mouse(MouseButton::Middle), true)],
            "barrel buttons are not injected as mouse buttons"
        );
        assert_eq!(out.barrel(out.wanted(buttons(0b001), true)), 0b001);
    }

    #[test]
    fn unsupported_actions_are_reported_and_disabled() {
        let help = ButtonAction::Keys(keys::parse_chord("Help").unwrap());
        let (mut out, rejected) = ButtonOutput::new(
            &[help, ButtonAction::Mouse(MouseButton::Left)],
            false,
            sink(&Log::default(), &Rc::new(RefCell::new(false))),
        );
        assert_eq!(rejected.len(), 1);
        assert!(rejected[0].contains("pen button 1"), "{}", rejected[0]);
        out.apply(out.wanted(buttons(0b01), true)).unwrap();
    }

    #[test]
    fn buttons_beyond_the_configured_ones_do_nothing() {
        let (mut out, log, _) = output(&[ButtonAction::Mouse(MouseButton::Right)], false);
        out.apply(out.wanted(buttons(0b110), true)).unwrap();
        assert!(log.borrow().is_empty());
    }
}
