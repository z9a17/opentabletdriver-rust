//! A deterministic synthetic PTH-660 pen trace shaped like osu! play inside
//! the benchmark profile's tablet area: hovering jumps between targets, taps,
//! slider arcs, spinner circles and rises into the Sense band. Reports use the
//! 192-byte `0x10` layout that both `protocol::parse` and upstream's
//! `IntuosV2ReportParser` read. The trace is not a recording of a player.

use std::f64::consts::PI;

pub const REPORT_BYTES: usize = 192;

/// The benchmark profile's tablet area in raw units: 85 x 47.8125 mm centered
/// at (110, 23.90625) mm, at 200 units per mm.
const CENTER: (f64, f64) = (22_000.0, 4_781.25);
const HALF: (f64, f64) = (8_500.0, 4_781.25);

pub struct Trace {
    bytes: Vec<u8>,
}

impl Trace {
    pub fn len(&self) -> usize {
        self.bytes.len() / REPORT_BYTES
    }

    pub fn report(&self, index: usize) -> &[u8] {
        &self.bytes[index * REPORT_BYTES..][..REPORT_BYTES]
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// FNV-1a over every byte, so both harnesses can check they replay the
    /// same reports.
    pub fn fnv1a64(&self) -> u64 {
        self.bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
            (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3)
        })
    }
}

/// xorshift64*: small, fast and identical on every platform.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }

    fn range(&mut self, low: f64, high: f64) -> f64 {
        low + (high - low) * self.unit()
    }

    fn count(&mut self, low: u32, high: u32) -> u32 {
        low + (self.next() % u64::from(high - low + 1)) as u32
    }
}

#[derive(Clone, Copy)]
struct Pen {
    x: f64,
    y: f64,
    pressure: f64,
    in_range: bool,
    distance: u8,
}

struct Writer {
    bytes: Vec<u8>,
    limit: usize,
    rng: Rng,
    tilt: (f64, f64),
}

impl Writer {
    fn full(&self) -> bool {
        self.bytes.len() / REPORT_BYTES >= self.limit
    }

    fn push(&mut self, pen: Pen) {
        if self.full() {
            return;
        }
        // Sensor noise of a couple of raw units, and a slowly drifting tilt.
        let x = (pen.x + self.rng.range(-2.0, 2.0))
            .round()
            .clamp(0.0, 44_800.0) as u32;
        let y = (pen.y + self.rng.range(-2.0, 2.0))
            .round()
            .clamp(0.0, 29_600.0) as u32;
        self.tilt.0 = (self.tilt.0 + self.rng.range(-0.5, 0.5)).clamp(-30.0, 30.0);
        self.tilt.1 = (self.tilt.1 + self.rng.range(-0.5, 0.5)).clamp(-30.0, 30.0);
        let pressure = pen.pressure.round().clamp(0.0, 8_191.0) as u16;
        let mut report = [0u8; REPORT_BYTES];
        report[0] = 0x10;
        // Sense is set whenever the tablet reports the pen; In Range marks the
        // lower hover band; the tip switch follows the firmware's own
        // pressure threshold.
        report[1] = 0x40 | if pen.in_range { 0x20 } else { 0 } | u8::from(pressure > 20);
        report[2..5].copy_from_slice(&x.to_le_bytes()[..3]);
        report[5..8].copy_from_slice(&y.to_le_bytes()[..3]);
        report[8..10].copy_from_slice(&pressure.to_le_bytes());
        report[10] = self.tilt.0.round() as i8 as u8;
        report[11] = self.tilt.1.round() as i8 as u8;
        report[16] = pen.distance;
        self.bytes.extend_from_slice(&report);
    }
}

fn hover(x: f64, y: f64, distance: u8) -> Pen {
    Pen {
        x,
        y,
        pressure: 0.0,
        in_range: true,
        distance,
    }
}

fn contact(x: f64, y: f64, pressure: f64) -> Pen {
    Pen {
        x,
        y,
        pressure,
        in_range: true,
        distance: 0,
    }
}

/// A target inside the area; about one in thirty lands just outside it, so
/// clipping runs too.
fn target(rng: &mut Rng) -> (f64, f64) {
    let reach = if rng.unit() < 1.0 / 30.0 { 1.08 } else { 0.9 };
    (
        CENTER.0 + rng.range(-reach, reach) * HALF.0,
        CENTER.1 + rng.range(-reach, reach) * HALF.1,
    )
}

fn ease(t: f64) -> f64 {
    0.5 - 0.5 * (PI * t).cos()
}

pub fn osu(reports: usize, seed: u64) -> Trace {
    let mut w = Writer {
        bytes: Vec::with_capacity(reports * REPORT_BYTES),
        limit: reports,
        rng: Rng(seed | 1),
        tilt: (8.0, -4.0),
    };
    let mut at = CENTER;
    while !w.full() {
        let kind = w.rng.unit();
        if kind < 0.6 {
            // Jump while hovering, then tap.
            let to = target(&mut w.rng);
            let steps = w.rng.count(6, 24);
            for step in 1..=steps {
                let t = ease(f64::from(step) / f64::from(steps));
                let distance = w.rng.count(4, 20) as u8;
                w.push(hover(
                    at.0 + (to.0 - at.0) * t,
                    at.1 + (to.1 - at.1) * t,
                    distance,
                ));
            }
            at = to;
            let hold = w.rng.count(3, 10);
            let peak = w.rng.range(1_500.0, 4_500.0);
            for step in 1..=hold {
                let pressure = peak * (PI * f64::from(step) / f64::from(hold + 1)).sin();
                w.push(contact(at.0, at.1, pressure));
            }
            w.push(hover(at.0, at.1, 2));
        } else if kind < 0.85 {
            // Slider: a pressed quadratic curve to the next target.
            let to = target(&mut w.rng);
            let bend = w.rng.range(-0.4, 0.4);
            let control = (
                (at.0 + to.0) / 2.0 - (to.1 - at.1) * bend,
                (at.1 + to.1) / 2.0 + (to.0 - at.0) * bend,
            );
            let steps = w.rng.count(20, 60);
            let peak = w.rng.range(1_500.0, 4_500.0);
            for step in 0..=steps {
                let t = f64::from(step) / f64::from(steps);
                let u = 1.0 - t;
                let pressure = peak + w.rng.range(-150.0, 150.0);
                w.push(contact(
                    u * u * at.0 + 2.0 * u * t * control.0 + t * t * to.0,
                    u * u * at.1 + 2.0 * u * t * control.1 + t * t * to.1,
                    pressure,
                ));
            }
            at = to;
            w.push(hover(at.0, at.1, 2));
        } else if kind < 0.95 {
            // Spinner: pressed circles around the area's center.
            let radius = w.rng.range(1_200.0, 2_800.0);
            let per_turn = f64::from(w.rng.count(40, 70));
            let turns = w.rng.range(1.5, 3.0);
            let peak = w.rng.range(1_500.0, 4_500.0);
            let steps = (per_turn * turns) as u32;
            for step in 0..=steps {
                let angle = 2.0 * PI * f64::from(step) / per_turn;
                let pressure = peak + w.rng.range(-150.0, 150.0);
                w.push(contact(
                    CENTER.0 + radius * angle.cos(),
                    CENTER.1 + radius * angle.sin() * 0.9,
                    pressure,
                ));
            }
            at = (CENTER.0 + radius, CENTER.1);
            w.push(hover(at.0, at.1, 2));
        } else {
            // Lift into the Sense band, above In Range, and come back.
            let steps = w.rng.count(8, 20);
            for step in 0..steps {
                let rise = (PI * f64::from(step) / f64::from(steps)).sin();
                let drift = (w.rng.range(-30.0, 30.0), w.rng.range(-30.0, 30.0));
                w.push(Pen {
                    x: at.0 + drift.0,
                    y: at.1 + drift.1,
                    pressure: 0.0,
                    in_range: rise < 0.3,
                    distance: (20.0 + 40.0 * rise) as u8,
                });
            }
        }
    }
    Trace { bytes: w.bytes }
}
