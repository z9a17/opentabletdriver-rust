//! Nonblocking debugger tap for one selected session. Metadata and packet
//! bytes are read under the same lock; other sessions cannot overwrite them.
use std::cell::Cell;
pub mod stream;
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use std::sync::{
    Mutex,
    atomic::{AtomicU32, AtomicU64, Ordering},
};

// HID report lengths are u16. Keep complete packets from the larger
// configured endpoints too; truncation makes debugger decoding misleading.
pub const MAX_BYTES: usize = u16::MAX as usize;
pub const ARM_REPORTS: u32 = 2_000;
static ARMED: AtomicU32 = AtomicU32::new(0);
static NEXT_ID: AtomicU64 = AtomicU64::new(1);
static SELECTED: AtomicU64 = AtomicU64::new(0);
thread_local! { static SESSION: Cell<u64> = const { Cell::new(0) }; }

struct Capture {
    bytes: [u8; MAX_BYTES],
    length: usize,
    sequence: u64,
    devices: Vec<(u64, Device, usize, Option<String>)>,
    keys: Vec<(u64, String)>,
    selected_key: Option<String>,
}
static CAPTURE: Mutex<Capture> = Mutex::new(Capture {
    bytes: [0; MAX_BYTES],
    length: 0,
    sequence: 0,
    devices: Vec::new(),
    keys: Vec::new(),
    selected_key: None,
});

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Device {
    pub name: String,
    pub parser: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub device: Option<Device>,
    pub sequence: u64,
    pub bytes: Vec<u8>,
}

/// Thread-bound registration. The first live session owns the tap; when it
/// ends, the next registered session becomes selected and its counters reset.
pub struct Registration {
    id: u64,
    previous: u64,
    _thread_bound: std::marker::PhantomData<std::rc::Rc<()>>,
}
impl Registration {
    pub fn new(device: Device) -> Self {
        Self::with_reports(device, MAX_BYTES, None)
    }
    /// Register the largest input report buffer actually opened for this session.
    pub fn with_reports(device: Device, report_length: usize, auxiliary_parser: Option<String>) -> Self {
        Self::with_selection_key(device, report_length, auxiliary_parser, None)
    }
    /// The daemon supplies an opaque physical-device identity, outside the
    /// report path. Reconnection retains explicit selection without choosing
    /// another tablet when this one is absent.
    pub fn with_selection_key(device: Device, report_length: usize, auxiliary_parser: Option<String>, key: Option<String>) -> Self {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let previous = SESSION.replace(id);
        stream::register(stream::SessionMetadata { session: id, name: device.name.clone(), parser: device.parser.clone(),
            auxiliary_parser: auxiliary_parser.clone(), key: key.clone(), active: true });
        if let Ok(mut capture) = CAPTURE.lock() {
            capture.devices.push((id, device, report_length, auxiliary_parser));
            let wanted = key.as_ref().is_some_and(|key| capture.selected_key.as_ref() == Some(key));
            if let Some(key) = key { capture.keys.push((id, key)); }
            if SELECTED.load(Ordering::Relaxed) == 0 && (capture.selected_key.is_none() || wanted) {
                capture.length = 0;
                capture.sequence = 0;
                SELECTED.store(id, Ordering::Release);
            }
        }
        Self {
            id,
            previous,
            _thread_bound: std::marker::PhantomData,
        }
    }
}
impl Drop for Registration {
    fn drop(&mut self) {
        SESSION.set(self.previous);
        stream::unregister(self.id);
        if let Ok(mut capture) = CAPTURE.lock() {
            capture.devices.retain(|(id, _, _, _)| *id != self.id);
            capture.keys.retain(|(id, _)| *id != self.id);
            if SELECTED.load(Ordering::Relaxed) == self.id {
                capture.length = 0;
                capture.sequence = 0;
                SELECTED.store(
                    if let Some(key) = &capture.selected_key {
                        capture.keys.iter().find(|(_, candidate)| candidate == key).map_or(0, |(id, _)| *id)
                    } else { capture.devices.first().map_or(0, |(id, _, _, _)| *id) },
                    Ordering::Release,
                );
            }
        }
        FULL.freeze_session(self.id);
    }
}

#[inline]
pub fn record(bytes: &[u8]) {
    stream::record(SESSION.get(), bytes, false);
    record_sample(bytes);
    if FULL.state.load(Ordering::Relaxed) & ACTIVE != 0 {
        FULL.record(SESSION.get(), bytes, Instant::now(), false);
    }
}

/// The source's read completion time, before decoding and IPC coalescing.
#[inline]
pub fn record_at(bytes: &[u8], ready: Instant, auxiliary: bool) {
    stream::record(SESSION.get(), bytes, auxiliary);
    record_sample(bytes);
    if FULL.state.load(Ordering::Relaxed) & ACTIVE != 0 {
        FULL.record(SESSION.get(), bytes, ready, auxiliary);
    }
}

#[inline]
fn record_sample(bytes: &[u8]) {
    if ARMED.load(Ordering::Relaxed) != 0 {
        let id = SESSION.get();
        if id != 0 && SELECTED.load(Ordering::Acquire) == id {
            record_for(id, bytes);
        }
    }
}
#[cold]
fn record_for(id: u64, bytes: &[u8]) {
    if let Ok(mut capture) = CAPTURE.try_lock() {
        if SELECTED.load(Ordering::Relaxed) != id {
            return;
        }
        if ARMED
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |armed| {
                armed.checked_sub(1)
            })
            .is_err()
        {
            return;
        }
        let length = bytes.len().min(MAX_BYTES);
        capture.bytes[..length].copy_from_slice(&bytes[..length]);
        capture.length = length;
        capture.sequence = capture.sequence.saturating_add(1);
    }
}

/// Select one live registered tablet, preserving any owned full-rate capture.
/// An active/frozen lease or unresolved tap reservations require its owner to
/// drain/release first. Selecting the already selected identity is harmless.
/// Establish the first daemon-selected identity before any peer starts. This
/// does not select an unrelated live session while the desired tablet is absent.
pub fn prefer_device_key(key: &str) -> Result<(), CaptureError> {
    let mut capture = CAPTURE.lock().map_err(|_| CaptureError::Busy)?;
    let full = FULL.ring.lock().map_err(|_| CaptureError::Busy)?;
    if full.is_some() { return Err(CaptureError::Busy); }
    capture.selected_key = Some(key.to_owned());
    let selected = capture.keys.iter().find(|(_, candidate)| candidate == key).map_or(0, |(id, _)| *id);
    capture.length = 0;
    capture.sequence = 0;
    SELECTED.store(selected, Ordering::Release);
    Ok(())
}

pub fn select_device_key(key: &str) -> Result<(), CaptureError> {
    let mut capture = CAPTURE.lock().map_err(|_| CaptureError::Busy)?;
    let target = capture.keys.iter().find(|(_, candidate)| candidate == key).map(|(id, _)| *id)
        .ok_or(CaptureError::Invalid("device has no live debugger endpoint"))?;
    if SELECTED.load(Ordering::Acquire) != target {
        let mut full = FULL.ring.lock().map_err(|_| CaptureError::Busy)?;
        if let Some(ring) = full.as_mut() {
            let now = Instant::now();
            FULL.expire(ring, now);
            let state = FULL.state.load(Ordering::Acquire);
            if state & ACTIVE != 0 || FULL.resolved.load(Ordering::Acquire) != state & SEQUENCE_MASK ||
                FULL.micros(now) < FULL.deadline_us.load(Ordering::Acquire) {
                return Err(CaptureError::Busy);
            }
        }
        capture.length = 0;
        capture.sequence = 0;
        SELECTED.store(target, Ordering::Release);
    }
    capture.selected_key = Some(key.to_owned());
    Ok(())
}

pub fn poll() -> Snapshot {
    ARMED.store(ARM_REPORTS, Ordering::Relaxed);
    match CAPTURE.lock() {
        Ok(capture) => {
            let selected = SELECTED.load(Ordering::Relaxed);
            Snapshot {
                device: capture
                    .devices
                    .iter()
                    .find(|(id, _, _, _)| *id == selected)
                    .map(|(_, device, _, _)| device.clone()),
                sequence: capture.sequence,
                bytes: capture.bytes[..capture.length].to_vec(),
            }
        }
        Err(_) => Snapshot {
            device: None,
            sequence: 0,
            bytes: Vec::new(),
        },
    }
}

// Full-rate capture has its own lease and storage, independently of the sampled
// debugger. A reservation atomically includes epoch + active bit + sequence:
// stopping freezes its exact final sequence and cannot race an old epoch into a
// replacement. The control thread never reuses counters while reservations are
// pending. Every report path uses try_lock and preallocated buffers only.
const ACTIVE: u64 = 1 << 31;
const SEQUENCE_MASK: u64 = ACTIVE - 1;
pub const MIN_CAPTURE_BYTES: usize = 64 * 1024;
pub const MAX_CAPTURE_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_CAPTURE_LIMIT: usize = 512;
pub const MIN_LEASE_MS: u32 = 1_000;
pub const MAX_LEASE_MS: u32 = 30_000;
// The transport has 256 KiB frames. Reserve 16 KiB for escaped metadata,
// counters and envelope. Each packet's fixed JSON fields need <160 bytes even
// at u64::MAX; hex is exactly two bytes per raw byte.
pub const CAPTURE_PACKET_JSON_BUDGET: usize = 240 * 1024;
const PACKET_JSON_OVERHEAD: usize = 160;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptureStopReason { Requested, LeaseExpired, SessionEnded, SequenceLimit }

#[derive(Clone, Debug)]
pub struct CaptureStatus {
    pub epoch: u64,
    pub session: u64,
    pub device: Device,
    pub auxiliary_parser: Option<String>,
    pub report_length: usize,
    pub capacity_reports: usize,
    pub started_unix_ms: u64,
    pub active: bool,
    /// Final once inactive; sequence reservations precede the nonblocking tap.
    pub last_sequence: u64,
    pub resolved_reports: u64,
    pub pending_reports: u64,
    pub lost_tap: u64,
    pub overflow: u64,
    pub oversized: u64,
    pub acknowledged_sequence: u64,
    pub stop_reason: Option<CaptureStopReason>,
    pub lease_remaining_ms: u64,
}

#[derive(Clone, Debug)]
pub struct CapturePacket {
    pub sequence: u64,
    pub elapsed_us: u64,
    pub auxiliary: bool,
    pub bytes: Vec<u8>,
}
#[derive(Clone, Debug)]
pub struct CaptureBatch {
    pub capture: CaptureStatus,
    pub packets: Vec<CapturePacket>,
    /// Advances over resolved gaps too; pending reservations are never skipped.
    pub next_sequence: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptureError { Invalid(&'static str), Busy, Conflict }

#[derive(Clone, Copy, Default)]
struct Slot { sequence: u64, elapsed_us: u64, length: usize, auxiliary: bool }
struct Ring {
    start_id: u64,
    capacity_bytes: usize,
    initial_lease_ms: u32,
    epoch: u64,
    session: u64,
    device: Device,
    auxiliary_parser: Option<String>,
    origin: Instant,
    started_unix_ms: u64,
    report_length: usize,
    slots: Vec<Slot>,
    bytes: Vec<u8>,
    head: usize,
    count: usize,
    overflow: u64,
    oversized: u64,
    acknowledged: u64,
    stop_reason: Option<CaptureStopReason>,
}
struct FullCapture {
    state: AtomicU64,
    next_epoch: AtomicU64,
    session: AtomicU64,
    deadline_us: AtomicU64,
    lost: AtomicU64,
    resolved: AtomicU64,
    clock: std::sync::OnceLock<Instant>,
    ring: Mutex<Option<Ring>>,
}
impl FullCapture {
    const fn new() -> Self {
        Self {
            state: AtomicU64::new(0), next_epoch: AtomicU64::new(0),
            session: AtomicU64::new(0), deadline_us: AtomicU64::new(0),
            lost: AtomicU64::new(0), resolved: AtomicU64::new(0),
            clock: std::sync::OnceLock::new(), ring: Mutex::new(None),
        }
    }
    fn micros(&self, at: Instant) -> u64 {
        at.saturating_duration_since(*self.clock.get().expect("capture clock initialized"))
            .as_micros().min(u128::from(u64::MAX)) as u64
    }
    fn expire(&self, ring: &mut Ring, now: Instant) {
        let state = self.state.load(Ordering::Acquire);
        // Expiration and renewal share this control-thread mutex. The report
        // producer never freezes a lease using a deadline sampled before a
        // concurrent renewal. The daemon polls even without incoming reports.
        if state & ACTIVE != 0 && state & SEQUENCE_MASK == SEQUENCE_MASK {
            self.state.fetch_and(!ACTIVE, Ordering::AcqRel);
            ring.stop_reason = Some(CaptureStopReason::SequenceLimit);
        } else if state & ACTIVE != 0 && self.micros(now) >= self.deadline_us.load(Ordering::Acquire) {
            self.state.fetch_and(!ACTIVE, Ordering::AcqRel);
            ring.stop_reason = Some(CaptureStopReason::LeaseExpired);
        }
    }
    fn status(&self, ring: &Ring, now: Instant) -> CaptureStatus {
        // Completion is published after a loss increment; sampling the state
        // last ensures newly resolved reservations never exceed last_sequence.
        let resolved = self.resolved.load(Ordering::Acquire);
        let lost = self.lost.load(Ordering::Acquire);
        let state = self.state.load(Ordering::Acquire);
        let last_sequence = state & SEQUENCE_MASK;
        let active = state & ACTIVE != 0;
        CaptureStatus {
            epoch: ring.epoch, session: ring.session, device: ring.device.clone(),
            auxiliary_parser: ring.auxiliary_parser.clone(), report_length: ring.report_length,
            capacity_reports: ring.slots.len(), started_unix_ms: ring.started_unix_ms,
            active, last_sequence, resolved_reports: resolved,
            pending_reports: last_sequence.saturating_sub(resolved),
            lost_tap: lost, overflow: ring.overflow,
            oversized: ring.oversized, acknowledged_sequence: ring.acknowledged,
            stop_reason: ring.stop_reason,
            // Ownership remains leased while frozen so abandoned Stop or
            // disconnect drains cannot block every future capture forever.
            lease_remaining_ms: self.deadline_us.load(Ordering::Acquire)
                .saturating_sub(self.micros(now)) / 1_000,
        }
    }
    fn start(&self, start_id: u64, session: u64, device: Device, report_length: usize,
             auxiliary_parser: Option<String>, capacity_bytes: usize, lease_ms: u32,
             now: Instant) -> Result<CaptureStatus, CaptureError> {
        validate_capture_start(capacity_bytes, lease_ms)?;
        if start_id == 0 { return Err(CaptureError::Invalid("start ID must be nonzero")); }
        if session == 0 || report_length == 0 || report_length > MAX_BYTES {
            return Err(CaptureError::Invalid("no usable selected report endpoint"));
        }
        if device.name.len() > 512 || device.parser.len() > 512 ||
            auxiliary_parser.as_ref().is_some_and(|s| s.len() > 512) {
            return Err(CaptureError::Invalid("capture metadata exceeds 512 bytes"));
        }
        let capacity = capacity_bytes / (report_length + std::mem::size_of::<Slot>());
        if capacity == 0 { return Err(CaptureError::Invalid("budget cannot hold one complete report")); }
        let mut guard = self.ring.lock().map_err(|_| CaptureError::Busy)?;
        self.clock.get_or_init(|| now);
        if let Some(previous) = guard.as_mut() {
            self.expire(previous, now);
            if previous.start_id == start_id {
                if previous.capacity_bytes != capacity_bytes || previous.initial_lease_ms != lease_ms {
                    return Err(CaptureError::Invalid("start ID was reused with different parameters"));
                }
                return Ok(self.status(previous, now));
            }
            let state = self.state.load(Ordering::Acquire);
            if state & ACTIVE != 0 || self.resolved.load(Ordering::Acquire) != state & SEQUENCE_MASK ||
                self.micros(now) < self.deadline_us.load(Ordering::Acquire) {
                return Err(CaptureError::Busy);
            }
        }
        let epoch = self.next_epoch.fetch_update(Ordering::Relaxed, Ordering::Relaxed,
            |epoch| (epoch < u64::from(u32::MAX)).then_some(epoch + 1))
            .map_err(|_| CaptureError::Invalid("capture epoch exhausted"))? + 1;
        let ring = Ring {
            start_id, capacity_bytes, initial_lease_ms: lease_ms,
            epoch, session, device, auxiliary_parser, origin: now,
            started_unix_ms: SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default()
                .as_millis().min(u128::from(u64::MAX)) as u64,
            report_length, slots: vec![Slot::default(); capacity], bytes: vec![0; capacity * report_length],
            head: 0, count: 0, overflow: 0, oversized: 0, acknowledged: 0, stop_reason: None,
        };
        *guard = Some(ring);
        self.session.store(session, Ordering::Relaxed);
        self.lost.store(0, Ordering::Relaxed);
        self.resolved.store(0, Ordering::Relaxed);
        self.deadline_us.store(self.micros(now).saturating_add(u64::from(lease_ms) * 1_000), Ordering::Release);
        self.state.store((epoch << 32) | ACTIVE, Ordering::Release);
        Ok(self.status(guard.as_ref().unwrap(), now))
    }
    fn retry_start(&self, start_id: u64, capacity_bytes: usize, lease_ms: u32,
                   now: Instant) -> Result<Option<CaptureStatus>, CaptureError> {
        let mut guard = self.ring.lock().map_err(|_| CaptureError::Busy)?;
        let Some(ring) = guard.as_mut().filter(|ring| ring.start_id == start_id) else { return Ok(None); };
        if ring.capacity_bytes != capacity_bytes || ring.initial_lease_ms != lease_ms {
            return Err(CaptureError::Invalid("start ID was reused with different parameters"));
        }
        self.expire(ring, now);
        Ok(Some(self.status(ring, now)))
    }
    fn record(&self, session: u64, bytes: &[u8], ready: Instant, auxiliary: bool) {
        let mut state = self.state.load(Ordering::Acquire);
        if session == 0 || self.session.load(Ordering::Relaxed) != session { return; }
        loop {
            if state & ACTIVE == 0 { return; }
            if state & SEQUENCE_MASK == SEQUENCE_MASK { return; }
            match self.state.compare_exchange(state, state + 1, Ordering::AcqRel, Ordering::Acquire) {
                Ok(_) => break,
                Err(changed) => {
                    // A stop/replacement must never reserve against another epoch.
                    if changed >> 32 != state >> 32 { return; }
                    state = changed;
                }
            }
        }
        let sequence = (state & SEQUENCE_MASK) + 1;
        if let Ok(mut guard) = self.ring.try_lock() {
            let ring = guard.as_mut().expect("active capture owns preallocated storage");
            if bytes.len() > ring.report_length {
                ring.oversized += 1;
            } else {
                if ring.count == ring.slots.len() {
                    ring.head = (ring.head + 1) % ring.slots.len();
                    ring.count -= 1;
                    ring.overflow += 1;
                }
                let index = (ring.head + ring.count) % ring.slots.len();
                let start = index * ring.report_length;
                ring.bytes[start..start + bytes.len()].copy_from_slice(bytes);
                ring.slots[index] = Slot { sequence, length: bytes.len(), auxiliary,
                    elapsed_us: ready.saturating_duration_since(ring.origin).as_micros()
                        .min(u128::from(u64::MAX)) as u64 };
                ring.count += 1;
            }
            self.resolved.fetch_add(1, Ordering::Release);
        } else {
            self.lost.fetch_add(1, Ordering::Relaxed);
            self.resolved.fetch_add(1, Ordering::Release);
        }
    }
    fn checked<'a>(&self, guard: &'a mut Option<Ring>, epoch: u64, session: u64)
        -> Result<&'a mut Ring, CaptureError> {
        let ring = guard.as_mut().ok_or(CaptureError::Conflict)?;
        if ring.epoch != epoch || ring.session != session { return Err(CaptureError::Conflict); }
        Ok(ring)
    }
    fn read(&self, epoch: u64, session: u64, after_sequence: u64, limit: usize,
            acknowledge_through: u64, lease_ms: u32, now: Instant) -> Result<CaptureBatch, CaptureError> {
        validate_capture_read(after_sequence, limit, acknowledge_through, lease_ms)?;
        let mut guard = self.ring.lock().map_err(|_| CaptureError::Busy)?;
        let ring = self.checked(&mut guard, epoch, session)?;
        self.expire(ring, now);
        let last = self.state.load(Ordering::Acquire) & SEQUENCE_MASK;
        if after_sequence > last || after_sequence < ring.acknowledged {
            return Err(CaptureError::Invalid("cursor is outside the retained capture sequence"));
        }
        if after_sequence > self.resolved.load(Ordering::Acquire) {
            return Err(CaptureError::Invalid("cursor skips unresolved report reservations"));
        }
        // A duplicate request can repeat a previous acknowledgment safely.
        if acknowledge_through > ring.acknowledged {
            ring.acknowledged = acknowledge_through;
            while ring.count != 0 && ring.slots[ring.head].sequence <= acknowledge_through {
                ring.head = (ring.head + 1) % ring.slots.len();
                ring.count -= 1;
            }
        }
        // Valid reads renew ownership even after Stop, while keeping the tap
        // frozen. Long drains stay protected; abandoned frozen rings expire.
        self.deadline_us.store(self.micros(now).saturating_add(u64::from(lease_ms) * 1_000), Ordering::Release);
        let mut packets = Vec::new();
        let mut budget = 0;
        let mut next = after_sequence;
        let mut remaining = false;
        for offset in 0..ring.count {
            let index = (ring.head + offset) % ring.slots.len();
            let slot = ring.slots[index];
            if slot.sequence <= after_sequence { continue; }
            let cost = slot.length * 2 + PACKET_JSON_OVERHEAD;
            if packets.len() == limit || budget + cost > CAPTURE_PACKET_JSON_BUDGET {
                remaining = true;
                break;
            }
            budget += cost;
            let start = index * ring.report_length;
            packets.push(CapturePacket { sequence: slot.sequence, elapsed_us: slot.elapsed_us,
                auxiliary: slot.auxiliary, bytes: ring.bytes[start..start + slot.length].to_vec() });
            next = slot.sequence;
        }
        let status = self.status(ring, now);
        // Only one selected session thread produces reports. Once all its
        // reservations resolve, gaps above the last retained report are final.
        if !remaining && status.pending_reports == 0 { next = status.last_sequence; }
        Ok(CaptureBatch { capture: status, packets, next_sequence: next })
    }
    fn stop(&self, epoch: u64, session: u64, now: Instant) -> Result<CaptureStatus, CaptureError> {
        let mut guard = self.ring.lock().map_err(|_| CaptureError::Busy)?;
        let ring = self.checked(&mut guard, epoch, session)?;
        self.expire(ring, now);
        if self.state.fetch_and(!ACTIVE, Ordering::AcqRel) & ACTIVE != 0 {
            ring.stop_reason = Some(CaptureStopReason::Requested);
        }
        Ok(self.status(ring, now))
    }
    fn release(&self, epoch: u64, session: u64) -> Result<(), CaptureError> {
        let mut guard = self.ring.lock().map_err(|_| CaptureError::Busy)?;
        if guard.is_none() && self.state.load(Ordering::Acquire) >> 32 == epoch &&
            self.session.load(Ordering::Acquire) == session {
            return Ok(());
        }
        self.checked(&mut guard, epoch, session)?;
        let state = self.state.load(Ordering::Acquire);
        if state & ACTIVE != 0 || self.resolved.load(Ordering::Acquire) != state & SEQUENCE_MASK {
            return Err(CaptureError::Busy);
        }
        *guard = None;
        Ok(())
    }
    fn freeze_session(&self, session: u64) {
        if self.session.load(Ordering::Acquire) != session { return; }
        if let Ok(mut guard) = self.ring.lock() {
            if let Some(ring) = guard.as_mut().filter(|ring| ring.session == session) {
                if self.state.fetch_and(!ACTIVE, Ordering::AcqRel) & ACTIVE != 0 {
                    ring.stop_reason = Some(CaptureStopReason::SessionEnded);
                }
            }
        }
    }
}
static FULL: FullCapture = FullCapture::new();

pub fn validate_capture_start(capacity_bytes: usize, lease_ms: u32) -> Result<(), CaptureError> {
    if !(MIN_CAPTURE_BYTES..=MAX_CAPTURE_BYTES).contains(&capacity_bytes) {
        return Err(CaptureError::Invalid("capacity must be between 64 KiB and 8 MiB"));
    }
    validate_lease(lease_ms)
}
fn validate_lease(lease_ms: u32) -> Result<(), CaptureError> {
    if !(MIN_LEASE_MS..=MAX_LEASE_MS).contains(&lease_ms) {
        return Err(CaptureError::Invalid("lease must be between 1000 and 30000 ms"));
    }
    Ok(())
}
pub fn validate_capture_read(after: u64, limit: usize, acknowledge: u64, lease_ms: u32)
    -> Result<(), CaptureError> {
    if limit == 0 || limit > MAX_CAPTURE_LIMIT {
        return Err(CaptureError::Invalid("read limit must be between 1 and 512"));
    }
    if acknowledge > after { return Err(CaptureError::Invalid("acknowledgment exceeds read cursor")); }
    validate_lease(lease_ms)
}
pub fn capture_start(start_id: u64, capacity_bytes: usize, lease_ms: u32) -> Result<CaptureStatus, CaptureError> {
    validate_capture_start(capacity_bytes, lease_ms)?;
    if start_id == 0 { return Err(CaptureError::Invalid("start ID must be nonzero")); }
    if let Some(status) = FULL.retry_start(start_id, capacity_bytes, lease_ms, Instant::now())? {
        return Ok(status);
    }
    let (session, device, report_length, auxiliary_parser) = {
        let capture = CAPTURE.lock().map_err(|_| CaptureError::Busy)?;
        let selected = SELECTED.load(Ordering::Acquire);
        capture.devices.iter().find(|(id, _, _, _)| *id == selected).cloned()
            .ok_or(CaptureError::Invalid("no selected tablet session"))?
    };
    let status = FULL.start(start_id, session, device, report_length, auxiliary_parser,
        capacity_bytes, lease_ms, Instant::now())?;
    // Registration may have ended between the metadata snapshot and arming.
    if SELECTED.load(Ordering::Acquire) != session {
        FULL.freeze_session(session);
        return FULL.stop(status.epoch, session, Instant::now());
    }
    Ok(status)
}
pub fn capture_read(epoch: u64, session: u64, after: u64, limit: usize, acknowledge: u64,
                    lease_ms: u32) -> Result<CaptureBatch, CaptureError> {
    FULL.read(epoch, session, after, limit, acknowledge, lease_ms, Instant::now())
}
pub fn capture_stop(epoch: u64, session: u64) -> Result<CaptureStatus, CaptureError> {
    FULL.stop(epoch, session, Instant::now())
}
pub fn capture_release(epoch: u64, session: u64) -> Result<(), CaptureError> {
    FULL.release(epoch, session)
}
/// Called by the daemon's bounded control poll even when no reports arrive.
pub fn capture_poll() {
    if FULL.state.load(Ordering::Acquire) & ACTIVE == 0 { return; }
    if let Ok(mut guard) = FULL.ring.try_lock() {
        if let Some(ring) = guard.as_mut() { FULL.expire(ring, Instant::now()); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    fn begin(engine: &FullCapture, report_length: usize, capacity: usize, now: Instant) -> CaptureStatus {
        engine.start(1, 7, Device { name: "fixture".into(), parser: "primary".into() },
            report_length, Some("auxiliary".into()), capacity, 1_000, now).unwrap()
    }
    #[test]
    fn full_capture_retry_ack_stop_release_and_epoch_are_consistent() {
        let engine = FullCapture::new();
        let now = Instant::now();
        let started = begin(&engine, 8, MIN_CAPTURE_BYTES, now);
        engine.record(7, &[1, 2], now + Duration::from_micros(100), false);
        engine.record(7, &[3], now + Duration::from_micros(200), true);
        let retry = begin(&engine, 8, MIN_CAPTURE_BYTES, now);
        assert_eq!(retry.epoch, started.epoch);
        assert_eq!(retry.last_sequence, 2);
        let first = engine.read(started.epoch, 7, 0, 512, 0, 1_000, now).unwrap();
        let repeated = engine.read(started.epoch, 7, 0, 512, 0, 1_000, now).unwrap();
        assert_eq!(first.packets.iter().map(|p| &p.bytes).collect::<Vec<_>>(),
            repeated.packets.iter().map(|p| &p.bytes).collect::<Vec<_>>());
        assert_eq!(first.next_sequence, 2);
        assert_eq!(first.packets[0].elapsed_us, 100);
        assert!(first.packets[1].auxiliary);
        let acknowledged = engine.read(started.epoch, 7, 2, 512, 2, 1_000, now).unwrap();
        assert!(acknowledged.packets.is_empty());
        assert_eq!(acknowledged.capture.acknowledged_sequence, 2);
        assert!(engine.read(started.epoch, 7, 0, 512, 0, 1_000, now).is_err());
        let stopped = engine.stop(started.epoch, 7, now).unwrap();
        engine.record(7, &[4], now, false);
        assert_eq!(stopped.last_sequence, 2);
        assert_eq!(stopped.stop_reason, Some(CaptureStopReason::Requested));
        assert_eq!(engine.stop(started.epoch, 7, now).unwrap().last_sequence, 2);
        engine.release(started.epoch, 7).unwrap();
        engine.release(started.epoch, 7).unwrap();
        let next = engine.start(2, 7, Device { name: "fixture".into(), parser: "primary".into() },
            8, None, MIN_CAPTURE_BYTES, 1_000, now).unwrap();
        assert_ne!(started.epoch, next.epoch);
        assert!(matches!(engine.stop(started.epoch, 7, now), Err(CaptureError::Conflict)));
    }
    #[test]
    fn full_capture_counts_contention_overflow_oversize_and_skips_only_resolved_gaps() {
        let engine = FullCapture::new();
        let now = Instant::now();
        let started = begin(&engine, 32_768, MIN_CAPTURE_BYTES, now);
        assert_eq!(started.capacity_reports, 1);
        // Holding the control lock must never block the report tap.
        let guard = engine.ring.lock().unwrap();
        engine.record(7, &[1], now, false);
        drop(guard);
        engine.record(7, &[2], now, false);
        engine.record(7, &[3], now, false);
        engine.record(7, &[4; 32_769], now, false);
        let batch = engine.read(started.epoch, 7, 0, 512, 0, 1_000, now).unwrap();
        assert_eq!(batch.capture.last_sequence, 4);
        assert_eq!(batch.capture.resolved_reports, 4);
        assert_eq!(batch.capture.lost_tap, 1);
        assert_eq!(batch.capture.overflow, 1);
        assert_eq!(batch.capture.oversized, 1);
        assert_eq!(batch.packets.len(), 1);
        assert_eq!(batch.packets[0].sequence, 3);
        assert_eq!(batch.packets[0].bytes, [3]);
        assert_eq!(batch.next_sequence, 4);
    }
    #[test]
    fn full_capture_freezes_pending_reservations_without_reusing_their_counters() {
        let engine = FullCapture::new();
        let now = Instant::now();
        let started = begin(&engine, 8, MIN_CAPTURE_BYTES, now);
        // A report reserved before Stop but has not yet attempted its try_lock.
        engine.state.fetch_add(1, Ordering::AcqRel);
        let stopped = engine.stop(started.epoch, 7, now).unwrap();
        assert_eq!(stopped.last_sequence, 1);
        assert_eq!(stopped.pending_reports, 1);
        assert_eq!(engine.release(started.epoch, 7), Err(CaptureError::Busy));
        let pending = engine.read(started.epoch, 7, 0, 512, 0, 1_000, now).unwrap();
        assert_eq!(pending.next_sequence, 0);
        assert!(engine.read(started.epoch, 7, 1, 512, 1, 1_000, now).is_err());
        // That report fails the nonblocking tap while Stop owns the lock.
        engine.lost.fetch_add(1, Ordering::Relaxed);
        engine.resolved.fetch_add(1, Ordering::Release);
        let drained = engine.read(started.epoch, 7, 0, 512, 0, 1_000, now).unwrap();
        assert_eq!(drained.next_sequence, 1);
        assert_eq!(drained.capture.pending_reports, 0);
        assert_eq!(drained.capture.lost_tap, 1);
        engine.release(started.epoch, 7).unwrap();
    }
    #[test]
    fn full_capture_lease_renewal_expiry_disconnect_and_foreign_session() {
        let engine = FullCapture::new();
        let now = Instant::now();
        let started = begin(&engine, 8, MIN_CAPTURE_BYTES, now);
        engine.record(99, &[9], now, false);
        engine.read(started.epoch, 7, 0, 512, 0, 1_000, now + Duration::from_millis(500)).unwrap();
        engine.record(7, &[1], now + Duration::from_millis(1_200), false);
        let expired = engine.read(started.epoch, 7, 0, 512, 0, 1_000,
            now + Duration::from_millis(1_500)).unwrap();
        assert!(!expired.capture.active);
        assert_eq!(expired.capture.last_sequence, 1);
        assert_eq!(expired.capture.stop_reason, Some(CaptureStopReason::LeaseExpired));
        assert_eq!(expired.capture.lease_remaining_ms, 1_000);
        let replacement = engine.start(2, 8, Device { name: "other".into(), parser: "parser".into() },
            8, None, MIN_CAPTURE_BYTES, 1_000, now + Duration::from_secs(3)).unwrap();
        assert_ne!(replacement.epoch, started.epoch);
        engine.freeze_session(7);
        assert!(engine.read(replacement.epoch, 8, 0, 512, 0, 1_000,
            now + Duration::from_secs(3)).unwrap().capture.active);
        engine.freeze_session(8);
        let ended = engine.stop(replacement.epoch, 8, now + Duration::from_secs(3)).unwrap();
        assert_eq!(ended.stop_reason, Some(CaptureStopReason::SessionEnded));
    }
    #[test]
    fn full_capture_byte_budget_preserves_complete_largest_reports() {
        let engine = FullCapture::new();
        let now = Instant::now();
        let started = begin(&engine, MAX_BYTES, MIN_CAPTURE_BYTES * 3, now);
        let bytes = vec![0xab; MAX_BYTES];
        engine.record(7, &bytes, now, false);
        engine.record(7, &bytes, now, false);
        engine.stop(started.epoch, 7, now).unwrap();
        let first = engine.read(started.epoch, 7, 0, 512, 0, 1_000, now).unwrap();
        assert_eq!(first.packets.len(), 1);
        assert_eq!(first.packets[0].bytes.len(), MAX_BYTES);
        assert_eq!(first.next_sequence, 1);
        let second = engine.read(started.epoch, 7, 1, 512, 1, 1_000, now).unwrap();
        assert_eq!(second.packets.len(), 1);
        assert_eq!(second.next_sequence, 2);
        assert_eq!(second.capture.overflow, 0);
    }
    #[test]
    fn full_capture_limits_and_start_reuse_reject_malformed_clients() {
        assert!(validate_capture_start(MIN_CAPTURE_BYTES - 1, 1_000).is_err());
        assert!(validate_capture_start(MAX_CAPTURE_BYTES + 1, 1_000).is_err());
        assert!(validate_capture_start(MIN_CAPTURE_BYTES, 0).is_err());
        assert!(validate_capture_read(0, 0, 0, 1_000).is_err());
        assert!(validate_capture_read(0, 513, 0, 1_000).is_err());
        assert!(validate_capture_read(0, 1, 1, 1_000).is_err());
        let engine = FullCapture::new();
        let now = Instant::now();
        let started = begin(&engine, 8, MIN_CAPTURE_BYTES, now);
        assert!(engine.start(1, 7, started.device, 8, None, MIN_CAPTURE_BYTES * 2,
            1_000, now).is_err());
        assert!(engine.read(started.epoch, 7, 1, 1, 0, 1_000, now).is_err());
    }
    #[test]
    fn abandoned_requested_and_disconnected_captures_release_ownership_after_lease() {
        let now = Instant::now();
        for reason in [CaptureStopReason::Requested, CaptureStopReason::SessionEnded] {
            let engine = FullCapture::new();
            let started = begin(&engine, 8, MIN_CAPTURE_BYTES, now);
            if reason == CaptureStopReason::Requested {
                engine.stop(started.epoch, 7, now).unwrap();
            } else { engine.freeze_session(7); }
            let replacement = |at| engine.start(2, 8, Device { name: "new".into(), parser: "parser".into() },
                8, None, MIN_CAPTURE_BYTES, 1_000, at);
            assert!(matches!(replacement(now + Duration::from_millis(999)), Err(CaptureError::Busy)));
            let reclaimed = replacement(now + Duration::from_millis(1_000)).unwrap();
            assert!(reclaimed.active);
            assert_ne!(reclaimed.epoch, started.epoch);
            assert!(matches!(engine.stop(started.epoch, 7, now), Err(CaptureError::Conflict)));
        }
    }
    #[test]
    fn frozen_drain_renews_ownership_without_rearming_the_report_tap() {
        let engine = FullCapture::new();
        let now = Instant::now();
        let started = begin(&engine, 8, MIN_CAPTURE_BYTES, now);
        engine.stop(started.epoch, 7, now).unwrap();
        let drain = engine.read(started.epoch, 7, 0, 512, 0, 1_000,
            now + Duration::from_millis(900)).unwrap();
        assert!(!drain.capture.active);
        assert_eq!(drain.capture.stop_reason, Some(CaptureStopReason::Requested));
        assert_eq!(drain.capture.lease_remaining_ms, 1_000);
        engine.record(7, &[1], now + Duration::from_millis(1_200), false);
        let replacement = |at| engine.start(2, 8, Device { name: "new".into(), parser: "parser".into() },
            8, None, MIN_CAPTURE_BYTES, 1_000, at);
        assert!(matches!(replacement(now + Duration::from_millis(1_200)), Err(CaptureError::Busy)));
        let final_status = engine.stop(started.epoch, 7, now + Duration::from_millis(1_300)).unwrap();
        assert_eq!(final_status.last_sequence, 0);
        assert_eq!(final_status.stop_reason, Some(CaptureStopReason::Requested));
        assert!(replacement(now + Duration::from_millis(1_900)).unwrap().active);
    }
    #[test]
    fn report_crossing_old_deadline_cannot_freeze_a_control_read_renewal() {
        let engine = FullCapture::new();
        let now = Instant::now();
        let started = begin(&engine, 8, MIN_CAPTURE_BYTES, now);
        // Read took its timestamp at 900 ms and acquired the mutex. Its report
        // producer reaches the old deadline before Read publishes renewal.
        // Previously the producer froze ACTIVE from that stale deadline while
        // Read held the mutex, despite the valid concurrent renewal.
        let mut guard = engine.ring.lock().unwrap();
        engine.record(7, &[1], now + Duration::from_millis(1_000), false);
        engine.deadline_us.store(1_900_000, Ordering::Release);
        engine.expire(guard.as_mut().unwrap(), now + Duration::from_millis(900));
        drop(guard);
        let batch = engine.read(started.epoch, 7, 0, 512, 0, 1_000,
            now + Duration::from_millis(1_000)).unwrap();
        assert!(batch.capture.active);
        assert_eq!(batch.capture.stop_reason, None);
        assert_eq!(batch.capture.last_sequence, 1);
        assert_eq!(batch.capture.lost_tap, 1);
        assert_eq!(batch.capture.pending_reports, 0);
    }
    #[test]
    fn control_poll_expiration_freezes_sequence_limit_explicitly() {
        let engine = FullCapture::new();
        let now = Instant::now();
        let started = begin(&engine, 8, MIN_CAPTURE_BYTES, now);
        engine.state.store((started.epoch << 32) | ACTIVE | SEQUENCE_MASK, Ordering::Release);
        engine.resolved.store(SEQUENCE_MASK, Ordering::Release);
        let mut guard = engine.ring.lock().unwrap();
        let ring = guard.as_mut().unwrap();
        engine.expire(ring, now);
        let status = engine.status(ring, now);
        assert!(!status.active);
        assert_eq!(status.stop_reason, Some(CaptureStopReason::SequenceLimit));
        assert_eq!(status.last_sequence, SEQUENCE_MASK);
    }
    #[test]
    fn packets_labels_and_lifetimes_belong_to_one_session() {
        let first = Registration::with_selection_key(Device {
            name: "A".into(),
            parser: "ParserA".into(),
        }, 8, None, Some("fixture-a".into()));
        ARMED.store(0, Ordering::Relaxed);
        record(&[1]);
        assert!(poll().bytes.is_empty());
        record(&[2]);
        let second = Registration::with_selection_key(Device {
            name: "B".into(),
            parser: "ParserB".into(),
        }, 8, None, Some("fixture-b".into()));
        record(&[3]);
        let snapshot = poll();
        assert_eq!(snapshot.device.unwrap().name, "A");
        assert_eq!(snapshot.bytes, [2]);
        assert_eq!(snapshot.sequence, 1);
        select_device_key("fixture-b").unwrap();
        assert!(poll().bytes.is_empty());
        record(&[4]);
        assert_eq!(poll().device.unwrap().name, "B");
        assert_eq!(poll().bytes, [4]);
        let capture = capture_start(7_777, MIN_CAPTURE_BYTES, 1_000).unwrap();
        assert_eq!(select_device_key("fixture-a"), Err(CaptureError::Busy));
        record(&[5]);
        let stopped = capture_stop(capture.epoch, capture.session).unwrap();
        assert_eq!(stopped.pending_reports, 0);
        // Frozen ownership remains exclusive until the original token drains.
        assert_eq!(select_device_key("fixture-a"), Err(CaptureError::Busy));
        let batch = capture_read(capture.epoch, capture.session, 0, 512, 0, 1_000).unwrap();
        assert_eq!(batch.packets[0].bytes, [5]);
        capture_release(capture.epoch, capture.session).unwrap();
        select_device_key("fixture-a").unwrap();
        drop(second);
        assert_eq!(poll().device.unwrap().name, "A");
        drop(first);
        assert_eq!(poll().device, None);
        assert!(poll().bytes.is_empty());
        // The same stable identity reconnects with a new raw endpoint session.
        let reconnected = Registration::with_selection_key(Device { name: "A".into(), parser: "ParserA".into() },
            8, None, Some("fixture-a".into()));
        assert_eq!(poll().device.unwrap().name, "A");
        record(&[6]);
        assert_eq!(poll().bytes, [6]);
        drop(reconnected);
        // This fixture is the sole global registration test; restore its
        // preference so independent local FullCapture fixtures stay isolated.
        CAPTURE.lock().unwrap().selected_key = None;
    }
}
