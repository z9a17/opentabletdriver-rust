//! Shared, bounded ownership of held keyboard and mouse actions.
//!
//! Upstream v0.6.7 `BindingState` suppresses duplicate transitions per binding,
//! while `KeyBinding`/`MouseBinding` forward directly to their output services.
//! This module supplies the cross-binding ownership and acknowledged-output
//! layer required by B01; it does not run bindings or replace their semantics.
//!
//! All storage is inline and operations allocate nothing. Update desired holds,
//! then repeatedly obtain `next_pending`, send its event, and acknowledge only
//! an accepted event. Dropping the guard after an error leaves the event pending.
//! Send one transition at a time: a successful prefix remains acknowledged when
//! a later event fails. Cleanup removes desired holds but retains successfully
//! emitted presses until their releases are acknowledged. The caller must retry
//! cleanup before discarding this state; dropping it cannot release OS input.

use std::fmt;

/// Stable device identity and binding identity within that device.
///
/// Use a new device value for a new connection generation, and release the old
/// generation on disconnect. IDs must be unique within one `ActionState`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ActionOwner {
    pub device: u64,
    pub binding: u32,
}

/// Keyboard/Keypad usage ID from USB HID usage page 0x07, not an OS key code.
/// The seven pinned media controls use `0x1000 | consumer_usage` as a synthetic
/// domain within the existing u16 command ABI; they are not page-0x07 usages.
/// Original logical keys use `0x2000 | VK`, `0x4000 | CGKeyCode`, or
/// `0x8000 | evdev` in that same ABI.
///
/// Values 0..=3 are reserved/no-key/error reports and cannot be held. Other IDs
/// retain their portable identity; an adapter must reject unsupported usages.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct KeyboardUsage(u16);

impl KeyboardUsage {
    pub const fn new(usage: u16) -> Option<Self> {
        if usage >= 4 { Some(Self(usage)) } else { None }
    }

    pub const fn usage(self) -> u16 {
        self.0
    }

    pub const fn is_modifier(self) -> bool {
        (self.0 >= 0xe0 && self.0 <= 0xe7)
            || matches!(
                self.windows_virtual_code(),
                Some(0x10..=0x12 | 0x5b..=0x5c | 0xa0..=0xa5)
            )
            || matches!(self.macos_virtual_code(), Some(54..=56 | 58..=62))
            || matches!(
                self.linux_event_code(),
                Some(29 | 42 | 54 | 56 | 97 | 100 | 125 | 126)
            )
    }

    /// Original Windows keys are logical virtual keys; native usages remain
    /// physical keys. This synthetic domain retains the u16 command ABI.
    pub const fn windows_virtual_key(code: u16) -> Option<Self> {
        if code > 0 && code <= 255 {
            Some(Self(0x2000 | code))
        } else {
            None
        }
    }
    pub const fn windows_virtual_code(self) -> Option<u16> {
        if self.0 > 0x2000 && self.0 <= 0x20ff {
            Some(self.0 & 0xff)
        } else {
            None
        }
    }

    /// CGKeyCode zero is the real A key; original None is handled as no-op
    /// by the name resolver and must never erase this encoded identity.
    pub const fn macos_virtual_key(code: u16) -> Option<Self> {
        if code < 128 {
            Some(Self(0x4000 | code))
        } else {
            None
        }
    }
    pub const fn macos_virtual_code(self) -> Option<u16> {
        if self.0 >= 0x4000 && self.0 < 0x4080 {
            Some(self.0 & 0x7f)
        } else {
            None
        }
    }
    pub const fn linux_event_key(code: u16) -> Option<Self> {
        if code > 0 && code < 768 {
            Some(Self(0x8000 | code))
        } else {
            None
        }
    }
    pub const fn linux_event_code(self) -> Option<u16> {
        if self.0 > 0x8000 && self.0 < 0x8300 {
            Some(self.0 & 0x3ff)
        } else {
            None
        }
    }

    pub const fn consumer_key(self) -> Option<ConsumerKey> {
        match self.0 {
            0x10e2 => Some(ConsumerKey::Mute),
            0x10e9 => Some(ConsumerKey::VolumeUp),
            0x10ea => Some(ConsumerKey::VolumeDown),
            0x10cd => Some(ConsumerKey::PlayPause),
            0x10b6 => Some(ConsumerKey::PreviousSong),
            0x10b5 => Some(ConsumerKey::NextSong),
            0x10b7 => Some(ConsumerKey::StopSong),
            _ => None,
        }
    }
}

/// The consumer controls exposed by the pinned original Windows keyboard.
/// Adapters retain this identity for shared holds and select their actual
/// platform key code; platforms may reject controls absent from their API.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u16)]
pub enum ConsumerKey {
    Mute = 0xe2,
    VolumeUp = 0xe9,
    VolumeDown = 0xea,
    PlayPause = 0xcd,
    PreviousSong = 0xb6,
    NextSong = 0xb5,
    StopSong = 0xb7,
}
impl ConsumerKey {
    pub const fn keyboard_usage(self) -> KeyboardUsage {
        KeyboardUsage(0x1000 | self as u16)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    Backward,
    Forward,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Action {
    Key(KeyboardUsage),
    Mouse(MouseButton),
}

impl Action {
    fn is_modifier(self) -> bool {
        matches!(self, Self::Key(key) if key.is_modifier())
    }
}

/// One press or release. Adapters must report actual acceptance, not intention.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ActionTransition {
    pub action: Action,
    pub pressed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CapacityError {
    /// Distinct actions, including output still awaiting release, fill storage.
    Actions,
    /// Distinct (owner, action) holds fill storage.
    Holds,
}

impl fmt::Display for CapacityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Actions => {
                "held action capacity exceeded; release pending output or increase capacity"
            }
            Self::Holds => "binding ownership capacity exceeded",
        })
    }
}

impl std::error::Error for CapacityError {}

#[derive(Clone, Copy)]
struct ActionEntry {
    action: Action,
    desired_holds: usize,
    emitted: bool,
}

#[derive(Clone, Copy)]
struct Hold {
    owner: ActionOwner,
    action_index: usize,
}

/// Fixed capacities bound every update and transition scan.
///
/// `ACTIONS` counts distinct actions; `HOLDS` counts distinct owner/action pairs.
/// An emitted action awaiting release occupies a slot even with no owners.
/// Capacity errors leave state unchanged. Release operations cannot exhaust
/// capacity. A state instance must exclusively own its adapter's synthetic
/// actions; separate states cannot coordinate overlapping injected input.
pub struct ActionState<const ACTIONS: usize = 64, const HOLDS: usize = 128> {
    actions: [Option<ActionEntry>; ACTIONS],
    holds: [Option<Hold>; HOLDS],
    action_mask: u64,
    pending_mask: u64,
    hold_mask: u128,
    pending_count: usize,
}

impl<const ACTIONS: usize, const HOLDS: usize> Default for ActionState<ACTIONS, HOLDS> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const ACTIONS: usize, const HOLDS: usize> ActionState<ACTIONS, HOLDS> {
    pub const fn new() -> Self {
        Self {
            actions: [None; ACTIONS],
            holds: [None; HOLDS],
            action_mask: 0,
            pending_mask: 0,
            hold_mask: 0,
            pending_count: 0,
        }
    }

    #[inline]
    fn sync_pending(&mut self, index: usize) {
        let is_pending =
            self.actions[index].is_some_and(|entry| (entry.desired_holds != 0) != entry.emitted);
        if ACTIONS <= 64 {
            let bit = 1u64 << index;
            let was_pending = self.pending_mask & bit != 0;
            if is_pending != was_pending {
                if is_pending {
                    self.pending_mask |= bit;
                    self.pending_count += 1;
                } else {
                    self.pending_mask &= !bit;
                    self.pending_count -= 1;
                }
            }
        } else {
            self.pending_count = self
                .actions
                .iter()
                .filter(|slot| {
                    slot.is_some_and(|entry| (entry.desired_holds != 0) != entry.emitted)
                })
                .count();
        }
    }

    /// Add/remove one owner's hold; returns whether desired ownership changed.
    /// Repeated presses and releases are idempotent. This emits no input.
    pub fn set_held(
        &mut self,
        owner: ActionOwner,
        action: Action,
        held: bool,
    ) -> Result<bool, CapacityError> {
        let action_index = self.action_index(action);
        let existing = action_index.and_then(|target| self.find_hold(owner, target));
        if !held {
            if let Some(index) = existing {
                self.remove_hold(index);
                return Ok(true);
            }
            return Ok(false);
        }
        if existing.is_some() {
            return Ok(false);
        }
        // Find both slots before mutating, so either capacity failure is atomic.
        let hold_index = self.find_free_hold().ok_or(CapacityError::Holds)?;
        let index = match action_index {
            Some(index) => index,
            None => self.find_free_action().ok_or(CapacityError::Actions)?,
        };
        let entry = self.actions[index].get_or_insert(ActionEntry {
            action,
            desired_holds: 0,
            emitted: false,
        });
        entry.desired_holds += 1;
        if ACTIONS <= 64 {
            self.action_mask |= 1u64 << index;
        }
        self.sync_pending(index);
        self.holds[hold_index] = Some(Hold {
            owner,
            action_index: index,
        });
        if HOLDS <= 128 {
            self.hold_mask |= 1u128 << hold_index;
        }
        Ok(true)
    }

    /// Remove one binding's holds; output releases still require acknowledgement.
    pub fn release_owner(&mut self, owner: ActionOwner) {
        self.release_matching(|candidate| candidate == owner);
    }

    /// Remove every binding for a disconnected/reconfigured device generation.
    pub fn release_device(&mut self, device: u64) {
        self.release_matching(|owner| owner.device == device);
    }

    /// Clear desired ownership for shutdown, preserving unacknowledged cleanup.
    pub fn release_all(&mut self) {
        self.release_matching(|_| true);
    }

    pub fn is_desired(&self, action: Action) -> bool {
        self.action_index(action)
            .and_then(|index| self.actions[index])
            .is_some_and(|entry| entry.desired_holds != 0)
    }

    pub fn is_emitted(&self, action: Action) -> bool {
        self.action_index(action)
            .and_then(|index| self.actions[index])
            .is_some_and(|entry| entry.emitted)
    }

    /// Whether desired ownership differs from the last acknowledged OS state.
    #[inline]
    pub fn has_pending(&self) -> bool {
        self.pending_count != 0
    }

    /// Borrow the next event until it is accepted or its guard is dropped.
    ///
    /// All pending releases precede presses. For chords, non-modifier releases
    /// precede modifier releases, and modifier presses precede other presses.
    /// Within each group portable action order is stable, independent of slot
    /// reuse and binding update order. This is held-state reconciliation, not a
    /// queue of taps: a press removed before output is accepted is cancelled.
    #[inline]
    pub fn next_pending(&mut self) -> Option<PendingTransition<'_, ACTIONS, HOLDS>> {
        if self.pending_count == 0 {
            return None;
        }
        let index = self.pending_index()?;
        let entry = self.actions[index]?;
        Some(PendingTransition {
            transition: ActionTransition {
                action: entry.action,
                pressed: entry.desired_holds != 0,
            },
            state: self,
            index,
        })
    }

    #[inline]
    fn action_index(&self, action: Action) -> Option<usize> {
        if ACTIONS <= 64 {
            let mut bits = self.action_mask;
            while bits != 0 {
                let index = bits.trailing_zeros() as usize;
                bits &= bits - 1;
                if self.actions[index].is_some_and(|entry| entry.action == action) {
                    return Some(index);
                }
            }
            None
        } else {
            self.actions
                .iter()
                .position(|entry| entry.is_some_and(|entry| entry.action == action))
        }
    }

    #[inline]
    fn find_free_action(&self) -> Option<usize> {
        if ACTIONS <= 64 {
            let index = (!self.action_mask).trailing_zeros() as usize;
            (index < ACTIONS).then_some(index)
        } else {
            self.actions.iter().position(Option::is_none)
        }
    }

    #[inline]
    fn find_hold(&self, owner: ActionOwner, action_index: usize) -> Option<usize> {
        if HOLDS <= 128 {
            let mut bits = self.hold_mask;
            while bits != 0 {
                let index = bits.trailing_zeros() as usize;
                bits &= bits - 1;
                if self.holds[index]
                    .is_some_and(|hold| hold.owner == owner && hold.action_index == action_index)
                {
                    return Some(index);
                }
            }
            None
        } else {
            self.holds.iter().position(|slot| {
                slot.is_some_and(|hold| hold.owner == owner && hold.action_index == action_index)
            })
        }
    }

    #[inline]
    fn find_free_hold(&self) -> Option<usize> {
        if HOLDS <= 128 {
            let index = (!self.hold_mask).trailing_zeros() as usize;
            (index < HOLDS).then_some(index)
        } else {
            self.holds.iter().position(Option::is_none)
        }
    }

    fn pending_index(&self) -> Option<usize> {
        if ACTIONS <= 64 {
            let mut bits = self.pending_mask;
            let mut best: Option<((bool, bool, Action), usize)> = None;
            while bits != 0 {
                let index = bits.trailing_zeros() as usize;
                bits &= bits - 1;
                let Some(entry) = self.actions[index] else {
                    continue;
                };
                let pressed = entry.desired_holds != 0;
                if pressed == entry.emitted {
                    continue;
                }
                let modifier_order = entry.action.is_modifier() != pressed;
                let key = (pressed, modifier_order, entry.action);
                if best.is_none_or(|(best_key, _)| key < best_key) {
                    best = Some((key, index));
                }
            }
            return best.map(|(_, index)| index);
        }
        self.actions
            .iter()
            .enumerate()
            .filter_map(|(index, entry)| {
                let entry = entry.as_ref()?;
                let pressed = entry.desired_holds != 0;
                if pressed == entry.emitted {
                    return None;
                }
                // false sorts before true: releases, then presses.
                let modifier_order = entry.action.is_modifier() != pressed;
                Some(((pressed, modifier_order, entry.action), index))
            })
            .min_by_key(|(order, _)| *order)
            .map(|(_, index)| index)
    }

    fn remove_hold(&mut self, index: usize) {
        if let Some(hold) = self.holds[index].take() {
            if HOLDS <= 128 {
                self.hold_mask &= !(1u128 << index);
            }
            let action_index = hold.action_index;
            let entry = self.actions[action_index]
                .as_mut()
                .expect("a live hold retains its action slot");
            entry.desired_holds -= 1;
            self.reclaim_slot(action_index);
            self.sync_pending(action_index);
        }
    }

    fn release_matching(&mut self, matches: impl Fn(ActionOwner) -> bool) {
        if HOLDS <= 128 {
            let mut bits = self.hold_mask;
            while bits != 0 {
                let index = bits.trailing_zeros() as usize;
                bits &= bits - 1;
                if self.holds[index].is_some_and(|hold| matches(hold.owner)) {
                    self.remove_hold(index);
                }
            }
        } else {
            for index in 0..HOLDS {
                if self.holds[index].is_some_and(|hold| matches(hold.owner)) {
                    self.remove_hold(index);
                }
            }
        }
    }

    #[inline]
    fn reclaim_slot(&mut self, index: usize) {
        if self.actions[index].is_some_and(|entry| entry.desired_holds == 0 && !entry.emitted) {
            self.actions[index] = None;
            if ACTIONS <= 64 {
                self.action_mask &= !(1u64 << index);
            }
        }
    }
}

/// Exclusive output transaction. Dropping it without acknowledgement means the
/// event was not accepted. Desired state cannot change while this guard exists,
/// and the guard cannot be copied or acknowledged twice.
#[must_use = "send the transition and acknowledge only successful output"]
pub struct PendingTransition<'a, const ACTIONS: usize, const HOLDS: usize> {
    state: &'a mut ActionState<ACTIONS, HOLDS>,
    index: usize,
    transition: ActionTransition,
}

impl<const ACTIONS: usize, const HOLDS: usize> PendingTransition<'_, ACTIONS, HOLDS> {
    pub fn transition(&self) -> ActionTransition {
        self.transition
    }

    /// Call only after the adapter confirms that this exact event was accepted.
    pub fn acknowledge(self) {
        let entry = self.state.actions[self.index]
            .as_mut()
            .expect("an exclusive pending guard retains its action slot");
        entry.emitted = self.transition.pressed;
        self.state.reclaim_slot(self.index);
        self.state.sync_pending(self.index);
    }
}
