//! Sends the core's mouse packets with `SendInput`.

use std::cell::Cell;
use std::io;
use std::marker::PhantomData;
use std::mem::size_of;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering, compiler_fence};

use windows_sys::Win32::Foundation::SetLastError;
use windows_sys::Win32::System::Threading::FlushProcessWriteBuffers;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP,
    MOUSEEVENTF_MOVE, MOUSEEVENTF_VIRTUALDESK, MOUSEINPUT, SendInput,
};

use otd_core::output::owners::{OutputOwners, Owner};
use otd_core::output::{MousePacket, flags};

static OWNERS: Mutex<OutputOwners> = Mutex::new(OutputOwners::new());

// Asymmetric single-owner fast path: when exactly one `SessionOutput` is active
// and no abandoned-owner recovery is pending, `send` avoids acquiring `OWNERS`
// and issues zero `LOCK`-prefixed instructions. Cold transitions (`new`,
// `finish`, `drop`) set `SLOW_PATH`, call `FlushProcessWriteBuffers()`, drain
// `FAST_BUSY`, and sync the sole owner's state back into `OWNERS`.
static ACTIVE_OWNERS: AtomicUsize = AtomicUsize::new(0);
static HAS_RECOVERY: AtomicBool = AtomicBool::new(false);
static SLOW_PATH: AtomicBool = AtomicBool::new(true);
static FAST_BUSY: AtomicBool = AtomicBool::new(false);
static FAST_CONTACT: AtomicBool = AtomicBool::new(false);
static FAST_POS_VALID: AtomicBool = AtomicBool::new(false);
static FAST_POS_XY: AtomicU64 = AtomicU64::new(0);
static FAST_OWNER: Mutex<Option<Owner>> = Mutex::new(None);

#[inline]
fn pack_pos(dx: i32, dy: i32) -> u64 {
    u64::from(dx as u32) | (u64::from(dy as u32) << 32)
}

#[inline]
fn unpack_pos(packed: u64) -> (i32, i32) {
    (packed as u32 as i32, (packed >> 32) as u32 as i32)
}

/// Demotes from single-owner fast path to `OWNERS` slow path and replays the
/// fast owner's contact and absolute position into `OWNERS` without injecting
/// OS input. Must be called while holding `OWNERS`.
fn demote_fast_to_owners(owners: &mut OutputOwners) {
    if SLOW_PATH.load(Ordering::Acquire) {
        return;
    }
    SLOW_PATH.store(true, Ordering::Relaxed);
    unsafe { FlushProcessWriteBuffers() };
    while FAST_BUSY.load(Ordering::Acquire) {
        std::hint::spin_loop();
    }
    let owner = FAST_OWNER.lock().ok().and_then(|mut slot| slot.take());
    if let Some(owner) = owner {
        let contact = FAST_CONTACT.load(Ordering::Relaxed);
        let _ = owners.send(
            owner,
            MousePacket {
                dx: 0,
                dy: 0,
                flags: if contact {
                    flags::LEFTDOWN
                } else {
                    flags::LEFTUP
                },
            },
            |_| Ok(()),
        );
        if FAST_POS_VALID.load(Ordering::Relaxed) {
            let (dx, dy) = unpack_pos(FAST_POS_XY.load(Ordering::Relaxed));
            let _ = owners.send(
                owner,
                MousePacket {
                    dx,
                    dy,
                    flags: flags::MOVE | flags::ABSOLUTE | flags::VIRTUALDESK,
                },
                |_| Ok(()),
            );
        } else {
            let _ = owners.send(
                owner,
                MousePacket {
                    dx: 1,
                    dy: 0,
                    flags: flags::MOVE,
                },
                |_| Ok(()),
            );
        }
    }
}

pub struct SessionOutput {
    owner: Option<Owner>,
    // Single-owner fast path relies on a single `SessionOutput` not being
    // called concurrently from multiple threads (`Send` + `!Sync`).
    _not_sync: PhantomData<Cell<()>>,
}

impl SessionOutput {
    pub fn new() -> io::Result<Self> {
        let mut owners = OWNERS
            .lock()
            .map_err(|_| io::Error::other("output ownership lock poisoned"))?;
        demote_fast_to_owners(&mut owners);
        let owner = owners.register();
        let count = ACTIVE_OWNERS.fetch_add(1, Ordering::AcqRel) + 1;
        if count == 1 && !HAS_RECOVERY.load(Ordering::Acquire) {
            FAST_CONTACT.store(false, Ordering::Relaxed);
            FAST_POS_VALID.store(false, Ordering::Relaxed);
            if let Ok(mut slot) = FAST_OWNER.lock() {
                *slot = Some(owner);
                SLOW_PATH.store(false, Ordering::Release);
            }
        }
        Ok(Self {
            owner: Some(owner),
            _not_sync: PhantomData,
        })
    }

    #[inline]
    pub fn send(&self, packet: MousePacket) -> io::Result<()> {
        self.send_with(packet, send_input)
    }

    #[inline]
    fn send_with(
        &self,
        packet: MousePacket,
        sink: impl FnMut(MousePacket) -> io::Result<()>,
    ) -> io::Result<()> {
        let owner = self
            .owner
            .ok_or_else(|| io::Error::other("output session ended"))?;
        if !SLOW_PATH.load(Ordering::Acquire) {
            struct FastBusyGuard;
            impl Drop for FastBusyGuard {
                #[inline]
                fn drop(&mut self) {
                    FAST_BUSY.store(false, Ordering::Release);
                }
            }
            FAST_BUSY.store(true, Ordering::Relaxed);
            let _busy = FastBusyGuard;
            compiler_fence(Ordering::SeqCst);
            if !SLOW_PATH.load(Ordering::Acquire) {
                return send_fast_with(packet, sink);
            }
        }
        self.send_slow_with(owner, packet, sink)
    }

    #[cold]
    fn send_slow_with(
        &self,
        owner: Owner,
        packet: MousePacket,
        mut sink: impl FnMut(MousePacket) -> io::Result<()>,
    ) -> io::Result<()> {
        let mut owners = OWNERS
            .lock()
            .map_err(|_| io::Error::other("output ownership lock poisoned"))?;
        owners.recover(&mut sink)?;
        HAS_RECOVERY.store(false, Ordering::Release);
        owners.send(owner, packet, sink)
    }

    pub fn finish(&mut self) -> io::Result<()> {
        self.finish_with(send_input)
    }

    fn finish_with(&mut self, sink: impl FnOnce(MousePacket) -> io::Result<()>) -> io::Result<()> {
        if let Some(owner) = self.owner {
            let mut owners = OWNERS
                .lock()
                .map_err(|_| io::Error::other("output ownership lock poisoned"))?;
            demote_fast_to_owners(&mut owners);
            owners.release(owner, sink)?;
            self.owner = None;
            ACTIVE_OWNERS.fetch_sub(1, Ordering::AcqRel);
        }
        Ok(())
    }
}

#[inline]
fn send_fast_with(
    mut packet: MousePacket,
    mut sink: impl FnMut(MousePacket) -> io::Result<()>,
) -> io::Result<()> {
    let prev_contact = FAST_CONTACT.load(Ordering::Relaxed);
    let next_contact = if packet.flags & flags::LEFTUP != 0 {
        false
    } else if packet.flags & flags::LEFTDOWN != 0 {
        true
    } else {
        prev_contact
    };
    packet.flags &= !(flags::LEFTDOWN | flags::LEFTUP);
    if !prev_contact && next_contact {
        packet.flags |= flags::LEFTDOWN;
    }
    if prev_contact && !next_contact {
        packet.flags |= flags::LEFTUP;
    }
    let absolute = packet.flags & (flags::MOVE | flags::ABSOLUTE) == flags::MOVE | flags::ABSOLUTE;
    let position = (packet.dx, packet.dy);
    if absolute
        && FAST_POS_VALID.load(Ordering::Relaxed)
        && unpack_pos(FAST_POS_XY.load(Ordering::Relaxed)) == position
        && packet.flags & (flags::LEFTDOWN | flags::LEFTUP) == 0
    {
        packet.flags &= !flags::MOVE;
    }
    if packet.flags & (flags::MOVE | flags::LEFTDOWN | flags::LEFTUP) != 0 {
        sink(packet)?;
    }
    FAST_CONTACT.store(next_contact, Ordering::Relaxed);
    if absolute {
        FAST_POS_XY.store(pack_pos(position.0, position.1), Ordering::Relaxed);
        FAST_POS_VALID.store(true, Ordering::Relaxed);
    } else if packet.flags & flags::MOVE != 0 {
        FAST_POS_VALID.store(false, Ordering::Relaxed);
    }
    Ok(())
}

impl Drop for SessionOutput {
    fn drop(&mut self) {
        if let Some(owner) = self.owner
            && let Ok(mut owners) = OWNERS.lock()
        {
            demote_fast_to_owners(&mut owners);
            owners.abandon(owner);
            HAS_RECOVERY.store(true, Ordering::Release);
            ACTIVE_OWNERS.fetch_sub(1, Ordering::AcqRel);
        }
    }
}

// The core uses Windows' own flag values, so packets pass through unchanged.
const _: () = assert!(
    flags::MOVE == MOUSEEVENTF_MOVE
        && flags::LEFTDOWN == MOUSEEVENTF_LEFTDOWN
        && flags::LEFTUP == MOUSEEVENTF_LEFTUP
        && flags::VIRTUALDESK == MOUSEEVENTF_VIRTUALDESK
        && flags::ABSOLUTE == MOUSEEVENTF_ABSOLUTE
);

#[inline]
pub fn send_input(packet: MousePacket) -> Result<(), std::io::Error> {
    let input = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: packet.dx,
                dy: packet.dy,
                mouseData: 0,
                dwFlags: packet.flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    unsafe { SetLastError(0) };
    if unsafe { SendInput(1, &input, size_of::<INPUT>() as i32) } != 1 {
        let error = std::io::Error::last_os_error();
        return Err(if error.raw_os_error().is_some_and(|code| code != 0) {
            error
        } else {
            std::io::Error::other("SendInput accepted no mouse event; input may be blocked")
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_owner_fast_path_deduplicates_and_promotes_cleanly() {
        let mut first = SessionOutput::new().unwrap();
        assert!(
            !SLOW_PATH.load(Ordering::Acquire),
            "sole owner should enter lock-free fast path"
        );
        let mut sent = Vec::new();
        let move_a = MousePacket {
            dx: 1000,
            dy: 2000,
            flags: flags::MOVE | flags::ABSOLUTE | flags::VIRTUALDESK | flags::LEFTDOWN,
        };
        first
            .send_with(move_a, |p| {
                sent.push(p);
                Ok(())
            })
            .unwrap();
        // Repeated move at same coordinates without button change deduplicates.
        first
            .send_with(
                MousePacket {
                    dx: 1000,
                    dy: 2000,
                    flags: flags::MOVE | flags::ABSOLUTE | flags::VIRTUALDESK,
                },
                |p| {
                    sent.push(p);
                    Ok(())
                },
            )
            .unwrap();
        assert_eq!(sent.len(), 1);

        // Second owner triggers promotion and preserves first owner's held contact.
        let mut second = SessionOutput::new().unwrap();
        assert!(SLOW_PATH.load(Ordering::Acquire));
        second
            .send_with(
                MousePacket {
                    dx: 0,
                    dy: 0,
                    flags: flags::LEFTDOWN,
                },
                |p| {
                    sent.push(p);
                    Ok(())
                },
            )
            .unwrap();
        // First owner already held left button; second's press must not re-emit LEFTDOWN.
        assert_eq!(sent.len(), 1);
        first
            .finish_with(|p| {
                sent.push(p);
                Ok(())
            })
            .unwrap();
        // Second still holds left button; first's release must not lift it.
        assert_eq!(sent.len(), 1);
        second
            .finish_with(|p| {
                sent.push(p);
                Ok(())
            })
            .unwrap();
        assert_eq!(sent.len(), 2);
        assert_eq!(sent[1].flags, flags::LEFTUP);
    }
}
