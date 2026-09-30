//! Pen output: position, pressure, tilt and eraser with a hover/contact
//! lifecycle, for platform pen devices (Windows synthetic pointer injection,
//! a Linux virtual tablet). OpenTabletDriver has no such mode in core on
//! Windows; its Windows Ink plugin drives a VMulti digitizer. This follows
//! that plugin's observable behavior
//! (<https://github.com/X9VoiD/VoiDPlugins/tree/a69fe346b27512a34bda5e4a9481795b1ce2264b/src/OutputMode/WindowsInk>):
//! pressure is reported only while the tip binding holds contact, and a
//! switch between pen and eraser leaves range and enters it again with the
//! other tool.
//!
//! Every transition is committed only after the sink accepts its packet, so a
//! failed packet is retried by the next sample or by `release`.

use std::io;

/// Where a packet sits in the pen's lifecycle. Platform adapters derive
/// their flags from it, and never need their own state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PenPhase {
    /// In range without contact; also the first packet in range.
    Hover,
    /// Contact starts.
    Down,
    /// Contact continues.
    Contact,
    /// Contact ends; the pen stays in range.
    Up,
    /// The pen leaves range.
    Leave,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PenPacket {
    pub phase: PenPhase,
    /// Desktop pixels, inside the virtual screen.
    pub x: f64,
    pub y: f64,
    /// Fraction of the tablet's maximum pressure, 0 without contact.
    pub pressure: f32,
    /// Degrees in -90..=90, when the tablet reports tilt.
    pub tilt: Option<[f32; 2]>,
    /// The eraser end, or the pen tip inverted.
    pub eraser: bool,
    /// Barrel buttons held: bit 0 is barrel button 1, bit 2 is button 3.
    pub barrel: u8,
}

/// A platform pen device.
pub trait PenSink {
    fn send(&mut self, packet: PenPacket) -> io::Result<()>;
}

impl<F: FnMut(PenPacket) -> io::Result<()>> PenSink for F {
    fn send(&mut self, packet: PenPacket) -> io::Result<()> {
        self(packet)
    }
}

/// One input sample for the pen device.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PenSample {
    pub x: f64,
    pub y: f64,
    pub pressure: Option<u32>,
    pub tilt: Option<[f32; 2]>,
    pub eraser: bool,
    /// The tip (or eraser) binding holds contact.
    pub contact: bool,
    /// Barrel buttons held, as `PenPacket::barrel`.
    pub barrel: u8,
}

pub struct PenOutput {
    sink: Box<dyn PenSink>,
    max_pressure: f32,
    /// The last accepted packet, while the pen is in range.
    last: Option<PenPacket>,
}

impl PenOutput {
    pub fn new(sink: Box<dyn PenSink>, max_pressure: u32) -> Self {
        Self {
            sink,
            max_pressure: max_pressure.max(1) as f32,
            last: None,
        }
    }

    pub fn in_range(&self) -> bool {
        self.last.is_some()
    }

    pub fn in_contact(&self) -> bool {
        self.last
            .is_some_and(|last| matches!(last.phase, PenPhase::Down | PenPhase::Contact))
    }

    /// Sends what changed since the last accepted packet; returns whether
    /// anything was sent.
    pub fn sample(&mut self, sample: PenSample) -> io::Result<bool> {
        if !sample.x.is_finite() || !sample.y.is_finite() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "nonfinite pen position",
            ));
        }
        let mut sent = false;
        // A tool switch leaves range and enters again with the other tool.
        if self.last.is_some_and(|last| last.eraser != sample.eraser) {
            sent |= self.release()?;
        }
        let tilt = sample
            .tilt
            .filter(|tilt| tilt.iter().all(|value| value.is_finite()))
            .map(|tilt| tilt.map(|value| value.clamp(-90.0, 90.0)));
        let pressure = if sample.contact {
            sample
                .pressure
                .map_or(0.0, |raw| (raw as f32 / self.max_pressure).clamp(0.0, 1.0))
        } else {
            0.0
        };
        let packet = |phase| PenPacket {
            phase,
            x: sample.x,
            y: sample.y,
            pressure,
            tilt,
            eraser: sample.eraser,
            barrel: sample.barrel,
        };
        // Contact starts in range, as a physical pen reaches the surface.
        if sample.contact && self.last.is_none() {
            self.emit(PenPacket {
                pressure: 0.0,
                ..packet(PenPhase::Hover)
            })?;
            sent = true;
        }
        let phase = match (self.in_contact(), sample.contact) {
            (false, false) => PenPhase::Hover,
            (false, true) => PenPhase::Down,
            (true, true) => PenPhase::Contact,
            (true, false) => PenPhase::Up,
        };
        let packet = packet(phase);
        if matches!(phase, PenPhase::Hover | PenPhase::Contact)
            && self.last.is_some_and(|last| same_state(last, packet))
        {
            return Ok(sent);
        }
        self.emit(packet)?;
        Ok(true)
    }

    /// A report without a position: only contact can change, where the pen
    /// was last. Out of range there is nothing to touch.
    pub fn contact(
        &mut self,
        contact: bool,
        pressure: Option<u32>,
        barrel: u8,
    ) -> io::Result<bool> {
        let Some(last) = self.last else {
            return Ok(false);
        };
        self.sample(PenSample {
            x: last.x,
            y: last.y,
            pressure: pressure.or_else(|| {
                (last.pressure > 0.0).then(|| (last.pressure * self.max_pressure).round() as u32)
            }),
            tilt: last.tilt,
            eraser: last.eraser,
            contact,
            barrel,
        })
    }

    /// Lifts the pen and leaves range: range loss, cleanup, a tool switch.
    pub fn release(&mut self) -> io::Result<bool> {
        let Some(last) = self.last else {
            return Ok(false);
        };
        if self.in_contact() {
            self.emit(PenPacket {
                phase: PenPhase::Up,
                pressure: 0.0,
                ..last
            })?;
        }
        // Leaving range lets go of the barrel buttons as well.
        let leave = PenPacket {
            phase: PenPhase::Leave,
            pressure: 0.0,
            barrel: 0,
            ..last
        };
        self.sink.send(leave)?;
        self.last = None;
        Ok(true)
    }

    fn emit(&mut self, packet: PenPacket) -> io::Result<()> {
        self.sink.send(packet)?;
        self.last = Some(packet);
        Ok(())
    }
}

/// Unchanged position, pressure, tilt, tool and barrel buttons.
fn same_state(last: PenPacket, next: PenPacket) -> bool {
    let contact = |phase| matches!(phase, PenPhase::Down | PenPhase::Contact);
    contact(last.phase) == contact(next.phase)
        && last.x == next.x
        && last.y == next.y
        && last.pressure == next.pressure
        && last.tilt == next.tilt
        && last.eraser == next.eraser
        && last.barrel == next.barrel
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    fn output() -> (PenOutput, Rc<RefCell<Vec<PenPacket>>>) {
        let packets = Rc::new(RefCell::new(Vec::new()));
        let sink = Rc::clone(&packets);
        (
            PenOutput::new(
                Box::new(move |packet| {
                    sink.borrow_mut().push(packet);
                    Ok(())
                }),
                1000,
            ),
            packets,
        )
    }

    fn at(x: f64, pressure: u32, contact: bool) -> PenSample {
        PenSample {
            x,
            y: 20.0,
            pressure: Some(pressure),
            tilt: Some([10.0, -120.0]),
            eraser: false,
            contact,
            barrel: 0,
        }
    }

    fn phases(packets: &Rc<RefCell<Vec<PenPacket>>>) -> Vec<PenPhase> {
        packets
            .borrow_mut()
            .drain(..)
            .map(|packet| packet.phase)
            .collect()
    }

    #[test]
    fn hover_contact_and_range_loss_follow_the_pen_lifecycle() {
        use PenPhase::*;
        let (mut pen, packets) = output();
        assert!(pen.sample(at(1.0, 0, false)).unwrap());
        assert!(!pen.sample(at(1.0, 0, false)).unwrap(), "unchanged hover");
        pen.sample(at(2.0, 300, false)).unwrap();
        pen.sample(at(2.0, 500, true)).unwrap();
        pen.sample(at(2.0, 600, true)).unwrap();
        assert!(
            !pen.sample(at(2.0, 600, true)).unwrap(),
            "unchanged contact"
        );
        pen.sample(at(3.0, 0, false)).unwrap();
        assert!(pen.release().unwrap());
        assert!(!pen.release().unwrap(), "already out of range");
        let sent = packets.borrow().clone();
        assert_eq!(
            phases(&packets),
            [Hover, Hover, Down, Contact, Up, Leave],
            "{sent:?}"
        );
        // Pressure only in contact; tilt clamped to what pens report.
        assert_eq!(sent[1].pressure, 0.0);
        assert_eq!(sent[2].pressure, 0.5);
        assert_eq!(sent[3].pressure, 0.6);
        assert_eq!(sent[4].pressure, 0.0);
        assert_eq!(sent[2].tilt, Some([10.0, -90.0]));
        assert_eq!((sent[5].x, sent[5].y), (3.0, 20.0));
    }

    #[test]
    fn contact_out_of_range_enters_first_and_release_lifts_before_leaving() {
        use PenPhase::*;
        let (mut pen, packets) = output();
        pen.sample(at(5.0, 2000, true)).unwrap();
        assert!(pen.in_contact());
        pen.release().unwrap();
        let sent = packets.borrow().clone();
        assert_eq!(phases(&packets), [Hover, Down, Up, Leave]);
        assert_eq!(sent[0].pressure, 0.0);
        assert_eq!(sent[1].pressure, 1.0, "clamped to the maximum");
        assert!(!pen.in_range());
    }

    #[test]
    fn switching_tools_leaves_and_reenters_range() {
        use PenPhase::*;
        let (mut pen, packets) = output();
        pen.sample(at(1.0, 400, true)).unwrap();
        pen.sample(PenSample {
            eraser: true,
            ..at(1.0, 400, true)
        })
        .unwrap();
        let sent = packets.borrow().clone();
        assert_eq!(phases(&packets), [Hover, Down, Up, Leave, Hover, Down]);
        assert!(!sent[3].eraser && sent[4].eraser && sent[5].eraser);
    }

    #[test]
    fn positionless_reports_change_contact_where_the_pen_is() {
        use PenPhase::*;
        let (mut pen, packets) = output();
        assert!(
            !pen.contact(true, Some(10), 0).unwrap(),
            "no pen, no contact"
        );
        pen.sample(at(7.0, 0, false)).unwrap();
        pen.contact(true, Some(250), 0).unwrap();
        pen.contact(true, None, 0).unwrap();
        pen.contact(false, None, 0).unwrap();
        let sent = packets.borrow().clone();
        assert_eq!(phases(&packets), [Hover, Down, Up]);
        assert_eq!((sent[1].x, sent[1].pressure), (7.0, 0.25));
    }

    #[test]
    fn a_barrel_button_change_sends_a_packet_and_leaves_the_rest_alone() {
        use PenPhase::*;
        let (mut pen, packets) = output();
        pen.sample(at(1.0, 0, false)).unwrap();
        pen.sample(PenSample {
            barrel: 0b001,
            ..at(1.0, 0, false)
        })
        .unwrap();
        assert!(
            !pen.sample(PenSample {
                barrel: 0b001,
                ..at(1.0, 0, false)
            })
            .unwrap(),
            "a held barrel button is not resent"
        );
        pen.sample(at(1.0, 0, false)).unwrap();
        let sent = packets.borrow().clone();
        assert_eq!(phases(&packets), [Hover, Hover, Hover]);
        assert_eq!(
            sent.iter().map(|packet| packet.barrel).collect::<Vec<_>>(),
            [0, 1, 0]
        );
        assert!(
            sent.iter()
                .all(|packet| (packet.x, packet.y) == (1.0, 20.0))
        );
    }

    #[test]
    fn leaving_range_releases_the_barrel_buttons() {
        use PenPhase::*;
        let (mut pen, packets) = output();
        pen.sample(PenSample {
            barrel: 0b011,
            ..at(1.0, 0, false)
        })
        .unwrap();
        pen.release().unwrap();
        let sent = packets.borrow().clone();
        assert_eq!(phases(&packets), [Hover, Leave]);
        assert_eq!((sent[0].barrel, sent[1].barrel), (0b011, 0));
    }

    #[test]
    fn a_positionless_report_carries_the_barrel_state() {
        let (mut pen, packets) = output();
        pen.sample(at(4.0, 0, false)).unwrap();
        pen.contact(false, None, 0b100).unwrap();
        assert_eq!(packets.borrow().last().unwrap().barrel, 0b100);
    }

    #[test]
    fn a_refused_packet_is_retried_and_nonfinite_positions_are_rejected() {
        use PenPhase::*;
        let refuse = Rc::new(RefCell::new(true));
        let packets = Rc::new(RefCell::new(Vec::new()));
        let (sink_refuse, sink_packets) = (Rc::clone(&refuse), Rc::clone(&packets));
        let mut pen = PenOutput::new(
            Box::new(move |packet: PenPacket| {
                if packet.phase == Down && *sink_refuse.borrow() {
                    return Err(io::Error::other("busy"));
                }
                sink_packets.borrow_mut().push(packet);
                Ok(())
            }),
            100,
        );
        assert!(pen.sample(at(1.0, 50, true)).is_err());
        assert!(pen.in_range() && !pen.in_contact());
        *refuse.borrow_mut() = false;
        pen.sample(at(1.0, 50, true)).unwrap();
        assert_eq!(phases(&packets), [Hover, Down]);
        assert!(pen.sample(at(f64::NAN, 50, true)).is_err());
        assert!(pen.in_contact(), "state is unchanged by a rejected sample");
    }

    #[test]
    fn samples_allocate_nothing() {
        let mut pen = PenOutput::new(Box::new(|_| Ok(())), 8191);
        crate::test_alloc::assert_no_allocations(|| {
            for index in 0..10_000u32 {
                let contact = index % 7 > 2;
                pen.sample(PenSample {
                    eraser: index % 1000 > 900,
                    ..at(f64::from(index % 50), index % 8191, contact)
                })
                .unwrap();
                if index % 500 == 0 {
                    pen.release().unwrap();
                }
            }
        });
    }
}
