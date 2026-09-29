//! Offline audit probes. All input and output are in memory; no OS input APIs.
use otd_core::{
    config::Profile,
    decoders::{ReportParser, TabletDecoder},
    display::{DisplayFingerprint, DisplaySnapshot},
    mapping::Rect,
    pipeline::ReportPipeline,
    plugins::{Filters, PipelineRuntime},
    protocol::PenReport,
    reports::{DeviceId, EndpointId, ReportKind, ReportMetadata, ReportValues, SessionId},
    session::{self, Displays, Mode, Read, ReportSource},
};

#[test]
fn all_parser_adapters_survive_bounded_malformed_transport_inputs() {
    use otd_core::decoders::{PenDecoder, TYPE_NAMES};
    let metadata = ReportMetadata {
        device: DeviceId(0),
        session: SessionId(0),
        endpoint: EndpointId(0),
        received_at: Duration::ZERO,
        sequence: 0,
    };
    let mut seed = 0x123456789abcdef0u64;
    let mut cases = 0;
    for name in TYPE_NAMES {
        let mut parser = ReportParser::for_type(name).unwrap();
        let mut decoder =
            TabletDecoder::for_parser(name, otd_core::spec::TabletSpec::PTH_660).unwrap();
        let mut storage = [0u8; 512];
        for length in (0..=64).chain([127, 128, 191, 192, 255, 512]) {
            for pattern in 0..256 {
                let raw = &mut storage[..length];
                for byte in raw.iter_mut() {
                    seed ^= seed << 13;
                    seed ^= seed >> 7;
                    seed ^= seed << 17;
                    *byte = if pattern == 0 {
                        0
                    } else if pattern == 255 {
                        255
                    } else {
                        seed as u8
                    };
                }
                if let Some(id) = raw.first_mut() {
                    *id = pattern as u8;
                }
                let expected = parser.parse(raw, metadata);
                if let Ok(Some(decoded)) = decoder.decode(raw) {
                    let (_, report) = expected.unwrap();
                    assert_eq!(
                        decoded.raw, report.raw,
                        "{name}, length={length}, id={pattern}"
                    );
                }
                cases += 1;
            }
        }
    }
    assert_eq!(cases, 963_328);
}
use std::{
    cell::Cell,
    io,
    rc::Rc,
    time::{Duration, Instant},
};

struct Display;
fn snapshot() -> DisplaySnapshot {
    let screen = Rect {
        left: 0,
        top: 0,
        right: 1920,
        bottom: 1080,
    };
    DisplaySnapshot {
        virtual_screen: screen,
        monitors: vec![screen],
    }
}
impl Displays for Display {
    fn fingerprint(&mut self) -> DisplayFingerprint {
        snapshot().fingerprint()
    }
    fn snapshot(&mut self) -> Result<DisplaySnapshot, String> {
        Ok(snapshot())
    }
}

struct Source {
    ticks: Rc<Cell<u32>>,
    reads: u32,
    ticks_at_first_read: u32,
}
impl ReportSource for Source {
    fn label(&self) -> &str {
        "in-memory source already stopped"
    }
    fn now(&self) -> Instant {
        Instant::now()
    }
    fn next(&mut self, _: Duration) -> io::Result<Read<'_>> {
        self.reads += 1;
        self.ticks_at_first_read = self.ticks.get();
        Ok(Read::Ended)
    }
}

struct Timers {
    ticks: Rc<Cell<u32>>,
    limit: u32,
}
impl Filters for Timers {
    fn has_pre(&self) -> bool {
        false
    }
    fn has_pixels(&self) -> bool {
        false
    }
    fn process_pre(&mut self, p: (f32, f32), _: PenReport, _: Instant) -> (f32, f32) {
        p
    }
    fn process_pixels(&mut self, p: (f32, f32), _: PenReport, _: Instant) -> (f32, f32) {
        p
    }
    fn reset(&mut self) {}
    fn take_failure(&mut self) -> Option<&str> {
        None
    }
    fn next_tick(&mut self) -> Option<Duration> {
        (self.ticks.get() < self.limit).then_some(Duration::ZERO)
    }
    fn tick(&mut self, _: Instant, _: &mut dyn PipelineRuntime) -> io::Result<()> {
        self.ticks.set(self.ticks.get() + 1);
        Ok(())
    }
}

#[test]
fn overdue_timers_do_not_starve_stopped_source() {
    let ticks = Rc::new(Cell::new(0));
    let mut filters = Timers {
        ticks: ticks.clone(),
        limit: 10_000,
    };
    let mut source = Source {
        ticks,
        reads: 0,
        ticks_at_first_read: 0,
    };
    session::run(
        &mut source,
        &mut Display,
        &Profile::default(),
        Mode::Driver,
        &mut TabletDecoder::pth_660(),
        &mut filters,
        |_| Ok(()),
        &|_| {},
    )
    .unwrap();
    assert_eq!(source.reads, 1);
    assert!(source.ticks_at_first_read <= 1);
}

#[test]
fn rejected_activation_never_reads_ticks_or_sends() {
    let ticks = Rc::new(Cell::new(0));
    let mut filters = Timers {
        ticks: ticks.clone(),
        limit: 10_000,
    };
    let mut source = Source {
        ticks: ticks.clone(),
        reads: 0,
        ticks_at_first_read: 0,
    };
    session::run_gated(
        &mut source,
        &mut Display,
        &Profile::default(),
        Mode::Driver,
        &mut TabletDecoder::pth_660(),
        &mut filters,
        |_| panic!("output before activation"),
        &|_| {},
        || Ok(false),
    )
    .unwrap();
    assert_eq!((source.reads, ticks.get()), (0, 0));
}

#[test]
fn idle_sessions_retry_failed_cleanup_without_a_timer_or_busy_polling() {
    struct Input {
        step: u8,
        bytes: [u8; 17],
        waits: Vec<Duration>,
        ended: Rc<Cell<bool>>,
    }
    impl ReportSource for Input {
        fn label(&self) -> &str {
            "cleanup fixture"
        }
        fn now(&self) -> Instant {
            Instant::now()
        }
        fn next(&mut self, wait: Duration) -> io::Result<Read<'_>> {
            self.waits.push(wait);
            self.step += 1;
            match self.step {
                1 | 2 => {
                    self.bytes[0] = 0x10;
                    self.bytes[1] = if self.step == 1 { 0x61 } else { 0 };
                    self.bytes[8] = if self.step == 1 { 100 } else { 0 };
                    Ok(Read::Report {
                        bytes: &self.bytes,
                        ready: Instant::now(),
                        queued: false,
                    })
                }
                3..=7 => Ok(Read::Idle),
                _ => {
                    self.ended.set(true);
                    Ok(Read::Ended)
                }
            }
        }
    }
    let ended = Rc::new(Cell::new(false));
    let mut source = Input {
        step: 0,
        bytes: [0; 17],
        waits: Vec::new(),
        ended: ended.clone(),
    };
    let mut releases = 0;
    session::run(
        &mut source,
        &mut Display,
        &Profile::default(),
        Mode::Driver,
        &mut TabletDecoder::pth_660(),
        &mut otd_core::plugins::NoFilters,
        |packet| {
            if packet.flags & otd_core::output::flags::LEFTUP != 0 {
                releases += 1;
                assert!(!ended.get(), "cleanup must recover before session exit");
                if releases <= 3 {
                    return Err(io::Error::other("output temporarily unavailable"));
                }
            }
            Ok(())
        },
        &|_| {},
    )
    .unwrap();
    assert_eq!(releases, 4);
    assert!(source.waits.contains(&Duration::from_millis(50)));
    assert!(
        source
            .waits
            .iter()
            .all(|wait| *wait >= Duration::from_millis(50))
    );
}

#[test]
fn failed_release_recovers_on_timer_tick() {
    let profile = Profile::default();
    let mapper = snapshot().mapper(&profile).unwrap();
    let mut pipeline = ReportPipeline::new(&profile).unwrap();
    let pen = PenReport {
        id: 0x10,
        x: 10000,
        y: 10000,
        pressure: 100,
        in_range: true,
        sense: true,
        tip_switch: true,
        eraser: false,
        tilt: [0, 0],
        rotation: None,
        hover_distance: None,
    };
    let ticks = Rc::new(Cell::new(0));
    let mut filters = Timers {
        ticks: ticks.clone(),
        limit: 100,
    };
    pipeline
        .process(pen, Instant::now(), Some(mapper), &mut filters, |_| Ok(()))
        .unwrap();
    assert!(
        pipeline
            .release_all(|_| Err(io::Error::other("simulated transient output failure")))
            .is_err()
    );
    let mut cleanup_attempts = 0;
    for _ in 0..100 {
        pipeline
            .process_tick(Instant::now(), Some(mapper), &mut filters, |_| {
                cleanup_attempts += 1;
                Ok(())
            })
            .unwrap();
    }
    assert_eq!(ticks.get(), 100);
    assert_eq!(cleanup_attempts, 1);
    assert_eq!(filters.next_tick(), None);
}

#[test]
fn retained_timer_output_cannot_restore_lost_contact() {
    struct Retained {
        contact: ReportValues,
    }
    impl Filters for Retained {
        fn has_pre(&self) -> bool {
            false
        }
        fn has_pixels(&self) -> bool {
            false
        }
        fn process_pre(&mut self, p: (f32, f32), _: PenReport, _: Instant) -> (f32, f32) {
            p
        }
        fn process_pixels(&mut self, p: (f32, f32), _: PenReport, _: Instant) -> (f32, f32) {
            p
        }
        fn reset(&mut self) {}
        fn take_failure(&mut self) -> Option<&str> {
            None
        }
        fn tick(&mut self, _: Instant, runtime: &mut dyn PipelineRuntime) -> io::Result<()> {
            runtime.output(ReportKind::Data, &self.contact, &[])
        }
    }
    let profile = Profile::default();
    let mapper = snapshot().mapper(&profile).unwrap();
    let mut pipeline = ReportPipeline::new(&profile).unwrap();
    let mut filters = Retained {
        contact: ReportValues {
            pressure: Some(100),
            tip_switch: Some(true),
            ..Default::default()
        },
    };
    let down = PenReport {
        id: 0x10,
        x: 10000,
        y: 10000,
        pressure: 100,
        in_range: true,
        sense: true,
        tip_switch: true,
        eraser: false,
        tilt: [0, 0],
        rotation: None,
        hover_distance: None,
    };
    let lost = PenReport {
        pressure: 0,
        in_range: false,
        sense: false,
        tip_switch: false,
        ..down
    };
    let mut packets = Vec::new();
    pipeline
        .process(down, Instant::now(), Some(mapper), &mut filters, |p| {
            packets.push(p);
            Ok(())
        })
        .unwrap();
    pipeline
        .process(lost, Instant::now(), Some(mapper), &mut filters, |p| {
            packets.push(p);
            Ok(())
        })
        .unwrap();
    pipeline
        .process_tick(Instant::now(), Some(mapper), &mut filters, |p| {
            packets.push(p);
            Ok(())
        })
        .unwrap();
    let transitions: Vec<_> = packets.iter().map(|p| p.flags & 6).collect();
    assert_eq!(transitions, [2, 4]);
}

#[test]
fn toml_threshold_uses_selected_tablet_pressure_range() {
    let name = "Gaomon M7";
    let spec = otd_core::config::spec_for_tablet(name).unwrap();
    assert_eq!(spec.max_pressure, 16383);
    let text =
        "schema_version = 1\ntablet = \"Gaomon M7\"\n[bindings]\ntip_threshold_raw = 16383\n";
    let profile = Profile::from_toml_text(text, std::path::Path::new("in-memory.toml")).unwrap();
    assert_eq!(profile.contact.tip_threshold_raw, Some(16383));
}

#[test]
fn prefixed_parser_payload_reaches_filters_without_transport_prefix() {
    struct Packet {
        sent: bool,
        raw: [u8; 9],
    }
    impl ReportSource for Packet {
        fn label(&self) -> &str {
            "synthetic prefixed packet"
        }
        fn now(&self) -> Instant {
            Instant::now()
        }
        fn next(&mut self, _: Duration) -> io::Result<Read<'_>> {
            if self.sent {
                return Ok(Read::Ended);
            }
            self.sent = true;
            Ok(Read::Report {
                bytes: &self.raw,
                ready: Instant::now(),
                queued: false,
            })
        }
    }
    struct Capture {
        raw: Vec<u8>,
    }
    impl Filters for Capture {
        fn has_pre(&self) -> bool {
            false
        }
        fn has_pixels(&self) -> bool {
            false
        }
        fn process_pre(&mut self, p: (f32, f32), _: PenReport, _: Instant) -> (f32, f32) {
            p
        }
        fn process_pixels(&mut self, p: (f32, f32), _: PenReport, _: Instant) -> (f32, f32) {
            p
        }
        fn reset(&mut self) {}
        fn take_failure(&mut self) -> Option<&str> {
            None
        }
        fn dispatch(
            &mut self,
            input: otd_core::plugins::DispatchInput<'_>,
            _: &mut dyn PipelineRuntime,
        ) -> io::Result<()> {
            self.raw = input.raw.to_vec();
            Ok(())
        }
    }
    let raw = [0xAA, 0x01, 0, 100, 0, 200, 0, 100, 0];
    let name = "OpenTabletDriver.Configurations.Parsers.SkipByteTabletReportParser";
    let metadata = ReportMetadata {
        device: DeviceId(0),
        session: SessionId(0),
        endpoint: EndpointId(0),
        received_at: Duration::ZERO,
        sequence: 0,
    };
    let mut parser = ReportParser::for_type(name).unwrap();
    let (_, envelope) = parser.parse(&raw, metadata).unwrap();
    assert_eq!(envelope.raw, &raw[1..]);
    let mut source = Packet { sent: false, raw };
    let mut filter = Capture { raw: vec![] };
    let mut decoder = TabletDecoder::for_parser(name, otd_core::spec::TabletSpec::PTH_660).unwrap();
    session::run(
        &mut source,
        &mut Display,
        &Profile::default(),
        Mode::Driver,
        &mut decoder,
        &mut filter,
        |_| Ok(()),
        &|_| {},
    )
    .unwrap();
    assert_eq!(filter.raw, envelope.raw);
}

#[test]
fn paused_mapping_ignores_overdue_timer_deadline() {
    let reads = Rc::new(Cell::new(0u32));
    struct Disconnect {
        snapshots: u32,
    }
    impl Displays for Disconnect {
        fn fingerprint(&mut self) -> DisplayFingerprint {
            snapshot().fingerprint()
        }
        fn snapshot(&mut self) -> Result<DisplaySnapshot, String> {
            self.snapshots += 1;
            let mut value = snapshot();
            if self.snapshots > 1 {
                value.monitors.clear();
            }
            Ok(value)
        }
    }
    struct Idle {
        reads: Rc<Cell<u32>>,
        start: Instant,
        zero_waits: u32,
    }
    impl ReportSource for Idle {
        fn label(&self) -> &str {
            "synthetic disconnected monitor"
        }
        fn now(&self) -> Instant {
            self.start + Duration::from_secs(u64::from(self.reads.get()) * 2)
        }
        fn next(&mut self, timeout: Duration) -> io::Result<Read<'_>> {
            if timeout.is_zero() {
                self.zero_waits += 1;
            }
            self.reads.set(self.reads.get() + 1);
            Ok(if self.reads.get() > 100 {
                Read::Ended
            } else {
                Read::Idle
            })
        }
    }
    struct Timer {
        reads: Rc<Cell<u32>>,
    }
    impl Filters for Timer {
        fn has_pre(&self) -> bool {
            false
        }
        fn has_pixels(&self) -> bool {
            false
        }
        fn process_pre(&mut self, p: (f32, f32), _: PenReport, _: Instant) -> (f32, f32) {
            p
        }
        fn process_pixels(&mut self, p: (f32, f32), _: PenReport, _: Instant) -> (f32, f32) {
            p
        }
        fn reset(&mut self) {}
        fn take_failure(&mut self) -> Option<&str> {
            None
        }
        fn next_tick(&mut self) -> Option<Duration> {
            Some(if self.reads.get() == 0 {
                Duration::from_millis(1)
            } else {
                Duration::ZERO
            })
        }
        fn tick(&mut self, _: Instant, _: &mut dyn PipelineRuntime) -> io::Result<()> {
            panic!("suspended mapping should skip tick")
        }
    }
    let mut source = Idle {
        reads: reads.clone(),
        start: Instant::now(),
        zero_waits: 0,
    };
    let mut filter = Timer { reads };
    let profile = Profile {
        monitor: Some(0),
        ..Profile::default()
    };
    session::run(
        &mut source,
        &mut Disconnect { snapshots: 0 },
        &profile,
        Mode::Driver,
        &mut TabletDecoder::pth_660(),
        &mut filter,
        |_| Ok(()),
        &|_| {},
    )
    .unwrap();
    assert_eq!(source.zero_waits, 0);
}

#[test]
fn display_snapshot_failure_retries_while_reports_flow() {
    let reads = Rc::new(Cell::new(0u32));
    struct Changing {
        reads: Rc<Cell<u32>>,
        snapshots: u32,
    }
    impl Displays for Changing {
        fn fingerprint(&mut self) -> DisplayFingerprint {
            let mut value = snapshot().fingerprint();
            if self.reads.get() > 0 {
                value.virtual_screen.right = 2560;
            }
            value
        }
        fn snapshot(&mut self) -> Result<DisplaySnapshot, String> {
            self.snapshots += 1;
            if self.snapshots == 2 {
                return Err("simulated transient display failure".into());
            }
            let mut value = snapshot();
            if self.snapshots > 2 {
                value.virtual_screen.right = 2560;
                value.monitors[0].right = 2560;
            }
            Ok(value)
        }
    }
    struct Reports {
        reads: Rc<Cell<u32>>,
        start: Instant,
        raw: [u8; 8],
    }
    impl ReportSource for Reports {
        fn label(&self) -> &str {
            "continuous in-memory reports"
        }
        fn now(&self) -> Instant {
            self.start + Duration::from_secs(u64::from(self.reads.get()) * 2)
        }
        fn next(&mut self, _: Duration) -> io::Result<Read<'_>> {
            self.reads.set(self.reads.get() + 1);
            if self.reads.get() > 10 {
                return Ok(Read::Ended);
            }
            Ok(Read::Report {
                bytes: &self.raw,
                ready: self.now(),
                queued: false,
            })
        }
    }
    let mut displays = Changing {
        reads: reads.clone(),
        snapshots: 0,
    };
    let mut source = Reports {
        reads,
        start: Instant::now(),
        raw: [1, 0, 100, 0, 200, 0, 0, 0],
    };
    let mut decoder = TabletDecoder::for_parser(
        "OpenTabletDriver.Plugin.Tablet.TabletReportParser",
        otd_core::spec::TabletSpec::PTH_660,
    )
    .unwrap();
    session::run(
        &mut source,
        &mut displays,
        &Profile::default(),
        Mode::Driver,
        &mut decoder,
        &mut otd_core::plugins::NoFilters,
        |_| Ok(()),
        &|_| {},
    )
    .unwrap();
    assert_eq!(displays.snapshots, 3);
}
