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
        self.0 >= 0xe0 && self.0 <= 0xe7
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
        let existing = self.holds.iter().position(|slot| {
            slot.is_some_and(|hold| hold.owner == owner && Some(hold.action_index) == action_index)
        });
        if !held {
            if let Some(index) = existing {
                self.remove_hold(index);
                self.reclaim_released();
                return Ok(true);
            }
            return Ok(false);
        }
        if existing.is_some() {
            return Ok(false);
        }
        // Find both slots before mutating, so either capacity failure is atomic.
        let hold_index = self
            .holds
            .iter()
            .position(Option::is_none)
            .ok_or(CapacityError::Holds)?;
        let index = match action_index {
            Some(index) => index,
            None => self
                .actions
                .iter()
                .position(Option::is_none)
                .ok_or(CapacityError::Actions)?,
        };
        let entry = self.actions[index].get_or_insert(ActionEntry {
            action,
            desired_holds: 0,
            emitted: false,
        });
        entry.desired_holds += 1;
        self.holds[hold_index] = Some(Hold {
            owner,
            action_index: index,
        });
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
    pub fn has_pending(&self) -> bool {
        self.pending_index().is_some()
    }

    /// Borrow the next event until it is accepted or its guard is dropped.
    ///
    /// All pending releases precede presses. For chords, non-modifier releases
    /// precede modifier releases, and modifier presses precede other presses.
    /// Within each group portable action order is stable, independent of slot
    /// reuse and binding update order. This is held-state reconciliation, not a
    /// queue of taps: a press removed before output is accepted is cancelled.
    #[must_use]
    pub fn next_pending(&mut self) -> Option<PendingTransition<'_, ACTIONS, HOLDS>> {
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

    fn action_index(&self, action: Action) -> Option<usize> {
        self.actions
            .iter()
            .position(|entry| entry.is_some_and(|entry| entry.action == action))
    }

    fn pending_index(&self) -> Option<usize> {
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
            if let Some(entry) = self.actions[hold.action_index].as_mut() {
                entry.desired_holds -= 1;
            }
        }
    }

    fn release_matching(&mut self, matches: impl Fn(ActionOwner) -> bool) {
        for index in 0..HOLDS {
            if self.holds[index].is_some_and(|hold| matches(hold.owner)) {
                self.remove_hold(index);
            }
        }
        self.reclaim_released();
    }

    fn reclaim_released(&mut self) {
        for slot in &mut self.actions {
            if slot.is_some_and(|entry| entry.desired_holds == 0 && !entry.emitted) {
                *slot = None;
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
        if let Some(entry) = self.state.actions[self.index].as_mut() {
            entry.emitted = self.transition.pressed;
        }
        self.state.reclaim_released();
    }
}
