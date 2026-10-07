//! Independent all-session debugger subscriptions. Producers never allocate or
//! wait for a client; packet storage is bounded and allocated on explicit enable.
use std::collections::{BTreeSet, VecDeque};
use std::sync::{Mutex, atomic::{AtomicBool, AtomicU64, Ordering}};

const CAPACITY_BYTES: usize = 8 * 1024 * 1024;
const CAPACITY_REPORTS: usize = 4096;
const MAX_SUBSCRIBERS: usize = 8;
static ENABLED: AtomicBool = AtomicBool::new(false);
static LOST: AtomicU64 = AtomicU64::new(0);
static NEXT: AtomicU64 = AtomicU64::new(1);
static RING: Mutex<Option<Ring>> = Mutex::new(None);
static METADATA: Mutex<VecDeque<SessionMetadata>> = Mutex::new(VecDeque::new());

#[derive(Clone, Debug)]
pub struct SessionMetadata {
    pub session: u64,
    pub name: String,
    pub parser: String,
    pub auxiliary_parser: Option<String>,
    pub key: Option<String>,
    pub active: bool,
}

pub(super) fn register(metadata: SessionMetadata) {
    if let Ok(mut sessions) = METADATA.lock() {
        while sessions.len() >= 128 {
            let Some(index) = sessions.iter().position(|session| !session.active) else { return; };
            sessions.remove(index);
        }
        sessions.push_back(metadata);
    }
}
pub(super) fn unregister(id: u64) {
    if let Ok(mut sessions) = METADATA.lock() {
        if let Some(session) = sessions.iter_mut().find(|session| session.session == id) { session.active = false; }
    }
}
/// Retired metadata remains bounded so already queued final reports still carry
/// their original parser identity when a tablet disappears between polls.
pub fn sessions() -> Vec<SessionMetadata> {
    METADATA.lock().map_or_else(|_| Vec::new(), |sessions| sessions.iter().cloned().collect())
}

#[derive(Clone, Copy, Default)]
struct Slot { sequence: u64, gap_generation: u64, session: u64, offset: usize, length: usize, auxiliary: bool }
struct Ring {
    subscribers: BTreeSet<u64>,
    slots: Box<[Slot]>,
    bytes: Box<[u8]>,
    head: usize,
    count: usize,
    write: usize,
    used: usize,
    sequence: u64,
}
impl Ring {
    fn new() -> Self {
        Self { subscribers: BTreeSet::new(), slots: vec![Slot::default(); CAPACITY_REPORTS].into_boxed_slice(),
            bytes: vec![0; CAPACITY_BYTES].into_boxed_slice(), head: 0, count: 0, write: 0, used: 0, sequence: 0 }
    }
    fn push(&mut self, session: u64, bytes: &[u8], auxiliary: bool, gap_generation: u64) {
        while self.count == self.slots.len() || self.bytes.len() - self.used < bytes.len() {
            self.used -= self.slots[self.head].length;
            self.head = (self.head + 1) % self.slots.len();
            self.count -= 1;
        }
        self.sequence = self.sequence.saturating_add(1);
        let index = (self.head + self.count) % self.slots.len();
        self.slots[index] = Slot { sequence: self.sequence, gap_generation, session, offset: self.write, length: bytes.len(), auxiliary };
        let first = bytes.len().min(self.bytes.len() - self.write);
        self.bytes[self.write..self.write + first].copy_from_slice(&bytes[..first]);
        self.bytes[..bytes.len() - first].copy_from_slice(&bytes[first..]);
        self.write = (self.write + bytes.len()) % self.bytes.len();
        self.used += bytes.len();
        self.count += 1;
    }
    fn packet(&self, slot: Slot) -> Packet {
        let first = slot.length.min(self.bytes.len() - slot.offset);
        let mut bytes = Vec::with_capacity(slot.length);
        bytes.extend_from_slice(&self.bytes[slot.offset..slot.offset + first]);
        bytes.extend_from_slice(&self.bytes[..slot.length - first]);
        Packet { sequence: slot.sequence, gap_generation: slot.gap_generation, session: slot.session, auxiliary: slot.auxiliary, bytes }
    }
}

#[derive(Debug)]
pub struct Packet {
    pub sequence: u64,
    /// Changes before the first retained packet following a dropped tap attempt.
    /// Consumers reset stateful decoders before crossing this boundary.
    pub gap_generation: u64,
    pub session: u64,
    pub auxiliary: bool,
    pub bytes: Vec<u8>,
}
pub struct Batch { pub packets: Vec<Packet>, pub lost: u64 }
pub struct Subscription { id: u64, cursor: u64, lost: u64 }

pub fn subscribe() -> Result<Subscription, String> {
    let mut ring = RING.lock().map_err(|_| "debug stream lock poisoned")?;
    let ring = ring.get_or_insert_with(Ring::new);
    if ring.subscribers.len() >= MAX_SUBSCRIBERS { return Err("debug stream subscriber limit reached".into()); }
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    if id == 0 { return Err("debug stream subscription IDs exhausted".into()); }
    ring.subscribers.insert(id);
    let subscription = Subscription { id, cursor: ring.sequence, lost: LOST.load(Ordering::Acquire) };
    ENABLED.store(true, Ordering::Release);
    Ok(subscription)
}
impl Subscription {
    pub fn read(&mut self, limit: usize, max_bytes: usize) -> Result<Batch, String> {
        if !(1..=64).contains(&limit) || !(super::MAX_BYTES..=256 * 1024).contains(&max_bytes) {
            return Err("debug stream read exceeds its packet or byte bounds".into());
        }
        let ring = RING.lock().map_err(|_| "debug stream lock poisoned")?;
        let ring = ring.as_ref().filter(|ring| ring.subscribers.contains(&self.id)).ok_or("debug stream subscription expired")?;
        let losses = LOST.load(Ordering::Acquire);
        let mut lost = losses.saturating_sub(self.lost);
        if ring.count != 0 { lost = lost.saturating_add(ring.slots[ring.head].sequence.saturating_sub(self.cursor.saturating_add(1))); }
        let mut packets = Vec::with_capacity(limit);
        let mut bytes = 0;
        for offset in 0..ring.count {
            let slot = ring.slots[(ring.head + offset) % ring.slots.len()];
            if slot.sequence <= self.cursor { continue; }
            if packets.len() == limit || bytes + slot.length > max_bytes { break; }
            bytes += slot.length;
            packets.push(ring.packet(slot));
            self.cursor = slot.sequence;
        }
        self.lost = losses;
        Ok(Batch { packets, lost })
    }
}
impl Drop for Subscription {
    fn drop(&mut self) {
        let mut ring = RING.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(active) = ring.as_mut() {
            active.subscribers.remove(&self.id);
            if active.subscribers.is_empty() { ENABLED.store(false, Ordering::Release); *ring = None; }
        }
    }
}

#[inline]
pub(super) fn record(session: u64, bytes: &[u8], auxiliary: bool) {
    if !ENABLED.load(Ordering::Relaxed) || session == 0 { return; }
    if bytes.len() > super::MAX_BYTES {
        LOST.fetch_add(1, Ordering::AcqRel);
        return;
    }
    let Ok(mut ring) = RING.try_lock() else { LOST.fetch_add(1, Ordering::AcqRel); return; };
    if let Some(ring) = ring.as_mut() {
        ring.push(session, bytes, auxiliary, LOST.load(Ordering::Acquire));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn variable_size_ring_preserves_wrap_and_eviction_without_allocating_on_push() {
        let mut ring = Ring { subscribers: BTreeSet::new(), slots: vec![Slot::default(); 3].into_boxed_slice(),
            bytes: vec![0; 11].into_boxed_slice(), head: 0, count: 0, write: 0, used: 0, sequence: 0 };
        ring.push(1, &[1; 5], false, 0);
        ring.push(2, &[2; 5], true, 0);
        ring.push(1, &[3; 5], false, 1);
        assert_eq!(ring.count, 2);
        assert_eq!(ring.packet(ring.slots[ring.head]).bytes, [2; 5]);
        let latest = ring.packet(ring.slots[(ring.head + 1) % ring.slots.len()]);
        assert_eq!(latest.bytes, [3; 5]);
        assert_eq!(latest.sequence, 3);
        assert_eq!(latest.gap_generation, 1);
        crate::test_alloc::assert_no_allocations(|| ring.push(2, &[4; 5], true, 1));
    }
}
