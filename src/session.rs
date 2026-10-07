//! Windows device sessions: overlapped HID readers for the pen collection and
//! the tablet's auxiliary collection (express keys, wheels), which feed the
//! core's session loop at reader priority, ending on device removal or a stop
//! event.

use std::io;
use std::ptr;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{
    ERROR_IO_PENDING, ERROR_OPERATION_ABORTED, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT,
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
    // Rust drops fields in declaration order; WinUSB must be freed first.
    winusb: Option<crate::winusb::Interface>,
    handle: OwnedHandle,
    event: Event,
    buffer: Box<[u8]>,
    operation: OVERLAPPED,
    pending: bool,
}

impl Reader {
    fn open(
        candidate: &Candidate,
        identifier: &DeviceIdentifier,
        driver_access: bool,
    ) -> io::Result<Self> {
        let handle = if driver_access && initialization_writes(identifier) {
            candidate.open(true)?
        } else {
            candidate.open_read()?
        };
        let event = Event::create(true)?;
        let winusb = if candidate.endpoint.transport == otd_core::endpoint_match::Transport::WinUsb {
            let interface = crate::winusb::Interface::open(&handle)?;
            if interface.input_length() != candidate.input_length || interface.input_length() == 0 {
                return Err(io::Error::new(io::ErrorKind::InvalidData,
                    "WinUSB input pipe changed since device discovery"));
            }
            Some(interface)
        } else { None };
        Ok(Self {
            operation: OVERLAPPED {
                hEvent: event.raw(),
                ..Default::default()
            },
            winusb,
            handle,
            event,
            // ReadFile needs room for the collection's whole input report.
            buffer: vec![0; usize::from(candidate.input_length.max(1))].into_boxed_slice(),
            pending: false,
        })
    }

    /// Starts a read unless one is pending. `true` means a report was
    /// already waiting in the HID class driver and is complete.
    fn start(&mut self) -> io::Result<bool> {
        if self.pending {
            return Ok(false);
        }
        // ReadFile resets the event when it starts the read.
        // https://learn.microsoft.com/windows/win32/api/fileapi/nf-fileapi-readfile
        self.operation = OVERLAPPED {
            hEvent: self.event.raw(),
            ..Default::default()
        };
        if let Some(winusb) = &self.winusb {
            if unsafe { ResetEvent(self.event.raw()) } == 0 { return Err(io::Error::last_os_error()); }
            let immediate = unsafe { winusb.read(&mut self.buffer, &self.operation) }?;
            self.pending = !immediate;
            return Ok(immediate);
        }
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
            return Ok(true);
        }
        let error = io::Error::last_os_error();
        if error.raw_os_error() != Some(ERROR_IO_PENDING as i32) {
            return Err(error);
        }
        self.pending = true;
        Ok(false)
    }

    /// The length of a completed read, or `None` if it was cancelled.
    fn finish(&mut self) -> io::Result<Option<usize>> {
        self.pending = false;
        let mut transferred = 0u32;
        let completion = if let Some(winusb) = &self.winusb {
            winusb.completed(&self.operation, false).map(|length| { transferred = length; })
        } else if unsafe { GetOverlappedResult(self.handle.raw(), &self.operation, &mut transferred, 0) } == 0 {
            Err(io::Error::last_os_error())
        } else { Ok(()) };
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
        if !self.pending {
            return;
        }
        if let Some(winusb) = &self.winusb {
            winusb.cancel(&self.operation);
            self.pending = false;
            return;
        }
        unsafe { CancelIoEx(self.handle.raw(), &self.operation) };
        let mut ignored = 0;
        // Cancellation is only a request; wait before reusing the buffer.
        unsafe { GetOverlappedResult(self.handle.raw(), &self.operation, &mut ignored, 1) };
        self.pending = false;
    }

    fn initialize(&self, candidate: &Candidate, identifier: &DeviceIdentifier,
        configuration: &otd_core::tablets::TabletConfiguration, stop: &Event) -> io::Result<()> {
        if let Some(winusb) = &self.winusb { winusb.initialize(candidate, identifier, configuration, stop) }
        else { hid::initialize(candidate, &self.handle, identifier, configuration, stop) }
    }
}

impl Drop for Reader {
    fn drop(&mut self) {
        self.cancel();
    }
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
        let pen = Reader::open(candidate, &selected.identifier, driver_access)?;
        let auxiliary = selected.auxiliary.as_ref().and_then(|(endpoint, identifier)| {
            Reader::open(endpoint, identifier, driver_access)
                .inspect_err(|error| {
                    eprintln!("Auxiliary collection not opened; express keys and wheels do nothing: {error}");
                })
                .ok()
        });
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
    fn auxiliary_decoder(&mut self, selected: &SelectedDevice<'_>, prefer_original: bool) -> io::Result<Option<RuntimeDecoder>> {
        let Some((_, identifier)) = selected.auxiliary.as_ref() else { return Ok(None); };
        if self.auxiliary.is_none() { return Ok(None); }
        if !hid::parser_supported(identifier.parser()) && !prefer_original {
            crate::plugins::load_parser_registry().map_err(io::Error::other)?;
        }
        RuntimeDecoder::for_graph(identifier.parser(), selected.spec, prefer_original).map(Some).map_err(io::Error::other)
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

    /// A synchronous ReadFile completion never enters the multi-event wait.
    /// Recheck Stop before exposing it so a queued stream cannot starve Stop.
    /// An asynchronous completion came from that wait, where Stop has
    /// precedence, so it needs no second check.
    fn pen_report(&mut self, queued: bool) -> io::Result<Read<'_>> {
        let ready = Instant::now();
        let Some(length) = self.pen.finish()? else {
            return Ok(Read::Ended);
        };
        if queued && self.stopped()? {
            return Ok(Read::Ended);
        }
        Ok(Read::Report {
            bytes: &self.pen.buffer[..length],
            ready,
            queued,
        })
    }

    fn auxiliary_report(&mut self, queued: bool) -> io::Result<Read<'_>> {
        let ready = Instant::now();
        let finished = self.auxiliary.as_mut().map(Reader::finish);
        match finished {
            Some(Ok(Some(length))) => {
                if queued && self.stopped()? {
                    return Ok(Read::Ended);
                }
                match &self.auxiliary {
                    Some(reader) => Ok(Read::Auxiliary {
                        bytes: &reader.buffer[..length],
                        ready,
                        queued,
                    }),
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
    fn shared_output(&self) -> bool {
        true
    }

    fn label(&self) -> &str {
        &self.label
    }

    fn now(&self) -> Instant {
        Instant::now()
    }

    // Each system call here costs about 0.2 us per report. Stop needs no
    // check of its own: the wait below sees it first, and a synchronous
    // completion checks it in `pen_report`/`auxiliary_report`.
    fn next(&mut self, timeout: Duration) -> io::Result<Read<'_>> {
        if self.pen.start()? {
            // The report was already waiting in the HID class driver.
            return self.pen_report(true);
        }
        if let Some(auxiliary) = &mut self.auxiliary {
            match auxiliary.start() {
                Ok(true) => return self.auxiliary_report(true),
                Ok(false) => {}
                Err(error) => return Ok(self.lose_auxiliary(error.to_string())),
            }
        }
        let until = Instant::now() + timeout;
        // An already-due deadline is an input/stop poll, not a new timer wakeup.
        let precise = !timeout.is_zero() && timeout < PRECISE_WAIT;
        let timer = if precise {
            self.arm_timer(timeout)?
        } else {
            ptr::null_mut()
        };
        // Stop first, so it takes precedence when several are signaled. The
        // rare auxiliary reports come before the pen's, so a pen stream
        // cannot starve them.
        let mut handles = [ptr::null_mut(); 5];
        let mut count = 0;
        let mut add = |handle: HANDLE| {
            handles[count] = handle;
            count += 1;
            count - 1
        };
        add(self.stop.raw());
        let auxiliary = self
            .auxiliary
            .as_ref()
            .map(|reader| add(reader.event.raw()));
        let pen = add(self.pen.event.raw());
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
                Ok(Some(index)) if index == pen => return self.pen_report(false),
                Ok(Some(index)) if Some(index) == auxiliary => {
                    return self.auxiliary_report(false);
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
    let prefer_original = matches!(mode, Mode::Driver) && plugins.needs_concrete_reports();
    let mut decoder = selected.decoder_for_graph(prefer_original)?;
    let mut auxiliary = source.auxiliary_decoder(selected, prefer_original)?;
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
    let pen = pen_device(&profile, mode)?;
    let actions = action_sink(mode)?.map(|sink| plugins.wrap_action_sink(&profile, &selected.configuration, sink))
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
        Ok(Self {
            source: HidSource::open(selected, notification, interrupt, true)?,
            selected,
        })
    }

    pub fn identifiers(&self) -> Vec<DeviceIdentifier> {
        self.source.identifiers(self.selected)
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
        for reader in std::iter::once(&self.source.pen)
            .chain(self.source.auxiliary.iter())
        {
            if let Some(interface) = &reader.winusb {
                interface.flush_input()?;
                continue;
            }
            if !unsafe {
                windows_sys::Win32::Devices::HumanInterfaceDevice::HidD_FlushQueue(reader.handle.raw())
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
        gate: impl FnOnce() -> io::Result<bool>,
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
        let prefer_original = plugins.needs_concrete_reports();
        let mut decoder = self.selected.decoder_for_graph(prefer_original)?;
        let mut auxiliary = source.auxiliary_decoder(self.selected, prefer_original)?;
        if decoder.is_managed() || auxiliary.as_ref().is_some_and(RuntimeDecoder::is_managed) {
            plugins.prepare_managed_decoder().map_err(io::Error::other)?;
        }
        let _debug = DebugDevice::set(self.selected, &source);
        announce_auxiliary(&source, self.selected, status);
        let _priority = ReaderPriority::for_driver(crate::experimental::mmcss_enabled(), status);
        let mut output = SessionOutput::new()?;
        let pen = pen_device(&profile, Mode::Driver)?;
        let actions = action_sink(Mode::Driver)?.map(|sink| plugins.wrap_action_sink(&profile, &self.selected.configuration, sink))
            .transpose().map_err(io::Error::other)?;
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
