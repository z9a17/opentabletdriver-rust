//! Windows device sessions: overlapped HID readers for the pen collection and
//! the tablet's auxiliary collection (express keys, wheels), which feed the
//! core's session loop at reader priority, ending on device removal or a stop
//! event.

use std::io;
use std::ptr;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{
    ERROR_IO_INCOMPLETE, ERROR_IO_PENDING, ERROR_OPERATION_ABORTED, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::Storage::FileSystem::ReadFile;
use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows_sys::Win32::System::Threading::{
    CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, CreateWaitableTimerExW, INFINITE, SetWaitableTimer,
    TIMER_ALL_ACCESS, WaitForMultipleObjects, WaitForSingleObject, ResetEvent,
};

/// Waits shorter than this use a high-resolution waitable timer: filter timer
/// ticks need sub-millisecond deadlines, which millisecond waits truncate.
const PRECISE_WAIT: Duration = Duration::from_millis(50);

use otd_core::decoders::PenDecoder;
use crate::dotnet::RuntimeDecoder;
pub use otd_core::session::Mode;
use otd_core::session::{Read, ReportSource};
use otd_core::tablets::DeviceIdentifier;

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

/// Whether initializing this endpoint writes feature or output reports, and
/// so needs a handle with write access.
fn initialization_writes(identifier: &DeviceIdentifier) -> bool {
    [
        &identifier.feature_init_report,
        &identifier.output_init_report,
    ]
    .into_iter()
    .any(|reports| {
        reports
            .as_ref()
            .is_some_and(|reports| reports.iter().any(|report| !report.0.is_empty()))
    })
}

/// One overlapped read at a time on one HID collection. A read left pending
/// by an idle timeout stays pending for the next call. Dropping the reader
/// cancels a pending read and waits for it, so the buffer is never freed
/// while the read can still write to it.
struct Reader {
    shared: crate::shared_devices::Registration,
    custom:Option<crate::custom_devices::Reader>,
    custom_length:Option<usize>,
    custom_ready:Option<Instant>,
    event: Event,
    buffer: Box<[u8]>,
    // Windows retains this address until completion, including across moves
    // of the reader/source between calls.
    operation: Box<OVERLAPPED>,
    state: ReadState,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum ReadState {
    #[default]
    Idle,
    Pending,
    Ready,
}

impl ReadState {
    fn completed(&mut self, result: &io::Result<()>) {
        if !matches!(result, Err(error) if error.raw_os_error() == Some(ERROR_IO_INCOMPLETE as i32)) {
            *self = Self::Idle;
        }
    }
}

impl Reader {
    fn open(
        candidate: &Candidate,
        identifier: &DeviceIdentifier,
        driver_access: bool,
        configuration: &otd_core::tablets::TabletConfiguration,
        output: Arc<crate::shared_devices::OutputGate>,
        auxiliary: bool,
        epoch: Option<u64>,
    ) -> io::Result<Self> {
        let event = Event::create(true)?;
        let custom=if let Some(endpoint)=candidate.managed_endpoint {
            let wake=Arc::clone(&output);
            Some(crate::custom_devices::Reader::open(endpoint,usize::from(candidate.input_length),Arc::new(move||{let _=wake.signal();}))?)
        }else{None};
        let io=if let Some(reader)=&custom {
            crate::shared_devices::SharedIo::new_custom(reader.handle(),candidate.endpoint.output_length,candidate.endpoint.feature_length)?
        }else{
        let handle = if driver_access {
            match candidate.open(true) {
                Ok(handle) => handle,
                Err(error) if initialization_writes(identifier) => return Err(error),
                Err(_) => candidate.open_read()?,
            }
        } else {
            candidate.open_read()?
        };
        let winusb = if candidate.endpoint.transport == otd_core::endpoint_match::Transport::WinUsb {
            let interface = crate::winusb::Interface::open(&handle)?;
            if interface.input_length() != candidate.input_length || interface.input_length() == 0 {
                return Err(io::Error::new(io::ErrorKind::InvalidData,
                    "WinUSB input pipe changed since device discovery"));
            }
            Some(interface)
        } else { None };
        crate::shared_devices::SharedIo::new(handle, winusb, candidate.endpoint.output_length,
            candidate.endpoint.feature_length)?
        };
        let shared = crate::shared_devices::Registration::new(candidate.path_text(), identifier.parser().to_owned(),
            auxiliary, Arc::new(io), output, usize::from(candidate.input_length.max(1)),
            serde_json::json!({"Properties":configuration,"Identifiers":[identifier]}),
            serde_json::to_value(identifier).map_err(io::Error::other)?,
            serde_json::to_value(configuration).map_err(io::Error::other)?, epoch).map_err(io::Error::other)?;
        Ok(Self {
            operation: Box::new(OVERLAPPED {
                hEvent: event.raw(),
                ..Default::default()
            }),
            shared,
            custom,custom_length:None,custom_ready:None,
            event,
            // ReadFile needs room for the collection's whole input report.
            buffer: vec![0; usize::from(candidate.input_length.max(1))].into_boxed_slice(),
            state: ReadState::Idle,
        })
    }

    /// Starts a read unless one is already outstanding. `true` means the
    /// complete report is retained in our buffer until `finish` consumes it.
    fn start(&mut self) -> io::Result<bool> {
        match self.state {
            ReadState::Ready => return Ok(true),
            ReadState::Pending => return Ok(false),
            ReadState::Idle => {}
        }
        if let Some(custom)=&mut self.custom {
            let report=custom.poll_report(&mut self.buffer)?;
            self.custom_length=report.map(|(length,_)|length);
            self.custom_ready=report.map(|(_,ready)|ready);
            if self.custom_length.is_some() { self.state = ReadState::Ready; }
            return Ok(self.state == ReadState::Ready);
        }
        // ReadFile resets the event when it starts the read.
        // https://learn.microsoft.com/windows/win32/api/fileapi/nf-fileapi-readfile
        *self.operation = OVERLAPPED {
            hEvent: self.event.raw(),
            ..Default::default()
        };
        if let Some(winusb) = &self.shared.endpoint.io.winusb {
            if unsafe { ResetEvent(self.event.raw()) } == 0 { return Err(io::Error::last_os_error()); }
            let immediate = unsafe { winusb.read(&mut self.buffer, &self.operation) }?;
            self.state = if immediate { ReadState::Ready } else { ReadState::Pending };
            return Ok(immediate);
        }
        let started = unsafe {
            ReadFile(
                self.shared.endpoint.io.native_handle()?.raw(),
                self.buffer.as_mut_ptr(),
                self.buffer.len() as u32,
                ptr::null_mut(),
                &mut *self.operation,
            )
        };
        if started != 0 {
            self.state = ReadState::Ready;
            return Ok(true);
        }
        let error = io::Error::last_os_error();
        if error.raw_os_error() != Some(ERROR_IO_PENDING as i32) {
            return Err(error);
        }
        self.state = ReadState::Pending;
        Ok(false)
    }

    /// The length of a completed read, or `None` if it was cancelled.
    fn finish(&mut self) -> io::Result<Option<usize>> {
        if self.custom.is_some() {
            self.state = ReadState::Idle;
            return Ok(self.custom_length.take());
        }
        if self.state == ReadState::Idle {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "No outstanding input read"));
        }
        let mut transferred = 0u32;
        let completion = if let Some(winusb) = &self.shared.endpoint.io.winusb {
            winusb.completed(&self.operation, false).map(|length| { transferred = length; })
        } else if unsafe { GetOverlappedResult(self.shared.endpoint.io.native_handle()?.raw(), &*self.operation, &mut transferred, 0) } == 0 {
            Err(io::Error::last_os_error())
        } else { Ok(()) };
        // An unsuccessful readiness probe must not relinquish ownership
        // of storage which the kernel can still write into.
        self.state.completed(&completion);
        if let Err(error) = completion {
            if error.raw_os_error() == Some(ERROR_OPERATION_ABORTED as i32) {
                return Ok(None);
            }
            return Err(error);
        }
        if transferred as usize > self.buffer.len() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "HID read exceeded its advertised input report length",
            ));
        }
        Ok(Some(transferred as usize))
    }

    fn cancel(&mut self) {
        if self.state == ReadState::Idle {
            return;
        }
        if self.custom.is_some() {
            self.custom_length = None;
            self.custom_ready = None;
            self.state = ReadState::Idle;
            return;
        }
        if let Some(winusb) = &self.shared.endpoint.io.winusb {
            winusb.cancel(&self.operation);
            self.state = ReadState::Idle;
            return;
        }
        let Ok(handle)=self.shared.endpoint.io.native_handle()else{return;};
        unsafe { CancelIoEx(handle.raw(), &*self.operation) };
        let mut ignored = 0;
        // Cancellation is only a request; wait before reusing the buffer.
        unsafe { GetOverlappedResult(handle.raw(), &*self.operation, &mut ignored, 1) };
        self.state = ReadState::Idle;
    }

    fn initialize(&self, candidate: &Candidate, identifier: &DeviceIdentifier,
        configuration: &otd_core::tablets::TabletConfiguration, stop: &Event) -> io::Result<()> {
        let result = if self.custom.is_some() {self.initialize_custom(identifier,configuration,stop)}
        else if let Some(winusb) = &self.shared.endpoint.io.winusb { winusb.initialize(candidate, identifier, configuration, stop) }
        else { hid::initialize(candidate, self.shared.endpoint.io.native_handle()?, identifier, configuration, stop) };
        if result.is_ok() { self.shared.endpoint.initialized.store(true, Ordering::Release); }
        result
    }
    fn initialize_custom(&self,identifier:&DeviceIdentifier,configuration:&otd_core::tablets::TabletConfiguration,stop:&Event)->io::Result<()> {
        let delay=configuration.attributes.as_ref().and_then(|attributes|attributes.get("FeatureInitDelayMs"))
            .map(|text|text.parse::<u32>().map_err(io::Error::other)).transpose()?.unwrap_or(0);
        if delay==u32::MAX {return Err(io::Error::other("Infinite custom initialization delay is unsupported"));}
        let cancelled=||if unsafe{WaitForSingleObject(stop.raw(),0)}==WAIT_OBJECT_0 {Err(io::Error::new(io::ErrorKind::Interrupted,"Custom initialization cancelled"))}else{Ok(())};
        for index in identifier.initialization_strings.iter().flatten() {cancelled()?;self.shared.endpoint.io.string(*index)?;}
        for report in identifier.feature_init_report.iter().flatten().filter(|report|!report.0.is_empty()) {
            cancelled()?;
            match unsafe{WaitForSingleObject(stop.raw(),delay)} {WAIT_TIMEOUT=>{},WAIT_OBJECT_0=>return Err(io::Error::new(io::ErrorKind::Interrupted,"Custom initialization cancelled")),_=>return Err(io::Error::last_os_error())}
            self.shared.endpoint.io.service(crate::managed_services::Operation::SetFeature,&mut report.0.clone())?;
        }
        for report in identifier.output_init_report.iter().flatten().filter(|report|!report.0.is_empty()) {cancelled()?;
            self.shared.endpoint.io.service(crate::managed_services::Operation::WriteStream,&mut report.0.clone())?;
        }
        Ok(())
    }
}

impl Drop for Reader {
    fn drop(&mut self) {
        self.cancel();
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum InputEndpoint {
    #[default]
    Pen,
    Auxiliary,
}

impl InputEndpoint {
    fn other(self) -> Self {
        match self { Self::Pen => Self::Auxiliary, Self::Auxiliary => Self::Pen }
    }
}

trait InputReader {
    /// Retain any complete, unconsumed report until `finish` consumes it.
    fn start(&mut self) -> io::Result<bool>;
}

impl InputReader for Reader {
    fn start(&mut self) -> io::Result<bool> { Reader::start(self) }
}

#[derive(Debug)]
enum InputSelection {
    Report(InputEndpoint, bool),
    AuxiliaryFailed(io::Error),
    Pending,
    Idle,
    Stopped,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InputPoll { Pending, Ready, Stopped }

/// Ownership transitions precede input, while queued reports precede a
/// managed-only Idle. In particular, a zero-time fairness poll must still
/// consume input when foreign producers keep the managed wake signaled.
fn select_awakened_input(managed: bool, transition: bool, stopped: bool,
    select: impl FnOnce() -> io::Result<InputSelection>) -> io::Result<InputSelection> {
    if stopped { return Ok(InputSelection::Stopped); }
    if transition { return Ok(InputSelection::Idle); }
    Ok(match select()? {
        InputSelection::Pending if managed => InputSelection::Idle,
        selection => selection,
    })
}

/// Start the preferred endpoint first. A pending read may already be
/// signaled, so check it before delivering an immediate fallback completion.
/// The ordinary all-pending path performs no additional poll or wakeup.
fn select_input<R: InputReader>(pen: &mut R, auxiliary: Option<&mut R>, preferred: InputEndpoint, poll_pending: bool,
    mut poll: impl FnMut(&R) -> io::Result<InputPoll>,
    mut stopped: impl FnMut() -> io::Result<bool>) -> io::Result<InputSelection> {
    let (first, second) = match preferred {
        InputEndpoint::Pen => (Some(pen), auxiliary),
        InputEndpoint::Auxiliary => (auxiliary, Some(pen)),
    };
    let mut first = first;
    let mut second = second;
    let start = |reader: &mut R, endpoint| match reader.start() {
        Ok(ready) => Ok(if ready { Some(InputSelection::Report(endpoint, true)) } else { None }),
        Err(error) if endpoint == InputEndpoint::Auxiliary => Ok(Some(InputSelection::AuxiliaryFailed(error))),
        Err(error) => Err(error),
    };
    let mut attempt = |reader: &mut R, endpoint, check_stop: bool| {
        let result = start(reader, endpoint);
        match result {
            Ok(None) => Ok(None),
            // A preferred pending endpoint is polled together with Stop below,
            // replacing the usual Stop-only check on an immediate fallback.
            Ok(Some(selection)) if !check_stop && matches!(selection, InputSelection::Report(..)) => Ok(Some(selection)),
            Ok(Some(selection)) => Ok(Some(if stopped()? { InputSelection::Stopped } else { selection })),
            Err(error) => if stopped()? { Ok(Some(InputSelection::Stopped)) } else { Err(error) },
        }
    };
    if let Some(reader) = first.as_deref_mut() && let Some(selection) = attempt(reader, preferred, true)? {
        return Ok(selection);
    }
    if let Some(reader) = second.as_deref_mut() && let Some(selection) = attempt(reader, preferred.other(), first.is_none())? {
        if matches!(selection, InputSelection::Report(..)) && let Some(first) = first.as_deref() {
            match poll(first)? {
                InputPoll::Stopped => return Ok(InputSelection::Stopped),
                InputPoll::Ready => return Ok(InputSelection::Report(preferred, false)),
                InputPoll::Pending => {}
            }
        }
        return Ok(selection);
    }
    // A managed wake may return Idle instead of reaching the kernel wait.
    // Probe retained pending completions here so repeated managed signals
    // cannot hide a ready input event. Ordinary input waits skip these probes.
    if poll_pending {
        for (reader, endpoint) in [(first.as_deref(), preferred), (second.as_deref(), preferred.other())] {
            if let Some(reader) = reader {
                match poll(reader)? {
                    InputPoll::Stopped => return Ok(InputSelection::Stopped),
                    InputPoll::Ready => return Ok(InputSelection::Report(endpoint, false)),
                    InputPoll::Pending => {}
                }
            }
        }
    }
    Ok(InputSelection::Pending)
}

/// The pen collection and, when the tablet has one and it opened, its
/// auxiliary collection. The pen endpoint decides the session's lifetime;
/// losing the auxiliary one only releases its actions.
struct HidSource<'a> {
    candidate: &'a Candidate,
    notification: &'a Notification,
    stop: &'a Event,
    pen: Reader,
    auxiliary: Option<Reader>,
    label: String,
    /// Created on the first short wait; sessions without timer-driven
    /// filters never wait less than a second and never create it.
    timer: Option<OwnedHandle>,
    output: Arc<crate::shared_devices::OutputGate>,
    preferred: InputEndpoint,
}

fn opened_identifiers(primary: &DeviceIdentifier, auxiliary: Option<&DeviceIdentifier>,
    auxiliary_opened: bool) -> Vec<DeviceIdentifier> {
    let mut identifiers = Vec::with_capacity(1 + usize::from(auxiliary_opened));
    identifiers.push(primary.clone());
    if let Some(auxiliary) = auxiliary.filter(|_| auxiliary_opened) {
        identifiers.push(auxiliary.clone());
    }
    identifiers
}

impl<'a> HidSource<'a> {
    fn open(
        selected: &'a SelectedDevice<'a>,
        notification: &'a Notification,
        stop: &'a Event,
        driver_access: bool,
    ) -> io::Result<Self> {
        let candidate = selected.pen;
        let output = crate::shared_devices::OutputGate::new()?;
        let pen = Reader::open(candidate, &selected.identifier, driver_access, &selected.configuration, Arc::clone(&output), false, None)?;
        let auxiliary = selected.auxiliary.as_ref().and_then(|(endpoint, identifier)| {
            Reader::open(endpoint, identifier, driver_access, &selected.configuration, Arc::clone(&output), true, Some(pen.shared.endpoint.epoch))
                .inspect_err(|error| {
                    eprintln!("Auxiliary collection not opened; express keys and wheels do nothing: {error}");
                })
                .ok()
        });
        let tablet = serde_json::json!({"Properties":selected.configuration,
            "Identifiers":opened_identifiers(&selected.identifier, selected.auxiliary.as_ref().map(|(_,id)|id), auxiliary.is_some())});
        pen.shared.tablet(tablet.clone());
        if let Some(reader)=&auxiliary { reader.shared.tablet(tablet); }
        crate::managed_host::publish_owned_devices();
        Ok(Self {
            label: format!(
                "HID input length {}, usage {:04x}:{:04x}",
                candidate.input_length, candidate.usage_page, candidate.usage
            ),
            candidate,
            notification,
            stop,
            pen,
            auxiliary,
            timer: None,
            output,
            preferred: InputEndpoint::Pen,
        })
    }

    fn identifiers(&self, selected: &SelectedDevice<'_>) -> Vec<DeviceIdentifier> {
        opened_identifiers(&selected.identifier,
            selected.auxiliary.as_ref().map(|(_, identifier)| identifier), self.auxiliary.is_some())
    }

    /// Initializes the pen endpoint, then the auxiliary one. A failure on the
    /// auxiliary endpoint closes it and leaves the pen running.
    fn initialize(
        &mut self,
        selected: &SelectedDevice<'_>,
        status: &impl Fn(&str),
    ) -> io::Result<()> {
        self.pen.initialize(
            selected.pen,
            &selected.identifier,
            &selected.configuration,
            self.stop,
        )?;
        if let (Some(reader), Some((endpoint, identifier))) = (&self.auxiliary, &selected.auxiliary)
            && let Err(error) = reader.initialize(
                endpoint,
                identifier,
                &selected.configuration,
                self.stop,
            )
        {
            if error.kind() == io::ErrorKind::Interrupted {
                return Err(error);
            }
            let message = format!(
                "Auxiliary collection initialization failed; express keys and wheels do nothing: {error}"
            );
            eprintln!("{message}");
            status(&message);
            self.auxiliary = None;
        }
        Ok(())
    }

    fn prepare_parsers(&self, selected: &SelectedDevice<'_>) -> io::Result<()> {
        let missing = std::iter::once(&selected.identifier)
            .chain(selected.auxiliary.as_ref().filter(|_| self.auxiliary.is_some()).map(|(_, identifier)| identifier))
            .any(|identifier| !hid::parser_supported(identifier.parser()));
        if missing { crate::plugins::load_parser_registry().map_err(io::Error::other)?; }
        Ok(())
    }

    fn needs_original_parser(&self, selected: &SelectedDevice<'_>) -> bool {
        use otd_core::tablets::{parser_support, ParserSupport};
        parser_support(selected.identifier.parser()) == ParserSupport::Missing
            || (self.auxiliary.is_some() && selected.auxiliary.as_ref().is_some_and(|(_, identifier)|
                parser_support(identifier.parser()) == ParserSupport::Missing))
    }

    /// An independently owned parser for an actually opened auxiliary endpoint.
    /// Construction failures abort startup instead of silently dropping input.
    fn auxiliary_decoder(&mut self, selected: &SelectedDevice<'_>, plugins: Option<&PluginChain>) -> io::Result<Option<RuntimeDecoder>> {
        let Some((_, identifier)) = selected.auxiliary.as_ref() else { return Ok(None); };
        if self.auxiliary.is_none() { return Ok(None); }
        if !hid::parser_supported(identifier.parser()) && plugins.is_none() {
            crate::plugins::load_parser_registry().map_err(io::Error::other)?;
        }
        match plugins { Some(plugins) => plugins.source_decoder_for_endpoint(identifier.parser(), selected.spec,true),
            None => RuntimeDecoder::for_parser(identifier.parser(), selected.spec) }.map(Some).map_err(io::Error::other)
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
        self.pen.cancel();
        if let Some(auxiliary) = &mut self.auxiliary {
            auxiliary.cancel();
        }
    }

    fn stopped(&self) -> io::Result<bool> {
        match unsafe { WaitForSingleObject(self.stop.raw(), 0) } {
            WAIT_OBJECT_0 => Ok(true),
            WAIT_TIMEOUT => Ok(false),
            _ => Err(io::Error::last_os_error()),
        }
    }

    fn select_input(&mut self, poll_pending: bool) -> io::Result<InputSelection> {
        let stop = self.stop.raw();
        select_input(&mut self.pen, self.auxiliary.as_mut(), self.preferred, poll_pending, |reader| {
            if reader.custom.is_some() {
                return match unsafe { WaitForSingleObject(stop, 0) } {
                    WAIT_OBJECT_0 => Ok(InputPoll::Stopped),
                    WAIT_TIMEOUT => Ok(InputPoll::Pending),
                    _ => Err(io::Error::last_os_error()),
                };
            }
            match wait(&[stop, reader.event.raw()], 0)? {
                Some(0) => Ok(InputPoll::Stopped),
                Some(_) => Ok(InputPoll::Ready),
                None => Ok(InputPoll::Pending),
            }
        }, || match unsafe { WaitForSingleObject(stop, 0) } {
            WAIT_OBJECT_0 => Ok(true),
            WAIT_TIMEOUT => Ok(false),
            _ => Err(io::Error::last_os_error()),
        })
    }

    fn input_report(&mut self, endpoint: InputEndpoint, queued: bool) -> io::Result<Read<'_>> {
        self.preferred = endpoint.other();
        match endpoint {
            InputEndpoint::Pen => self.pen_report(queued),
            InputEndpoint::Auxiliary => self.auxiliary_report(queued),
        }
    }

    /// Selection checks Stop before a queued completion; asynchronous
    /// completions come from a wait in which Stop has precedence.
    fn pen_report(&mut self, queued: bool) -> io::Result<Read<'_>> {
        let ready = self.pen.custom_ready.take().unwrap_or_else(Instant::now);
        let Some(length) = self.pen.finish()? else {
            return Ok(Read::Ended);
        };
        self.pen.shared.publish(&self.pen.buffer[..length]);
        Ok(Read::Report {
            bytes: &self.pen.buffer[..length],
            ready,
            queued,
        })
    }

    fn auxiliary_report(&mut self, queued: bool) -> io::Result<Read<'_>> {
        let ready = self.auxiliary.as_mut().and_then(|reader|reader.custom_ready.take()).unwrap_or_else(Instant::now);
        let finished = self.auxiliary.as_mut().map(Reader::finish);
        match finished {
            Some(Ok(Some(length))) => {
                match &self.auxiliary {
                    Some(reader) => { reader.shared.publish(&reader.buffer[..length]); Ok(Read::Auxiliary {
                        bytes: &reader.buffer[..length],
                        ready,
                        queued,
                    }) },
                    None => Ok(Read::Idle),
                }
            }
            Some(Ok(None)) => Ok(self.lose_auxiliary("read cancelled".into())),
            Some(Err(error)) => Ok(self.lose_auxiliary(error.to_string())),
            None => Ok(Read::Idle),
        }
    }

    fn lose_auxiliary(&mut self, reason: String) -> Read<'static> {
        eprintln!("Auxiliary collection stopped: {reason}");
        self.auxiliary = None;
        Read::AuxiliaryEnded
    }
}

impl ReportSource for HidSource<'_> {
    fn native_output_enabled(&self) -> bool { self.output.native_enabled() }
    fn output_started(&self) { self.output.start(); }
    fn output_acknowledged(&self, enabled: bool) { self.output.acknowledge(enabled); }
    fn shared_output(&self) -> bool {
        true
    }

    fn label(&self) -> &str {
        &self.label
    }

    fn now(&self) -> Instant {
        Instant::now()
    }

    // Blocking waits and pending-read probes put Stop first; selection also
    // checks it before returning any immediate completion.
    fn next(&mut self, timeout: Duration) -> io::Result<Read<'_>> {
        let selection = if unsafe { WaitForSingleObject(self.output.event(), 0) } == WAIT_OBJECT_0 {
            if unsafe { ResetEvent(self.output.event()) } == 0 { return Err(io::Error::last_os_error()); }
            let managed = self.output.take_managed_pending();
            select_awakened_input(managed, self.output.transition_pending(), self.stopped()?, || self.select_input(true))?
        } else { self.select_input(false)? };
        match selection {
            InputSelection::Report(endpoint, queued) => return self.input_report(endpoint, queued),
            InputSelection::AuxiliaryFailed(error) => return Ok(self.lose_auxiliary(error.to_string())),
            InputSelection::Stopped => { self.cancel(); return Ok(Read::Ended); }
            InputSelection::Idle => return Ok(Read::Idle),
            InputSelection::Pending => {}
        }
        let until = Instant::now() + timeout;
        // An already-due deadline is an input/stop poll, not a new timer wakeup.
        let precise = !timeout.is_zero() && timeout < PRECISE_WAIT;
        let timer = if precise {
            self.arm_timer(timeout)?
        } else {
            ptr::null_mut()
        };
        // Stop first, then alternate endpoints after every report so neither
        // backlogged collection can starve the other.
        let mut handles = [ptr::null_mut(); 6];
        let mut count = 0;
        let mut add = |handle: HANDLE| {
            handles[count] = handle;
            count += 1;
            count - 1
        };
        add(self.stop.raw());
        let output_change = add(self.output.event());
        let (pen, auxiliary) = if self.preferred == InputEndpoint::Pen {
            let pen = add(self.pen.event.raw());
            (pen, self.auxiliary.as_ref().map(|reader| add(reader.event.raw())))
        } else {
            let auxiliary = self.auxiliary.as_ref().map(|reader| add(reader.event.raw()));
            (add(self.pen.event.raw()), auxiliary)
        };
        let notification = add(self.notification.event());
        // The timer is last, so a non-precise wait leaves it out.
        let timer_index = add(timer);
        let waited = if precise {
            timer_index + 1
        } else {
            timer_index
        };
        loop {
            // Other devices' notifications do not extend the wait.
            let result = if precise {
                wait(&handles[..waited], INFINITE)
            } else {
                let remaining = until.saturating_duration_since(Instant::now());
                let millis = u32::try_from(remaining.as_millis()).unwrap_or(u32::MAX);
                wait(&handles[..waited], millis)
            };
            match result {
                Ok(Some(index)) if index == output_change => {
                    if unsafe { ResetEvent(self.output.event()) } == 0 { return Err(io::Error::last_os_error()); }
                    let managed = self.output.take_managed_pending();
                    match select_awakened_input(managed, self.output.transition_pending(), self.stopped()?, || self.select_input(true))? {
                        InputSelection::Report(endpoint, queued) => return self.input_report(endpoint, queued),
                        InputSelection::AuxiliaryFailed(error) => return Ok(self.lose_auxiliary(error.to_string())),
                        InputSelection::Stopped => { self.cancel(); return Ok(Read::Ended); }
                        InputSelection::Pending | InputSelection::Idle => {}
                    }
                    return Ok(Read::Idle);
                }
                Ok(Some(index)) if index == pen => return self.input_report(InputEndpoint::Pen, false),
                Ok(Some(index)) if Some(index) == auxiliary => {
                    return self.input_report(InputEndpoint::Auxiliary, false);
                }
                // Some HID device arrived or left; only this one matters.
                Ok(Some(index)) if index == notification && self.candidate.is_present() => {}
                Ok(Some(index)) if index == timer_index => return Ok(Read::Idle),
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

pub fn run(
    selected: &SelectedDevice<'_>,
    profile: &Profile,
    notification: &Notification,
    stop_event: &Event,
    mode: Mode,
    plugins: &mut PluginChain,
    status: &impl Fn(&str),
) -> io::Result<()> {
    let mut source = HidSource::open(
        selected,
        notification,
        stop_event,
        matches!(mode, Mode::Driver),
    )?;
    if matches!(mode, Mode::Driver) {
        source.initialize(selected, status)?;
    }
    let profile = profile
        .for_tablet(selected.spec)
        .map_err(io::Error::other)?;
    source.prepare_parsers(selected)?;
    if matches!(mode, Mode::Driver) {
        plugins.bind_identifiers(&profile, &selected.configuration, &source.identifiers(selected))
            .map_err(io::Error::other)?;
    }
    if matches!(mode, Mode::Driver) && source.needs_original_parser(selected) {
        plugins.prepare_managed_decoder().map_err(io::Error::other)?;
    }
    let mut decoder = if matches!(mode, Mode::Driver) { plugins.source_decoder(selected.identifier.parser(), selected.spec).map_err(io::Error::other)? } else { selected.decoder()? };
    let mut auxiliary = source.auxiliary_decoder(selected, matches!(mode, Mode::Driver).then_some(&*plugins))?;
    if matches!(mode, Mode::Driver) && (decoder.is_managed() || auxiliary.as_ref().is_some_and(RuntimeDecoder::is_managed)) {
        plugins.prepare_managed_decoder().map_err(io::Error::other)?;
    }
    plugins.reset();
    if let Some(name) = plugins.take_failure() {
        return Err(io::Error::other(format!("Plugin failed during reset notification: {name}")));
    }
    announce_auxiliary(&source, selected, status);
    if matches!(mode, Mode::Driver) {
        eprintln!("{}", plugins.describe(&profile));
    }
    let _debug = DebugDevice::set(selected, &source);
    let _priority = if matches!(mode, Mode::Driver) {
        ReaderPriority::for_driver(crate::experimental::mmcss_enabled(), status)
    } else {
        ReaderPriority::raise()
    };
    let mut output = if matches!(mode, Mode::Driver) {
        Some(SessionOutput::new()?)
    } else {
        None
    };
    if matches!(mode, Mode::Driver) { plugins.set_managed_wake(source.output.clone()).map_err(io::Error::other)?; }
    let pen = pen_device(&profile, mode)?;
    let actions = action_sink(mode)?.map(|sink| plugins.wrap_action_sink_with_wake(&profile, &selected.configuration, sink, source.output.clone()))
        .transpose().map_err(io::Error::other)?;
    let result = otd_core::session::run_gated_with_endpoints(
        &mut source,
        &mut WindowsDisplays,
        &profile,
        mode,
        &mut decoder,
        auxiliary
            .as_mut()
            .map(|decoder| decoder as &mut dyn PenDecoder),
        plugins,
        |packet| output.as_ref().map_or(Ok(()), |output| output.send(packet)),
        pen,
        actions,
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

fn announce_auxiliary(
    source: &HidSource<'_>,
    selected: &SelectedDevice<'_>,
    status: &impl Fn(&str),
) {
    if source.auxiliary.is_some() {
        status(&format!(
            "{} express keys and wheels connected",
            selected.configuration.name
        ));
    }
}

/// Key and button output for a driving session's buttons and wheels.
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
    fn set(selected: &SelectedDevice<'_>, source: &HidSource<'_>) -> Self {
        Self {
            _registration: otd_core::debug::Registration::with_selection_key(
                otd_core::debug::Device {
                    name: selected.configuration.name.clone(),
                    parser: selected.identifier.parser().to_owned(),
                },
                source.auxiliary.as_ref().map_or(source.pen.buffer.len(),
                    |auxiliary| source.pen.buffer.len().max(auxiliary.buffer.len())),
                source.auxiliary.as_ref().and(selected.auxiliary.as_ref())
                    .map(|(_, identifier)| identifier.parser().to_owned()),
                crate::device_sessions::debug_key(),
            ),
        }
    }
}
/// A candidate owns its handles, events and read buffers before the old
/// worker pauses. Preparation never initializes the tablet or issues a read.
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
        for (reports, length, output) in [
            (
                &selected.identifier.feature_init_report,
                selected.pen.endpoint.feature_length,
                false,
            ),
            (
                &selected.identifier.output_init_report,
                selected.pen.endpoint.output_length,
                true,
            ),
        ] {
            for report in reports
                .iter()
                .flatten()
                .filter(|report| !report.0.is_empty())
            {
                let invalid = if selected.pen.endpoint.transport == otd_core::endpoint_match::Transport::WinUsb {
                    report.0.len() > u16::MAX as usize || (output && length == 0)
                } else { length == 0 || length > u16::MAX as u32 || report.0.len() > length as usize };
                if invalid {
                    return Err(io::Error::other(
                        "initialization report exceeds endpoint report length",
                    ));
                }
            }
        }
        let source = HidSource::open(selected, notification, interrupt, true)?;
        source.prepare_parsers(selected)?;
        Ok(Self { source, selected })
    }

    pub fn identifiers(&self) -> Vec<DeviceIdentifier> {
        self.source.identifiers(self.selected)
    }

    pub fn retire(&mut self)->io::Result<()> {
        retire_custom_readers(&mut self.source)
    }

    pub fn activate(&mut self) -> io::Result<()> {
        if !self.selected.pen.is_present() {
            return Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "prepared tablet endpoint disappeared before activation",
            ));
        }
        self.source
            .initialize(self.selected, &|line| eprintln!("{line}"))?;
        // The prepared handles may have queued input while the previous
        // worker was still reading. Flush only after that worker has
        // quiesced, so the replacement cannot replay its already-processed
        // pen/button reports.
        // https://learn.microsoft.com/windows-hardware/drivers/ddi/hidsdi/nf-hidsdi-hidd_flushqueue
        for reader in std::iter::once(&mut self.source.pen)
            .chain(self.source.auxiliary.iter_mut())
        {
            if let Some(custom)=&mut reader.custom {custom.flush()?;continue;}
            if let Some(interface) = &reader.shared.endpoint.io.winusb {
                interface.flush_input()?;
                continue;
            }
            if !unsafe {
                windows_sys::Win32::Devices::HumanInterfaceDevice::HidD_FlushQueue(reader.shared.endpoint.io.native_handle()?.raw())
            } {
                return Err(io::Error::last_os_error());
            }
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
        preset: Option<crate::binding_presets::Callback>,
        gate: impl FnOnce(&[DeviceIdentifier]) -> io::Result<bool>,
    ) -> io::Result<()> {
        let profile = profile
            .for_tablet(self.selected.spec)
            .map_err(io::Error::other)?;
        let mut source = self.source;
        source.prepare_parsers(self.selected)?;
        if plugins.bind_identifiers(&profile, &self.selected.configuration, &source.identifiers(self.selected))
            .map_err(io::Error::other)? {
            plugins.reset();
            if let Some(name) = plugins.take_failure() {
                return Err(io::Error::other(format!("Plugin failed during endpoint reset notification: {name}")));
            }
        }
        if source.needs_original_parser(self.selected) {
            plugins.prepare_managed_decoder().map_err(io::Error::other)?;
        }
        let mut decoder = plugins.source_decoder(self.selected.identifier.parser(), self.selected.spec).map_err(io::Error::other)?;
        let mut auxiliary = source.auxiliary_decoder(self.selected, Some(&*plugins))?;
        if decoder.is_managed() || auxiliary.as_ref().is_some_and(RuntimeDecoder::is_managed) {
            plugins.prepare_managed_decoder().map_err(io::Error::other)?;
        }
        let _debug = DebugDevice::set(self.selected, &source);
        announce_auxiliary(&source, self.selected, status);
        let _priority = ReaderPriority::for_driver(crate::experimental::mmcss_enabled(), status);
        let mut output = SessionOutput::new()?;
        plugins.set_managed_wake(source.output.clone()).map_err(io::Error::other)?;
        let pen = pen_device(&profile, Mode::Driver)?;
        let actions = action_sink(Mode::Driver)?.map(|sink| plugins.wrap_action_sink_with_wake(&profile, &self.selected.configuration, sink, source.output.clone()))
            .transpose().map_err(io::Error::other)?;
        let actions = match (actions, preset) { (Some(sink), Some(request)) => Some(crate::binding_presets::wrap(sink, request, profile.binding_inhibit)), (sink, _) => sink };
        let identifiers = source.identifiers(self.selected);
        let result = otd_core::session::run_gated_with_endpoints(
            &mut source,
            &mut WindowsDisplays,
            &profile,
            Mode::Driver,
            &mut decoder,
            auxiliary
                .as_mut()
                .map(|decoder| decoder as &mut dyn PenDecoder),
            plugins,
            |packet| output.send(packet),
            pen,
            actions,
            status,
            || gate(&identifiers),
        );
        output
            .finish()
            .map_err(otd_core::session::cleanup_failure)?;
        retire_custom_readers(&mut source).map_err(otd_core::session::cleanup_failure)?;
        result
    }
}

fn retire_custom_readers(source:&mut HidSource<'_>)->io::Result<()> {
    let mut failure=None;
    for reader in std::iter::once(&mut source.pen).chain(source.auxiliary.iter_mut()) {
        if let Some(custom)=&mut reader.custom {
            if let Err(error)=custom.close(std::time::Duration::from_secs(3)){failure.get_or_insert(error);}
        }
    }
    failure.map_or(Ok(()),Err)
}

pub fn wait_for_retry(notification: &Notification, stop_event: &Event) -> io::Result<bool> {
    match wait(&[notification.event(), stop_event.raw()], 2_000)? {
        Some(1) => Ok(false),
        _ => Ok(true),
    }
}

/// True only for a device notification. Stop, and a companion session
/// ending, wake immediately; the owner checks its stop flag and reaps finished
/// sessions before its next pass. The timeout is the owner's fallback rescan.
pub fn companion_wake(
    notification: &Notification,
    stop_event: &Event,
    timeout: Duration,
) -> io::Result<bool> {
    let millis = u32::try_from(timeout.as_millis()).unwrap_or(u32::MAX - 1);
    Ok(wait(&[stop_event.raw(), notification.event()], millis)? == Some(1))
}

#[cfg(test)]
mod identifier_metadata_tests {
    use super::*;
    #[test]
    fn discovered_but_unopened_auxiliary_is_not_a_live_identifier() {
        let primary = DeviceIdentifier { product_id: Some(2), report_parser: Some("Matched.Digitizer".into()), ..Default::default() };
        let auxiliary = DeviceIdentifier { product_id: Some(3), report_parser: Some("Matched.Auxiliary".into()), ..Default::default() };
        assert_eq!(opened_identifiers(&primary, Some(&auxiliary), true), [primary.clone(), auxiliary.clone()]);
        assert_eq!(opened_identifiers(&primary, Some(&auxiliary), false), [primary.clone()]);
        assert_eq!(opened_identifiers(&primary, None, false), [primary]);
    }
}

#[cfg(test)]
mod input_fairness_tests {
    use super::*;
    use std::{cell::Cell, collections::VecDeque};

    #[derive(Clone, Copy)]
    enum Completion { Immediate(u8), Pending(u8), Fault }

    /// One fixed completion slot per endpoint, as with OVERLAPPED input.
    /// Reissuing before consumption would discard a sequence number and fail
    /// the FIFO assertions below. No OS/device or managed runtime is opened.
    struct FakeReader {
        queued: VecDeque<Completion>,
        state: ReadState,
        report: Option<u8>,
        signaled: bool,
        starts: usize,
        consumed: Vec<u8>,
    }

    impl FakeReader {
        fn new(reports: impl IntoIterator<Item = Completion>) -> Self {
            Self { queued: reports.into_iter().collect(), state: ReadState::Idle,
                report: None, signaled: false, starts: 0, consumed: Vec::new() }
        }
        fn finish(&mut self) -> u8 {
            assert!(self.state == ReadState::Ready || self.signaled);
            self.state.completed(&Ok(()));
            self.signaled = false;
            let report = self.report.take().expect("report consumed exactly once");
            self.consumed.push(report);
            report
        }
        fn signal(&mut self) { assert_eq!(self.state, ReadState::Pending); self.signaled = true; }
    }

    impl InputReader for FakeReader {
        fn start(&mut self) -> io::Result<bool> {
            match self.state {
                ReadState::Ready => return Ok(true),
                ReadState::Pending => return Ok(false),
                ReadState::Idle => {}
            }
            self.starts += 1;
            match self.queued.pop_front().unwrap_or(Completion::Pending(255)) {
                Completion::Immediate(value) => {
                    self.report = Some(value); self.state = ReadState::Ready; Ok(true)
                }
                Completion::Pending(value) => {
                    self.report = Some(value); self.state = ReadState::Pending; Ok(false)
                }
                Completion::Fault => Err(io::Error::new(io::ErrorKind::BrokenPipe, "fake removed endpoint")),
            }
        }
    }

    fn select(pen: &mut FakeReader, auxiliary: Option<&mut FakeReader>, preferred: InputEndpoint,
        stop: bool, polls: &Cell<usize>) -> io::Result<InputSelection> {
        select_input(pen, auxiliary, preferred, false, |reader| {
            polls.set(polls.get() + 1);
            Ok(if stop { InputPoll::Stopped } else if reader.signaled { InputPoll::Ready } else { InputPoll::Pending })
        }, || Ok(stop))
    }

    fn deliver(selection: InputSelection, pen: &mut FakeReader, auxiliary: &mut FakeReader,
        preferred: &mut InputEndpoint) -> (InputEndpoint, u8) {
        let InputSelection::Report(endpoint, _) = selection else { panic!("expected report, got {selection:?}"); };
        *preferred = endpoint.other();
        let value = match endpoint { InputEndpoint::Pen => pen.finish(), InputEndpoint::Auxiliary => auxiliary.finish() };
        (endpoint, value)
    }

    #[test]
    fn two_immediate_backlogs_alternate_and_keep_each_endpoint_fifo() {
        let mut pen = FakeReader::new((0..8).map(Completion::Immediate));
        let mut auxiliary = FakeReader::new((10..18).map(Completion::Immediate));
        let mut preferred = InputEndpoint::Pen;
        let polls = Cell::new(0);
        for index in 0..16 {
            let selection = select(&mut pen, Some(&mut auxiliary), preferred, false, &polls).unwrap();
            let (endpoint, value) = deliver(selection, &mut pen, &mut auxiliary, &mut preferred);
            assert_eq!(endpoint, if index % 2 == 0 { InputEndpoint::Pen } else { InputEndpoint::Auxiliary });
            assert_eq!(value, if index % 2 == 0 { index / 2 } else { 10 + index / 2 });
        }
        assert_eq!(polls.get(), 0, "immediate preferred completions need no extra readiness probe");
        assert_eq!(pen.starts, 8);
        assert_eq!(auxiliary.starts, 8);
    }

    #[test]
    fn repeated_managed_wakes_deliver_queued_input_before_idle() {
        // Both HidSource wake paths use this selection before returning Idle.
        // Model a foreign producer that signals on every zero-time input poll.
        for signaled_pending in [false, true] {
            let mut pen = FakeReader::new((0..8).map(Completion::Immediate));
            let mut auxiliary = FakeReader::new((10..18).map(Completion::Immediate));
            let mut preferred = InputEndpoint::Pen;
            let polls = Cell::new(0);
            for index in 0..16 {
                if signaled_pending {
                    let first = match preferred { InputEndpoint::Pen => &mut pen, InputEndpoint::Auxiliary => &mut auxiliary };
                    assert!(first.start().unwrap());
                    first.state = ReadState::Pending;
                    first.signaled = true;
                }
                let selection = select_awakened_input(true, false, false,
                    || select_input(&mut pen, Some(&mut auxiliary), preferred, true, |reader| {
                        polls.set(polls.get() + 1);
                        Ok(if reader.signaled { InputPoll::Ready } else { InputPoll::Pending })
                    }, || Ok(false))).unwrap();
                let (endpoint, value) = deliver(selection, &mut pen, &mut auxiliary, &mut preferred);
                assert_eq!(endpoint, if index % 2 == 0 { InputEndpoint::Pen } else { InputEndpoint::Auxiliary });
                assert_eq!(value, if index % 2 == 0 { index / 2 } else { 10 + index / 2 });
            }
            assert_eq!(pen.consumed, (0..8).collect::<Vec<_>>());
            assert_eq!(auxiliary.consumed, (10..18).collect::<Vec<_>>());
            assert!(matches!(select_awakened_input(true, false, false,
                || select(&mut pen, Some(&mut auxiliary), preferred, false, &polls)).unwrap(), InputSelection::Idle));
        }
    }

    #[test]
    fn managed_wake_preserves_stop_transition_and_reader_notification_priority() {
        assert!(matches!(select_awakened_input(true, true, true,
            || panic!("Stop precedes input selection")).unwrap(), InputSelection::Stopped));
        assert!(matches!(select_awakened_input(true, false, true,
            || panic!("Stop precedes managed work")).unwrap(), InputSelection::Stopped));
        assert!(matches!(select_awakened_input(true, true, false,
            || panic!("Ownership transition precedes input selection")).unwrap(), InputSelection::Idle));
        // An ordinary custom-reader signal still probes its reader and retains
        // Pending for the normal initial wait when no report is ready yet.
        assert!(matches!(select_awakened_input(false, false, false,
            || Ok(InputSelection::Pending)).unwrap(), InputSelection::Pending));
        assert!(matches!(select_awakened_input(true, false, false,
            || Ok(InputSelection::AuxiliaryFailed(io::Error::other("removed")))).unwrap(), InputSelection::AuxiliaryFailed(_)));
        let mut pen = FakeReader::new([Completion::Pending(1)]);
        let mut auxiliary = FakeReader::new([Completion::Pending(10)]);
        // Stop can arrive after the wake's initial Stop check, while retained
        // read events are being polled. It still wins without consuming input.
        assert!(matches!(select_input(&mut pen, Some(&mut auxiliary), InputEndpoint::Pen, true,
            |_| Ok(InputPoll::Stopped), || Ok(false)).unwrap(), InputSelection::Stopped));
        assert!(pen.consumed.is_empty() && auxiliary.consumed.is_empty());
    }

    #[test]
    fn managed_wake_polls_both_pending_readers_and_consumes_each_completion_once() {
        for first in [InputEndpoint::Pen, InputEndpoint::Auxiliary] {
            for signal_both in [false, true] {
                let mut pen = FakeReader::new([Completion::Pending(1)]);
                let mut auxiliary = FakeReader::new([Completion::Pending(10)]);
                assert!(!pen.start().unwrap());
                assert!(!auxiliary.start().unwrap());
                // In the single-signal case, readiness beats preference; in
                // the dual-signal case the preferred endpoint wins first.
                let mut preferred = if signal_both { first } else { first.other() };
                match first { InputEndpoint::Pen => pen.signal(), InputEndpoint::Auxiliary => auxiliary.signal() }
                if signal_both {
                    match first.other() { InputEndpoint::Pen => pen.signal(), InputEndpoint::Auxiliary => auxiliary.signal() }
                }
                let select_wake = |pen: &mut FakeReader, auxiliary: &mut FakeReader, preferred| {
                    select_awakened_input(true, false, false,
                        || select_input(pen, Some(auxiliary), preferred, true,
                            |reader| Ok(if reader.signaled { InputPoll::Ready } else { InputPoll::Pending }),
                            || Ok(false))).unwrap()
                };
                let selected = select_wake(&mut pen, &mut auxiliary, preferred);
                assert!(matches!(selected, InputSelection::Report(endpoint, false) if endpoint == first));
                let expected = |endpoint| if endpoint == InputEndpoint::Pen { 1 } else { 10 };
                assert_eq!(deliver(selected, &mut pen, &mut auxiliary, &mut preferred), (first, expected(first)));
                assert_eq!(preferred, first.other());
                if !signal_both {
                    match preferred { InputEndpoint::Pen => pen.signal(), InputEndpoint::Auxiliary => auxiliary.signal() }
                }
                let selected = select_wake(&mut pen, &mut auxiliary, preferred);
                assert!(matches!(selected, InputSelection::Report(endpoint, false) if endpoint == first.other()));
                assert_eq!(deliver(selected, &mut pen, &mut auxiliary, &mut preferred), (first.other(), expected(first.other())));
                assert_eq!(pen.consumed, [1]);
                assert_eq!(auxiliary.consumed, [10]);
                assert!(matches!(select_wake(&mut pen, &mut auxiliary, preferred), InputSelection::Idle));
                assert_eq!(pen.consumed, [1]);
                assert_eq!(auxiliary.consumed, [10]);
            }
        }
    }

    #[test]
    fn signaled_pending_preferred_beats_immediate_fallback_without_overwriting_it() {
        for preferred in [InputEndpoint::Pen, InputEndpoint::Auxiliary] {
            let mut pen = FakeReader::new([Completion::Immediate(1), Completion::Immediate(2)]);
            let mut auxiliary = FakeReader::new([Completion::Immediate(10), Completion::Immediate(11)]);
            let first = match preferred { InputEndpoint::Pen => &mut pen, InputEndpoint::Auxiliary => &mut auxiliary };
            // Model a read submitted on an earlier call that completed between
            // calls: start() returns false although its event is now signaled.
            first.state = ReadState::Pending;
            first.report = Some(99);
            first.signaled = true;
            let polls = Cell::new(0);
            let selection = select(&mut pen, Some(&mut auxiliary), preferred, false, &polls).unwrap();
            assert!(matches!(selection, InputSelection::Report(endpoint, false) if endpoint == preferred));
            let mut next = preferred;
            assert_eq!(deliver(selection, &mut pen, &mut auxiliary, &mut next), (preferred, 99));
            let selection = select(&mut pen, Some(&mut auxiliary), next, false, &polls).unwrap();
            let (endpoint, _) = deliver(selection, &mut pen, &mut auxiliary, &mut next);
            assert_eq!(endpoint, preferred.other());
            let fallback = match endpoint { InputEndpoint::Pen => &pen, InputEndpoint::Auxiliary => &auxiliary };
            assert_eq!(fallback.starts, 1, "deferred immediate buffer must not be reissued");
            assert_eq!(fallback.consumed.len(), 1);
            assert_eq!(polls.get(), 1);
        }
    }

    #[test]
    fn pending_preferred_allows_fallback_then_wins_when_signaled() {
        let mut pen = FakeReader::new([Completion::Immediate(1), Completion::Immediate(2)]);
        let mut auxiliary = FakeReader::new([Completion::Pending(10)]);
        let polls = Cell::new(0);
        let mut preferred = InputEndpoint::Auxiliary;
        let selection = select(&mut pen, Some(&mut auxiliary), preferred, false, &polls).unwrap();
        assert_eq!(deliver(selection, &mut pen, &mut auxiliary, &mut preferred), (InputEndpoint::Pen, 1));
        auxiliary.signal();
        let selection = select(&mut pen, Some(&mut auxiliary), preferred, false, &polls).unwrap();
        assert_eq!(deliver(selection, &mut pen, &mut auxiliary, &mut preferred), (InputEndpoint::Auxiliary, 10));
        let selection = select(&mut pen, Some(&mut auxiliary), preferred, false, &polls).unwrap();
        assert_eq!(deliver(selection, &mut pen, &mut auxiliary, &mut preferred), (InputEndpoint::Pen, 2));
        assert_eq!(auxiliary.starts, 1);
        assert_eq!(pen.starts, 2);
    }

    #[test]
    fn all_pending_adds_no_poll_and_never_reissues_an_outstanding_read() {
        let mut pen = FakeReader::new([Completion::Pending(1)]);
        let mut auxiliary = FakeReader::new([Completion::Pending(10)]);
        let polls = Cell::new(0);
        for _ in 0..4 {
            assert!(matches!(select(&mut pen, Some(&mut auxiliary), InputEndpoint::Pen, false, &polls).unwrap(), InputSelection::Pending));
        }
        assert_eq!((pen.starts, auxiliary.starts, polls.get()), (1, 1, 0));
    }

    #[test]
    fn pending_preferred_poll_replaces_fallback_stop_only_check() {
        let mut pen = FakeReader::new([Completion::Immediate(1)]);
        let mut auxiliary = FakeReader::new([Completion::Pending(10)]);
        let selection = select_input(&mut pen, Some(&mut auxiliary), InputEndpoint::Auxiliary, false,
            |_| Ok(InputPoll::Pending), || panic!("combined preferred/Stop poll already checks Stop")).unwrap();
        assert!(matches!(selection, InputSelection::Report(InputEndpoint::Pen, true)));
    }

    #[test]
    fn stop_wins_over_immediate_and_pending_signaled_completions() {
        for pending in [false, true] {
            let mut pen = FakeReader::new([Completion::Immediate(1)]);
            let mut auxiliary = FakeReader::new([Completion::Immediate(10)]);
            if pending { auxiliary.state = ReadState::Pending; auxiliary.report = Some(99); auxiliary.signaled = true; }
            assert!(matches!(select(&mut pen, Some(&mut auxiliary), InputEndpoint::Auxiliary, true, &Cell::new(0)).unwrap(), InputSelection::Stopped));
            assert!(pen.consumed.is_empty());
            assert!(auxiliary.consumed.is_empty());
        }
        for preferred in [InputEndpoint::Pen, InputEndpoint::Auxiliary] {
            let mut pen = FakeReader::new([Completion::Fault]);
            let mut auxiliary = FakeReader::new([Completion::Fault]);
            assert!(matches!(select(&mut pen, Some(&mut auxiliary), preferred, true, &Cell::new(0)).unwrap(), InputSelection::Stopped));
        }
    }

    #[test]
    fn auxiliary_fault_is_isolated_and_keeps_pending_pen_owned() {
        let mut pen = FakeReader::new([Completion::Pending(1)]);
        let mut auxiliary = FakeReader::new([Completion::Fault]);
        assert!(matches!(select(&mut pen, Some(&mut auxiliary), InputEndpoint::Pen, false, &Cell::new(0)).unwrap(), InputSelection::AuxiliaryFailed(_)));
        assert_eq!(pen.state, ReadState::Pending);
        pen.signal();
        assert!(matches!(select(&mut pen, None, InputEndpoint::Auxiliary, false, &Cell::new(0)).unwrap(), InputSelection::Pending));
        // With no immediate fallback, the source waits on this existing pen
        // event and consumes its sole completion.
        assert_eq!(pen.finish(), 1);
        assert_eq!(pen.starts, 1);
    }

    #[test]
    fn pen_fault_and_poll_fault_propagate_without_consuming_fallback() {
        let mut pen = FakeReader::new([Completion::Fault]);
        let mut auxiliary = FakeReader::new([]);
        assert_eq!(select(&mut pen, Some(&mut auxiliary), InputEndpoint::Pen, false, &Cell::new(0)).unwrap_err().kind(), io::ErrorKind::BrokenPipe);
        let mut pen = FakeReader::new([Completion::Pending(1)]);
        let mut auxiliary = FakeReader::new([Completion::Immediate(10)]);
        let error = select_input(&mut pen, Some(&mut auxiliary), InputEndpoint::Pen, false,
            |_| Err(io::Error::other("fake wait failure")), || Ok(false)).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Other);
        assert_eq!(pen.state, ReadState::Pending);
        assert_eq!(auxiliary.state, ReadState::Ready);
        assert!(auxiliary.consumed.is_empty());
    }

    #[test]
    fn incomplete_completion_remains_owned_until_terminal_completion() {
        let mut pen = FakeReader::new([Completion::Pending(1)]);
        assert!(!pen.start().unwrap());
        pen.state.completed(&Err(io::Error::from_raw_os_error(ERROR_IO_INCOMPLETE as i32)));
        assert_eq!(pen.state, ReadState::Pending, "drop must still cancel and drain the outstanding operation");
        assert!(!pen.start().unwrap());
        assert_eq!(pen.starts, 1, "incomplete completion must not reuse kernel-owned storage");
        pen.signal();
        assert_eq!(pen.finish(), 1);
        assert_eq!(pen.state, ReadState::Idle);

        for error in [ERROR_OPERATION_ABORTED, 1167] { // ERROR_DEVICE_NOT_CONNECTED
            let mut state = ReadState::Pending;
            state.completed(&Err(io::Error::from_raw_os_error(error as i32)));
            assert_eq!(state, ReadState::Idle, "terminal completion relinquishes operation ownership");
        }
    }
}
