//! Windows device sessions: an overlapped HID reader that feeds the core's
//! session loop at reader priority, ending on device removal or a stop event.

use std::io;
use std::ptr;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{
    ERROR_IO_PENDING, ERROR_OPERATION_ABORTED, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::Storage::FileSystem::ReadFile;
use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows_sys::Win32::System::Threading::{
    CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, CreateWaitableTimerExW, INFINITE, ResetEvent,
    SetWaitableTimer, TIMER_ALL_ACCESS, WaitForMultipleObjects, WaitForSingleObject,
};

/// Waits shorter than this use a high-resolution waitable timer: filter timer
/// ticks need sub-millisecond deadlines, which millisecond waits truncate.
const PRECISE_WAIT: Duration = Duration::from_millis(50);

pub use otd_core::session::Mode;
use otd_core::session::{Read, ReportSource};

use crate::config::{OutputKind, Profile};
use crate::display::WindowsDisplays;
use crate::hid::{self, Candidate, Event, Notification, OwnedHandle, SelectedDevice};
use crate::output::SessionOutput;
use crate::plugins::PluginChain;
use crate::priority::ReaderPriority;
use otd_core::output::pen::PenSink;

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

/// Reads the pen collection with one overlapped read at a time. A read left
/// pending by an idle timeout stays pending for the next call. Dropping the
/// source cancels a pending read and waits for it, so the buffer is never
/// freed while the read can still write to it.
struct HidSource<'a> {
    candidate: &'a Candidate,
    notification: &'a Notification,
    stop: &'a Event,
    handle: OwnedHandle,
    read_event: Event,
    buffer: Box<[u8]>,
    operation: OVERLAPPED,
    pending: bool,
    label: String,
    /// Created on the first short wait; sessions without timer-driven
    /// filters never wait less than a second and never create it.
    timer: Option<OwnedHandle>,
}

impl<'a> HidSource<'a> {
    fn open(
        selected: &'a SelectedDevice<'a>,
        notification: &'a Notification,
        stop: &'a Event,
        driver_access: bool,
    ) -> io::Result<Self> {
        let candidate = selected.pen;
        let writes = driver_access
            && (selected
                .identifier
                .feature_init_report
                .as_ref()
                .is_some_and(|reports| reports.iter().any(|report| !report.0.is_empty()))
                || selected
                    .identifier
                    .output_init_report
                    .as_ref()
                    .is_some_and(|reports| reports.iter().any(|report| !report.0.is_empty())));
        let handle = if writes {
            candidate.open(true)?
        } else {
            candidate.open_read()?
        };
        let read_event = Event::create(true)?;
        Ok(Self {
            label: format!(
                "HID input length {}, usage {:04x}:{:04x}",
                candidate.input_length, candidate.usage_page, candidate.usage
            ),
            operation: OVERLAPPED {
                hEvent: read_event.raw(),
                ..Default::default()
            },
            candidate,
            notification,
            stop,
            handle,
            read_event,
            // ReadFile needs room for the collection's whole input report.
            buffer: vec![0; usize::from(candidate.input_length.max(1))].into_boxed_slice(),
            pending: false,
            timer: None,
        })
    }

    /// Arms the high-resolution timer to signal after `wait`.
    fn arm_timer(&mut self, wait: Duration) -> io::Result<HANDLE> {
        if self.timer.is_none() {
            self.timer = Some(OwnedHandle::new(unsafe {
                CreateWaitableTimerExW(
                    ptr::null(),
                    ptr::null(),
                    CREATE_WAITABLE_TIMER_HIGH_RESOLUTION,
                    TIMER_ALL_ACCESS,
                )
            })?);
        }
        let timer = self
            .timer
            .as_ref()
            .map(OwnedHandle::raw)
            .unwrap_or_default();
        // Relative due time in 100 ns units, rounded up so it never fires early.
        let due = -i64::try_from(wait.as_nanos().div_ceil(100).max(1)).unwrap_or(i64::MAX);
        if unsafe { SetWaitableTimer(timer, &due, 0, None, ptr::null(), 0) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(timer)
    }

    fn cancel(&mut self) {
        if !self.pending {
            return;
        }
        unsafe { CancelIoEx(self.handle.raw(), &self.operation) };
        let mut ignored = 0;
        // Cancellation is only a request; wait before reusing the buffer.
        unsafe { GetOverlappedResult(self.handle.raw(), &self.operation, &mut ignored, 1) };
        self.pending = false;
    }

    fn stopped(&self) -> io::Result<bool> {
        match unsafe { WaitForSingleObject(self.stop.raw(), 0) } {
            WAIT_OBJECT_0 => Ok(true),
            WAIT_TIMEOUT => Ok(false),
            _ => Err(io::Error::last_os_error()),
        }
    }

    fn completed(&self, queued: bool) -> io::Result<Read<'_>> {
        let ready = Instant::now();
        let mut transferred = 0u32;
        if unsafe { GetOverlappedResult(self.handle.raw(), &self.operation, &mut transferred, 0) }
            == 0
        {
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(ERROR_OPERATION_ABORTED as i32) {
                return Ok(Read::Ended);
            }
            return Err(error);
        }
        // A synchronous ReadFile completion never enters the multi-event wait.
        // Recheck before exposing it so a queued stream cannot starve Stop.
        if self.stopped()? {
            return Ok(Read::Ended);
        }
        if transferred as usize > self.buffer.len() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "HID read exceeded its advertised input report length",
            ));
        }
        Ok(Read::Report {
            bytes: &self.buffer[..transferred as usize],
            ready,
            queued,
        })
    }
}

impl ReportSource for HidSource<'_> {
    fn shared_output(&self) -> bool {
        true
    }

    fn label(&self) -> &str {
        &self.label
    }

    fn now(&self) -> Instant {
        Instant::now()
    }

    fn next(&mut self, timeout: Duration) -> io::Result<Read<'_>> {
        if self.stopped()? {
            self.cancel();
            return Ok(Read::Ended);
        }
        if !self.pending {
            if unsafe { ResetEvent(self.read_event.raw()) } == 0 {
                return Err(io::Error::last_os_error());
            }
            self.operation = OVERLAPPED {
                hEvent: self.read_event.raw(),
                ..Default::default()
            };
            let started = unsafe {
                ReadFile(
                    self.handle.raw(),
                    self.buffer.as_mut_ptr(),
                    self.buffer.len() as u32,
                    ptr::null_mut(),
                    &mut self.operation,
                )
            };
            if started != 0 {
                // The report was already waiting in the HID class driver.
                return self.completed(true);
            }
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(ERROR_IO_PENDING as i32) {
                return Err(error);
            }
            self.pending = true;
        }
        let until = Instant::now() + timeout;
        // An already-due deadline is an input/stop poll, not a new timer wakeup.
        let precise = !timeout.is_zero() && timeout < PRECISE_WAIT;
        let timer = if precise {
            self.arm_timer(timeout)?
        } else {
            ptr::null_mut()
        };
        let handles = [
            self.stop.raw(),
            self.read_event.raw(),
            self.notification.event(),
            timer,
        ];
        loop {
            // Other devices' notifications do not extend the wait.
            let result = if precise {
                wait(&handles, INFINITE)
            } else {
                let remaining = until.saturating_duration_since(Instant::now());
                let millis = u32::try_from(remaining.as_millis()).unwrap_or(u32::MAX);
                wait(&handles[..3], millis)
            };
            match result {
                // Stop takes precedence when both stop and read are signaled.
                Ok(Some(1)) => {
                    self.pending = false;
                    return self.completed(false);
                }
                // Some HID device arrived or left; only this one matters.
                Ok(Some(2)) if self.candidate.is_present() => {}
                Ok(Some(3)) => return Ok(Read::Idle),
                Ok(Some(_)) => {
                    self.cancel();
                    return Ok(Read::Ended);
                }
                Ok(None) => return Ok(Read::Idle),
                Err(error) => {
                    self.cancel();
                    return Err(error);
                }
            }
        }
    }
}

impl Drop for HidSource<'_> {
    fn drop(&mut self) {
        self.cancel();
    }
}

pub fn run(
    selected: &SelectedDevice<'_>,
    profile: &Profile,
    notification: &Notification,
    stop_event: &Event,
    mode: Mode,
    plugins: &mut PluginChain,
    status: &impl Fn(&str),
) -> io::Result<()> {
    plugins.reset();
    let mut source = HidSource::open(
        selected,
        notification,
        stop_event,
        matches!(mode, Mode::Driver),
    )?;
    if matches!(mode, Mode::Driver) {
        hid::initialize(
            selected.pen,
            &source.handle,
            &selected.identifier,
            &selected.configuration,
            stop_event,
        )?;
    }
    let profile = profile
        .for_tablet(selected.spec)
        .map_err(io::Error::other)?;
    let mut decoder = selected.decoder()?;
    let _debug = DebugDevice::set(selected);
    let _priority = ReaderPriority::raise();
    let mut output = if matches!(mode, Mode::Driver) {
        Some(SessionOutput::new()?)
    } else {
        None
    };
    let pen = pen_device(&profile, mode)?;
    let result = otd_core::session::run_gated_with_devices(
        &mut source,
        &mut WindowsDisplays,
        &profile,
        mode,
        &mut decoder,
        plugins,
        |packet| output.as_ref().map_or(Ok(()), |output| output.send(packet)),
        pen,
        action_sink(mode)?,
        status,
        || Ok(true),
    );
    if let Some(output) = &mut output {
        output
            .finish()
            .map_err(otd_core::session::cleanup_failure)?;
    }
    result
}

/// Key and button output for a driving session's pen side buttons.
fn action_sink(mode: Mode) -> io::Result<Option<Box<dyn otd_core::output::buttons::ActionSink>>> {
    if matches!(mode, Mode::Driver) {
        Ok(Some(Box::new(crate::action_output::SessionActions::new()?)))
    } else {
        Ok(None)
    }
}

/// The synthetic pen for a driving session whose profile has pen output.
fn pen_device(profile: &Profile, mode: Mode) -> io::Result<Option<Box<dyn PenSink>>> {
    if profile.output != OutputKind::Pen || !matches!(mode, Mode::Driver) {
        return Ok(None);
    }
    Ok(Some(Box::new(crate::pen_output::SyntheticPen::new()?)))
}

/// Names the session's tablet for the tablet debugger while it runs.
struct DebugDevice {
    _registration: otd_core::debug::Registration,
}
impl DebugDevice {
    fn set(selected: &SelectedDevice<'_>) -> Self {
        Self {
            _registration: otd_core::debug::Registration::new(otd_core::debug::Device {
                name: selected.configuration.name.clone(),
                parser: selected.identifier.parser().to_owned(),
            }),
        }
    }
}
/// A candidate owns its handle, event and read buffer before the old worker
/// pauses. Preparation never initializes the tablet or issues a read.
pub(crate) struct PreparedSession<'a> {
    source: HidSource<'a>,
    selected: &'a SelectedDevice<'a>,
}

impl<'a> PreparedSession<'a> {
    pub fn new(
        selected: &'a SelectedDevice<'a>,
        notification: &'a Notification,
        interrupt: &'a Event,
    ) -> io::Result<Self> {
        // Validate deterministic initialization failures before quiescing the
        // current worker. Actual device writes belong to activation only.
        if let Some(delay) = selected
            .configuration
            .attributes
            .as_ref()
            .and_then(|attributes| attributes.get("FeatureInitDelayMs"))
        {
            let delay = delay
                .parse::<u32>()
                .map_err(|_| io::Error::other("invalid FeatureInitDelayMs"))?;
            if delay == u32::MAX {
                return Err(io::Error::other(
                    "infinite feature initialization delay is unsupported",
                ));
            }
        }
        for (reports, length) in [
            (
                &selected.identifier.feature_init_report,
                selected.pen.endpoint.feature_length,
            ),
            (
                &selected.identifier.output_init_report,
                selected.pen.endpoint.output_length,
            ),
        ] {
            for report in reports
                .iter()
                .flatten()
                .filter(|report| !report.0.is_empty())
            {
                if length == 0 || length > u16::MAX as u32 || report.0.len() > length as usize {
                    return Err(io::Error::other(
                        "initialization report exceeds endpoint report length",
                    ));
                }
            }
        }
        Ok(Self {
            source: HidSource::open(selected, notification, interrupt, true)?,
            selected,
        })
    }

    pub fn activate(&mut self) -> io::Result<()> {
        if !self.selected.pen.is_present() {
            return Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "prepared tablet endpoint disappeared before activation",
            ));
        }
        hid::initialize(
            self.selected.pen,
            &self.source.handle,
            &self.selected.identifier,
            &self.selected.configuration,
            self.source.stop,
        )?;
        // The prepared handle may have queued input while the previous worker
        // was still reading. Flush only after that worker has quiesced, so the
        // replacement cannot replay its already-processed pen/button reports.
        // https://learn.microsoft.com/windows-hardware/drivers/ddi/hidsdi/nf-hidsdi-hidd_flushqueue
        if !unsafe {
            windows_sys::Win32::Devices::HumanInterfaceDevice::HidD_FlushQueue(
                self.source.handle.raw(),
            )
        } {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    /// Activation runs after portable mapping/pipeline setup. Neither reports
    /// nor timer callbacks reach plugins/output before gate success.
    pub fn run(
        self,
        profile: &Profile,
        plugins: &mut PluginChain,
        status: &impl Fn(&str),
        gate: impl FnOnce() -> io::Result<bool>,
    ) -> io::Result<()> {
        let profile = profile
            .for_tablet(self.selected.spec)
            .map_err(io::Error::other)?;
        let mut decoder = self.selected.decoder()?;
        let _debug = DebugDevice::set(self.selected);
        let mut source = self.source;
        let _priority = ReaderPriority::raise();
        let mut output = SessionOutput::new()?;
        let pen = pen_device(&profile, Mode::Driver)?;
        let result = otd_core::session::run_gated_with_devices(
            &mut source,
            &mut WindowsDisplays,
            &profile,
            Mode::Driver,
            &mut decoder,
            plugins,
            |packet| output.send(packet),
            pen,
            action_sink(Mode::Driver)?,
            status,
            gate,
        );
        output
            .finish()
            .map_err(otd_core::session::cleanup_failure)?;
        result
    }
}

pub fn wait_for_retry(notification: &Notification, stop_event: &Event) -> io::Result<bool> {
    match wait(&[notification.event(), stop_event.raw()], 2_000)? {
        Some(1) => Ok(false),
        _ => Ok(true),
    }
}

/// True only for a device notification. Stop wakes immediately; the owner
/// checks its stop flag before its next pass. Timeouts only reap finished jobs.
pub fn companion_wake(notification: &Notification, stop_event: &Event) -> io::Result<bool> {
    Ok(wait(&[stop_event.raw(), notification.event()], 2_000)? == Some(1))
}
