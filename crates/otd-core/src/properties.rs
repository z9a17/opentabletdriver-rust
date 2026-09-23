//! Property tests (F05). Seeded pseudo-random reports, settings and event
//! orders must not cause a panic, a non-finite or out-of-range output, a
//! filter call out of stage order, or a button left held. The generator is
//! deterministic, so a failing case reproduces; messages name the case.

use std::cell::{Cell, RefCell};
use std::io;
use std::path::Path;
use std::rc::Rc;
use std::time::{Duration, Instant};

use crate::config::Profile;
use crate::display::{DisplayFingerprint, DisplaySnapshot};
use crate::mapping::Rect;
use crate::output::{MousePacket, flags};
use crate::pipeline::ReportPipeline;
use crate::plugins::{Filters, NoFilters};
use crate::protocol::{self, MAX_PRESSURE, MAX_X, MAX_Y, ParseError, PenReport};
use crate::session::{self, Displays, Mode, Read, ReportSource};

/// xorshift64*: small and identical on every platform.
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

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }

    fn chance(&mut self, percent: u64) -> bool {
        self.below(100) < percent
    }

    fn byte(&mut self) -> u8 {
        self.next() as u8
    }
}

/// Settings as TOML lines; `profile` replaces one value to build variants.
const ABSOLUTE: &[(&str, &str, &str)] = &[
    ("absolute", "clipping", "true"),
    ("absolute", "limiting", "false"),
    ("absolute.display", "Width", "2560.0"),
    ("absolute.display", "Height", "1440.0"),
    ("absolute.display", "X", "1280.0"),
    ("absolute.display", "Y", "720.0"),
    ("absolute.display", "Rotation", "0.0"),
    ("absolute.tablet", "Width", "85.0"),
    ("absolute.tablet", "Height", "47.8125"),
    ("absolute.tablet", "X", "110.0"),
    ("absolute.tablet", "Y", "23.90625"),
    ("absolute.tablet", "Rotation", "0.0"),
    ("bindings", "tip_enabled", "true"),
    ("bindings", "eraser_enabled", "true"),
    ("bindings", "tip_threshold_raw", "82"),
    ("bindings", "eraser_threshold_raw", "82"),
    ("[radial_follow]", "outer_radius", "0.7039"),
    ("[radial_follow]", "inner_radius", "0.302"),
    ("[radial_follow]", "smoothing_coefficient", "0.302"),
    ("[radial_follow]", "soft_knee_scale", "0.603"),
    ("[radial_follow]", "smoothing_leak_coefficient", "0.201"),
];

const RELATIVE: &[(&str, &str, &str)] = &[
    ("relative", "x_sensitivity", "10.0"),
    ("relative", "y_sensitivity", "10.0"),
    ("relative", "rotation", "0.0"),
    ("relative", "reset_delay_ms", "100"),
    ("bindings", "tip_enabled", "true"),
    ("bindings", "eraser_enabled", "true"),
    ("bindings", "tip_threshold_raw", "82"),
    ("bindings", "eraser_threshold_raw", "82"),
];

/// TOML for `lines`, with the setting at `replace` set to `value`.
fn profile(lines: &[(&str, &str, &str)], replace: Option<(usize, &str)>) -> String {
    let mut text = String::new();
    let mut table = "";
    for (index, (section, key, default)) in lines.iter().enumerate() {
        if *section != table {
            table = section;
            text.push_str(&format!("\n[{section}]\n"));
        }
        let value = match replace {
            Some((at, value)) if at == index => value,
            _ => default,
        };
        text.push_str(&format!("{key} = {value}\n"));
    }
    text
}

fn desktop() -> DisplaySnapshot {
    let rect = |left, top, right, bottom| Rect {
        left,
        top,
        right,
        bottom,
    };
    DisplaySnapshot {
        virtual_screen: rect(0, 0, 4160, 1440),
        monitors: vec![rect(0, 0, 2560, 1440), rect(2560, 0, 4160, 900)],
    }
}

/// Mostly plausible pen reports, with boundary and out-of-range fields, and
/// some empty, short, foreign or random buffers.
fn random_report(rng: &mut Rng, buffer: &mut Vec<u8>) {
    buffer.clear();
    match rng.below(12) {
        0 => {
            let length = rng.below(17) as usize;
            buffer.extend((0..length).map(|_| rng.byte()));
            if !buffer.is_empty() && rng.chance(70) {
                buffer[0] = if rng.chance(50) { 0x10 } else { 0x1e };
            }
        }
        1 => {
            buffer.resize(192, 0);
            buffer[0] = rng.byte();
        }
        2 => buffer.extend((0..rng.below(300)).map(|_| rng.byte())),
        _ => {
            buffer.resize(
                if rng.chance(90) {
                    192
                } else {
                    17 + rng.below(8) as usize
                },
                0,
            );
            buffer[0] = if rng.chance(90) { 0x10 } else { 0x1e };
            buffer[1] =
                [0x00, 0x20, 0x40, 0x60, 0x61, 0x70, 0x71, 0x7f, rng.byte()][rng.below(9) as usize];
            let coordinate = |rng: &mut Rng, max: u32| match rng.below(10) {
                0 => 0,
                1 => max,
                2 => max + 1 + rng.below(1000) as u32,
                3 => 0x00ff_ffff,
                _ => rng.below(u64::from(max) + 1) as u32,
            };
            let (x, y) = (coordinate(rng, MAX_X), coordinate(rng, MAX_Y));
            let pressure: u16 = match rng.below(8) {
                0 => 0,
                1 => MAX_PRESSURE,
                2 => MAX_PRESSURE + 1 + rng.below(100) as u16,
                3 => 81 + rng.below(3) as u16,
                _ => rng.below(u64::from(MAX_PRESSURE) + 1) as u16,
            };
            if buffer[0] == 0x10 {
                buffer[2..5].copy_from_slice(&x.to_le_bytes()[..3]);
                buffer[5..8].copy_from_slice(&y.to_le_bytes()[..3]);
                buffer[8..10].copy_from_slice(&pressure.to_le_bytes());
            } else {
                buffer[2] = buffer[1] & 0x11;
                buffer[3..6].copy_from_slice(&x.to_le_bytes()[..3]);
                buffer[6..9].copy_from_slice(&y.to_le_bytes()[..3]);
                buffer[9..11].copy_from_slice(&pressure.to_le_bytes());
            }
            let end = buffer.len().min(17);
            for byte in &mut buffer[10..end] {
                *byte = rng.byte();
            }
        }
    }
}

/// Checks each packet a sink accepts and tracks whether the left button is
/// held. A press while held, or a release while not, is a failure.
#[derive(Default)]
struct Buttons {
    held: bool,
    presses: u32,
}

impl Buttons {
    fn accept(&mut self, packet: MousePacket, relative: bool, case: &str) {
        let known =
            flags::MOVE | flags::LEFTDOWN | flags::LEFTUP | flags::ABSOLUTE | flags::VIRTUALDESK;
        assert_eq!(packet.flags & !known, 0, "{case}: unknown flags {packet:?}");
        assert!(
            packet.flags & flags::LEFTDOWN == 0 || packet.flags & flags::LEFTUP == 0,
            "{case}: press and release in one packet"
        );
        if packet.flags & flags::MOVE != 0 && !relative {
            assert!(
                (0..=65_535).contains(&packet.dx) && (0..=65_535).contains(&packet.dy),
                "{case}: absolute position out of range {packet:?}"
            );
        }
        if packet.flags & flags::LEFTDOWN != 0 {
            assert!(!self.held, "{case}: pressed while already held");
            self.held = true;
            self.presses += 1;
        }
        if packet.flags & flags::LEFTUP != 0 {
            assert!(self.held, "{case}: released without a press");
            self.held = false;
        }
    }
}

#[test]
fn any_bytes_parse_to_a_valid_report_or_a_clean_rejection() {
    let mut rng = Rng(0x5eed_0001);
    let mut buffer = Vec::new();
    for _ in 0..200_000 {
        random_report(&mut rng, &mut buffer);
        match protocol::parse(&buffer) {
            Ok(Some(report)) => {
                assert!(matches!(report.id, 0x10 | 0x1e), "{buffer:02x?}");
                assert!(report.x <= MAX_X && report.y <= MAX_Y, "{buffer:02x?}");
                assert!(report.pressure <= MAX_PRESSURE, "{buffer:02x?}");
            }
            Ok(None) => assert!(!matches!(buffer[0], 0x10 | 0x1e), "{buffer:02x?}"),
            Err(ParseError::Empty) => assert!(buffer.is_empty()),
            Err(ParseError::Short { need, got, .. }) => assert!(got < need && got == buffer.len()),
            Err(ParseError::Position { .. } | ParseError::Pressure(_)) => {}
        }
    }
}

/// Random reports with jittery, repeated, backward and jumping timestamps,
/// through every profile, with an output that fails a fifth of the time.
/// Whatever happens, accepted packets stay in range, presses and releases
/// alternate, and releasing at the end leaves nothing held.
#[test]
fn random_reports_and_failing_output_never_leave_a_button_held() {
    let plain: Vec<_> = ABSOLUTE[..ABSOLUTE.len() - 5].to_vec();
    let profiles = [
        profile(ABSOLUTE, None),
        profile(&plain, None),
        profile(RELATIVE, None),
        "monitor = 0\n".to_owned(),
    ];
    let desktop = desktop();
    let mut rng = Rng(0x5eed_0002);
    let mut buffer = Vec::new();
    let (mut presses, mut injected) = (0, 0);
    for case in 0..400 {
        let text = &profiles[case % profiles.len()];
        let profile = Profile::from_toml_text(text, Path::new("property")).unwrap();
        let relative = profile.relative.is_some();
        let mapper = (!relative).then(|| desktop.mapper(&profile).unwrap());
        let mut pipeline = ReportPipeline::new(&profile).unwrap();
        let case = format!("case {case}");
        let mut buttons = Buttons::default();
        let start = Instant::now();
        let mut offset = Duration::ZERO;
        for _ in 0..300 {
            random_report(&mut rng, &mut buffer);
            offset = match rng.below(20) {
                0 => offset.saturating_sub(Duration::from_millis(rng.below(50))),
                1 => offset + Duration::from_secs(rng.below(3_600)),
                2 => offset,
                _ => offset + Duration::from_micros(4_000 + rng.below(2_000)),
            };
            let fail = rng.chance(20);
            if let Ok(Some(pen)) = protocol::parse(&buffer) {
                let result =
                    pipeline.process(pen, start + offset, mapper, &mut NoFilters, |packet| {
                        if fail {
                            return Err(io::Error::other("injected output failure"));
                        }
                        buttons.accept(packet, relative, &case);
                        Ok(())
                    });
                assert!(
                    result.is_ok() || fail,
                    "{case}: failed without an injected failure"
                );
                injected += u32::from(result.is_err());
            }
        }
        let _ = pipeline.release_all(|packet| {
            buttons.accept(packet, relative, &case);
            Ok(())
        });
        assert!(!buttons.held, "{case}: a button stayed held");
        presses += buttons.presses;
    }
    assert!(
        presses > 1_000 && injected > 1_000,
        "{presses} presses, {injected} failures"
    );
}

/// Every numeric setting set to NaN, infinity or an extreme value is either
/// rejected when the profile loads or harmless: reports then produce only
/// in-range packets and no panic.
#[test]
fn non_finite_and_extreme_settings_are_rejected_or_harmless() {
    let values = [
        "nan",
        "inf",
        "-inf",
        "1e308",
        "-1e308",
        "0.0",
        "-1.0",
        "1e-308",
        "123456789.0",
        "0",
        "8191",
        "8192",
        "65535",
        "65536",
        "-1",
    ];
    let desktop = desktop();
    let mut rng = Rng(0x5eed_0003);
    let mut buffer = Vec::new();
    let (mut accepted, mut rejected) = (0, 0);
    for lines in [ABSOLUTE, RELATIVE] {
        for index in 0..lines.len() {
            for value in values {
                let text = profile(lines, Some((index, value)));
                let case = format!("{} = {value}", lines[index].1);
                let Ok(profile) = Profile::from_toml_text(&text, Path::new("property")) else {
                    rejected += 1;
                    continue;
                };
                let relative = profile.relative.is_some();
                let mapper = if relative {
                    None
                } else {
                    match desktop.mapper(&profile) {
                        Ok(mapper) => Some(mapper),
                        Err(_) => continue,
                    }
                };
                let Ok(mut pipeline) = ReportPipeline::new(&profile) else {
                    continue;
                };
                accepted += 1;
                let mut buttons = Buttons::default();
                let start = Instant::now();
                for step in 0..200u32 {
                    random_report(&mut rng, &mut buffer);
                    if let Ok(Some(pen)) = protocol::parse(&buffer) {
                        let now = start + Duration::from_millis(u64::from(step) * 5);
                        let _ = pipeline.process(pen, now, mapper, &mut NoFilters, |packet| {
                            buttons.accept(packet, relative, &case);
                            Ok(())
                        });
                    }
                }
                let _ = pipeline.release_all(|packet| {
                    buttons.accept(packet, relative, &case);
                    Ok(())
                });
                assert!(!buttons.held, "{case}: a button stayed held");
            }
        }
    }
    assert!(
        accepted > 50 && rejected > 50,
        "{accepted} accepted, {rejected} rejected"
    );
}

/// Records the order of filter calls within one report.
struct StageRecorder {
    pre: bool,
    pixels: bool,
    calls: String,
}

impl Filters for StageRecorder {
    fn has_pre(&self) -> bool {
        self.pre
    }

    fn process_pre(&mut self, position: (f32, f32), _: PenReport, _: Instant) -> (f32, f32) {
        self.calls.push('p');
        position
    }

    fn has_pixels(&self) -> bool {
        self.pixels
    }

    fn process_pixels(&mut self, position: (f32, f32), _: PenReport, _: Instant) -> (f32, f32) {
        assert!(
            position.0.is_finite() && position.1.is_finite(),
            "pixel filter got {position:?}"
        );
        self.calls.push('x');
        position
    }

    fn reset(&mut self) {
        self.calls.push('r');
    }

    fn take_failure(&mut self) -> Option<&str> {
        None
    }
}

/// PreTransform filters run before mapping and Pixels filters after it, at
/// most once each per report; a report without a detected pen resets them
/// instead; relative mode never calls Pixels filters.
#[test]
fn filters_run_in_stage_order() {
    let desktop = desktop();
    let mut rng = Rng(0x5eed_0004);
    let mut buffer = Vec::new();
    let mut seen = [0u32; 3];
    for case in 0..200 {
        let relative = case % 3 == 2;
        let text = profile(if relative { RELATIVE } else { ABSOLUTE }, None);
        let profile = Profile::from_toml_text(&text, Path::new("property")).unwrap();
        let mapper = (!relative).then(|| desktop.mapper(&profile).unwrap());
        let mut pipeline = ReportPipeline::new(&profile).unwrap();
        let mut filters = StageRecorder {
            pre: rng.chance(50),
            pixels: rng.chance(50),
            calls: String::new(),
        };
        let start = Instant::now();
        for step in 0..200u32 {
            random_report(&mut rng, &mut buffer);
            let Ok(Some(pen)) = protocol::parse(&buffer) else {
                continue;
            };
            filters.calls.clear();
            let now = start + Duration::from_millis(u64::from(step) * 5);
            let _ = pipeline.process(pen, now, mapper, &mut filters, |_| Ok(()));
            let detected = pen.in_range || pen.sense;
            let calls = filters.calls.as_str();
            for (count, call) in seen.iter_mut().zip(['p', 'x', 'r']) {
                *count += u32::from(calls.contains(call));
            }
            if !detected {
                assert_eq!(calls, "r", "case {case}: {pen:?}");
            } else {
                let pre = if filters.pre { "p" } else { "" };
                let allowed = [pre.to_owned(), format!("{pre}x")];
                assert!(
                    allowed.iter().any(|a| a == calls),
                    "case {case}: calls {calls:?} for {pen:?}"
                );
                if relative || !filters.pixels {
                    assert!(!calls.contains('x'), "case {case}");
                }
            }
        }
    }
    assert!(
        seen.iter().all(|&count| count > 500),
        "calls seen: {seen:?}"
    );
}

type Script = Rc<RefCell<Vec<Option<Vec<u8>>>>>;

/// Serves a random script of reports and idle periods, then ends.
struct ScriptedSource {
    clock: Rc<Cell<Instant>>,
    script: Script,
    current: Vec<u8>,
}

impl ReportSource for ScriptedSource {
    fn label(&self) -> &str {
        "property script"
    }

    fn now(&self) -> Instant {
        self.clock.get()
    }

    fn next(&mut self, _timeout: Duration) -> io::Result<Read<'_>> {
        let Some(event) = self.script.borrow_mut().pop() else {
            return Ok(Read::Ended);
        };
        self.clock.set(self.clock.get() + Duration::from_millis(5));
        Ok(match event {
            Some(bytes) => {
                self.current = bytes;
                Read::Report {
                    bytes: &self.current,
                    ready: self.clock.get(),
                    queued: false,
                }
            }
            None => {
                self.clock
                    .set(self.clock.get() + Duration::from_millis(1_100));
                Read::Idle
            }
        })
    }
}

/// A desktop whose selected monitor sometimes disappears and comes back.
struct FlakyDisplays {
    clock: Rc<Cell<Instant>>,
    start: Instant,
    period_ms: u64,
}

impl FlakyDisplays {
    fn current(&self) -> DisplaySnapshot {
        let elapsed = self
            .clock
            .get()
            .saturating_duration_since(self.start)
            .as_millis() as u64;
        let mut snapshot = desktop();
        if (elapsed / self.period_ms) % 2 == 1 {
            snapshot.monitors.clear();
        }
        snapshot
    }
}

impl Displays for FlakyDisplays {
    fn fingerprint(&mut self) -> DisplayFingerprint {
        self.current().fingerprint()
    }

    fn snapshot(&mut self) -> Result<DisplaySnapshot, String> {
        Ok(self.current())
    }
}

/// Whole sessions over random scripts, with monitors coming and going and a
/// failing output, end with every accepted press released.
#[test]
fn sessions_end_with_nothing_held() {
    let mut rng = Rng(0x5eed_0005);
    let text = "monitor = 0\n[bindings]\ntip_threshold_raw = 82\n";
    let profile = Profile::from_toml_text(text, Path::new("property")).unwrap();
    let mut buffer = Vec::new();
    let mut presses = 0;
    for case in 0..150 {
        let script: Vec<Option<Vec<u8>>> = (0..rng.below(400))
            .map(|_| {
                if rng.chance(3) {
                    None
                } else {
                    random_report(&mut rng, &mut buffer);
                    Some(buffer.clone())
                }
            })
            .collect();
        let clock = Rc::new(Cell::new(Instant::now()));
        let mut source = ScriptedSource {
            clock: clock.clone(),
            script: Rc::new(RefCell::new(script.into_iter().rev().collect())),
            current: Vec::new(),
        };
        let mut displays = FlakyDisplays {
            clock: clock.clone(),
            start: clock.get(),
            period_ms: 500 + rng.below(3_000),
        };
        let buttons = RefCell::new(Buttons::default());
        let failures = Cell::new(rng.next());
        let case = format!("session case {case}");
        let result = session::run(
            &mut source,
            &mut displays,
            &profile,
            Mode::Driver,
            &mut NoFilters,
            |packet| {
                // Fail about one packet in eight, except the final release.
                let bits = failures.get();
                failures.set(bits.rotate_left(3) ^ 0x9e37_79b9_7f4a_7c15);
                if bits.is_multiple_of(8) && packet.flags & flags::MOVE != 0 {
                    return Err(io::Error::other("injected output failure"));
                }
                buttons.borrow_mut().accept(packet, false, &case);
                Ok(())
            },
            &|_: &str| {},
        );
        assert!(result.is_ok(), "{case}: {result:?}");
        assert!(!buttons.borrow().held, "{case}: a button stayed held");
        presses += buttons.borrow().presses;
    }
    assert!(presses > 500, "{presses} presses");
}
