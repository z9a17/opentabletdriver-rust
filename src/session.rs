use std::io;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{
    ERROR_IO_PENDING, ERROR_OPERATION_ABORTED, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::Storage::FileSystem::ReadFile;
use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows_sys::Win32::System::Threading::{ResetEvent, WaitForMultipleObjects};

use crate::config::Profile;
use crate::display::DisplaySnapshot;
use crate::hid::{self, Candidate, Event, Notification};
use crate::mapping::Mapper;
use crate::output::MouseOutput;
use crate::protocol;
use crate::state;

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

#[derive(Default)]
struct CaptureTrace {
    initial: u8,
    ignored: u8,
    last_state: Option<(bool, bool, bool)>,
}

impl CaptureTrace {
    fn pen(&mut self, bytes: &[u8], pen: protocol::PenReport) {
        let state = (pen.proximity, pen.tip_switch, pen.eraser);
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

fn same_device_present(candidate: &Candidate) -> bool {
    hid::enumerate().is_ok_and(|devices| {
        devices
            .iter()
            .any(|d| d.path == candidate.path && d.is_pen())
    })
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
            "id={:02x} x={} y={} pressure={} proximity={} tip={} eraser={} tilt={:?} bytes={}",
            pen.id,
            pen.x,
            pen.y,
            pen.pressure,
            pen.proximity,
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
    snapshot: &mut DisplaySnapshot,
    mapper: &mut Option<Mapper>,
    profile: &Profile,
    output: &mut MouseOutput,
) {
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
            if let Err(release_error) = output.release_all() {
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
) -> io::Result<()> {
    let handle = candidate.open_read()?;
    let read_event = Event::create(true)?;
    let mut buffer = Box::new([0u8; hid::PEN_REPORT_LENGTH as usize]);
    let mut operation = OVERLAPPED {
        hEvent: read_event.raw(),
        ..Default::default()
    };
    let mut snapshot = DisplaySnapshot::read().map_err(io::Error::other)?;
    let mut mapper = Some(snapshot.mapper(profile).map_err(io::Error::other)?);
    let mut output = MouseOutput::new();
    let mut next_refresh = Instant::now() + Duration::from_secs(1);
    let mut last_output_warning = Instant::now() - Duration::from_secs(10);
    let mut counters = Counters::default();
    let mut capture_trace = CaptureTrace::default();
    let mut finished = false;

    eprintln!(
        "PTH-660 connected: HID input length {}, usage {:04x}:{:04x}",
        candidate.input_length, candidate.usage_page, candidate.usage
    );
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
                            if !same_device_present(candidate) {
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
                            if Instant::now() >= next_refresh {
                                refresh_display(&mut snapshot, &mut mapper, profile, &mut output);
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
                    } else if let Some(active_mapper) = mapper {
                        match output.emit(state::frame(pen, profile.contact), active_mapper) {
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
            if Instant::now() >= next_refresh {
                refresh_display(&mut snapshot, &mut mapper, profile, &mut output);
                next_refresh = Instant::now() + Duration::from_secs(1);
            }
        }
        Ok(())
    })();
    if let Err(error) = output.release_all() {
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
    outcome
}

pub fn wait_for_retry(notification: &Notification, stop_event: &Event) -> io::Result<bool> {
    match wait(&[notification.event(), stop_event.raw()], 2_000)? {
        Some(1) => Ok(false),
        _ => Ok(true),
    }
}
