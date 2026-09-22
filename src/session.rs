use std::io;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{
    ERROR_IO_PENDING, ERROR_OPERATION_ABORTED, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::Storage::FileSystem::ReadFile;
use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows_sys::Win32::System::Threading::{ResetEvent, WaitForMultipleObjects};

use crate::config::Profile;
use crate::display::{DisplayFingerprint, DisplaySnapshot};
use crate::hid::{self, Candidate, Event, Notification};
use crate::mapping::Mapper;
use crate::output::send_input;
use crate::pipeline::ReportPipeline;
use crate::priority::ReaderPriority;
use crate::protocol;

#[derive(Clone, Copy)]
pub enum Mode {
    Driver,
    Capture { deadline: Instant, limit: u32 },
}

#[derive(Default)]
struct Counters {
    read: u64,
    accepted: u64,
    ignored: u64,
    malformed: u64,
    injected: u64,
    output_failures: u64,
}

/// Time from a completed read to the end of its output, in whole
/// microseconds, and how often a read found its report already queued, which
/// means the loop fell behind the tablet. Fixed-size; recording does not
/// allocate. The last bucket holds everything from 1 ms up.
struct Timing {
    micros: Box<[u32; Timing::BUCKETS]>,
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
            seen += u64::from(*count);
            if seen >= target {
                return if micros == Self::BUCKETS - 1 {
                    "1 ms or more".to_owned()
                } else {
                    format!("{micros} us")
                };
            }
        }
        unreachable!("the buckets hold every report")
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

fn wait(handles: &[HANDLE], timeout: u32) -> io::Result<Option<usize>> {
    let result =
        unsafe { WaitForMultipleObjects(handles.len() as u32, handles.as_ptr(), 0, timeout) };
    if result == WAIT_TIMEOUT {
        Ok(None)
    } else if result < WAIT_OBJECT_0 + handles.len() as u32 {
        Ok(Some((result - WAIT_OBJECT_0) as usize))
    } else {
        Err(io::Error::last_os_error())
    }
}

fn complete_cancelled_read(handle: HANDLE, operation: &mut OVERLAPPED) {
    unsafe { CancelIoEx(handle, operation) };
    let mut ignored = 0;
    // Cancellation is only a request; wait before reusing the buffer/operation.
    unsafe { GetOverlappedResult(handle, operation, &mut ignored, 1) };
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

fn refresh_display(
    snapshot: &mut Option<DisplaySnapshot>,
    fingerprint: &mut DisplayFingerprint,
    mapper: &mut Option<Mapper>,
    profile: &Profile,
    pipeline: &mut ReportPipeline,
) {
    let Some(snapshot) = snapshot else {
        // Relative mouse motion is independent of the monitor topology.
        return;
    };
    *fingerprint = DisplayFingerprint::read();
    let Ok(current) = DisplaySnapshot::read() else {
        return;
    };
    if current == *snapshot {
        return;
    }
    match current.mapper(profile) {
        Ok(new_mapper) => {
            *mapper = Some(new_mapper);
            eprintln!("display mapping refreshed");
        }
        Err(error) => {
            *mapper = None;
            if let Err(release_error) = pipeline.release_all(send_input) {
                eprintln!("could not release buttons after display change: {release_error}");
            }
            eprintln!("display mapping paused: {error}");
        }
    }
    *snapshot = current;
}

pub fn run(
    candidate: &Candidate,
    profile: &Profile,
    notification: &Notification,
    stop_event: &Event,
    mode: Mode,
    plugins: &mut crate::plugins::PluginChain,
    status: &impl Fn(&str),
) -> io::Result<()> {
    plugins.reset();
    let handle = candidate.open_read()?;
    let _priority = ReaderPriority::raise();
    let read_event = Event::create(true)?;
    let mut buffer = Box::new([0u8; hid::PEN_REPORT_LENGTH as usize]);
    let mut operation = OVERLAPPED {
        hEvent: read_event.raw(),
        ..Default::default()
    };
    let mut pipeline = ReportPipeline::new(profile).map_err(io::Error::other)?;
    let mut snapshot = if !pipeline.is_relative() {
        Some(DisplaySnapshot::read().map_err(io::Error::other)?)
    } else {
        None
    };
    let mut mapper = snapshot
        .as_ref()
        .map(|s| s.mapper(profile))
        .transpose()
        .map_err(io::Error::other)?;
    let mut fingerprint = DisplayFingerprint::read();
    let mut next_refresh = Instant::now() + Duration::from_secs(1);
    let mut last_output_warning = Instant::now() - Duration::from_secs(10);
    let mut counters = Counters::default();
    let mut timing = Timing::new();
    let mut capture_trace = CaptureTrace::default();
    let mut finished = false;

    eprintln!(
        "PTH-660 connected: HID input length {}, usage {:04x}:{:04x}",
        candidate.input_length, candidate.usage_page, candidate.usage
    );
    status("PTH-660 connected; receiving pen input");
    let outcome = (|| -> io::Result<()> {
        while !finished {
            if let Mode::Capture { deadline, limit } = mode
                && (Instant::now() >= deadline || counters.read >= u64::from(limit))
            {
                break;
            }
            if unsafe { ResetEvent(read_event.raw()) } == 0 {
                return Err(io::Error::last_os_error());
            }
            operation = OVERLAPPED {
                hEvent: read_event.raw(),
                ..Default::default()
            };
            let started = unsafe {
                ReadFile(
                    handle.raw(),
                    buffer.as_mut_ptr(),
                    buffer.len() as u32,
                    std::ptr::null_mut(),
                    &mut operation,
                )
            };
            if started == 0
                && io::Error::last_os_error().raw_os_error() != Some(ERROR_IO_PENDING as i32)
            {
                return Err(io::Error::last_os_error());
            }
            if started == 0 {
                loop {
                    let timeout = if let Mode::Capture { deadline, .. } = mode {
                        deadline
                            .saturating_duration_since(Instant::now())
                            .as_millis()
                            .min(1_000) as u32
                    } else {
                        1_000
                    };
                    let event = match wait(
                        &[read_event.raw(), notification.event(), stop_event.raw()],
                        timeout,
                    ) {
                        Ok(event) => event,
                        Err(error) => {
                            complete_cancelled_read(handle.raw(), &mut operation);
                            return Err(error);
                        }
                    };
                    match event {
                        Some(0) => break,
                        Some(1) => {
                            if !candidate.is_present() {
                                complete_cancelled_read(handle.raw(), &mut operation);
                                finished = true;
                                break;
                            }
                        }
                        Some(2) => {
                            complete_cancelled_read(handle.raw(), &mut operation);
                            finished = true;
                            break;
                        }
                        None => {
                            if let Mode::Capture { deadline, .. } = mode
                                && Instant::now() >= deadline
                            {
                                complete_cancelled_read(handle.raw(), &mut operation);
                                finished = true;
                                break;
                            }
                            // Idle: compare the full monitor layout.
                            if Instant::now() >= next_refresh {
                                refresh_display(
                                    &mut snapshot,
                                    &mut fingerprint,
                                    &mut mapper,
                                    profile,
                                    &mut pipeline,
                                );
                                next_refresh = Instant::now() + Duration::from_secs(1);
                            }
                        }
                        _ => unreachable!(),
                    }
                }
            }
            if finished {
                break;
            }
            let ready = Instant::now();
            if started != 0 {
                timing.queued += 1;
            }
            let mut transferred = 0u32;
            if unsafe { GetOverlappedResult(handle.raw(), &operation, &mut transferred, 0) } == 0 {
                let error = io::Error::last_os_error();
                if error.raw_os_error() == Some(ERROR_OPERATION_ABORTED as i32) {
                    break;
                }
                return Err(error);
            }
            if transferred as usize > buffer.len() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "HID read exceeded its advertised input report length",
                ));
            }
            counters.read += 1;
            let bytes = &buffer[..transferred as usize];
            match protocol::parse(bytes) {
                Ok(Some(pen)) => {
                    counters.accepted += 1;
                    if let Mode::Capture { .. } = mode {
                        capture_trace.pen(bytes, pen);
                    } else if pipeline.is_relative() || mapper.is_some() {
                        // While an absolute mapping is paused, reports do not
                        // reach the filters either.
                        let emitted = pipeline.process(pen, ready, mapper, plugins, send_input);
                        if let Some(name) = plugins.take_failure() {
                            status(&format!(
                                "Disabled failing plugin: {name}. Restart to retry."
                            ));
                        }
                        match emitted {
                            Ok(true) => counters.injected += 1,
                            Ok(false) => {}
                            Err(error) => {
                                counters.output_failures += 1;
                                if last_output_warning.elapsed() >= Duration::from_secs(5) {
                                    eprintln!("SendInput failed: {error}");
                                    last_output_warning = Instant::now();
                                }
                            }
                        }
                        timing.record(ready.elapsed());
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
            if Instant::now() >= next_refresh {
                if snapshot.is_some() && DisplayFingerprint::read() != fingerprint {
                    refresh_display(
                        &mut snapshot,
                        &mut fingerprint,
                        &mut mapper,
                        profile,
                        &mut pipeline,
                    );
                }
                next_refresh = Instant::now() + Duration::from_secs(1);
            }
        }
        Ok(())
    })();
    if let Err(error) = pipeline.release_all(send_input) {
        eprintln!("could not release mouse buttons: {error}");
    }
    eprintln!(
        "session ended: read={} accepted={} ignored={} malformed={} injected={} output_failures={}",
        counters.read,
        counters.accepted,
        counters.ignored,
        counters.malformed,
        counters.injected,
        counters.output_failures
    );
    if let Some(summary) = timing.summary() {
        eprintln!("{summary}");
        status(&summary);
    }
    outcome
}

pub fn wait_for_retry(notification: &Notification, stop_event: &Event) -> io::Result<bool> {
    match wait(&[notification.event(), stop_event.raw()], 2_000)? {
        Some(1) => Ok(false),
        _ => Ok(true),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
