//! One device session: waits for reports, decodes them, runs them through the
//! pipeline and sends the packets, and follows display changes. The platform
//! supplies the device and its clock (`ReportSource`), the desktop layout
//! (`Displays`), the DLL filters and the output sink, so the loop runs the
//! same on a real tablet and in tests.

use std::io;
use std::time::{Duration, Instant};

use crate::config::Profile;
use crate::decoders::PenDecoder;
use crate::display::{DisplayFingerprint, DisplaySnapshot};
use crate::mapping::Mapper;
use crate::output::MousePacket;
use crate::output::buttons::ActionSink;
use crate::output::pen::PenSink;
use crate::pipeline::ReportPipeline;
use crate::plugins::Filters;
use crate::protocol;

/// A session cannot safely reconnect after its acknowledged output failed to
/// release. Preserve this distinction through the platform session adapter.
#[derive(Debug)]
struct OutputCleanupError(io::Error);

impl std::fmt::Display for OutputCleanupError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "output cleanup failed: {}", self.0)
    }
}

impl std::error::Error for OutputCleanupError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.0)
    }
}

pub fn is_cleanup_failure(error: &io::Error) -> bool {
    error
        .get_ref()
        .is_some_and(|inner| inner.is::<OutputCleanupError>())
}
#[derive(Clone, Copy)]
pub enum Mode {
    Driver,
    Capture { deadline: Instant, limit: u32 },
}

/// What a device produced.
pub enum Read<'a> {
    /// A report. `ready` is when its read completed; `queued` means it was
    /// already waiting when the read started, so the loop was behind.
    Report {
        bytes: &'a [u8],
        ready: Instant,
        queued: bool,
    },
    /// A report from the tablet's separate auxiliary endpoint (express keys,
    /// wheels), decoded with that endpoint's own parser.
    Auxiliary {
        bytes: &'a [u8],
        ready: Instant,
        queued: bool,
    },
    /// The auxiliary endpoint failed or went away while the pen endpoint
    /// keeps working. What it held is released.
    AuxiliaryEnded,
    /// Nothing arrived within the timeout; a pending read stays pending.
    Idle,
    /// The device went away or a stop was requested.
    Ended,
}

/// A device endpoint that delivers input reports.
pub trait ReportSource {
    /// A hosted original OutputMode may take exclusive authority after native
    /// actions have been released. Sources wake next() when this changes.
    fn native_output_enabled(&self) -> bool { true }
    fn output_started(&self) {}
    fn output_acknowledged(&self, _enabled: bool) {}
    fn shared_output(&self) -> bool {
        false
    }
    /// Describes the endpoint for the log.
    fn label(&self) -> &str;
    /// The clock `Read::Report::ready` is measured on.
    fn now(&self) -> Instant;
    /// Waits up to `timeout` for the next report.
    fn next(&mut self, timeout: Duration) -> io::Result<Read<'_>>;
}

/// The desktop layout, read from the report thread.
pub trait Displays {
    /// Called once a second while reports flow, so it must be cheap.
    fn fingerprint(&mut self) -> DisplayFingerprint;
    /// A backend may detect rectangle changes which preserve aggregate bounds.
    fn topology_changed(&mut self) -> bool { false }
    fn snapshot(&mut self) -> Result<DisplaySnapshot, String>;
}

#[derive(Default)]
struct Counters {
    read: u64,
    accepted: u64,
    ignored: u64,
    malformed: u64,
    output_commits: u64,
    output_failures: u64,
}

/// Time from a completed read to the end of its output, in whole
/// microseconds, and how often a read found its report already queued, which
/// means the loop fell behind the tablet. Fixed-size; recording does not
/// allocate. The last bucket holds everything from 1 ms up.
struct Timing {
    micros: Box<[u64; Timing::BUCKETS]>,
    reports: u64,
    max: Duration,
    queued: u64,
}

impl Timing {
    const BUCKETS: usize = 1001;

    fn new() -> Self {
        Self {
            micros: Box::new([0; Self::BUCKETS]),
            reports: 0,
            max: Duration::ZERO,
            queued: 0,
        }
    }

    fn record(&mut self, elapsed: Duration) {
        let micros = usize::try_from(elapsed.as_micros()).unwrap_or(usize::MAX);
        self.micros[micros.min(Self::BUCKETS - 1)] += 1;
        self.reports += 1;
        self.max = self.max.max(elapsed);
    }

    /// The whole microseconds below which the given fraction of reports fell.
    fn percentile(&self, fraction: f64) -> String {
        let target = ((self.reports as f64 * fraction).ceil() as u64).max(1);
        let mut seen = 0;
        for (micros, count) in self.micros.iter().enumerate() {
            seen += *count;
            if seen >= target {
                return if micros == Self::BUCKETS - 1 {
                    "1 ms or more".to_owned()
                } else {
                    format!("{micros} us")
                };
            }
        }
        "unavailable".to_owned()
    }

    fn summary(&self) -> Option<String> {
        (self.reports > 0).then(|| {
            format!(
                "Processed {} reports; read to output p50 {}, p99 {}, max {} us; {} reads found a report already waiting",
                self.reports,
                self.percentile(0.5),
                self.percentile(0.99),
                self.max.as_micros(),
                self.queued
            )
        })
    }
}

#[derive(Default)]
struct CaptureTrace {
    initial: u8,
    ignored: u8,
    last_state: Option<(bool, bool, bool, bool)>,
}

impl CaptureTrace {
    fn pen(&mut self, bytes: &[u8], pen: protocol::PenReport) {
        let state = (pen.in_range, pen.sense, pen.tip_switch, pen.eraser);
        if self.initial < 5 || self.last_state != Some(state) {
            trace_report(bytes, Some(pen));
            self.initial = self.initial.saturating_add(1);
        }
        self.last_state = Some(state);
    }

    fn ignored(&mut self, bytes: &[u8]) {
        if self.ignored < 5 {
            trace_report(bytes, None);
            self.ignored += 1;
        }
    }
}

fn trace_report(bytes: &[u8], pen: Option<protocol::PenReport>) {
    let prefix = &bytes[..bytes.len().min(24)];
    let mut hex = String::with_capacity(prefix.len() * 2);
    for byte in prefix {
        use std::fmt::Write;
        let _ = write!(hex, "{byte:02x}");
    }
    if let Some(pen) = pen {
        println!(
            "id={:02x} x={} y={} pressure={} in_range={} sense={} tip={} eraser={} tilt={:?} bytes={}",
            pen.id,
            pen.x,
            pen.y,
            pen.pressure,
            pen.in_range,
            pen.sense,
            pen.tip_switch,
            pen.eraser,
            pen.tilt,
            hex
        );
    } else {
        println!(
            "id={:02x} ignored len={} bytes={}",
            bytes.first().copied().unwrap_or(0),
            bytes.len(),
            hex
        );
    }
}

/// The absolute mapping and the display state it was built from. Relative
/// sessions have no layout.
struct Layout {
    snapshot: Option<DisplaySnapshot>,
    fingerprint: DisplayFingerprint,
    mapper: Option<Mapper>,
    snapshot_failed: bool,
}

impl Layout {
    /// Rebuilds the mapping if the monitor layout changed. An invalid layout
    /// pauses absolute output and releases a held button.
    fn refresh(
        &mut self,
        displays: &mut impl Displays,
        profile: &Profile,
        pipeline: &mut ReportPipeline,
        send: impl FnOnce(MousePacket) -> io::Result<()>,
    ) {
        let Some(snapshot) = &mut self.snapshot else {
            // Relative mouse motion is independent of the monitor topology.
            return;
        };
        let fingerprint = displays.fingerprint();
        let current = match displays.snapshot() {
            Ok(current) => current,
            Err(error) => {
                self.mapper = None;
                if !self.snapshot_failed {
                    if let Err(release_error) = pipeline.release_all(send) {
                        eprintln!("could not release buttons after display failure: {release_error}");
                    }
                    eprintln!("display mapping paused: {error}");
                }
                self.snapshot_failed = true;
                return;
            }
        };
        self.fingerprint = fingerprint;
        if current == *snapshot && !self.snapshot_failed {
            return;
        }
        self.snapshot_failed = false;
        match current.mapper(profile) {
            Ok(mapper) => {
                self.mapper = Some(mapper);
                eprintln!("display mapping refreshed");
            }
            Err(error) => {
                self.mapper = None;
                if let Err(release_error) = pipeline.release_all(send) {
                    eprintln!("could not release buttons after display change: {release_error}");
                }
                eprintln!("display mapping paused: {error}");
            }
        }
        *snapshot = current;
    }
}

/// Runs one session until the source ends, the capture deadline or report
/// limit is reached, or an error occurs. A held button is released before it
/// returns.
#[allow(clippy::too_many_arguments)] // Each is an independent platform seam.
pub fn run(
    source: &mut impl ReportSource,
    displays: &mut impl Displays,
    profile: &Profile,
    mode: Mode,
    decoder: &mut impl PenDecoder,
    filters: &mut impl Filters,
    send: impl FnMut(MousePacket) -> io::Result<()>,
    status: &impl Fn(&str),
) -> io::Result<()> {
    run_gated(
        source,
        displays,
        profile,
        mode,
        decoder,
        filters,
        send,
        status,
        || Ok(true),
    )
}

pub fn cleanup_failure(error: io::Error) -> io::Error {
    io::Error::other(OutputCleanupError(error))
}

/// Validates mapping and pipeline setup before activation, then gates all
/// report and timer callbacks together, including the first overdue tick.
#[allow(clippy::too_many_arguments)]
pub fn run_gated(
    source: &mut impl ReportSource,
    displays: &mut impl Displays,
    profile: &Profile,
    mode: Mode,
    decoder: &mut impl PenDecoder,
    filters: &mut impl Filters,
    send: impl FnMut(MousePacket) -> io::Result<()>,
    status: &impl Fn(&str),
    gate: impl FnOnce() -> io::Result<bool>,
) -> io::Result<()> {
    run_gated_with_pen(
        source, displays, profile, mode, decoder, filters, send, None, status, gate,
    )
}

/// `run_gated` with the platform's pen device, which a profile with pen
/// output needs. Pen packets go to it; `send` then receives none.
#[allow(clippy::too_many_arguments)]
pub fn run_gated_with_pen(
    source: &mut impl ReportSource,
    displays: &mut impl Displays,
    profile: &Profile,
    mode: Mode,
    decoder: &mut impl PenDecoder,
    filters: &mut impl Filters,
    send: impl FnMut(MousePacket) -> io::Result<()>,
    pen: Option<Box<dyn PenSink>>,
    status: &impl Fn(&str),
    gate: impl FnOnce() -> io::Result<bool>,
) -> io::Result<()> {
    run_gated_with_devices(
        source, displays, profile, mode, decoder, filters, send, pen, None, status, gate,
    )
}

/// `run_gated_with_pen` with the platform's key and button output, which the
/// profile's pen side buttons use. Without it they can only drive a pen
/// device's barrel buttons.
#[allow(clippy::too_many_arguments)]
pub fn run_gated_with_devices(
    source: &mut impl ReportSource,
    displays: &mut impl Displays,
    profile: &Profile,
    mode: Mode,
    decoder: &mut impl PenDecoder,
    filters: &mut impl Filters,
    send: impl FnMut(MousePacket) -> io::Result<()>,
    pen: Option<Box<dyn PenSink>>,
    actions: Option<Box<dyn ActionSink>>,
    status: &impl Fn(&str),
    gate: impl FnOnce() -> io::Result<bool>,
) -> io::Result<()> {
    run_gated_with_endpoints(
        source, displays, profile, mode, decoder, None, filters, send, pen, actions, status, gate,
    )
}

/// `run_gated_with_devices` for a source that also reads the tablet's
/// auxiliary endpoint. `auxiliary` decodes its reports; without one they use
/// the pen endpoint's decoder.
#[allow(clippy::too_many_arguments)]
pub fn run_gated_with_endpoints(
    source: &mut impl ReportSource,
    displays: &mut impl Displays,
    profile: &Profile,
    mode: Mode,
    decoder: &mut impl PenDecoder,
    mut auxiliary: Option<&mut dyn PenDecoder>,
    filters: &mut impl Filters,
    mut send: impl FnMut(MousePacket) -> io::Result<()>,
    pen: Option<Box<dyn PenSink>>,
    actions: Option<Box<dyn ActionSink>>,
    status: &impl Fn(&str),
    gate: impl FnOnce() -> io::Result<bool>,
) -> io::Result<()> {
    let mut pipeline = ReportPipeline::new(profile).map_err(io::Error::other)?;
    if let Some(sink) = actions
        && matches!(mode, Mode::Driver)
    {
        for message in pipeline.set_action_sink(sink) {
            eprintln!("Binding not applied: {message}");
            status(&format!("Binding not applied: {message}"));
        }
    }
    match pen {
        Some(sink) if pipeline.wants_pen() => pipeline.set_pen_sink(sink),
        None if pipeline.wants_pen() && matches!(mode, Mode::Driver) => {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "pen output is not available here; choose mouse output",
            ));
        }
        _ => {}
    }
    if source.shared_output() {
        pipeline.share_output();
    }
    let snapshot = if pipeline.is_relative() {
        None
    } else {
        Some(displays.snapshot().map_err(io::Error::other)?)
    };
    let mapper = snapshot
        .as_ref()
        .map(|s| s.mapper(profile))
        .transpose()
        .map_err(io::Error::other)?;
    let mut layout = Layout {
        snapshot,
        fingerprint: displays.fingerprint(),
        mapper,
        snapshot_failed: false,
    };
    let start = source.now();
    let mut next_refresh = start + Duration::from_secs(1);
    let mut last_output_warning: Option<Instant> = None;
    let mut counters = Counters::default();
    let mut timing = Timing::new();
    let mut capture_trace = CaptureTrace::default();

    if !gate()? {
        return Ok(());
    }
    source.output_started();
    let mut native_output = true;
    source.output_acknowledged(native_output);

    eprintln!("Tablet connected: {}", source.label());
    status("Tablet connected; receiving pen input");
    let outcome = (|| -> io::Result<()> {
        loop {
            let requested_output = source.native_output_enabled();
            if requested_output != native_output {
                pipeline.release_all(&mut send).map_err(|error| io::Error::other(OutputCleanupError(error)))?;
                filters.reset();
                decoder.reset();
                if let Some(decoder)=auxiliary.as_deref_mut() { decoder.reset(); }
                native_output=requested_output;
                source.output_acknowledged(native_output);
            }
            // Timer-driven filters tick on this thread, between reads.
            let filter_tick = match mode {
                Mode::Driver if native_output && (pipeline.is_relative() || layout.mapper.is_some()) => {
                    filters.next_tick()
                }
                Mode::Driver => None,
                Mode::Capture { .. } => None,
            };
            let binding_tick = if native_output && matches!(mode, Mode::Driver) { pipeline.next_binding_tick(source.now()) } else { None };
            let mut tick = filter_tick.into_iter().chain(binding_tick).min();
            if pipeline.needs_cleanup() || tick.is_some_and(|wait| wait.is_zero()) {
                let mut output = |packet| {
                    send(packet)?;
                    counters.output_commits += 1;
                    Ok(())
                };
                let ticked = if pipeline.needs_cleanup() {
                    pipeline.release_all(&mut output).map(|_| ())
                } else {
                    let now = source.now();
                    let filtered = if filter_tick.is_some_and(|wait| wait.is_zero()) {
                        pipeline.process_tick(now, layout.mapper, filters, &mut output).map(|_| ())
                    } else { Ok(()) };
                    filtered.and_then(|()| {
                        if binding_tick.is_some_and(|wait| wait.is_zero()) {
                            pipeline.process_binding_tick_with_output(now, layout.mapper, &mut output)
                        } else { Ok(()) }
                    })
                };
                if let Some(name) = filters.take_failure() {
                    status(&format!(
                        "Disabled failing plugin: {name}. Restart to retry."
                    ));
                }
                if let Err(error) = ticked {
                    counters.output_failures += 1;
                    let now = source.now();
                    if last_output_warning.is_none_or(|last| now - last >= Duration::from_secs(5)) {
                        eprintln!("Report pipeline failed: {error}");
                        last_output_warning = Some(now);
                    }
                }
                // Always reach the input/stop poll after one tick, even if a
                // slow or failed timer still reports an overdue deadline.
                let filter_tick = match mode {
                    Mode::Driver if native_output && (pipeline.is_relative() || layout.mapper.is_some()) => filters.next_tick(),
                    _ => None,
                };
                let binding_tick = if native_output && matches!(mode, Mode::Driver) { pipeline.next_binding_tick(source.now()) } else { None };
                tick = filter_tick.into_iter().chain(binding_tick).min();
            }
            let timeout = match mode {
                Mode::Capture { deadline, limit } => {
                    let now = source.now();
                    if now >= deadline || counters.read >= u64::from(limit) {
                        break;
                    }
                    deadline
                        .saturating_duration_since(now)
                        .min(Duration::from_secs(1))
                }
                // Input can wake this early, but a locked desktop must not
                // turn an idle failed release into a 1 kHz polling loop.
                Mode::Driver if pipeline.needs_cleanup() => Duration::from_millis(50),
                Mode::Driver => tick.map_or(Duration::from_secs(1), |wait| {
                    wait.min(Duration::from_secs(1))
                }),
            };
            let (bytes, ready, queued, from_auxiliary) = match source.next(timeout)? {
                Read::Ended => break,
                Read::AuxiliaryEnded => {
                    if let Some(decoder) = auxiliary.as_deref_mut() {
                        decoder.reset();
                    }
                    if let Err(error) = pipeline.release_auxiliary() {
                        counters.output_failures += 1;
                        eprintln!("could not release express keys: {error}");
                    }
                    eprintln!("Auxiliary endpoint lost; pen input continues.");
                    status("Express keys and wheels disconnected; pen input continues");
                    continue;
                }
                Read::Idle => {
                    let now = source.now();
                    if matches!(mode, Mode::Capture { deadline, .. } if now >= deadline) {
                        break;
                    }
                    // Idle: compare the full monitor layout.
                    if now >= next_refresh {
                        layout.refresh(displays, profile, &mut pipeline, &mut send);
                        next_refresh = source.now() + Duration::from_secs(1);
                    }
                    continue;
                }
                Read::Report {
                    bytes,
                    ready,
                    queued,
                } => (bytes, ready, queued, false),
                Read::Auxiliary {
                    bytes,
                    ready,
                    queued,
                } => (bytes, ready, queued, true),
            };
            if queued {
                timing.queued += 1;
            }
            counters.read += 1;
            crate::debug::record_at(bytes, ready, from_auxiliary);
            if !native_output && matches!(mode, Mode::Driver) { continue; }
            let decoded = match auxiliary.as_deref_mut() {
                Some(decoder) if from_auxiliary => decoder.decode_input(bytes),
                _ => decoder.decode_input(bytes),
            };
            let mut timed_output = false;
            match decoded {
                Ok(Some(decoded)) => {
                    counters.accepted += 1;
                    if let Mode::Capture { .. } = mode {
                        if let Some(pen) = decoded.pen() {
                            capture_trace.pen(bytes, pen);
                        } else {
                            capture_trace.ignored(bytes);
                        }
                    } else {
                        // Paused mappings still enter the pipeline's cleanup
                        // gate, which retries failed releases without filters.
                        let emitted = pipeline.process_input(
                            decoded,
                            ready,
                            layout.mapper,
                            filters,
                            |packet| {
                                send(packet)?;
                                // Count acknowledged prefixes even if a later
                                // emission from this input fails.
                                counters.output_commits += 1;
                                Ok(())
                            },
                        );
                        // Pen packets bypass the mouse sink's commit count.
                        if pipeline.wants_pen()
                            && let Ok(stats) = &emitted
                        {
                            counters.output_commits += stats.packets;
                        }
                        if let Some(name) = filters.take_failure() {
                            status(&format!(
                                "Disabled failing plugin: {name}. Restart to retry."
                            ));
                        }
                        if let Err(error) = emitted {
                            counters.output_failures += 1;
                            let now = source.now();
                            if last_output_warning
                                .is_none_or(|last| now - last >= Duration::from_secs(5))
                            {
                                eprintln!("Report pipeline failed: {error}");
                                last_output_warning = Some(now);
                            }
                        }
                        timed_output = true;
                    }
                }
                Ok(None) => {
                    counters.ignored += 1;
                    if let Mode::Capture { .. } = mode {
                        capture_trace.ignored(bytes);
                    }
                }
                Err(error) => {
                    counters.malformed += 1;
                    if let Mode::Capture { .. } = mode {
                        eprintln!("malformed report: {error:?}");
                        capture_trace.ignored(bytes);
                    }
                }
            }
            // While reports flow, re-read the monitors only when the cheap
            // fingerprint changed; the idle wait compares the full layout.
            let now = source.now();
            if timed_output {
                timing.record(now.saturating_duration_since(ready));
            }
            if now >= next_refresh {
                if layout.snapshot.is_some()
                    && (layout.snapshot_failed || displays.fingerprint() != layout.fingerprint || displays.topology_changed())
                {
                    layout.refresh(displays, profile, &mut pipeline, &mut send);
                }
                next_refresh = source.now() + Duration::from_secs(1);
            }
        }
        Ok(())
    })();
    decoder.reset();
    if let Some(decoder) = auxiliary {
        decoder.reset();
    }
    let cleanup = pipeline.release_all(&mut send).map(|_| ());
    if let Err(error) = &cleanup {
        eprintln!("could not release mouse buttons: {error}");
    }
    eprintln!(
        "session ended: read={} accepted={} ignored={} malformed={} output_commits={} output_failures={}",
        counters.read,
        counters.accepted,
        counters.ignored,
        counters.malformed,
        counters.output_commits,
        counters.output_failures
    );
    if let Some(summary) = timing.summary() {
        eprintln!("{summary}");
        status(&summary);
    }
    // Cleanup failure takes precedence: reconnecting would discard ownership
    // of an action that the OS may still consider held.
    cleanup.map_err(|error| io::Error::other(OutputCleanupError(error)))?;
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mapping::Rect;
    use crate::output::flags;
    use crate::plugins::NoFilters;
    use crate::test_alloc;
    use std::cell::{Cell, RefCell};
    use std::collections::VecDeque;
    use std::rc::Rc;

    #[test]
    fn timing_summary_reports_percentiles_and_late_reads() {
        let mut timing = Timing::new();
        assert!(timing.summary().is_none());
        for _ in 0..98 {
            timing.record(Duration::from_nanos(5_600));
        }
        timing.record(Duration::from_micros(40));
        timing.record(Duration::from_millis(3));
        timing.queued = 2;
        assert_eq!(
            timing.summary().unwrap(),
            concat!(
                "Processed 100 reports; read to output p50 5 us, p99 40 us, max 3000 us; ",
                "2 reads found a report already waiting"
            )
        );
        let mut slow = Timing::new();
        slow.record(Duration::from_millis(2));
        assert_eq!(slow.percentile(0.5), "1 ms or more");
    }

    #[test]
    fn timing_bucket_survives_more_than_u32_max_reports() {
        let mut timing = Timing::new();
        timing.micros[0] = u64::from(u32::MAX);
        timing.reports = u64::from(u32::MAX);
        timing.record(Duration::ZERO);
        assert_eq!(timing.micros[0], 4_294_967_296);
        assert_eq!(timing.percentile(0.99), "0 us");
        assert!(timing.summary().unwrap().contains("4294967296 reports"));
    }

    // Captured report prefixes: hover and contact at nearly the same place.
    const HOVER: [u8; 17] = [
        0x10, 0x60, 0x14, 0x56, 0x00, 0xa3, 0x16, 0x00, 0x00, 0x00, 0x07, 0x04, 0, 0, 0, 0, 0x28,
    ];
    const CONTACT: [u8; 17] = [
        0x10, 0x61, 0x11, 0x55, 0x00, 0x8c, 0x12, 0x00, 0x0e, 0x11, 0x00, 0x07, 0, 0, 0, 0, 0x19,
    ];

    enum Event {
        Report(&'static [u8], bool),
        Auxiliary(&'static [u8]),
        AuxiliaryEnded,
        Idle,
    }

    /// Replays scripted events; each one advances a fake clock shared with
    /// the test. Ends when the script runs out.
    struct FakeSource {
        clock: Rc<Cell<Instant>>,
        events: VecDeque<(u64, Event)>,
        start: Instant,
        waits: Vec<Duration>,
    }

    impl FakeSource {
        fn new(clock: Rc<Cell<Instant>>, events: Vec<(u64, Event)>) -> Self {
            let start = clock.get();
            Self {
                clock,
                events: events.into(),
                start,
                waits: Vec::new(),
            }
        }
    }

    impl ReportSource for FakeSource {
        fn label(&self) -> &str {
            "fake pen endpoint"
        }

        fn now(&self) -> Instant {
            self.clock.get()
        }

        fn next(&mut self, timeout: Duration) -> io::Result<Read<'_>> {
            self.waits.push(timeout);
            let Some((at_ms, event)) = self.events.pop_front() else {
                return Ok(Read::Ended);
            };
            let at = self.start + Duration::from_millis(at_ms);
            self.clock.set(at);
            Ok(match event {
                Event::Report(bytes, queued) => Read::Report {
                    bytes,
                    ready: at,
                    queued,
                },
                Event::Auxiliary(bytes) => Read::Auxiliary {
                    bytes,
                    ready: at,
                    queued: false,
                },
                Event::AuxiliaryEnded => Read::AuxiliaryEnded,
                Event::Idle => Read::Idle,
            })
        }
    }

    /// Desktop layouts that take effect at scripted times on the shared clock.
    struct FakeDisplays {
        clock: Rc<Cell<Instant>>,
        start: Instant,
        schedule: Vec<(u64, DisplaySnapshot)>,
    }

    impl FakeDisplays {
        fn current(&self) -> &DisplaySnapshot {
            let now = self.clock.get();
            self.schedule
                .iter()
                .rev()
                .find(|(at_ms, _)| self.start + Duration::from_millis(*at_ms) <= now)
                .map(|(_, snapshot)| snapshot)
                .expect("the schedule starts at 0 ms")
        }
    }

    impl Displays for FakeDisplays {
        // Like the Windows fingerprint, this does not allocate.
        fn fingerprint(&mut self) -> DisplayFingerprint {
            self.current().fingerprint()
        }

        fn snapshot(&mut self) -> Result<DisplaySnapshot, String> {
            Ok(self.current().clone())
        }
    }

    /// Monitors of the given widths side by side, 1080 px high.
    fn monitors(widths: &[i32]) -> DisplaySnapshot {
        let mut left = 0;
        let monitors: Vec<Rect> = widths
            .iter()
            .map(|width| {
                let rect = Rect {
                    left,
                    top: 0,
                    right: left + width,
                    bottom: 1080,
                };
                left += width;
                rect
            })
            .collect();
        DisplaySnapshot {
            virtual_screen: Rect {
                left: 0,
                top: 0,
                right: left.max(1920),
                bottom: 1080,
            },
            monitors,
        }
    }

    /// Runs a session over the events and returns the packets and statuses.
    fn session(
        profile: &Profile,
        mode: impl FnOnce(Instant) -> Mode,
        events: Vec<(u64, Event)>,
        schedule: Vec<(u64, DisplaySnapshot)>,
        filters: &mut impl Filters,
    ) -> (Vec<MousePacket>, Vec<String>) {
        let clock = Rc::new(Cell::new(Instant::now()));
        let start = clock.get();
        let mode = mode(start);
        let mut displays = FakeDisplays {
            clock: clock.clone(),
            start,
            schedule,
        };
        let mut source = FakeSource::new(clock, events);
        let packets = RefCell::new(Vec::new());
        let statuses = RefCell::new(Vec::new());
        run(
            &mut source,
            &mut displays,
            profile,
            mode,
            &mut crate::decoders::TabletDecoder::pth_660(),
            filters,
            |packet| {
                packets.borrow_mut().push(packet);
                Ok(())
            },
            &|message: &str| statuses.borrow_mut().push(message.to_owned()),
        )
        .unwrap();
        (packets.into_inner(), statuses.into_inner())
    }

    /// Monitor 0 selected, 1 % tip threshold.
    fn profile() -> Profile {
        let mut profile = Profile {
            monitor: Some(0),
            ..Profile::default()
        };
        profile.contact.tip_threshold_raw = Some(82);
        profile
    }

    const ABSOLUTE: u32 = flags::MOVE | flags::ABSOLUTE | flags::VIRTUALDESK;

    #[test]
    fn session_sends_reports_and_releases_when_the_device_ends() {
        let (packets, statuses) = session(
            &profile(),
            |_| Mode::Driver,
            vec![
                (100, Event::Report(&HOVER, false)),
                (105, Event::Report(&CONTACT, true)),
                (110, Event::Report(&[0x13, 0x64], false)),
            ],
            vec![(0, monitors(&[1920]))],
            &mut NoFilters,
        );
        let flags: Vec<u32> = packets.iter().map(|p| p.flags).collect();
        // The 0x13 report reaches the pipeline as a plain device report, as
        // upstream passes one to its filters, and moves nothing.
        assert_eq!(flags, [ABSOLUTE, ABSOLUTE | flags::LEFTDOWN, flags::LEFTUP]);
        assert_eq!(statuses[0], "Tablet connected; receiving pen input");
        assert!(
            statuses[1].starts_with("Processed 3 reports")
                && statuses[1].ends_with("1 reads found a report already waiting"),
            "{}",
            statuses[1]
        );
    }

    // IntuosV2 auxiliary reports: the first express key down, then up.
    const KEY_DOWN: [u8; 10] = [0x11, 0x01, 0, 0, 0, 0, 0, 0, 0, 0];

    #[test]
    fn native_scroll_repeats_between_reads_and_capture_and_activation_gate_send_nothing() {
        const KEY_UP: [u8; 10] = [0x11, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        for (capture, activated) in [(false, true), (true, true), (false, false)] {
            let mut profile = profile();
            profile.aux_buttons = vec!["scroll:vertical:-120:10".parse().unwrap()];
            let clock = Rc::new(Cell::new(Instant::now()));
            let start = clock.get();
            let mut displays = FakeDisplays { clock: clock.clone(), start, schedule: vec![(0, monitors(&[1920]))] };
            let mut source = FakeSource::new(clock.clone(), vec![
                (100, Event::Auxiliary(&KEY_DOWN)), (135, Event::Idle),
                (145, Event::Idle), (150, Event::Auxiliary(&KEY_UP)), (190, Event::Idle),
            ]);
            let pulses = Rc::new(RefCell::new(Vec::new()));
            let log = pulses.clone();
            let timing = clock.clone();
            let sink = crate::output::buttons::LocalActions::new(|_| Ok(()), |_| true)
                .with_scroll(move |pulse| { log.borrow_mut().push((timing.get(), pulse)); Ok(()) });
            let mut auxiliary = crate::decoders::TabletDecoder::for_parser(
                "OpenTabletDriver.Configurations.Parsers.Wacom.IntuosV2.IntuosV2ReportParser", profile.tablet,
            ).unwrap();
            let mode = if capture { Mode::Capture { deadline: start + Duration::from_secs(1), limit: 10 } } else { Mode::Driver };
            run_gated_with_endpoints(
                &mut source, &mut displays, &profile, mode,
                &mut crate::decoders::TabletDecoder::pth_660(), Some(&mut auxiliary), &mut NoFilters,
                |_| Ok(()), None, Some(Box::new(sink)), &|_: &str| {}, || Ok(activated),
            ).unwrap();
            if !activated {
                assert!(source.waits.is_empty(), "activation rejection never reads input");
            }
            if capture || !activated {
                assert!(pulses.borrow().is_empty(), "capture or activation rejection must not inject scroll");
            } else {
                let pulses = pulses.borrow();
                assert_eq!(pulses.iter().map(|(at, _)| at.duration_since(start).as_millis()).collect::<Vec<_>>(), [100, 135, 145]);
                assert!(pulses.iter().all(|(_, pulse)| pulse.delta == 120));
                assert_eq!(source.waits, [Duration::from_secs(1), Duration::from_millis(10), Duration::from_millis(10), Duration::from_millis(10), Duration::from_secs(1), Duration::from_secs(1)], "release restores idle polling and missed periods do not busy-loop");
            }
        }
    }

    #[test]
    fn auxiliary_reads_use_their_decoder_and_losing_them_keeps_the_pen() {
        use crate::actions::{Action, MouseButton};
        let mut profile = profile();
        profile.aux_buttons = vec!["mouse:forward".parse().unwrap()];
        let clock = Rc::new(Cell::new(Instant::now()));
        let start = clock.get();
        let mut displays = FakeDisplays {
            clock: clock.clone(),
            start,
            schedule: vec![(0, monitors(&[1920]))],
        };
        let mut source = FakeSource::new(
            clock,
            vec![
                (100, Event::Report(&HOVER, false)),
                (101, Event::Auxiliary(&KEY_DOWN)),
                (102, Event::AuxiliaryEnded),
                (103, Event::Report(&HOVER, false)),
            ],
        );
        let actions = Rc::new(RefCell::new(Vec::new()));
        let log = actions.clone();
        let sink = crate::output::buttons::LocalActions::new(
            move |transition: crate::actions::ActionTransition| {
                log.borrow_mut().push((transition.action, transition.pressed));
                Ok(())
            },
            |_| true,
        );
        let statuses = RefCell::new(Vec::new());
        let mut auxiliary = crate::decoders::TabletDecoder::for_parser(
            "OpenTabletDriver.Configurations.Parsers.Wacom.IntuosV2.IntuosV2ReportParser",
            profile.tablet,
        )
        .unwrap();
        let packets = RefCell::new(0);
        run_gated_with_endpoints(
            &mut source,
            &mut displays,
            &profile,
            Mode::Driver,
            &mut crate::decoders::TabletDecoder::pth_660(),
            Some(&mut auxiliary),
            &mut NoFilters,
            |_| {
                *packets.borrow_mut() += 1;
                Ok(())
            },
            None,
            Some(Box::new(sink)),
            &|message: &str| statuses.borrow_mut().push(message.to_owned()),
            || Ok(true),
        )
        .unwrap();
        let forward = Action::Mouse(MouseButton::Forward);
        assert_eq!(*actions.borrow(), [(forward, true), (forward, false)]);
        assert_eq!(*packets.borrow(), 1, "the second hover at the same place moves nothing");
        assert!(
            statuses
                .borrow()
                .iter()
                .any(|status| status.contains("Express keys and wheels disconnected")),
            "{:?}",
            statuses.borrow()
        );
    }

    #[test]
    fn pen_profiles_drive_the_pen_device_instead_of_the_mouse() {
        use crate::output::pen::{PenPacket, PenPhase};
        let mut profile = profile();
        profile.output = crate::config::OutputKind::Pen;
        let events = || {
            vec![
                (100, Event::Report(&HOVER, false)),
                (105, Event::Report(&CONTACT, false)),
                (110, Event::Report(&HOVER, false)),
            ]
        };
        let clock = Rc::new(Cell::new(Instant::now()));
        let start = clock.get();
        let mut displays = FakeDisplays {
            clock: clock.clone(),
            start,
            schedule: vec![(0, monitors(&[1920]))],
        };
        let mut source = FakeSource::new(clock.clone(), events());
        let mouse = RefCell::new(Vec::new());
        let pen: Rc<RefCell<Vec<PenPacket>>> = Rc::default();
        let sink = Rc::clone(&pen);
        run_gated_with_pen(
            &mut source,
            &mut displays,
            &profile,
            Mode::Driver,
            &mut crate::decoders::TabletDecoder::pth_660(),
            &mut NoFilters,
            |packet| {
                mouse.borrow_mut().push(packet);
                Ok(())
            },
            Some(Box::new(move |packet| {
                sink.borrow_mut().push(packet);
                Ok(())
            })),
            &|_| {},
            || Ok(true),
        )
        .unwrap();
        assert!(mouse.borrow().is_empty(), "{:?}", mouse.borrow());
        let pen = pen.borrow();
        let phases: Vec<_> = pen.iter().map(|packet| packet.phase).collect();
        // Hover, touch, lift, then the session's cleanup leaves range.
        assert_eq!(
            phases,
            [
                PenPhase::Hover,
                PenPhase::Down,
                PenPhase::Up,
                PenPhase::Leave
            ]
        );
        assert!(
            pen[1].pressure > 0.0 && pen[1].pressure < 1.0,
            "{:?}",
            pen[1]
        );
        assert!(pen[1].tilt.is_some());
        for packet in pen.iter() {
            assert!((0.0..1920.0).contains(&packet.x) && (0.0..1080.0).contains(&packet.y));
        }

        // A platform without a pen device refuses the profile.
        let mut source = FakeSource::new(clock, events());
        let error = run(
            &mut source,
            &mut displays,
            &profile,
            Mode::Driver,
            &mut crate::decoders::TabletDecoder::pth_660(),
            &mut NoFilters,
            |_| Ok(()),
            &|_| {},
        )
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Unsupported);
    }

    #[test]
    fn idle_refresh_follows_a_new_layout_and_pauses_when_the_monitor_goes() {
        let (packets, _) = session(
            &profile(),
            |_| Mode::Driver,
            vec![
                (100, Event::Report(&CONTACT, false)),
                (1_200, Event::Idle),
                (1_300, Event::Report(&CONTACT, false)),
                (2_500, Event::Idle),
                (2_600, Event::Report(&CONTACT, false)),
            ],
            vec![
                (0, monitors(&[1920])),
                (1_000, monitors(&[1920, 1920])),
                (2_000, monitors(&[])),
            ],
            &mut NoFilters,
        );
        // Pressed on one monitor, moved once a second monitor doubled the
        // desktop, released when monitor 0 disappeared; nothing after that.
        assert_eq!(packets.len(), 3, "{packets:?}");
        assert_eq!(packets[0].flags, ABSOLUTE | flags::LEFTDOWN);
        assert_eq!(packets[1].flags, ABSOLUTE);
        assert!(
            (packets[1].dx - packets[0].dx / 2).abs() <= 20,
            "{packets:?}"
        );
        assert_eq!(packets[2].flags, flags::LEFTUP);
    }

    #[test]
    fn a_fingerprint_change_while_reports_flow_rebuilds_the_mapping() {
        let (packets, _) = session(
            &profile(),
            |_| Mode::Driver,
            vec![
                (100, Event::Report(&HOVER, false)),
                (1_200, Event::Report(&CONTACT, false)),
                (1_205, Event::Report(&HOVER, false)),
            ],
            vec![(0, monitors(&[1920])), (1_000, monitors(&[1920, 1920]))],
            &mut NoFilters,
        );
        // The report at 1.2 s still uses the old mapping; the fingerprint
        // check after it switches to the doubled desktop for the next one.
        assert_eq!(packets.len(), 3, "{packets:?}");
        assert!(
            (packets[2].dx - packets[0].dx / 2).abs() <= 20,
            "{packets:?}"
        );
    }

    #[test]
    fn steady_state_reports_do_not_allocate() {
        /// Counts the loop's allocations between two report reads.
        struct Measured {
            inner: FakeSource,
            reads: usize,
            window: (usize, usize),
            count: Option<test_alloc::Count>,
            allocations: Option<usize>,
        }

        impl ReportSource for Measured {
            fn label(&self) -> &str {
                self.inner.label()
            }

            fn now(&self) -> Instant {
                self.inner.now()
            }

            fn next(&mut self, timeout: Duration) -> io::Result<Read<'_>> {
                self.reads += 1;
                if self.reads == self.window.0 {
                    self.count = Some(test_alloc::Count::start());
                } else if self.reads == self.window.1 {
                    self.allocations = self.count.take().map(test_alloc::Count::finish);
                }
                self.inner.next(timeout)
            }
        }

        // Two seconds of 1 kHz reports that press and release on every other
        // one; the window spans the once-a-second fingerprint check.
        let events = (0..2_000)
            .map(|i| {
                let report: &'static [u8] = if i % 2 == 0 { &HOVER } else { &CONTACT };
                (100 + i, Event::Report(report, false))
            })
            .collect();
        let clock = Rc::new(Cell::new(Instant::now()));
        let start = clock.get();
        let mut displays = FakeDisplays {
            clock: clock.clone(),
            start,
            schedule: vec![(0, monitors(&[1920]))],
        };
        let mut source = Measured {
            inner: FakeSource::new(clock, events),
            reads: 0,
            window: (10, 2_000),
            count: None,
            allocations: None,
        };
        let packets = Cell::new(0);
        run(
            &mut source,
            &mut displays,
            &profile(),
            Mode::Driver,
            &mut crate::decoders::TabletDecoder::pth_660(),
            &mut NoFilters,
            |_| {
                packets.set(packets.get() + 1);
                Ok(())
            },
            &|_: &str| {},
        )
        .unwrap();
        // Every report changes the contact; the last press is released.
        assert_eq!(packets.get(), 2_001);
        assert_eq!(source.allocations, Some(0));
    }

    #[test]
    fn capture_reads_without_sending_and_stops_at_its_limit() {
        let (packets, statuses) = session(
            &profile(),
            |start| Mode::Capture {
                deadline: start + Duration::from_secs(10),
                limit: 2,
            },
            vec![
                (100, Event::Report(&HOVER, false)),
                (105, Event::Report(&CONTACT, false)),
                (110, Event::Report(&CONTACT, false)),
            ],
            vec![(0, monitors(&[1920]))],
            &mut NoFilters,
        );
        assert!(packets.is_empty());
        // No report reached output, so there is no timing summary.
        assert_eq!(statuses, ["Tablet connected; receiving pen input"]);
    }

    #[test]
    fn capture_stops_at_its_deadline_while_idle() {
        let (packets, _) = session(
            &profile(),
            |start| Mode::Capture {
                deadline: start + Duration::from_millis(500),
                limit: 100,
            },
            vec![
                (100, Event::Idle),
                (600, Event::Idle),
                (700, Event::Report(&HOVER, false)),
            ],
            vec![(0, monitors(&[1920]))],
            &mut NoFilters,
        );
        assert!(packets.is_empty());
    }

    #[test]
    fn due_filter_timers_tick_between_reads() {
        struct Timed(u32);
        impl Filters for Timed {
            fn next_tick(&mut self) -> Option<Duration> {
                Some(if self.0 < 3 {
                    Duration::ZERO
                } else {
                    Duration::from_secs(5)
                })
            }
            fn tick(
                &mut self,
                _: Instant,
                runtime: &mut dyn crate::plugins::PipelineRuntime,
            ) -> io::Result<()> {
                self.0 += 1;
                let mut values = crate::reports::ReportValues {
                    position: Some([5000.0, 5000.0]),
                    pressure: Some(0),
                    ..Default::default()
                };
                if runtime.transform(crate::reports::ReportKind::Data, &mut values)? {
                    runtime.output(crate::reports::ReportKind::Data, &values, &[])?;
                }
                Ok(())
            }
            fn has_pre(&self) -> bool {
                false
            }
            fn process_pre(
                &mut self,
                position: (f32, f32),
                _: protocol::PenReport,
                _: Instant,
            ) -> (f32, f32) {
                position
            }
            fn has_pixels(&self) -> bool {
                false
            }
            fn process_pixels(
                &mut self,
                position: (f32, f32),
                _: protocol::PenReport,
                _: Instant,
            ) -> (f32, f32) {
                position
            }
            fn reset(&mut self) {}
            fn take_failure(&mut self) -> Option<&str> {
                None
            }
        }
        let mut timed = Timed(0);
        let (packets, _) = session(
            &profile(),
            |_| Mode::Driver,
            vec![(100, Event::Report(&HOVER, false))],
            vec![(0, monitors(&[1920]))],
            &mut timed,
        );
        // The source ends on its second read. Due timers cannot postpone that
        // shutdown, and the pre-input tick must not move a stale cursor.
        assert_eq!(timed.0, 2);
        // The physical report moves the cursor, then its between-read tick.
        assert_eq!(packets.len(), 2, "{packets:?}");
    }

    #[test]
    fn a_filter_failure_is_reported_once() {
        struct Failing(bool);
        impl Filters for Failing {
            fn has_pre(&self) -> bool {
                true
            }
            fn process_pre(
                &mut self,
                position: (f32, f32),
                _: protocol::PenReport,
                _: Instant,
            ) -> (f32, f32) {
                position
            }
            fn has_pixels(&self) -> bool {
                false
            }
            fn process_pixels(
                &mut self,
                position: (f32, f32),
                _: protocol::PenReport,
                _: Instant,
            ) -> (f32, f32) {
                position
            }
            fn reset(&mut self) {}
            fn take_failure(&mut self) -> Option<&str> {
                std::mem::take(&mut self.0).then_some("Example filter")
            }
        }
        let (_, statuses) = session(
            &profile(),
            |_| Mode::Driver,
            vec![
                (100, Event::Report(&HOVER, false)),
                (105, Event::Report(&HOVER, false)),
            ],
            vec![(0, monitors(&[1920]))],
            &mut Failing(true),
        );
        let failures = statuses
            .iter()
            .filter(|s| s.starts_with("Disabled failing plugin: Example filter"))
            .count();
        assert_eq!(failures, 1);
    }
}
