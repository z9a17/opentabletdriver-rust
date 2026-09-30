//! Nonblocking debugger tap for one selected session. Metadata and packet
//! bytes are read under the same lock; other sessions cannot overwrite them.
use std::cell::Cell;
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
    devices: Vec<(u64, Device)>,
}
static CAPTURE: Mutex<Capture> = Mutex::new(Capture {
    bytes: [0; MAX_BYTES],
    length: 0,
    sequence: 0,
    devices: Vec::new(),
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
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let previous = SESSION.replace(id);
        if let Ok(mut capture) = CAPTURE.lock() {
            capture.devices.push((id, device));
            if SELECTED.load(Ordering::Relaxed) == 0 {
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
        if let Ok(mut capture) = CAPTURE.lock() {
            capture.devices.retain(|(id, _)| *id != self.id);
            if SELECTED.load(Ordering::Relaxed) == self.id {
                capture.length = 0;
                capture.sequence = 0;
                SELECTED.store(
                    capture.devices.first().map_or(0, |(id, _)| *id),
                    Ordering::Release,
                );
            }
        }
    }
}

#[inline]
pub fn record(bytes: &[u8]) {
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

pub fn poll() -> Snapshot {
    ARMED.store(ARM_REPORTS, Ordering::Relaxed);
    match CAPTURE.lock() {
        Ok(capture) => {
            let selected = SELECTED.load(Ordering::Relaxed);
            Snapshot {
                device: capture
                    .devices
                    .iter()
                    .find(|(id, _)| *id == selected)
                    .map(|(_, device)| device.clone()),
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn packets_labels_and_lifetimes_belong_to_one_session() {
        let first = Registration::new(Device {
            name: "A".into(),
            parser: "ParserA".into(),
        });
        ARMED.store(0, Ordering::Relaxed);
        record(&[1]);
        assert!(poll().bytes.is_empty());
        record(&[2]);
        let second = Registration::new(Device {
            name: "B".into(),
            parser: "ParserB".into(),
        });
        record(&[3]);
        let snapshot = poll();
        assert_eq!(snapshot.device.unwrap().name, "A");
        assert_eq!(snapshot.bytes, [2]);
        assert_eq!(snapshot.sequence, 1);
        drop(second);
        assert_eq!(poll().device.unwrap().name, "A");
        drop(first);
        assert_eq!(poll().device, None);
        assert!(poll().bytes.is_empty());
    }
}
