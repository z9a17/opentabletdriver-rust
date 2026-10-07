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
use std::time::{Duration, Instant};

use crate::actions::{
    Action, ActionOwner, ActionState, ActionTransition, KeyboardUsage, MouseButton,
};
use crate::keys;
use crate::reports::Buttons;

/// The axes accepted by upstream IMouseScrollHandler.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScrollAxis { Vertical, Horizontal }

/// One native MouseScrollBinding. Amount and interval use the upstream
/// properties: Scroll() sends -Amount, once on press and at every interval.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScrollAction {
    pub axis: ScrollAxis,
    pub amount: i32,
    pub interval_ms: u32,
}

impl ScrollAction {
    pub fn validate(self) -> Result<Self, String> {
        if self.amount == 0 || self.interval_ms == 0 || self.interval_ms > i32::MAX as u32 {
            return Err("scroll amount must be nonzero and interval 1..2147483647 milliseconds".into());
        }
        Ok(self)
    }
    pub fn pulse(self) -> ScrollPulse {
        ScrollPulse { axis: self.axis, delta: self.amount.wrapping_neg() }
    }
}

/// A pointer scroll call, in the upstream pointer's units. Windows treats 120
/// units as a wheel detent. Platform adapters perform their own unit conversion.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScrollPulse { pub axis: ScrollAxis, pub delta: i32 }

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
    Scroll(ScrollAction),
    /// Rust extension: retain a native hold until the next rising edge.
    Toggle(Box<ButtonAction>),
    /// A deferred preset switch; never reads settings inside report dispatch.
    Preset(crate::presets::PresetName),
    /// An unchanged OpenTabletDriver IStateBinding, constructed once per slot.
    Managed(crate::plugins::PluginConfig),
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
            && self
                .buttons
                .iter()
                .all(|action| *action == ButtonAction::None)
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
            "dotnet" => {
                let config: crate::plugins::PluginConfig = serde_json::from_str(value).map_err(|error| format!("invalid managed binding: {error}"))?;
                config.validate()?;
                if config.kind != crate::plugins::PluginKind::Dotnet { return Err("managed bindings require kind = dotnet".into()); }
                Ok(Self::Managed(config))
            }
            "preset" => crate::presets::PresetName::parse(value).map(Self::Preset),
            "toggle" => {
                if !value.split_once(':').is_some_and(|(kind, _)| ["keys", "key", "mouse", "barrel"].iter().any(|valid| kind.eq_ignore_ascii_case(valid))) {
                    return Err("toggle requires a native key/chord, mouse or barrel action".into());
                }
                let action = value.parse::<Self>()?;
                if !matches!(action, Self::Keys(_) | Self::Mouse(_) | Self::Barrel(_)) {
                    return Err("toggle requires a native key/chord, mouse or barrel action".into());
                }
                Ok(Self::Toggle(Box::new(action)))
            }
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
            "scroll" => {
                let (axis, amount, interval_ms) = match value {
                    "up" => (ScrollAxis::Vertical, -120, 300),
                    "down" => (ScrollAxis::Vertical, 120, 300),
                    "left" => (ScrollAxis::Horizontal, 120, 300),
                    "right" => (ScrollAxis::Horizontal, -120, 300),
                    _ => {
                        let fields = value.split(':').collect::<Vec<_>>();
                        if fields.len() != 2 && fields.len() != 3 { return Err("scroll requires up/down/left/right or vertical|horizontal:AMOUNT[:INTERVAL_MS]".into()); }
                        let axis = match fields[0] { "vertical" => ScrollAxis::Vertical, "horizontal" => ScrollAxis::Horizontal, _ => return Err("scroll axis must be vertical or horizontal".into()) };
                        let amount = fields[1].parse::<i32>().map_err(|_| "scroll amount must be a signed 32-bit integer")?;
                        let interval_ms = if fields.len() == 3 { fields[2].parse::<u32>().map_err(|_| "scroll interval must be a positive millisecond integer")? } else { 300 };
                        (axis, amount, interval_ms)
                    }
                };
                ScrollAction { axis, amount, interval_ms }.validate().map(Self::Scroll)
            }
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
            Self::Keys(keys) => write!(f, "keys:{}", keys::native_chord_text(keys)),
            Self::Scroll(action) => write!(f, "scroll:{}:{}:{}", match action.axis { ScrollAxis::Vertical => "vertical", ScrollAxis::Horizontal => "horizontal" }, action.amount, action.interval_ms),
            Self::Toggle(action) => write!(f, "toggle:{action}"),
            Self::Preset(name) => write!(f, "preset:{}", name.as_str()),
            Self::Managed(config) => write!(f, "dotnet:{}", serde_json::to_string(config).map_err(|_| fmt::Error)?),
        }
    }
}

/// Where held actions become operating-system input. One sink serves one
/// tablet session; the platform decides whether sessions share ownership.
pub trait ActionSink {
    fn inhibited_binding(&self) -> Option<u32> { None }
    fn supports_presets(&self) -> bool { false }
    fn preset(&mut self, _owner: u32, _name: &crate::presets::PresetName) -> io::Result<()> {
        Err(io::Error::new(io::ErrorKind::Unsupported, "preset bindings require a guarded driver control owner"))
    }
    fn has_managed(&self) -> bool { false }
    fn managed_next_tick(&self) -> Option<Duration> { None }
    fn managed_tick(&mut self) -> io::Result<()> { Ok(()) }
    fn supports_managed(&self, _config: &crate::plugins::PluginConfig) -> bool { false }
    fn set_report(&mut self, _kind: crate::reports::ReportKind, _values: &crate::reports::ReportValues, _raw: &[u8]) -> io::Result<()> { Ok(()) }
    fn managed_binding(&mut self, _owner: u32, _config: &crate::plugins::PluginConfig, _pressed: bool) -> io::Result<()> {
        Err(io::Error::new(io::ErrorKind::Unsupported, "unchanged managed bindings are unavailable"))
    }
    /// Whether the platform can inject this action.
    fn supports(&self, action: Action) -> bool;
    fn supports_scroll(&self) -> bool { false }
    /// Emit a pulse; it has no held OS state and bypasses key/button ownership.
    fn scroll(&mut self, _pulse: ScrollPulse) -> io::Result<()> {
        Err(io::Error::new(io::ErrorKind::Unsupported, "pointer scroll output is unavailable"))
    }
    fn next_managed_command(&mut self) -> Option<crate::plugins::ManagedCommand> { None }
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
    supports: Box<dyn Fn(Action) -> bool>,
    scroll: Option<Box<dyn FnMut(ScrollPulse) -> io::Result<()>>>,
}

impl<F: FnMut(ActionTransition) -> io::Result<()>> LocalActions<F> {
    pub fn with_scroll(mut self, scroll: impl FnMut(ScrollPulse) -> io::Result<()> + 'static) -> Self {
        self.scroll = Some(Box::new(scroll));
        self
    }
    pub fn new(send: F, supports: impl Fn(Action) -> bool + 'static) -> Self {
        Self {
            state: Box::default(),
            send,
            supports: Box::new(supports),
            scroll: None,
        }
    }
}

impl<F: FnMut(ActionTransition) -> io::Result<()>> ActionSink for LocalActions<F> {
    fn supports(&self, action: Action) -> bool {
        (self.supports)(action)
    }

    fn supports_scroll(&self) -> bool { self.scroll.is_some() }
    fn scroll(&mut self, pulse: ScrollPulse) -> io::Result<()> {
        match &mut self.scroll {
            Some(scroll) => scroll(pulse),
            None => Err(io::Error::new(io::ErrorKind::Unsupported, "pointer scroll output is unavailable")),
        }
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
    scroll: Option<ScrollAction>,
    next_scroll: Option<Instant>,
    managed: Option<crate::plugins::PluginConfig>,
    toggle: bool,
    active: bool,
    preset: Option<crate::presets::PresetName>,
    blocked: std::cell::Cell<bool>,
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
    clockwise: Slot,
    counter_clockwise: Slot,
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
    inhibition_active: std::cell::Cell<bool>,
    pen: Group,
    contact: Group,
    aux: Group,
    mouse: Group,
    mouse_scroll: Group,
    now: Instant,
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
    scroll: &mut Option<ScrollAction>,
) -> Box<[Action]> {
    let actions: Vec<Action> = match binding {
        ButtonAction::Toggle(action) => {
            if matches!(action.as_ref(), ButtonAction::Keys(_) | ButtonAction::Mouse(_) | ButtonAction::Barrel(1..=3)) {
                return resolve(action, pen, barrel, sink, what, rejected, scroll);
            }
            rejected.push(format!("{}: toggle requires a native key/chord, mouse or barrel action", what()));
            Vec::new()
        }
        ButtonAction::Preset(_) => {
            if !sink.supports_presets() { rejected.push(format!("{}: preset bindings require a guarded driver control owner", what())); }
            Vec::new()
        }
        ButtonAction::None => Vec::new(),
        ButtonAction::Managed(config) => {
            if config.enabled && !sink.supports_managed(config) { rejected.push(format!("{}: unchanged managed binding {} is unavailable", what(), config.type_name)); }
            Vec::new()
        }
        ButtonAction::Scroll(action) => {
            if let Err(error) = action.validate() { rejected.push(format!("{} ({binding}): {error}", what())); }
            else if sink.supports_scroll() { *scroll = Some(*action); }
            else { rejected.push(format!("{} ({binding}): pointer scroll output is unavailable", what())); }
            Vec::new()
        }
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
            let mut scroll = None;
            let actions = resolve(binding, pen, &mut barrel, sink, &|| name(index), rejected, &mut scroll);
            Slot { actions, barrel, scroll, next_scroll: None,
                toggle: matches!(binding, ButtonAction::Toggle(_)), active: false,
                preset: match binding { ButtonAction::Preset(name) if sink.supports_presets() => Some(name.clone()), _ => None },
                blocked: std::cell::Cell::new(sink.inhibited_binding() == Some(owners + index as u32)),
                managed: match binding { ButtonAction::Managed(config) if config.enabled && sink.supports_managed(config) => Some(config.clone()), _ => None } }
        })
        .collect();
    Group {
        slots,
        held: 0,
        owners,
    }
}

impl Group {
    fn observe_buttons(&self, buttons: Option<Buttons>) {
        if let Some(buttons) = buttons {
            for (index, slot) in self.slots.iter().enumerate().take(buttons.len()) {
                if buttons.get(index) == Some(false) { slot.blocked.set(false); }
            }
        }
    }
    fn active(&self) -> bool { self.held != 0 || self.slots.iter().any(|slot| slot.active) }
    /// Cleanup releases latched holds too, even when their physical key is up.
    fn release(&mut self, sink: &mut dyn ActionSink, now: Instant) -> io::Result<()> {
        let result = self.hold(sink, 0, now);
        for (index, slot) in self.slots.iter_mut().enumerate() {
            if slot.toggle && slot.active {
                for action in slot.actions.iter() { sink.hold(self.owners + index as u32, *action, false)?; }
                slot.active = false;
            }
        }
        result
    }
    fn next_tick(&self, now: Instant) -> Option<Duration> {
        self.slots.iter().filter_map(|slot| slot.next_scroll)
            .map(|deadline| deadline.saturating_duration_since(now)).min()
    }
    fn tick(&mut self, sink: &mut dyn ActionSink, now: Instant) -> io::Result<()> {
        for slot in &mut self.slots {
            if slot.next_scroll.is_some_and(|deadline| deadline <= now) {
                let scroll = slot.scroll.expect("a scroll deadline belongs to a scroll action");
                // One pulse, then schedule from now: missed intervals never
                // flood the pointer or starve the input/stop poll.
                slot.next_scroll = None;
                sink.scroll(scroll.pulse())?;
                slot.next_scroll = now.checked_add(Duration::from_millis(u64::from(scroll.interval_ms)));
            }
        }
        Ok(())
    }
    fn clear_timers(&mut self) {
        for slot in &mut self.slots { slot.next_scroll = None; }
    }

    fn wanted(&self, buttons: Buttons) -> u64 {
        let mut wanted = 0;
        for index in 0..self.slots.len().min(buttons.len()) {
            if buttons.get(index) == Some(true) {
                if !self.slots[index].blocked.get() { wanted |= 1 << index; }
            }
        }
        wanted
    }

    /// Records the holds that reach `wanted`; the caller flushes.
    fn hold(&mut self, sink: &mut dyn ActionSink, wanted: u64, now: Instant) -> io::Result<()> {
        let changed = wanted ^ self.held;
        for index in 0..self.slots.len() {
            let bit = 1u64 << index;
            if changed & bit == 0 {
                continue;
            }
            let down = wanted & bit != 0;
            let slot = &mut self.slots[index];
            if let Some(name) = &slot.preset { if down { sink.preset(self.owners + index as u32, name)?; } }
            if slot.toggle && !down { self.held &= !bit; continue; }
            let output_down = if slot.toggle { !slot.active } else { down };
            if let Some(config) = &self.slots[index].managed {
                sink.managed_binding(self.owners + index as u32, config, down)?;
            }
            for action in self.slots[index].actions.iter() {
                sink.hold(self.owners + index as u32, *action, output_down)?;
            }
            self.slots[index].active = output_down;
            if let Some(scroll) = self.slots[index].scroll {
                if down {
                    sink.flush()?;
                    sink.scroll(scroll.pulse())?;
                    self.slots[index].next_scroll = now.checked_add(Duration::from_millis(u64::from(scroll.interval_ms)));
                } else { self.slots[index].next_scroll = None; }
            }
            self.held = (self.held & !bit) | (wanted & bit);
        }
        Ok(())
    }
}

impl ButtonOutput {
    /// Stable per-slot identities used by the managed host before input begins.
    pub fn managed_slots(profile: &crate::config::Profile) -> Vec<(u32, crate::plugins::PluginConfig)> {
        let mut slots = Vec::new();
        for (owner, config) in [(1024, &profile.managed_tip_binding), (1025, &profile.managed_eraser_binding)] { if let Some(config) = config.as_ref().filter(|config| config.enabled) { slots.push((owner, config.clone())); } }
        let mut add = |owner, action: &ButtonAction| { if let ButtonAction::Managed(config) = action { if config.enabled { slots.push((owner, config.clone())); } } };
        for (base, actions) in [(0, profile.pen_buttons.as_slice()), (AUX_OWNERS, profile.aux_buttons.as_slice()), (WHEEL_ROTATION_OWNERS + 2 * crate::reports::MAX_WHEELS as u32, profile.mouse_buttons.as_slice())] {
            for (index, action) in actions.iter().take(64).enumerate() { add(base + index as u32, action); }
        }
        let scroll = WHEEL_ROTATION_OWNERS + 2 * crate::reports::MAX_WHEELS as u32 + 64;
        add(scroll, &profile.mouse_scroll_down); add(scroll + 1, &profile.mouse_scroll_up);
        for (wheel, bindings) in profile.wheels.iter().take(crate::reports::MAX_WHEELS).enumerate() {
            for (index, action) in bindings.buttons.iter().take(64).enumerate() { add(WHEEL_BUTTON_OWNERS + 64 * wheel as u32 + index as u32, action); }
            add(WHEEL_ROTATION_OWNERS + 2 * wheel as u32, &bindings.clockwise);
            add(WHEEL_ROTATION_OWNERS + 2 * wheel as u32 + 1, &bindings.counter_clockwise);
        }
        slots
    }
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
                inhibition_active: std::cell::Cell::new(sink.inhibited_binding().is_some()),
                pen,
                contact: Group::default(),
                aux: Group::default(),
                mouse: Group::default(),
                mouse_scroll: Group::default(),
                now: Instant::now(),
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
                let mut scroll = None;
                let actions = resolve(action, false, &mut ignored, sink,
                    &|| format!("wheel {} {direction}", wheel + 1), &mut rejected, &mut scroll);
                Slot { actions, barrel: 0, scroll, next_scroll: None,
                    toggle: matches!(action, ButtonAction::Toggle(_)), active: false,
                    preset: match action { ButtonAction::Preset(name) if sink.supports_presets() => Some(name.clone()), _ => None },
                    blocked: std::cell::Cell::new(false),
                    managed: match action { ButtonAction::Managed(config) if config.enabled && sink.supports_managed(config) => Some(config.clone()), _ => None } }
            };
            let clockwise = actions(&binding.clockwise, "clockwise");
            let counter_clockwise = actions(&binding.counter_clockwise, "counter-clockwise");
            let degrees_per_step = spec.degrees_per_step().unwrap_or(0.0);
            if degrees_per_step == 0.0 && !(clockwise.actions.is_empty() && counter_clockwise.actions.is_empty() && clockwise.scroll.is_none() && counter_clockwise.scroll.is_none() && clockwise.preset.is_none() && counter_clockwise.preset.is_none()) {
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
    pub fn wanted_with_pressure(&self, buttons: Option<Buttons>, present: bool, pressure: Option<u32>, drag_only: bool) -> u64 {
        let wanted = self.wanted(buttons, present);
        if drag_only && !pressure.is_some_and(|pressure| pressure > 0) {
            // BindingState retains a press when pressure drops: only gate rising edges.
            wanted & self.pen.held
        } else { wanted }
    }

    pub fn set_contact_bindings(&mut self, tip: Option<&crate::plugins::PluginConfig>, eraser: Option<&crate::plugins::PluginConfig>) -> Vec<String> {
        let mut rejected = Vec::new();
        let actions = [tip.map_or(ButtonAction::None, |config| ButtonAction::Managed(config.clone())), eraser.map_or(ButtonAction::None, |config| ButtonAction::Managed(config.clone()))];
        self.contact = group(&actions, false, 1024, self.sink.as_ref(), &|index| if index == 0 { "tip binding".into() } else { "eraser binding".into() }, &mut rejected);
        rejected
    }
    pub fn apply_contact(&mut self, eraser: bool, pressed: bool) -> io::Result<()> {
        if self.contact.slots.is_empty() { return Ok(()); }
        if self.contact.slots[usize::from(eraser)].blocked.get() { return Ok(()); }
        let bit = 1u64 << u32::from(eraser);
        let wanted = (self.contact.held & !bit) | if pressed { bit } else { 0 };
        if wanted == self.contact.held && !self.unsettled { return Ok(()); }
        self.unsettled = true; self.contact.hold(self.sink.as_mut(), wanted, self.now)?; self.flush()
    }
    pub fn set_time(&mut self, now: Instant) { self.now = now; }
    /// Observe untouched source capabilities before any filter/remap. A changed
    /// threshold or a filter changing buttons must not arm a still-held source.
    pub fn observe_source(&self, values: &crate::reports::ReportValues) {
        if !self.inhibition_active.get() { return; }
        self.pen.observe_buttons(values.pen_buttons);
        self.aux.observe_buttons(values.aux_buttons);
        self.mouse.observe_buttons(values.mouse_buttons);
        if let Some([_, y]) = values.mouse_scroll {
            let bits = u64::from(y < 0.0) | (u64::from(y > 0.0) << 1);
            if let Ok(buttons) = Buttons::from_bits(bits, 2) { self.mouse_scroll.observe_buttons(Some(buttons)); }
        }
        if let Some(wheels) = values.wheel_buttons {
            for (group, buttons) in self.wheel_buttons.iter().zip(wheels.as_slice()) { group.observe_buttons(Some(*buttons)); }
        }
        if values.pressure == Some(0) {
            for slot in &self.contact.slots { slot.blocked.set(false); }
        }
        let remaining = [&self.pen, &self.contact, &self.aux, &self.mouse, &self.mouse_scroll].into_iter()
            .chain(self.wheel_buttons.iter()).any(|group| group.slots.iter().any(|slot| slot.blocked.get()));
        self.inhibition_active.set(remaining);
    }
    pub fn set_report(&mut self, kind: crate::reports::ReportKind, values: &crate::reports::ReportValues, raw: &[u8]) -> io::Result<()> { self.sink.set_report(kind, values, raw) }
    pub fn next_managed_command(&mut self) -> Option<crate::plugins::ManagedCommand> { self.sink.next_managed_command() }
    pub fn managed_hold(&mut self, owner: u32, action: Action, held: bool) -> io::Result<()> { self.sink.hold(owner, action, held) }
    pub fn managed_scroll(&mut self, pulse: ScrollPulse) -> io::Result<()> { self.sink.scroll(pulse) }
    pub fn managed_flush(&mut self) -> io::Result<usize> { self.sink.flush() }

    pub fn set_mouse_scroll(&mut self, up: &ButtonAction, down: &ButtonAction) -> Vec<String> {
        let mut rejected = Vec::new();
        self.mouse_scroll = group(&[down.clone(), up.clone()], false,
            WHEEL_ROTATION_OWNERS + 2 * crate::reports::MAX_WHEELS as u32 + 64,
            self.sink.as_ref(), &|index| if index == 0 { "mouse scroll down".into() } else { "mouse scroll up".into() }, &mut rejected);
        rejected
    }

    pub fn next_tick(&self, now: Instant) -> Option<Duration> {
        [&self.pen, &self.contact, &self.aux, &self.mouse, &self.mouse_scroll].into_iter()
            .chain(self.wheel_buttons.iter()).filter_map(|group| group.next_tick(now)).chain(self.sink.managed_next_tick()).min()
    }

    pub fn tick(&mut self, now: Instant) -> io::Result<()> {
        self.now = now;
        self.sink.managed_tick()?;
        for group in [&mut self.pen, &mut self.contact, &mut self.aux, &mut self.mouse, &mut self.mouse_scroll].into_iter().chain(self.wheel_buttons.iter_mut()) {
            group.tick(self.sink.as_mut(), now)?;
        }
        Ok(())
    }

    pub fn set_mouse(&mut self, bindings: &[ButtonAction]) -> Vec<String> {
        let mut rejected = Vec::new();
        self.mouse = group(bindings, false, WHEEL_ROTATION_OWNERS + 2 * crate::reports::MAX_WHEELS as u32, self.sink.as_ref(), &|index| format!("mouse button {}", index + 1), &mut rejected);
        rejected
    }

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
            .filter(|(index, slot)| if slot.toggle {
                if wanted & (1 << index) != 0 && self.pen.held & (1 << index) == 0 { !slot.active } else { slot.active }
            } else { wanted & (1 << index) != 0 })
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
        self.pen.hold(self.sink.as_mut(), wanted, self.now)?;
        self.flush()
    }

    /// Follows the express keys, wheel buttons and wheel rotation of a
    /// report, as upstream's `BindingHandler` does for `IAuxReport`,
    /// `IWheelButtonReport` and the wheel reports. Absent readings change
    /// nothing. Allocation-free.
    pub fn apply_auxiliary(&mut self, values: &crate::reports::ReportValues) -> io::Result<()> {
        use crate::reports::AnalogKind;

        let mut changed = self.unsettled;
        if let Some(buttons) = values.mouse_buttons {
            let wanted = self.mouse.wanted(buttons);
            if wanted != self.mouse.held {
                self.unsettled = true;
                changed = true;
                self.mouse.hold(self.sink.as_mut(), wanted, self.now)?;
            }
        }
        // BindingHandler evaluates Scroll.Y on IMouseReport only; an absent
        // mouse report preserves state. Repeated signs do not press again.
        if values.mouse_buttons.is_some() {
            let y = values.mouse_scroll.map_or(0.0, |scroll| scroll[1]);
            let wanted = u64::from(y < 0.0) | (u64::from(y > 0.0) << 1);
            if wanted != self.mouse_scroll.held {
                self.unsettled = true;
                changed = true;
                self.mouse_scroll.hold(self.sink.as_mut(), wanted, self.now)?;
            }
        }
        if let Some(buttons) = values.aux_buttons {
            let wanted = self.aux.wanted(buttons);
            if wanted != self.aux.held {
                self.unsettled = true;
                changed = true;
                self.aux.hold(self.sink.as_mut(), wanted, self.now)?;
            }
        }
        if let Some(wheels) = &values.wheel_buttons {
            for (group, buttons) in self.wheel_buttons.iter_mut().zip(wheels.as_slice()) {
                let wanted = group.wanted(*buttons);
                if wanted != group.held {
                    self.unsettled = true;
                    changed = true;
                    group.hold(self.sink.as_mut(), wanted, self.now)?;
                }
            }
        }
        if changed {
            self.flush()?;
        }
        if let Some(report) = values
            .absolute_analog
            .filter(|report| report.kind == AnalogKind::Wheel)
        {
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
        if let Some(report) = values
            .relative_analog
            .filter(|report| report.kind == AnalogKind::Wheel)
        {
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
        let rotation = &mut self.rotations[wheel];
        let slot = if clockwise { &mut rotation.clockwise } else { &mut rotation.counter_clockwise };
        if let Some(name) = &slot.preset { self.sink.preset(owner, name)?; }
        if slot.toggle {
            self.unsettled = true;
            let down = !slot.active;
            for action in slot.actions.iter() { self.sink.hold(owner, *action, down)?; }
            slot.active = down;
            return self.flush();
        }
        if let Some(scroll) = slot.scroll {
            self.sink.flush()?;
            self.sink.scroll(scroll.pulse())?;
        }
        if let Some(config) = &slot.managed {
            self.unsettled = true;
            let pressed = self.sink.managed_binding(owner, config, true);
            let released = self.sink.managed_binding(owner, config, false);
            pressed?; released?;
        }
        let actions = &slot.actions;
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
        let held = self.aux.active() || self.wheel_buttons.iter().any(Group::active)
            || self.rotations.iter().any(|rotation| rotation.clockwise.active || rotation.counter_clockwise.active);
        if !held && !self.unsettled {
            return Ok(false);
        }
        self.unsettled = true;
        self.aux.release(self.sink.as_mut(), self.now)?;
        for group in &mut self.wheel_buttons {
            group.release(self.sink.as_mut(), self.now)?;
        }
        for (wheel, rotation) in self.rotations.iter_mut().enumerate() {
            for (direction, slot) in [&mut rotation.clockwise, &mut rotation.counter_clockwise].into_iter().enumerate() {
                for action in slot.actions.iter() { self.sink.hold(WHEEL_ROTATION_OWNERS + 2 * wheel as u32 + direction as u32, *action, false)?; }
                slot.active = false;
            }
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
        for group in [&mut self.pen, &mut self.contact, &mut self.aux, &mut self.mouse, &mut self.mouse_scroll].into_iter().chain(self.wheel_buttons.iter_mut()) { group.clear_timers(); }
        let held = [&self.contact, &self.mouse_scroll, &self.mouse, &self.pen, &self.aux].into_iter().any(|group| group.active())
            || self.wheel_buttons.iter().any(Group::active)
            || self.rotations.iter().any(|rotation| rotation.clockwise.active || rotation.counter_clockwise.active);
        if !held && !self.unsettled && !self.sink.has_managed() {
            return Ok(false);
        }
        // Plugin Release must run before host-wide input ownership is dropped.
        // Continue native cleanup even if a managed release throws.
        let mut binding_error = None;
        for group in [&mut self.pen, &mut self.contact, &mut self.aux, &mut self.mouse, &mut self.mouse_scroll].into_iter().chain(self.wheel_buttons.iter_mut()) {
            if let Err(error) = group.release(self.sink.as_mut(), self.now) { if binding_error.is_none() { binding_error = Some(error); } }
        }
        self.pen.held = 0;
        self.contact.held = 0;
        self.mouse.held = 0;
        self.mouse_scroll.held = 0;
        self.aux.held = 0;
        for group in &mut self.wheel_buttons {
            group.held = 0;
        }
        self.unsettled = true;
        let sent = self.sink.release_all()?;
        for group in [&mut self.pen, &mut self.contact, &mut self.aux, &mut self.mouse, &mut self.mouse_scroll].into_iter().chain(self.wheel_buttons.iter_mut()) {
            for slot in &mut group.slots { slot.active = false; }
        }
        for rotation in &mut self.rotations { rotation.clockwise.active = false; rotation.counter_clockwise.active = false; }
        self.unsettled = false;
        if let Some(error) = binding_error { return Err(error); }
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

    #[test]
    fn unchanged_binding_slots_keep_distinct_owners_and_release_once() {
        #[derive(Default)] struct ManagedLog(Rc<RefCell<Vec<(u32, bool)>>>);
        impl ActionSink for ManagedLog {
            fn supports(&self, _: Action) -> bool { true }
            fn supports_managed(&self, _: &crate::plugins::PluginConfig) -> bool { true }
            fn managed_binding(&mut self, owner: u32, _: &crate::plugins::PluginConfig, pressed: bool) -> io::Result<()> { self.0.borrow_mut().push((owner, pressed)); Ok(()) }
            fn hold(&mut self, _: u32, _: Action, _: bool) -> io::Result<()> { Ok(()) }
            fn flush(&mut self) -> io::Result<usize> { Ok(0) }
            fn release_all(&mut self) -> io::Result<usize> { Ok(0) }
        }
        let config = crate::plugins::PluginConfig { path: "Original.dll".into(), kind: crate::plugins::PluginKind::Dotnet, enabled: true,
            type_name: "Original.Binding".into(), settings_json: r#"{"MissingIsNotNull":null,"Unknown":123}"#.into() };
        let action = ButtonAction::Managed(config.clone());
        assert_eq!(action.to_string().parse::<ButtonAction>().unwrap(), action);
        let log = ManagedLog::default(); let records = Rc::clone(&log.0);
        let (mut buttons, rejected) = ButtonOutput::new(&[action.clone(), action.clone()], false, Box::new(log));
        assert!(rejected.is_empty());
        buttons.apply(3).unwrap(); buttons.apply(3).unwrap(); buttons.apply(1).unwrap(); buttons.release_all().unwrap(); buttons.release_all().unwrap();
        assert_eq!(*records.borrow(), [(0, true), (1, true), (1, false), (0, false)]);
        let profile = crate::config::Profile { pen_buttons: vec![action.clone()], aux_buttons: vec![action.clone()], mouse_buttons: vec![action], ..Default::default() };
        let slots = ButtonOutput::managed_slots(&profile);
        assert_eq!(slots.len(), 3);
        assert_ne!(slots[0].0, slots[1].0); assert_ne!(slots[1].0, slots[2].0);
    }

    type Log = Rc<RefCell<Vec<(Action, bool)>>>;

    #[test]
    fn toggled_hold_shares_ownership_and_retries_failed_cleanup() {
        let (mut out, log, fail) = output(&["toggle:keys:Z".parse().unwrap(), "keys:Z".parse().unwrap()], false);
        for state in [1, 1, 0, 2, 3, 2, 0, 1, 0] { out.apply(state).unwrap(); }
        let key = "keys:Z".parse::<ButtonAction>().unwrap();
        let ButtonAction::Keys(keys) = key else { unreachable!() };
        assert_eq!(*log.borrow(), [(Action::Key(keys[0]), true), (Action::Key(keys[0]), false), (Action::Key(keys[0]), true)]);
        *fail.borrow_mut() = true;
        assert!(out.release_all().is_err());
        *fail.borrow_mut() = false;
        out.release_all().unwrap();
        out.release_all().unwrap();
        assert_eq!(log.borrow().last(), Some(&(Action::Key(keys[0]), false)));
        assert_eq!(log.borrow().len(), 4);
    }

    #[test]
    fn toggled_barrel_uses_effective_state_and_cleanup_clears_latch() {
        let (mut out, _, _) = output(&["toggle:barrel:1".parse().unwrap()], true);
        assert_eq!(out.barrel(1), 1);
        out.apply(1).unwrap(); out.apply(0).unwrap();
        assert_eq!(out.barrel(0), 1);
        assert_eq!(out.barrel(1), 0);
        out.apply(1).unwrap(); out.apply(0).unwrap();
        assert_eq!(out.barrel(0), 0);
        out.apply(1).unwrap(); out.apply(0).unwrap();
        out.release_all().unwrap();
        assert_eq!(out.barrel(0), 0);
    }

    #[test]
    fn self_switch_source_is_inhibited_until_real_release_without_blocking_other_keys() {
        struct Presets { inner: Box<dyn ActionSink>, requests: Rc<std::cell::Cell<usize>> }
        impl ActionSink for Presets {
            fn inhibited_binding(&self) -> Option<u32> { Some(0) }
            fn supports_presets(&self) -> bool { true }
            fn preset(&mut self, owner: u32, _: &crate::presets::PresetName) -> io::Result<()> {
                assert_eq!(owner, 0); self.requests.set(self.requests.get() + 1); Ok(())
            }
            fn supports(&self, action: Action) -> bool { self.inner.supports(action) }
            fn hold(&mut self, owner: u32, action: Action, held: bool) -> io::Result<()> { self.inner.hold(owner, action, held) }
            fn flush(&mut self) -> io::Result<usize> { self.inner.flush() }
            fn release_all(&mut self) -> io::Result<usize> { self.inner.release_all() }
        }
        let log = Log::default(); let fail = Rc::new(RefCell::new(false));
        let requests = Rc::new(std::cell::Cell::new(0));
        let sink = Presets { inner: sink(&log, &fail), requests: requests.clone() };
        let (mut out, rejected) = ButtonOutput::new(&["preset:Work".parse().unwrap(), "keys:Z".parse().unwrap()], false, Box::new(sink));
        assert!(rejected.is_empty());
        for state in [3, 3, 2, 3, 3] {
            out.observe_source(&crate::reports::ReportValues { pen_buttons: buttons(state), ..Default::default() });
            out.apply(out.wanted(buttons(state), true)).unwrap();
        }
        assert_eq!(requests.get(), 1);
        assert_eq!(log.borrow().len(), 1, "unrelated key is still held exactly once");
        out.release_all().unwrap();
        assert_eq!(log.borrow().len(), 2);
    }

    #[test]
    fn native_toggle_edges_and_cleanup_allocate_nothing_after_setup() {
        let sink = Box::new(LocalActions::new(|_| Ok(()), |_| true));
        let (mut out, rejected) = ButtonOutput::new(&["toggle:keys:Control+Z".parse().unwrap()], false, sink);
        assert!(rejected.is_empty());
        crate::test_alloc::assert_no_allocations(|| {
            for state in [1, 1, 0, 1, 0, 1, 0] { out.apply(state).unwrap(); }
            out.release_all().unwrap();
        });
    }

    #[test]
    fn contact_inhibition_requires_raw_zero_not_absence_or_a_new_threshold() {
        struct Contact { calls: Rc<RefCell<Vec<bool>>> }
        impl ActionSink for Contact {
            fn inhibited_binding(&self) -> Option<u32> { Some(1024) }
            fn supports(&self, _: Action) -> bool { true }
            fn supports_managed(&self, _: &crate::plugins::PluginConfig) -> bool { true }
            fn managed_binding(&mut self, owner: u32, _: &crate::plugins::PluginConfig, pressed: bool) -> io::Result<()> {
                assert_eq!(owner, 1024); self.calls.borrow_mut().push(pressed); Ok(())
            }
            fn hold(&mut self, _: u32, _: Action, _: bool) -> io::Result<()> { Ok(()) }
            fn flush(&mut self) -> io::Result<usize> { Ok(0) }
            fn release_all(&mut self) -> io::Result<usize> { Ok(0) }
        }
        let calls = Rc::new(RefCell::new(Vec::new()));
        let (mut out, _) = ButtonOutput::new(&[], false, Box::new(Contact { calls: calls.clone() }));
        let config = crate::plugins::PluginConfig { path: "Original.dll".into(), type_name: "Original.PresetBinding".into(),
            kind: crate::plugins::PluginKind::Dotnet, enabled: true, settings_json: "{}".into() };
        assert!(out.set_contact_bindings(Some(&config), None).is_empty());
        for pressure in [Some(100), None, Some(20)] {
            out.observe_source(&crate::reports::ReportValues { pressure, ..Default::default() });
            out.apply_contact(false, false).unwrap();
            out.apply_contact(false, true).unwrap();
        }
        assert!(calls.borrow().is_empty());
        out.observe_source(&crate::reports::ReportValues { pressure: Some(0), ..Default::default() });
        out.apply_contact(false, false).unwrap();
        out.apply_contact(false, true).unwrap();
        out.release_all().unwrap();
        assert_eq!(*calls.borrow(), [true, false]);
    }

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
            &[
                "mouse:middle".parse().unwrap(),
                "keys:Control+Z".parse().unwrap(),
            ],
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
        assert_eq!(
            taps(&log, page_down()),
            0,
            "10, then cleared, then 10 again"
        );
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
        assert!(
            log.borrow()
                .iter()
                .all(|(action, _)| *action != page_down())
        );
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
        assert_eq!(
            rejected.len(),
            1,
            "the second wheel does not exist: {rejected:?}"
        );
        assert!(
            rejected[0].starts_with("wheel 1 clockwise"),
            "{}",
            rejected[0]
        );
        let no_steps = [crate::spec::Wheel {
            steps: 0,
            buttons: 0,
        }];
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
    #[test]
    fn drag_only_gates_rising_edge_but_retains_hold_when_pressure_drops() {
        let (mut out, log, _) = output(&[ButtonAction::Mouse(MouseButton::Right)], false);
        for (bits, pressure, expected) in [(1, Some(0), 0), (1, Some(1), 1), (1, Some(0), 1), (1, None, 1), (0, Some(0), 0)] {
            let wanted = out.wanted_with_pressure(buttons(bits), true, pressure, true);
            assert_eq!(wanted, expected);
            out.apply(wanted).unwrap();
        }
        assert_eq!(&*log.borrow(), &[(right(), true), (right(), false)]);
    }

    #[test]
    fn mouse_group_owns_shared_actions_independently_and_cleanup_releases_them() {
        let (mut out, log, _) = output(&[ButtonAction::Mouse(MouseButton::Right)], false);
        assert!(out.set_mouse(&[ButtonAction::Mouse(MouseButton::Right)]).is_empty());
        out.apply(1).unwrap();
        let mut values = crate::reports::ReportValues::default();
        values.mouse_buttons = buttons(1);
        out.apply_auxiliary(&values).unwrap();
        out.apply(0).unwrap();
        assert_eq!(&*log.borrow(), &[(right(), true)]);
        assert!(!out.release_auxiliary().unwrap());
        assert!(out.release_all().unwrap());
        assert_eq!(&*log.borrow(), &[(right(), true), (right(), false)]);
    }

    #[test]
    fn mouse_and_drag_dispatch_allocate_nothing_after_setup() {
        let action: ButtonAction = "keys:Control+Z".parse().unwrap();
        let sink = Box::new(LocalActions::new(|_| Ok(()), |_| true));
        let (mut out, rejected) = ButtonOutput::new(&[action.clone()], false, sink);
        assert!(rejected.is_empty());
        assert!(out.set_mouse(&[action]).is_empty());
        crate::test_alloc::assert_no_allocations(|| {
            for bits in [1, 0, 1, 0] {
                let wanted = out.wanted_with_pressure(buttons(bits), true, Some(1), true);
                out.apply(wanted).unwrap();
                out.apply_auxiliary(&crate::reports::ReportValues { mouse_buttons: buttons(bits), ..Default::default() }).unwrap();
            }
            out.release_all().unwrap();
        });
    }

    #[test]
    fn scroll_actions_parse_signed_amounts_intervals_and_round_trip() {
        for text in ["scroll:up", "scroll:down", "scroll:left", "scroll:right", "scroll:vertical:-45:25", "scroll:horizontal:240"] {
            let action = text.parse::<ButtonAction>().unwrap();
            assert_eq!(action.to_string().parse::<ButtonAction>().unwrap(), action);
        }
        for text in ["scroll:vertical:0", "scroll:vertical:1:0", "scroll:vertical:1:2147483648", "scroll:vertical:2147483648", "scroll:diagonal:1"] {
            assert!(text.parse::<ButtonAction>().is_err(), "{text}");
        }
    }

    #[test]
    fn scroll_direction_edges_repeat_without_catching_up_and_release_cancels() {
        let pulses = Rc::new(RefCell::new(Vec::new()));
        let log = Rc::clone(&pulses);
        let sink = Box::new(LocalActions::new(|_| Ok(()), |_| true).with_scroll(move |pulse| { log.borrow_mut().push(pulse); Ok(()) }));
        let (mut out, _) = ButtonOutput::new(&[], false, sink);
        out.set_mouse_scroll(&"scroll:vertical:-120:10".parse().unwrap(), &"scroll:vertical:120:10".parse().unwrap());
        let start = Instant::now();
        out.set_time(start);
        let mut report = crate::reports::ReportValues { mouse_buttons: buttons(0), mouse_scroll: Some([0.0, 1.0]), ..Default::default() };
        out.apply_auxiliary(&report).unwrap();
        out.apply_auxiliary(&report).unwrap();
        assert_eq!(pulses.borrow().len(), 1, "same sign does not press again");
        assert_eq!(out.next_tick(start + Duration::from_millis(5)), Some(Duration::from_millis(5)));
        out.tick(start + Duration::from_millis(35)).unwrap();
        assert_eq!(pulses.borrow().len(), 2, "three missed periods produce one pulse");
        assert_eq!(out.next_tick(start + Duration::from_millis(35)), Some(Duration::from_millis(10)));
        assert!(!out.release_auxiliary().unwrap(), "mouse scroll belongs to the mouse stream");
        report.mouse_scroll = Some([0.0, -1.0]);
        out.set_time(start + Duration::from_millis(36));
        out.apply_auxiliary(&report).unwrap();
        assert_eq!(pulses.borrow()[2].delta, -120);
        report.mouse_scroll = Some([0.0, 0.0]);
        out.apply_auxiliary(&report).unwrap();
        assert_eq!(out.next_tick(start), None);
        out.tick(start + Duration::from_secs(2)).unwrap();
        assert_eq!(pulses.borrow().len(), 3);
    }

    #[test]
    fn failed_scroll_repeat_cancels_deadline_and_cleanup_stops_remaining_holds() {
        let fail = Rc::new(RefCell::new(false));
        let flag = Rc::clone(&fail);
        let sink = Box::new(LocalActions::new(|_| Ok(()), |_| true).with_scroll(move |_| {
            if *flag.borrow() { Err(io::Error::other("scroll rejected")) } else { Ok(()) }
        }));
        let (mut out, _) = ButtonOutput::new(&["scroll:vertical:120:5".parse().unwrap()], false, sink);
        let start = Instant::now();
        out.set_time(start);
        out.apply(1).unwrap();
        *fail.borrow_mut() = true;
        assert!(out.tick(start + Duration::from_millis(5)).is_err());
        assert_eq!(out.next_tick(start), None);
        out.release_all().unwrap();
        assert_eq!(out.wanted(None, true), 0);
    }

    #[test]
    fn native_scroll_dispatch_and_timer_tick_allocate_nothing_after_setup() {
        let count = Rc::new(std::cell::Cell::new(0));
        let counted = Rc::clone(&count);
        let sink = Box::new(LocalActions::new(|_| Ok(()), |_| true).with_scroll(move |_| { counted.set(counted.get() + 1); Ok(()) }));
        let (mut out, _) = ButtonOutput::new(&["scroll:vertical:-120:1".parse().unwrap()], false, sink);
        let start = Instant::now();
        crate::test_alloc::assert_no_allocations(|| {
            out.set_time(start);
            out.apply(1).unwrap();
            out.tick(start + Duration::from_millis(1)).unwrap();
            out.apply(0).unwrap();
            out.release_all().unwrap();
        });
        assert_eq!(count.get(), 2);
        assert_eq!(out.next_tick(start), None);
    }

    #[test]
    fn scroll_rotation_taps_do_not_start_repeating_timers() {
        let pulses = Rc::new(RefCell::new(Vec::new()));
        let log = pulses.clone();
        let sink = Box::new(LocalActions::new(|_| Ok(()), |_| true)
            .with_scroll(move |pulse| { log.borrow_mut().push(pulse); Ok(()) }));
        let (mut out, _) = ButtonOutput::new(&[], false, sink);
        let wheel = WheelBinding {
            clockwise: "scroll:horizontal:-240:1".parse().unwrap(),
            counter_clockwise: "scroll:vertical:120:1".parse().unwrap(),
            ..Default::default()
        };
        assert!(out.set_auxiliary(&[], &[wheel], crate::spec::TabletSpec::PTH_660.controls.wheels()).is_empty());
        let steps = |delta| ReportValues {
            relative_analog: Some(RelativeAnalogReport { kind: AnalogKind::Wheel, deltas: RelativeAnalog::from_slice(&[delta]).unwrap() }),
            ..Default::default()
        };
        out.apply_auxiliary(&steps(2)).unwrap();
        out.apply_auxiliary(&steps(-1)).unwrap();
        assert_eq!(*pulses.borrow(), [
            ScrollPulse { axis: ScrollAxis::Horizontal, delta: 240 },
            ScrollPulse { axis: ScrollAxis::Horizontal, delta: 240 },
            ScrollPulse { axis: ScrollAxis::Vertical, delta: -120 },
        ]);
        let now = Instant::now();
        assert_eq!(out.next_tick(now), None);
        out.tick(now + Duration::from_secs(1)).unwrap();
        assert_eq!(pulses.borrow().len(), 3);
    }

}
