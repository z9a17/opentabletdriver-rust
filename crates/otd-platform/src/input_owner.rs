//! One OS-code ownership domain for every native reader and original Desktop
//! input scope. Storage for native transitions is fixed; OS acceptance precedes
//! acknowledgement. Managed leases run on their separate cold service lane.
use crate::managed_services::Operation;
use serde_json::Value;
use std::{
    collections::BTreeMap,
    io,
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Code {
    Key(u16),
    Button(u8),
}
impl Code {
    fn index(self) -> io::Result<usize> {
        match self {
            Self::Key(code) if code < 768 => Ok(code as usize),
            Self::Button(button) if button < 5 => Ok(768 + button as usize),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Input code exceeds supported OS domain",
            )),
        }
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Owner {
    Native(u64, u32),
    Managed(u64),
}
#[derive(Clone, Copy)]
struct Hold {
    owner: Owner,
    index: usize,
    context: Option<PointerContext>,
}
/// Copy-only event context carried with ownership transitions and pending retries.
#[derive(Clone, Copy)]
pub struct PointerContext {
    pub position: Option<(f64, f64)>,
    pub click_position: Option<(f64, f64)>,
    pub delta: Option<(f64, f64)>,
    pub attributes: Option<otd_core::output::MouseAttributes>,
}
const DIRTY_WORDS: usize = 13; // 13 * 64 = 832 >= 773
const KEY_WORDS: usize = 12; // 12 * 64 = 768

/// Lock-free mirrors of the global `STATE.emitted` table and maintenance need.
static EMITTED_BUTTONS: AtomicU8 = AtomicU8::new(0);
static EMITTED_KEYS: [AtomicU64; KEY_WORDS] = [const { AtomicU64::new(0) }; KEY_WORDS];
static MAINTENANCE_NEEDED: AtomicBool = AtomicBool::new(false);

#[inline]
fn is_modifier_index(index: usize) -> bool {
    if cfg!(target_os = "linux") {
        matches!(index, 29 | 42 | 56 | 125 | 97 | 54 | 100 | 126)
    } else {
        matches!(index, 59 | 56 | 58 | 55 | 62 | 60 | 61 | 54)
    }
}

#[inline]
fn mirror_emitted_atomic(index: usize, pressed: bool) {
    if index < 768 {
        let word = &EMITTED_KEYS[index >> 6];
        let bit = 1u64 << (index & 63);
        if pressed {
            word.fetch_or(bit, Ordering::Release);
        } else {
            word.fetch_and(!bit, Ordering::Release);
        }
    } else if index < 773 {
        let bit = 1u8 << (index - 768);
        if pressed {
            EMITTED_BUTTONS.fetch_or(bit, Ordering::Release);
        } else {
            EMITTED_BUTTONS.fetch_and(!bit, Ordering::Release);
        }
    }
}

type Writer = Box<dyn FnMut(Code, bool, Option<PointerContext>) -> io::Result<()> + Send>;
struct State {
    holds: [Option<Hold>; 4096],
    active_holds: usize,
    desired: [usize; 773],
    emitted: [bool; 773],
    dirty: [u64; DIRTY_WORDS],
    positions: [Option<PointerContext>; 773],
    writer: Option<Writer>,
    leases: BTreeMap<u64, Instant>,
    mirror_global: bool,
}
impl State {
    #[inline]
    fn update_dirty(&mut self, index: usize) {
        let word = index >> 6;
        let bit = 1u64 << (index & 63);
        if self.emitted[index] != (self.desired[index] != 0) {
            self.dirty[word] |= bit;
        } else {
            self.dirty[word] &= !bit;
        }
    }
    #[inline]
    fn has_dirty(&self) -> bool {
        self.dirty.iter().any(|&word| word != 0)
    }
    #[inline]
    fn sync_maintenance_flag(&self) {
        if self.mirror_global {
            MAINTENANCE_NEEDED.store(
                !self.leases.is_empty() || self.has_dirty(),
                Ordering::Release,
            );
        }
    }
    fn remove_hold_at(&mut self, slot: usize) -> Hold {
        self.active_holds -= 1;
        let last = self.active_holds;
        if slot == last {
            self.holds[slot].take().unwrap()
        } else {
            let moved = self.holds[last].take();
            self.holds[slot].replace(moved.unwrap()).unwrap()
        }
    }
    fn maintenance_deadline(&self, now: Instant) -> Instant {
        // Native readers share this domain and can leave a failed release
        // after their last report, independently of managed queue admission.
        // Keep their existing bounded retry until they explicitly wake lane4.
        let retry = now + Duration::from_millis(250);
        self.leases
            .values()
            .copied()
            .min()
            .map_or(retry, |lease| lease.min(retry))
    }
    fn hold(
        &mut self,
        owner: Owner,
        code: Code,
        held: bool,
        position: Option<PointerContext>,
    ) -> io::Result<()> {
        let index = code.index()?;
        let existing = self.holds[..self.active_holds]
            .iter()
            .position(|hold| hold.is_some_and(|hold| hold.owner == owner && hold.index == index));
        if held && existing.is_none() {
            if self.active_holds >= self.holds.len() {
                return Err(io::Error::other("Shared input owner capacity reached"));
            }
            let slot = self.active_holds;
            self.holds[slot] = Some(Hold {
                owner,
                index,
                context: position,
            });
            self.active_holds += 1;
            self.desired[index] += 1;
            self.update_dirty(index);
        } else if !held {
            if let Some(slot) = existing {
                let removed = self.remove_hold_at(slot);
                self.desired[index] -= 1;
                self.update_dirty(index);
                if position.is_none() {
                    self.positions[index] = removed.context;
                }
            }
        } else if let Some(slot) = existing
            && position.is_some()
        {
            self.holds[slot].as_mut().unwrap().context = position;
        }
        if self.emitted[index] != (self.desired[index] != 0) && position.is_some() {
            self.positions[index] = position;
        }
        let result = match self.flush() {
            // An unrelated pending failure must not hide this accepted prefix
            // from LocalActions, otherwise it cannot release what was sent.
            Err(_) if self.emitted[index] == (self.desired[index] != 0) => Ok(()),
            Err(error) => {
                if held
                    && existing.is_none()
                    && let Some(slot) = self.holds[..self.active_holds].iter().position(|hold| {
                        hold.is_some_and(|hold| hold.owner == owner && hold.index == index)
                    })
                {
                    self.remove_hold_at(slot);
                    self.desired[index] -= 1;
                    self.update_dirty(index);
                }
                Err(error)
            }
            Ok(()) => Ok(()),
        };
        self.sync_maintenance_flag();
        result
    }
    fn flush(&mut self) -> io::Result<()> {
        if !self.has_dirty() {
            return Ok(());
        }
        // Releases precede presses. Native chord ordering is already supplied
        // by LocalActions; a retry never acknowledges an unaccepted prefix.
        for pressed in [false, true] {
            for modifier in [pressed, !pressed] {
                for word in 0..DIRTY_WORDS {
                    let mut bits = self.dirty[word];
                    while bits != 0 {
                        let bit = bits.trailing_zeros() as usize;
                        bits &= bits - 1;
                        let index = (word << 6) | bit;
                        if index >= 773 {
                            break;
                        }
                        let desired = self.desired[index] != 0;
                        if is_modifier_index(index) == modifier
                            && desired == pressed
                            && self.emitted[index] != desired
                        {
                            let code = if index < 768 {
                                Code::Key(index as u16)
                            } else {
                                Code::Button((index - 768) as u8)
                            };
                            (self.writer.as_mut().ok_or_else(|| {
                                io::Error::other("Actual platform input writer is unavailable")
                            })?)(code, pressed, self.positions[index])?;
                            self.emitted[index] = pressed;
                            self.positions[index] = None;
                            self.update_dirty(index);
                            if self.mirror_global {
                                mirror_emitted_atomic(index, pressed);
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }
    fn release(&mut self, owner: Owner) -> io::Result<()> {
        let mut slot = 0;
        while slot < self.active_holds {
            if self.holds[slot].is_some_and(|hold| hold.owner == owner) {
                let removed = self.remove_hold_at(slot);
                self.desired[removed.index] -= 1;
                self.update_dirty(removed.index);
                self.positions[removed.index] = removed.context;
            } else {
                slot += 1;
            }
        }
        let result = self.flush();
        self.sync_maintenance_flag();
        result
    }
    fn release_native(&mut self, id: u64) -> io::Result<()> {
        let mut slot = 0;
        while slot < self.active_holds {
            if self.holds[slot]
                .is_some_and(|hold| matches!(hold.owner, Owner::Native(owner, _) if owner == id))
            {
                let removed = self.remove_hold_at(slot);
                self.desired[removed.index] -= 1;
                self.update_dirty(removed.index);
                self.positions[removed.index] = removed.context;
            } else {
                slot += 1;
            }
        }
        let result = self.flush();
        self.sync_maintenance_flag();
        result
    }
}
static STATE: OnceLock<Mutex<State>> = OnceLock::new();
fn state() -> &'static Mutex<State> {
    STATE.get_or_init(|| {
        Mutex::new(State {
            holds: [None; 4096],
            active_holds: 0,
            desired: [0; 773],
            emitted: [false; 773],
            dirty: [0; DIRTY_WORDS],
            positions: [None; 773],
            writer: None,
            leases: BTreeMap::new(),
            mirror_global: true,
        })
    })
}
/// The factory is called once during cold resource setup. Keep the sole actual
/// OS writer alive through all readers and managed scope cleanup.
pub fn ensure(factory: impl FnOnce() -> io::Result<Writer>) -> io::Result<()> {
    let mut state = state()
        .lock()
        .map_err(|_| io::Error::other("Shared input ownership poisoned"))?;
    if state.writer.is_none() {
        state.writer = Some(factory()?);
    }
    Ok(())
}
pub struct Native {
    id: u64,
}
impl Native {
    pub fn new() -> io::Result<Self> {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let id = NEXT
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
            .map_err(|_| io::Error::other("Input owner IDs exhausted"))?;
        Ok(Self { id })
    }
    pub fn hold(&self, code: Code, held: bool) -> io::Result<()> {
        self.hold_at(code, held, None)
    }
    pub fn hold_at(&self, code: Code, held: bool, position: Option<(f64, f64)>) -> io::Result<()> {
        self.hold_action(0, code, held, position)
    }
    /// LocalActions merges bindings per portable action. Preserve that action's
    /// identity here: two distinct portable keys can alias one OS key code.
    pub fn hold_action(
        &self,
        action: u32,
        code: Code,
        held: bool,
        position: Option<(f64, f64)>,
    ) -> io::Result<()> {
        state()
            .lock()
            .map_err(|_| io::Error::other("Shared input ownership poisoned"))?
            .hold(
                Owner::Native(self.id, action),
                code,
                held,
                position.map(|position| PointerContext {
                    position: Some(position),
                    click_position: None,
                    delta: None,
                    attributes: None,
                }),
            )
    }
    pub fn hold_pointer(
        &self,
        action: u32,
        code: Code,
        held: bool,
        context: PointerContext,
    ) -> io::Result<bool> {
        let mut state = state()
            .lock()
            .map_err(|_| io::Error::other("Shared input ownership poisoned"))?;
        let index = code.index()?;
        let before = state.emitted[index];
        state.hold(Owner::Native(self.id, action), code, held, Some(context))?;
        Ok(before != state.emitted[index])
    }
    pub fn release(&self) -> io::Result<()> {
        state()
            .lock()
            .map_err(|_| io::Error::other("Shared input ownership poisoned"))?
            .release_native(self.id)
    }
}
impl Drop for Native {
    fn drop(&mut self) {
        if let Err(error) = self.release() {
            eprintln!("Shared native input cleanup failed: {error}");
        }
    }
}
pub fn button_down(button: u8) -> bool {
    button < 5 && (EMITTED_BUTTONS.load(Ordering::Acquire) & (1u8 << button)) != 0
}
pub fn buttons() -> u8 {
    EMITTED_BUTTONS.load(Ordering::Acquire)
}
pub fn key_mask(codes: [u16; 8]) -> u8 {
    codes.into_iter().enumerate().fold(0, |mask, (bit, code)| {
        let down = code < 768
            && (EMITTED_KEYS[(code >> 6) as usize].load(Ordering::Acquire) & (1u64 << (code & 63)))
                != 0;
        mask | ((down as u8) << bit)
    })
}
pub fn execute(operation: Operation, scope: u64, payload: &Value) -> Result<Value, String> {
    if scope == 0 {
        return Err("Original input scope must be nonzero".into());
    }
    let mut state = state()
        .lock()
        .map_err(|_| "Shared input ownership poisoned")?;
    if operation == Operation::InputRelease {
        state
            .release(Owner::Managed(scope))
            .map_err(|error| error.to_string())?;
        state.leases.remove(&scope);
        state.sync_maintenance_flag();
        return Ok(Value::Null);
    }
    if operation != Operation::InputHold {
        return Err("Invalid original input operation".into());
    }
    let kind = payload["type"]
        .as_str()
        .ok_or("Original input type required")?;
    if kind == "renew" && !state.leases.contains_key(&scope) {
        return Err("Original input scope has expired".into());
    }
    if !state.leases.contains_key(&scope) && state.leases.len() >= 256 {
        return Err("Original input scope capacity reached".into());
    }
    if kind != "renew" {
        let raw = payload["code"]
            .as_u64()
            .and_then(|code| u16::try_from(code).ok())
            .ok_or("Original input code required")?;
        let code = match kind {
            "key" => {
                let platform = if cfg!(target_os = "linux") {
                    "linux"
                } else {
                    "macos"
                };
                if payload["platform"] != platform
                    || (cfg!(target_os = "linux") && raw == 0)
                    || raw >= if cfg!(target_os = "linux") { 768 } else { 128 }
                {
                    return Err("Original keyboard platform/code is invalid".into());
                }
                Code::Key(raw)
            }
            "button" => Code::Button(match raw {
                0 => return Ok(Value::Null),
                1 => 0,
                2 => 2,
                3 => 1,
                4 => 3,
                5 => 4,
                _ => return Err("Unknown original mouse button".into()),
            }),
            _ => return Err("Unknown original input type".into()),
        };
        let held = payload["held"]
            .as_bool()
            .ok_or("Original input held state required")?;
        // Keep a lease even when an OS send fails: expiry retries all pending
        // cleanup, and a later successful event cannot escape ownership.
        let lease = payload["lease_ms"]
            .as_u64()
            .unwrap_or(15000)
            .clamp(1000, 60000);
        state
            .leases
            .insert(scope, Instant::now() + Duration::from_millis(lease));
        let repeat = held
            && matches!(code, Code::Key(_))
            && state.holds[..state.active_holds].iter().any(|hold| {
                hold.is_some_and(|hold| {
                    hold.owner == Owner::Managed(scope) && hold.index == code.index().unwrap()
                })
            });
        state
            .hold(Owner::Managed(scope), code, held, None)
            .map_err(|error| error.to_string())?;
        if repeat {
            (state
                .writer
                .as_mut()
                .ok_or("Actual platform input writer is unavailable")?)(
                code, true, None
            )
            .map_err(|error| error.to_string())?;
        }
    }
    let lease = payload["lease_ms"]
        .as_u64()
        .unwrap_or(15000)
        .clamp(1000, 60000);
    state
        .leases
        .insert(scope, Instant::now() + Duration::from_millis(lease));
    state.sync_maintenance_flag();
    Ok(Value::Null)
}
pub fn maintain() {
    if !MAINTENANCE_NEEDED.load(Ordering::Acquire) {
        return;
    }
    if let Ok(mut state) = state().lock() {
        let now = Instant::now();
        let expired = state
            .leases
            .iter()
            .filter_map(|(scope, expires)| (*expires <= now).then_some(*scope))
            .collect::<Vec<_>>();
        for scope in expired {
            match state.release(Owner::Managed(scope)) {
                Ok(()) => {
                    state.leases.remove(&scope);
                }
                Err(error) => {
                    state.leases.insert(scope, now + Duration::from_secs(1));
                    eprintln!("Original input lease cleanup failed: {error}");
                }
            }
        }
        // A retired native reader has no lease or future report to trigger retry.
        // The independent lane remains the cleanup owner even with no scopes.
        let _ = state.flush();
        state.sync_maintenance_flag();
    }
}
pub fn maintenance_deadline() -> Option<Instant> {
    Some(state().lock().ok()?.maintenance_deadline(Instant::now()))
}
pub fn shutdown() {
    if let Ok(mut state) = state().lock() {
        let scopes = state.leases.keys().copied().collect::<Vec<_>>();
        for scope in scopes {
            match state.release(Owner::Managed(scope)) {
                Ok(()) => {
                    state.leases.remove(&scope);
                }
                Err(error) => eprintln!("Original input shutdown cleanup failed: {error}"),
            }
        }
        for _ in 0..3 {
            if state.flush().is_ok() {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        eprintln!(
            "Shared input shutdown retains unacknowledged cleanup; actual output owner cannot be claimed clean"
        );
    }
}
#[cfg(test)]
mod fixtures {
    use super::*;
    fn owner(writer: Writer) -> State {
        State {
            holds: [None; 4096],
            active_holds: 0,
            desired: [0; 773],
            emitted: [false; 773],
            dirty: [0; DIRTY_WORDS],
            positions: [None; 773],
            writer: Some(writer),
            leases: BTreeMap::new(),
            mirror_global: false,
        }
    }
    #[test]
    fn maintenance_deadline_preserves_native_retry_and_tracks_earlier_managed_expiry() {
        let now = Instant::now();
        let mut state = owner(Box::new(|_, _, _| Ok(())));
        let native_retry = now + Duration::from_millis(250);
        assert_eq!(state.maintenance_deadline(now), native_retry);
        state.leases.insert(1, now + Duration::from_millis(80));
        state.leases.insert(2, now + Duration::from_secs(30));
        assert_eq!(
            state.maintenance_deadline(now),
            now + Duration::from_millis(80)
        );
        state.leases.insert(1, now + Duration::from_secs(1));
        assert_eq!(state.maintenance_deadline(now), native_retry);
        state.leases.insert(2, now - Duration::from_millis(1));
        assert!(
            state.maintenance_deadline(now) < now,
            "overdue leases must run immediately"
        );
        state.leases.clear();
        assert_eq!(
            state.maintenance_deadline(now),
            native_retry,
            "retired native cleanup has no managed lease"
        );
    }
    #[test]
    fn distinct_portable_aliases_and_managed_scope_require_the_last_release() {
        let events = std::sync::Arc::new(Mutex::new(Vec::new()));
        let captured = events.clone();
        let mut state = owner(Box::new(move |code, held, _| {
            captured.lock().unwrap().push((code, held));
            Ok(())
        }));
        state
            .hold(Owner::Native(1, 0x53), Code::Key(47), true, None)
            .unwrap();
        state
            .hold(Owner::Native(1, 0x9c), Code::Key(47), true, None)
            .unwrap();
        state
            .hold(Owner::Managed(2), Code::Key(47), true, None)
            .unwrap();
        state
            .hold(Owner::Native(1, 0x53), Code::Key(47), false, None)
            .unwrap();
        state.release_native(1).unwrap();
        assert_eq!(*events.lock().unwrap(), vec![(Code::Key(47), true)]);
        state.release(Owner::Managed(2)).unwrap();
        assert_eq!(
            *events.lock().unwrap(),
            vec![(Code::Key(47), true), (Code::Key(47), false)]
        );
    }
    #[test]
    fn retired_native_pending_release_remains_retryable_without_any_lease() {
        let failures = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(1));
        let output = failures.clone();
        let mut state = owner(Box::new(move |_, held, _| {
            if !held && output.swap(0, Ordering::Relaxed) != 0 {
                Err(io::Error::from(io::ErrorKind::WouldBlock))
            } else {
                Ok(())
            }
        }));
        state
            .hold(Owner::Native(1, 4), Code::Key(30), true, None)
            .unwrap();
        assert!(state.release_native(1).is_err());
        assert!(state.leases.is_empty());
        assert!(state.emitted[30]);
        assert_eq!(state.desired[30], 0);
        state.flush().unwrap();
        assert!(!state.emitted[30]);
    }
}
