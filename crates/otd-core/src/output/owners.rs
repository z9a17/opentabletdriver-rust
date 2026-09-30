//! Acknowledged mouse state shared by concurrent tablet sessions.
use super::{MousePacket, flags};
use std::io;

#[derive(Clone, Copy)]
pub struct Owner(usize);

#[derive(Default)]
struct State {
    contact: bool,
    abandoned: bool,
}

#[derive(Default)]
pub struct OutputOwners {
    owners: Vec<Option<State>>,
    held: usize,
    position: Option<(i32, i32)>,
    pending_recovery: bool,
}

impl OutputOwners {
    pub const fn new() -> Self {
        Self {
            owners: Vec::new(),
            held: 0,
            position: None,
            pending_recovery: false,
        }
    }

    pub fn register(&mut self) -> Owner {
        // A new session must restore its position even when a persistent
        // binding owner remains registered and the physical mouse has moved.
        self.position = None;
        if let Some(index) = self.owners.iter().position(Option::is_none) {
            self.owners[index] = Some(State::default());
            Owner(index)
        } else {
            let index = self.owners.len();
            self.owners.push(Some(State::default()));
            Owner(index)
        }
    }

    /// Neither local ownership nor global dedup state changes on send failure.
    pub fn send(
        &mut self,
        owner: Owner,
        mut packet: MousePacket,
        send: impl FnOnce(MousePacket) -> io::Result<()>,
    ) -> io::Result<()> {
        let state = self
            .owners
            .get(owner.0)
            .and_then(Option::as_ref)
            .ok_or_else(|| io::Error::other("mouse output owner is no longer registered"))?;
        let contact = if packet.flags & flags::LEFTUP != 0 {
            false
        } else if packet.flags & flags::LEFTDOWN != 0 {
            true
        } else {
            state.contact
        };
        let held = self.held - usize::from(state.contact) + usize::from(contact);
        packet.flags &= !(flags::LEFTDOWN | flags::LEFTUP);
        if self.held == 0 && held != 0 {
            packet.flags |= flags::LEFTDOWN;
        }
        if self.held != 0 && held == 0 {
            packet.flags |= flags::LEFTUP;
        }
        let absolute =
            packet.flags & (flags::MOVE | flags::ABSOLUTE) == flags::MOVE | flags::ABSOLUTE;
        let position = (packet.dx, packet.dy);
        // A physical mouse may have moved since our last packet. A contact
        // transition must restore the tablet position before pressing/releasing.
        if absolute
            && self.position == Some(position)
            && packet.flags & (flags::LEFTDOWN | flags::LEFTUP) == 0
        {
            packet.flags &= !flags::MOVE;
        }
        if packet.flags & (flags::MOVE | flags::LEFTDOWN | flags::LEFTUP) != 0 {
            send(packet)?;
        }
        self.held = held;
        if let Some(state) = &mut self.owners[owner.0] {
            state.contact = contact;
        }
        if absolute {
            self.position = Some(position);
        } else if packet.flags & flags::MOVE != 0 {
            self.position = None;
        }
        Ok(())
    }

    pub fn release(
        &mut self,
        owner: Owner,
        send: impl FnOnce(MousePacket) -> io::Result<()>,
    ) -> io::Result<()> {
        self.send(
            owner,
            MousePacket {
                dx: 0,
                dy: 0,
                flags: flags::LEFTUP,
            },
            send,
        )?;
        self.owners[owner.0] = None;
        if self.owners.iter().all(Option::is_none) {
            self.position = None;
        }
        Ok(())
    }

    /// A failed cleanup remains owned until recovery is acknowledged.
    pub fn abandon(&mut self, owner: Owner) {
        if let Some(state) = self.owners.get_mut(owner.0).and_then(Option::as_mut) {
            state.abandoned = true;
        }
        self.pending_recovery = true;
    }

    pub fn recover(
        &mut self,
        mut send: impl FnMut(MousePacket) -> io::Result<()>,
    ) -> io::Result<()> {
        if !self.pending_recovery {
            return Ok(());
        }
        for index in 0..self.owners.len() {
            if self.owners[index]
                .as_ref()
                .is_some_and(|state| state.abandoned)
            {
                self.release(Owner(index), &mut send)?;
            }
        }
        self.pending_recovery = false;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn alternating_tablets_restore_positions_after_the_other_moves() {
        let mut owners = OutputOwners::new();
        let a = owners.register();
        let b = owners.register();
        let mut first = super::super::MouseOutput::new();
        let mut second = super::super::MouseOutput::new();
        first.share_position();
        second.share_position();
        let mut positions = Vec::new();
        let mut output = |packet: MousePacket| {
            positions.push((packet.dx, packet.dy));
            Ok(())
        };
        first
            .emit_mapped(Some((100, 100)), false, |p| owners.send(a, p, &mut output))
            .unwrap();
        second
            .emit_mapped(Some((200, 200)), false, |p| owners.send(b, p, &mut output))
            .unwrap();
        first
            .emit_mapped(Some((100, 100)), false, |p| owners.send(a, p, &mut output))
            .unwrap();
        first
            .emit_mapped(Some((100, 100)), false, |p| owners.send(a, p, &mut output))
            .unwrap();
        assert_eq!(positions, [(100, 100), (200, 200), (100, 100)]);
    }

    #[test]
    fn shared_successful_output_does_not_allocate() {
        let mut owners = OutputOwners::new();
        let a = owners.register();
        let b = owners.register();
        crate::test_alloc::assert_no_allocations(|| {
            for i in 0..1000 {
                let packet = MousePacket {
                    dx: i,
                    dy: i,
                    flags: flags::MOVE | flags::ABSOLUTE,
                };
                owners
                    .send(if i % 2 == 0 { a } else { b }, packet, |_| Ok(()))
                    .unwrap();
            }
        });
    }
    #[test]
    fn overlapping_contacts_and_failed_cleanup_keep_their_owners() {
        let mut owners = OutputOwners::new();
        let a = owners.register();
        let b = owners.register();
        let down = MousePacket {
            dx: 0,
            dy: 0,
            flags: flags::LEFTDOWN,
        };
        let mut packets = Vec::new();
        owners
            .send(a, down, |p| {
                packets.push(p.flags);
                Ok(())
            })
            .unwrap();
        owners
            .send(b, down, |p| {
                packets.push(p.flags);
                Ok(())
            })
            .unwrap();
        owners
            .release(a, |p| {
                packets.push(p.flags);
                Ok(())
            })
            .unwrap();
        assert_eq!(packets, [flags::LEFTDOWN]);
        assert!(
            owners
                .release(b, |_| Err(io::Error::other("output unavailable")))
                .is_err()
        );
        owners.abandon(b);
        owners
            .recover(|p| {
                packets.push(p.flags);
                Ok(())
            })
            .unwrap();
        assert_eq!(packets, [flags::LEFTDOWN, flags::LEFTUP]);
    }
}
