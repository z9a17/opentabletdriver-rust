//! The tablet debugger's view of the report thread. While a debugger polls,
//! the session loop copies each packet into one fixed slot; otherwise it
//! only reads one atomic. The slot is never locked by the report thread in a
//! way that can block: a busy slot skips that packet.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

/// Largest packet kept. Longer packets are truncated in the snapshot only.
pub const MAX_BYTES: usize = 512;

/// How many packets one poll keeps the tap armed for (about two seconds at
/// 1 kHz), so the tap turns itself off when the debugger closes.
pub const ARM_REPORTS: u32 = 2_000;

struct Slot {
    bytes: [u8; MAX_BYTES],
    length: usize,
    sequence: u64,
}

static ARMED: AtomicU32 = AtomicU32::new(0);
static RECORDED: AtomicU64 = AtomicU64::new(0);
static SLOT: Mutex<Slot> = Mutex::new(Slot {
    bytes: [0; MAX_BYTES],
    length: 0,
    sequence: 0,
});
static DEVICE: Mutex<Option<Device>> = Mutex::new(None);

/// The tablet the running session reads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Device {
    pub name: String,
    pub parser: String,
}

/// The latest packet and how many packets were recorded since the daemon started.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub device: Option<Device>,
    pub sequence: u64,
    pub bytes: Vec<u8>,
}

/// Called by the session loop for every packet.
#[inline]
pub fn record(bytes: &[u8]) {
    if ARMED.load(Ordering::Relaxed) != 0 {
        record_armed(bytes);
    }
}

#[cold]
fn record_armed(bytes: &[u8]) {
    let _ = ARMED.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |armed| {
        armed.checked_sub(1)
    });
    let sequence = RECORDED.fetch_add(1, Ordering::Relaxed) + 1;
    if let Ok(mut slot) = SLOT.try_lock() {
        let length = bytes.len().min(MAX_BYTES);
        slot.bytes[..length].copy_from_slice(&bytes[..length]);
        slot.length = length;
        slot.sequence = sequence;
    }
}

/// Arms the tap and returns the latest recorded packet, if any.
pub fn poll() -> Snapshot {
    ARMED.store(ARM_REPORTS, Ordering::Relaxed);
    let device = DEVICE.lock().ok().and_then(|device| device.clone());
    let slot = SLOT.lock();
    let (sequence, bytes) = match &slot {
        Ok(slot) => (slot.sequence, slot.bytes[..slot.length].to_vec()),
        Err(_) => (0, Vec::new()),
    };
    Snapshot {
        device,
        sequence,
        bytes,
    }
}

/// Records which tablet the session reads; `None` when it ends.
pub fn set_device(device: Option<Device>) {
    if let Ok(mut slot) = DEVICE.lock() {
        *slot = device;
    }
    if let Ok(mut slot) = SLOT.lock() {
        slot.length = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_only_while_armed() {
        set_device(Some(Device {
            name: "Tablet".into(),
            parser: "Parser".into(),
        }));
        ARMED.store(0, Ordering::Relaxed);
        record(&[1, 2, 3]);
        assert!(poll().bytes.is_empty(), "disarmed packets are not kept");
        record(&[4, 5]);
        let snapshot = poll();
        assert_eq!(snapshot.bytes, [4, 5]);
        assert_eq!(snapshot.device.unwrap().name, "Tablet");
        ARMED.store(1, Ordering::Relaxed);
        record(&[6]);
        record(&[7]);
        assert_eq!(ARMED.load(Ordering::Relaxed), 0);
        set_device(None);
    }
}
