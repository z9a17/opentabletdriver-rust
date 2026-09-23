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
use windows_sys::Win32::System::Threading::{ResetEvent, WaitForMultipleObjects};

pub use otd_core::session::Mode;
use otd_core::session::{Read, ReportSource};

use crate::config::Profile;
use crate::display::WindowsDisplays;
use crate::hid::{self, Candidate, Event, Notification, OwnedHandle};
use crate::output::send_input;
use crate::plugins::PluginChain;
use crate::priority::ReaderPriority;

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
    buffer: Box<[u8; hid::PEN_REPORT_LENGTH as usize]>,
    operation: OVERLAPPED,
    pending: bool,
    label: String,
}

impl<'a> HidSource<'a> {
    fn open(
        candidate: &'a Candidate,
        notification: &'a Notification,
        stop: &'a Event,
    ) -> io::Result<Self> {
        let handle = candidate.open_read()?;
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
            buffer: Box::new([0; hid::PEN_REPORT_LENGTH as usize]),
            pending: false,
        })
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
    fn label(&self) -> &str {
        &self.label
    }

    fn now(&self) -> Instant {
        Instant::now()
    }

    fn next(&mut self, timeout: Duration) -> io::Result<Read<'_>> {
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
        let handles = [
            self.read_event.raw(),
            self.notification.event(),
            self.stop.raw(),
        ];
        loop {
            // Other devices' notifications do not extend the wait.
            let remaining = until.saturating_duration_since(Instant::now());
            let millis = u32::try_from(remaining.as_millis()).unwrap_or(u32::MAX);
            match wait(&handles, millis) {
                Ok(Some(0)) => {
                    self.pending = false;
                    return self.completed(false);
                }
                // Some HID device arrived or left; only this one matters.
                Ok(Some(1)) if self.candidate.is_present() => {}
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
    candidate: &Candidate,
    profile: &Profile,
    notification: &Notification,
    stop_event: &Event,
    mode: Mode,
    plugins: &mut PluginChain,
    status: &impl Fn(&str),
) -> io::Result<()> {
    plugins.reset();
    let mut source = HidSource::open(candidate, notification, stop_event)?;
    let _priority = ReaderPriority::raise();
    otd_core::session::run(
        &mut source,
        &mut WindowsDisplays,
        profile,
        mode,
        plugins,
        send_input,
        status,
    )
}

pub fn wait_for_retry(notification: &Notification, stop_event: &Event) -> io::Result<bool> {
    match wait(&[notification.event(), stop_event.raw()], 2_000)? {
        Some(1) => Ok(false),
        _ => Ok(true),
    }
}
