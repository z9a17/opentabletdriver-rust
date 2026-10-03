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

/// What one wheel, ring or dial does: upstream's `WheelBindingSettings`.
/// Each crossing of a rotation threshold presses and releases its action
/// once; a wheel button is held like an express key.
#[derive(Clone, Debug, PartialEq)]
pub struct WheelBinding {
    pub clockwise: ButtonAction,
    pub counter_clockwise: ButtonAction,
    /// Degrees of rotation per activation. `None` is one device step, the
    /// value upstream's `SetupWheelDefaults` writes into a new profile.
    pub clockwise_threshold: Option<f32>,
    pub counter_clockwise_threshold: Option<f32>,
    /// One action per wheel button, button 1 first.
    pub buttons: Vec<ButtonAction>,
}

impl Default for WheelBinding {
    fn default() -> Self {
        Self {
            clockwise: ButtonAction::None,
            counter_clockwise: ButtonAction::None,
            clockwise_threshold: None,
            counter_clockwise_threshold: None,
            buttons: Vec::new(),
        }
    }
}

impl WheelBinding {
    /// Whether this wheel does anything.
    pub fn is_unbound(&self) -> bool {
        self.clockwise == ButtonAction::None
            && self.counter_clockwise == ButtonAction::None
            && self.buttons.iter().all(|action| *action == ButtonAction::None)
    }
}

/// Upstream forces a threshold that is not positive to one degree
/// (`BindingSettings.VerifyWheelActivationThreshold`).
pub fn wheel_threshold(configured: Option<f32>, degrees_per_step: f64) -> f64 {
    match configured {
        Some(value) if value.is_finite() && value > 0.0 => f64::from(value),
        Some(_) => 1.0,
        None => degrees_per_step,
    }
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

/// One configured button, resolved for the output in use.
struct Slot {
    /// Held through the sink.
    actions: Box<[Action]>,
    /// The pen's barrel button number (bit 0 is button 1), for pen output.
    barrel: u8,
}

/// The binding owner IDs of each group in the shared action state. Two
/// groups holding the same key press it once and release it once.
const AUX_OWNERS: u32 = 64;
const WHEEL_BUTTON_OWNERS: u32 = 128;
const WHEEL_ROTATION_OWNERS: u32 = WHEEL_BUTTON_OWNERS + 64 * crate::reports::MAX_WHEELS as u32;

/// The buttons of one kind (pen, express keys or one wheel's buttons).
#[derive(Default)]
struct Group {
    slots: Vec<Slot>,
    /// Buttons currently wanted down, by index.
    held: u64,
    /// Owner ID of index 0.
    owners: u32,
}

/// One wheel's rotation: upstream's `WheelBindings` with a
/// `DeltaThresholdBindingState` per direction.
struct Rotation {
    clockwise: Box<[Action]>,
    counter_clockwise: Box<[Action]>,
    clockwise_threshold: f64,
    counter_clockwise_threshold: f64,
    steps: u32,
    degrees_per_step: f64,
    last: Option<u32>,
    clockwise_delta: f64,
    counter_clockwise_delta: f64,
}

impl Rotation {
    fn reset(&mut self) {
        self.last = None;
        self.clockwise_delta = 0.0;
        self.counter_clockwise_delta = 0.0;
    }

    /// Steps between two absolute readings, the short way round
    /// (upstream's `ComputeAbsoluteWheelDelta`): with 72 steps, 71 to 1 is 2.
    fn absolute_delta(&self, from: u32, to: u32) -> i32 {
        let steps = f64::from(self.steps);
        let delta = (f64::from(to) - f64::from(from) + steps * 1.5) % steps - steps / 2.0;
        delta as i32
    }
}

/// Follows the pen buttons, express keys and wheels of a session's reports
/// and presses their actions.
pub struct ButtonOutput {
    pen: Group,
    aux: Group,
    wheel_buttons: Vec<Group>,
    rotations: Vec<Rotation>,
    sink: Box<dyn ActionSink>,
    /// A flush failed, so the sink still differs from the wanted state.
    unsettled: bool,
}

/// The sink actions of `binding`. `pen` turns barrel buttons into the pen
/// device's own buttons (returned in `barrel`) instead of mouse clicks.
fn resolve(
    binding: &ButtonAction,
    pen: bool,
    barrel: &mut u8,
    sink: &dyn ActionSink,
    what: &dyn Fn() -> String,
    rejected: &mut Vec<String>,
) -> Box<[Action]> {
    let actions: Vec<Action> = match binding {
        ButtonAction::None => Vec::new(),
        ButtonAction::Barrel(number @ 1..=MAX_BARREL) if pen => {
            *barrel = 1 << (number - 1);
            Vec::new()
        }
        ButtonAction::Barrel(1) => vec![Action::Mouse(MouseButton::Right)],
        ButtonAction::Barrel(2) => vec![Action::Mouse(MouseButton::Middle)],
        ButtonAction::Barrel(_) => Vec::new(),
        ButtonAction::Mouse(button) => vec![Action::Mouse(*button)],
        ButtonAction::Keys(keys) => keys.iter().copied().map(Action::Key).collect(),
    };
    match actions.iter().find(|action| !sink.supports(**action)) {
        Some(action) => {
            rejected.push(format!(
                "{} ({binding}): {} cannot be sent on this platform",
                what(),
                describe(*action)
            ));
            Box::default()
        }
        None => actions.into_boxed_slice(),
    }
}

fn group(
    bindings: &[ButtonAction],
    pen: bool,
    owners: u32,
    sink: &dyn ActionSink,
    name: &dyn Fn(usize) -> String,
    rejected: &mut Vec<String>,
) -> Group {
    let slots = bindings
        .iter()
        .take(64)
        .enumerate()
        .map(|(index, binding)| {
            let mut barrel = 0;
            let actions = resolve(binding, pen, &mut barrel, sink, &|| name(index), rejected);
            Slot { actions, barrel }
        })
        .collect();
    Group {
        slots,
        held: 0,
        owners,
    }
}

impl Group {
    fn wanted(&self, buttons: Buttons) -> u64 {
        let mut wanted = 0;
        for index in 0..self.slots.len().min(buttons.len()) {
            if buttons.get(index) == Some(true) {
                wanted |= 1 << index;
            }
        }
        wanted
    }

    /// Records the holds that reach `wanted`; the caller flushes.
    fn hold(&mut self, sink: &mut dyn ActionSink, wanted: u64) -> io::Result<()> {
        let changed = wanted ^ self.held;
        for index in 0..self.slots.len() {
            let bit = 1u64 << index;
            if changed & bit == 0 {
                continue;
            }
            let down = wanted & bit != 0;
            for action in self.slots[index].actions.iter() {
                sink.hold(self.owners + index as u32, *action, down)?;
            }
            self.held = (self.held & !bit) | (wanted & bit);
        }
        Ok(())
    }
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
        let pen = group(
            bindings,
            pen,
            0,
            sink.as_ref(),
            &|index| format!("pen button {}", index + 1),
            &mut rejected,
        );
        (
            Self {
                pen,
                aux: Group::default(),
                wheel_buttons: Vec::new(),
                rotations: Vec::new(),
                sink,
                unsettled: false,
            },
            rejected,
        )
    }

    /// Adds the express keys and wheels. Barrel buttons on them click the
    /// mouse as on mouse output: the pen device's barrel follows the pen.
    /// `wheels` are the tablet's declared wheels; bindings for wheels it
    /// does not have are ignored, as upstream sizes them to the tablet.
    pub fn set_auxiliary(
        &mut self,
        aux: &[ButtonAction],
        bindings: &[WheelBinding],
        wheels: &[crate::spec::Wheel],
    ) -> Vec<String> {
        let mut rejected = Vec::new();
        let sink = self.sink.as_ref();
        self.aux = group(
            aux,
            false,
            AUX_OWNERS,
            sink,
            &|index| format!("express key {}", index + 1),
            &mut rejected,
        );
        self.wheel_buttons.clear();
        self.rotations.clear();
        for (wheel, (binding, spec)) in bindings.iter().zip(wheels).enumerate() {
            self.wheel_buttons.push(group(
                &binding.buttons,
                false,
                WHEEL_BUTTON_OWNERS + 64 * wheel as u32,
                sink,
                &|index| format!("wheel {} button {}", wheel + 1, index + 1),
                &mut rejected,
            ));
            let mut ignored = 0;
            let mut actions = |action, direction| {
                resolve(
                    action,
                    false,
                    &mut ignored,
                    sink,
                    &|| format!("wheel {} {direction}", wheel + 1),
                    &mut rejected,
                )
            };
            let clockwise = actions(&binding.clockwise, "clockwise");
            let counter_clockwise = actions(&binding.counter_clockwise, "counter-clockwise");
            let degrees_per_step = spec.degrees_per_step().unwrap_or(0.0);
            if degrees_per_step == 0.0 && !(clockwise.is_empty() && counter_clockwise.is_empty()) {
                rejected.push(format!(
                    "wheel {}: the tablet configuration has no step count, so its rotation does nothing",
                    wheel + 1
                ));
            }
            self.rotations.push(Rotation {
                clockwise,
                counter_clockwise,
                clockwise_threshold: wheel_threshold(binding.clockwise_threshold, degrees_per_step),
                counter_clockwise_threshold: wheel_threshold(
                    binding.counter_clockwise_threshold,
                    degrees_per_step,
                ),
                steps: spec.steps,
                degrees_per_step,
                last: None,
                clockwise_delta: 0.0,
                counter_clockwise_delta: 0.0,
            });
        }
        rejected
    }

    /// The pen buttons wanted down after a report. A report without a button
    /// reading leaves them as they are; a pen out of range holds none.
    pub fn wanted(&self, buttons: Option<Buttons>, present: bool) -> u64 {
        if !present {
            return 0;
        }
        match buttons {
            Some(buttons) => self.pen.wanted(buttons),
            None => self.pen.held,
        }
    }

    /// The barrel buttons of a pen device that `wanted` presses.
    pub fn barrel(&self, wanted: u64) -> u8 {
        self.pen
            .slots
            .iter()
            .enumerate()
            .filter(|(index, _)| wanted & (1 << index) != 0)
            .fold(0, |barrel, (_, slot)| barrel | slot.barrel)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.sink.flush()?;
        self.unsettled = false;
        Ok(())
    }

    /// Presses and releases pen buttons to reach `wanted`. Allocation-free,
    /// and does no work when nothing changed.
    pub fn apply(&mut self, wanted: u64) -> io::Result<()> {
        if wanted == self.pen.held && !self.unsettled {
            return Ok(());
        }
        self.unsettled = true;
        self.pen.hold(self.sink.as_mut(), wanted)?;
        self.flush()
    }

    /// Follows the express keys, wheel buttons and wheel rotation of a
    /// report, as upstream's `BindingHandler` does for `IAuxReport`,
    /// `IWheelButtonReport` and the wheel reports. Absent readings change
    /// nothing. Allocation-free.
    pub fn apply_auxiliary(&mut self, values: &crate::reports::ReportValues) -> io::Result<()> {
        use crate::reports::AnalogKind;

        let mut changed = self.unsettled;
        if let Some(buttons) = values.aux_buttons {
            let wanted = self.aux.wanted(buttons);
            if wanted != self.aux.held {
                self.unsettled = true;
                changed = true;
                self.aux.hold(self.sink.as_mut(), wanted)?;
            }
        }
        if let Some(wheels) = &values.wheel_buttons {
            for (group, buttons) in self.wheel_buttons.iter_mut().zip(wheels.as_slice()) {
                let wanted = group.wanted(*buttons);
                if wanted != group.held {
                    self.unsettled = true;
                    changed = true;
                    group.hold(self.sink.as_mut(), wanted)?;
                }
            }
        }
        if changed {
            self.flush()?;
        }
        if let Some(report) = values.absolute_analog.filter(|report| report.kind == AnalogKind::Wheel) {
            for (wheel, position) in report.positions.as_slice().iter().enumerate() {
                let Some(rotation) = self.rotations.get_mut(wheel) else {
                    break;
                };
                let Some(position) = *position else {
                    // The ring was let go: the next touch starts afresh.
                    rotation.reset();
                    continue;
                };
                let Some(last) = rotation.last.replace(position) else {
                    continue;
                };
                if rotation.steps == 0 {
                    continue;
                }
                let delta = rotation.absolute_delta(last, position);
                self.rotate(wheel, delta)?;
            }
        }
        if let Some(report) = values.relative_analog.filter(|report| report.kind == AnalogKind::Wheel) {
            for (wheel, delta) in report.deltas.as_slice().iter().enumerate() {
                if wheel >= self.rotations.len() {
                    break;
                }
                self.rotate(wheel, *delta)?;
            }
        }
        Ok(())
    }

    /// Upstream's `HandleWheelDelta`: a turn accumulates in its direction and
    /// clears the other; no movement clears both.
    fn rotate(&mut self, wheel: usize, steps: i32) -> io::Result<()> {
        let rotation = &mut self.rotations[wheel];
        let degrees = f64::from(steps) * rotation.degrees_per_step;
        let mut taps = 0u32;
        let clockwise = steps > 0;
        match steps {
            0 => {
                rotation.clockwise_delta = 0.0;
                rotation.counter_clockwise_delta = 0.0;
                return Ok(());
            }
            1.. => {
                rotation.counter_clockwise_delta = 0.0;
                rotation.clockwise_delta += degrees;
                while rotation.clockwise_delta >= rotation.clockwise_threshold {
                    rotation.clockwise_delta -= rotation.clockwise_threshold;
                    taps += 1;
                }
            }
            _ => {
                rotation.clockwise_delta = 0.0;
                rotation.counter_clockwise_delta += degrees;
                while rotation.counter_clockwise_delta <= -rotation.counter_clockwise_threshold {
                    rotation.counter_clockwise_delta += rotation.counter_clockwise_threshold;
                    taps += 1;
                }
            }
        }
        let owner = WHEEL_ROTATION_OWNERS + 2 * wheel as u32 + u32::from(!clockwise);
        for _ in 0..taps {
            self.tap(wheel, clockwise, owner)?;
        }
        Ok(())
    }

    /// Presses and releases a rotation action. A failure still leaves it
    /// released in the wanted state, so later reconciliation lets it go.
    fn tap(&mut self, wheel: usize, clockwise: bool, owner: u32) -> io::Result<()> {
        let rotation = &self.rotations[wheel];
        let actions = if clockwise {
            &rotation.clockwise
        } else {
            &rotation.counter_clockwise
        };
        if actions.is_empty() {
            return Ok(());
        }
        self.unsettled = true;
        let mut pressed = Ok(());
        for action in actions.iter() {
            pressed = pressed.and_then(|()| self.sink.hold(owner, *action, true));
        }
        let pressed = pressed.and_then(|()| self.sink.flush().map(|_| ()));
        for action in actions.iter() {
            self.sink.hold(owner, *action, false)?;
        }
        pressed?;
        self.flush()
    }

    /// Releases the express keys and wheel buttons, and forgets the wheel
    /// positions, when the endpoint that reports them is lost. Pen buttons
    /// stay as they are.
    pub fn release_auxiliary(&mut self) -> io::Result<bool> {
        for rotation in &mut self.rotations {
            rotation.reset();
        }
        let held = self.aux.held != 0 || self.wheel_buttons.iter().any(|group| group.held != 0);
        if !held && !self.unsettled {
            return Ok(false);
        }
        self.unsettled = true;
        self.aux.hold(self.sink.as_mut(), 0)?;
        for group in &mut self.wheel_buttons {
            group.hold(self.sink.as_mut(), 0)?;
        }
        let sent = self.sink.flush()?;
        self.unsettled = false;
        Ok(sent != 0)
    }

    /// Releases everything, including what an earlier failure left down.
    /// Returns whether any release was sent.
    pub fn release_all(&mut self) -> io::Result<bool> {
        for rotation in &mut self.rotations {
            rotation.reset();
        }
        let held = self.pen.held != 0
            || self.aux.held != 0
            || self.wheel_buttons.iter().any(|group| group.held != 0);
        if !held && !self.unsettled {
            return Ok(false);
        }
        self.pen.held = 0;
        self.aux.held = 0;
        for group in &mut self.wheel_buttons {
            group.held = 0;
        }
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

    use crate::reports::{
        AbsoluteAnalog, AbsoluteAnalogReport, AnalogKind, RelativeAnalog, RelativeAnalogReport,
        ReportValues, WheelButtons,
    };

    fn page_down() -> Action {
        key(0x4e)
    }

    fn page_up() -> Action {
        key(0x4b)
    }

    /// A PTH-660 (72-step ring) with PageDown clockwise and PageUp
    /// counter-clockwise at `threshold` degrees.
    fn ring_output(threshold: Option<f32>) -> (ButtonOutput, Log, Rc<RefCell<bool>>) {
        let wheel = WheelBinding {
            clockwise: "keys:PageDown".parse().unwrap(),
            counter_clockwise: "keys:PageUp".parse().unwrap(),
            clockwise_threshold: threshold,
            counter_clockwise_threshold: threshold,
            buttons: vec!["keys:Escape".parse().unwrap()],
        };
        let (mut out, log, fail) = output(&default_pen_buttons(), false);
        let rejected = out.set_auxiliary(
            &["mouse:middle".parse().unwrap(), "keys:Control+Z".parse().unwrap()],
            &[wheel],
            crate::spec::TabletSpec::PTH_660.controls.wheels(),
        );
        assert!(rejected.is_empty(), "{rejected:?}");
        (out, log, fail)
    }

    fn aux(bits: u64) -> ReportValues {
        ReportValues {
            aux_buttons: Some(Buttons::from_bits(bits, 8).unwrap()),
            ..ReportValues::default()
        }
    }

    fn ring(position: Option<u32>) -> ReportValues {
        ReportValues {
            absolute_analog: Some(AbsoluteAnalogReport {
                kind: AnalogKind::Wheel,
                positions: AbsoluteAnalog::from_slice(&[position]).unwrap(),
            }),
            ..ReportValues::default()
        }
    }

    fn turn(out: &mut ButtonOutput, positions: &[Option<u32>]) {
        for position in positions {
            out.apply_auxiliary(&ring(*position)).unwrap();
        }
    }

    /// Completed taps of `action`: presses each followed by its release.
    fn taps(log: &Log, action: Action) -> usize {
        let log = log.borrow();
        let mine: Vec<bool> = log
            .iter()
            .filter(|(sent, _)| *sent == action)
            .map(|(_, pressed)| *pressed)
            .collect();
        assert!(
            mine.chunks(2).all(|pair| pair == [true, false]),
            "{action:?} not tapped cleanly: {mine:?}"
        );
        mine.len() / 2
    }

    #[test]
    fn express_keys_are_held_like_buttons_and_ignore_the_pen_leaving() {
        let (mut out, log, _) = ring_output(None);
        out.apply_auxiliary(&aux(0b01)).unwrap();
        out.apply_auxiliary(&aux(0b01)).unwrap();
        assert_eq!(*log.borrow(), [(Action::Mouse(MouseButton::Middle), true)]);
        // The pen leaving range releases pen buttons only.
        out.apply(out.wanted(None, false)).unwrap();
        assert_eq!(log.borrow().len(), 1);
        out.apply_auxiliary(&aux(0b10)).unwrap();
        assert_eq!(
            log.borrow()[1..],
            [
                (Action::Mouse(MouseButton::Middle), false),
                (key(0xe0), true),
                (key(0x1d), true),
            ]
        );
        // A report without an aux reading changes nothing.
        out.apply_auxiliary(&ReportValues::default()).unwrap();
        assert_eq!(log.borrow().len(), 4);
        assert!(out.release_all().unwrap());
        assert_eq!(log.borrow()[4..], [(key(0x1d), false), (key(0xe0), false)]);
    }

    #[test]
    fn each_ring_step_taps_once_and_the_first_touch_only_records_the_position() {
        let (mut out, log, _) = ring_output(None);
        turn(&mut out, &[Some(10), Some(11), Some(12)]);
        assert_eq!(taps(&log, page_down()), 2);
        turn(&mut out, &[Some(11)]);
        assert_eq!(taps(&log, page_up()), 1);
        assert_eq!(taps(&log, page_down()), 2);
    }

    #[test]
    fn the_ring_wraps_around_the_short_way() {
        let (mut out, log, _) = ring_output(None);
        // 71 -> 0 is one step clockwise, not 71 counter-clockwise.
        turn(&mut out, &[Some(70), Some(71), Some(0), Some(1)]);
        assert_eq!(taps(&log, page_down()), 3);
        turn(&mut out, &[Some(0), Some(71)]);
        assert_eq!(taps(&log, page_up()), 2);
    }

    #[test]
    fn slow_turns_accumulate_until_the_threshold_and_letting_go_forgets() {
        // Three steps (15 degrees) per tap.
        let (mut out, log, _) = ring_output(Some(15.0));
        turn(&mut out, &(0..=7).map(Some).collect::<Vec<_>>());
        assert_eq!(taps(&log, page_down()), 2, "7 steps = 35 degrees");
        // Lifting the finger drops the 5 degrees left over.
        turn(&mut out, &[None, Some(8), Some(10)]);
        assert_eq!(taps(&log, page_down()), 2);
        turn(&mut out, &[Some(11)]);
        assert_eq!(taps(&log, page_down()), 3);
    }

    #[test]
    fn turning_back_clears_the_other_direction() {
        let (mut out, log, _) = ring_output(Some(15.0));
        turn(&mut out, &[Some(0), Some(2), Some(1), Some(3)]);
        assert_eq!(taps(&log, page_down()), 0, "10, then cleared, then 10 again");
        assert_eq!(taps(&log, page_up()), 0);
        turn(&mut out, &[Some(4)]);
        assert_eq!(taps(&log, page_down()), 1);
    }

    #[test]
    fn relative_wheels_turn_by_their_reported_steps() {
        let (mut out, log, _) = ring_output(None);
        let wheel = |delta| ReportValues {
            relative_analog: Some(RelativeAnalogReport {
                kind: AnalogKind::Wheel,
                deltas: RelativeAnalog::from_slice(&[delta]).unwrap(),
            }),
            ..ReportValues::default()
        };
        out.apply_auxiliary(&wheel(3)).unwrap();
        out.apply_auxiliary(&wheel(-2)).unwrap();
        assert_eq!((taps(&log, page_down()), taps(&log, page_up())), (3, 2));
    }

    #[test]
    fn generic_analog_channels_are_not_wheels() {
        let (mut out, log, _) = ring_output(None);
        for position in [1, 2, 3] {
            let mut values = ring(Some(position));
            values.absolute_analog.as_mut().unwrap().kind = AnalogKind::Generic;
            out.apply_auxiliary(&values).unwrap();
        }
        assert!(log.borrow().is_empty());
    }

    #[test]
    fn wheel_buttons_are_held_and_losing_the_endpoint_releases_only_its_actions() {
        let (mut out, log, _) = ring_output(None);
        out.apply(out.wanted(buttons(0b001), true)).unwrap();
        let wheel_button = ReportValues {
            wheel_buttons: Some(
                WheelButtons::from_slice(&[Buttons::from_bits(1, 1).unwrap()]).unwrap(),
            ),
            aux_buttons: Some(Buttons::from_bits(1, 8).unwrap()),
            ..ReportValues::default()
        };
        out.apply_auxiliary(&wheel_button).unwrap();
        turn(&mut out, &[Some(5)]);
        assert!(out.release_auxiliary().unwrap());
        let log = log.borrow();
        assert_eq!(
            log[..],
            // One flush sends its transitions in the action state's stable
            // portable order.
            [
                (right(), true),
                (key(0x29), true),
                (Action::Mouse(MouseButton::Middle), true),
                (key(0x29), false),
                (Action::Mouse(MouseButton::Middle), false),
            ],
            "the pen's right button stays down"
        );
        drop(log);
        // The ring position was forgotten: the next reading only records.
        let (mut out, log, _) = ring_output(None);
        turn(&mut out, &[Some(5)]);
        out.release_auxiliary().unwrap();
        turn(&mut out, &[Some(9)]);
        assert_eq!(taps(&log, page_down()), 0);
    }

    #[test]
    fn a_failed_tap_never_leaves_its_key_down() {
        let (mut out, log, fail) = ring_output(None);
        turn(&mut out, &[Some(0)]);
        *fail.borrow_mut() = true;
        assert!(out.apply_auxiliary(&ring(Some(1))).is_err());
        *fail.borrow_mut() = false;
        // The next report reconciles: nothing was sent, nothing is held.
        out.apply_auxiliary(&ring(Some(1))).unwrap();
        assert!(log.borrow().iter().all(|(action, _)| *action != page_down()));
        assert!(!out.release_all().unwrap());
    }

    #[test]
    fn wheels_the_tablet_lacks_and_unsendable_rotations_are_reported() {
        let (mut out, _, _) = output(&[], false);
        let wheel = WheelBinding {
            clockwise: "keys:Help".parse().unwrap(),
            ..WheelBinding::default()
        };
        let rejected = out.set_auxiliary(
            &[],
            &[wheel.clone(), wheel],
            crate::spec::TabletSpec::PTH_660.controls.wheels(),
        );
        assert_eq!(rejected.len(), 1, "the second wheel does not exist: {rejected:?}");
        assert!(rejected[0].starts_with("wheel 1 clockwise"), "{}", rejected[0]);
        let no_steps = [crate::spec::Wheel { steps: 0, buttons: 0 }];
        let rejected = out.set_auxiliary(
            &[],
            &[WheelBinding {
                clockwise: "mouse:left".parse().unwrap(),
                ..WheelBinding::default()
            }],
            &no_steps,
        );
        assert!(rejected[0].contains("no step count"), "{rejected:?}");
    }

    #[test]
    fn auxiliary_reports_allocate_nothing() {
        let (mut out, log, _) = ring_output(None);
        let reports: Vec<ReportValues> = (0..72)
            .map(|step| {
                let mut values = ring(Some(step));
                values.aux_buttons = Some(Buttons::from_bits(step as u64 % 4, 8).unwrap());
                values
            })
            .collect();
        // Warm the log so recording does not count as the output's allocation.
        log.borrow_mut().reserve(100_000);
        crate::test_alloc::assert_no_allocations(|| {
            for values in &reports {
                out.apply_auxiliary(values).unwrap();
            }
        });
        assert_eq!(taps(&log, page_down()), 71);
    }

    #[test]
    fn non_positive_thresholds_become_one_degree_and_none_is_one_step() {
        assert_eq!(wheel_threshold(Some(0.0), 5.0), 1.0);
        assert_eq!(wheel_threshold(Some(-3.0), 5.0), 1.0);
        assert_eq!(wheel_threshold(Some(f32::NAN), 5.0), 1.0);
        assert_eq!(wheel_threshold(None, 5.0), 5.0);
        assert_eq!(wheel_threshold(Some(30.0), 5.0), 30.0);
    }
}
