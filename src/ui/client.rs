//! Background daemon client. The UI only enqueues commands and drains bounded
//! snapshots; pipe calls, profile serialization and daemon launch stay here.
use crate::config::Profile;
use crate::control::{self, Command, ControlStatus, DriverState, Reply, Request, WorkerIdentity};
use std::io;
use std::path::Path;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver, SyncSender, TrySendError},
};
use std::time::Duration;
use windows_sys::Win32::Foundation::{HWND, WAIT_OBJECT_0};
use windows_sys::Win32::System::Threading::{
    GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
    WaitForSingleObject,
};
use windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW;

pub(super) enum ClientCommand {
    Experimental(crate::experimental::Settings),
    /// Attach if a worker already exists; otherwise start with the supplied profile.
    Start(Box<Profile>),
    /// Launch preference only: never revive a worker this panel already observed.
    AutoStart(Box<Profile>),
    /// Stop the current worker and wait for input cleanup before closing the panel.
    Close,
    Restart {
        expected: WorkerIdentity,
        profile: Box<Profile>,
    },
}

pub(super) enum ClientEvent {
    DaemonReady(Result<(), String>),
    Snapshot {
        status: ControlStatus,
        profile: Option<Box<Profile>>,
    },
    Offline(Option<String>),
    ActionFinished(Result<(), String>),
    CloseFinished(Result<Option<String>, String>),
    /// The daemon process ended with a failure code or left a crash record.
    DaemonExited(String),
}

pub(super) struct DaemonClient {
    commands: SyncSender<ClientCommand>,
    events: Receiver<ClientEvent>,
    cancelled: Arc<AtomicBool>,
}

impl DaemonClient {
    pub fn new(window: HWND) -> Result<Self, String> {
        let (commands, receiver) = mpsc::sync_channel(4);
        let (sender, events) = mpsc::sync_channel(8);
        let cancelled = Arc::new(AtomicBool::new(false));
        let stop = Arc::clone(&cancelled);
        let window = window as isize;
        std::thread::Builder::new()
            .name("daemon-ui-client".into())
            .spawn(move || {
                run(receiver, sender, stop, window);
            })
            .map_err(|error| error.to_string())?;
        Ok(Self {
            commands,
            events,
            cancelled,
        })
    }

    pub fn submit(&self, command: ClientCommand) -> Result<(), String> {
        self.commands
            .try_send(command)
            .map_err(|error| match error {
                TrySendError::Full(_) => {
                    "Daemon control queue is full; wait for the current request.".into()
                }
                TrySendError::Disconnected(_) => {
                    "Daemon client is no longer available; reopen the panel.".into()
                }
            })
    }

    pub fn drain(&self) -> Vec<ClientEvent> {
        self.events.try_iter().collect()
    }
}

impl Drop for DaemonClient {
    fn drop(&mut self) {
        // Normal panel close has already waited for cleanup on this worker.
        // Never join a pipe call or send an implicit command from Drop.
        self.cancelled.store(true, Ordering::Release);
    }
}

fn call(command: Command) -> io::Result<Reply> {
    let response = control::request(&Request::new(1, command), Duration::from_secs(5))?;
    match response.reply {
        Reply::Error { error } => Err(io::Error::other(format!(
            "{:?}: {}",
            error.code, error.message
        ))),
        reply => Ok(reply),
    }
}

fn profile_text(profile: &Profile) -> Result<String, String> {
    let text = profile.to_toml()?;
    if text.len() > control::MAX_PROFILE_BYTES {
        return Err(
            "Profile exceeds the daemon's 128 KiB limit; use foreground run for this profile."
                .into(),
        );
    }
    Ok(text)
}

fn execute(
    command: ClientCommand,
    cancelled: &AtomicBool,
    observed_active: &mut bool,
) -> Result<(), String> {
    if cancelled.load(Ordering::Acquire) {
        return Err("Panel detached before sending command.".into());
    }
    if let ClientCommand::Experimental(settings) = &command {
        crate::experimental::validate(settings)?;
        let status = match call(Command::Status).map_err(|error| error.to_string())? {
            Reply::Status { status } => status,
            _ => return Err("Unexpected daemon status response.".into()),
        };
        match call(Command::SetExperimental { expected: status.identity(), settings: settings.clone() })
            .map_err(|error| format!("{error}. Reopen Experimental settings to check saved values before retrying."))? {
            Reply::ExperimentalSaved => {}
            _ => return Err("Unexpected experimental settings response.".into()),
        }
        return crate::experimental::apply(&settings.ui_cpus).map_err(|error|
            format!("Settings were saved and driver affinity was applied, but GUI affinity failed: {error}. Reopen the panel to retry the saved GUI selection."));
    }
    if matches!(&command, ClientCommand::AutoStart(_)) && *observed_active {
        return Ok(());
    }
    let wire = match command {
        ClientCommand::Start(profile) | ClientCommand::AutoStart(profile) => {
            let text = profile_text(&profile)?;
            let status = crate::daemon::ensure_running(cancelled)?;
            if active(status.state) {
                *observed_active = true;
                return Ok(());
            }
            Command::StartIf {
                expected: status.identity(),
                profile_toml: text,
            }
        }
        ClientCommand::Close => return Err("Close must wait for the watched daemon on the client thread.".into()),
        ClientCommand::Experimental(_) => return Err("Experimental settings were not handled.".into()),
        ClientCommand::Restart { expected, profile } => Command::Restart {
            expected,
            profile_toml: profile_text(&profile)?,
        },
    };
    if cancelled.load(Ordering::Acquire) {
        return Err("Panel detached before sending command.".into());
    }
    call(wire).map(|_| ()).map_err(|error| {
        format!("{error}. Status will refresh; do not blindly retry a timed-out command.")
    })
}

pub(super) fn active(state: DriverState) -> bool {
    matches!(
        state,
        DriverState::Starting | DriverState::Running | DriverState::Stopping
    )
}

fn close_call(command: Command) -> io::Result<Reply> {
    let response = control::request(&Request::new(1, command), Duration::from_secs(1))?;
    match response.reply {
        Reply::Error { error } => Err(io::Error::other(format!("{:?}: {}", error.code, error.message))),
        reply => Ok(reply),
    }
}

/// Read the identity after earlier queued actions finish, so a pending Start
/// cannot leave input running after the panel goes away. StopIf protects a
/// worker replaced between this snapshot and the stop request.
fn stop_for_close(cancelled: &AtomicBool, watched: Option<&Watched>) -> Result<Option<String>, String> {
    let status = match close_call(Command::Status) {
        Ok(Reply::Status { status }) => status,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            // Shutdown can remove the pipe before worker cleanup finishes.
            if watched.is_some_and(|watched| watched.exit_code().is_none()) {
                return Err("The daemon is shutting down. The panel stayed open until the driver has stopped; close it again after shutdown finishes.".into());
            }
            return Ok(None);
        }
        Err(error) => return Err(format!("Cannot confirm driver shutdown: {error}")),
        _ => return Err("Unexpected daemon status response while closing.".into()),
    };
    if !active(status.state) {
        return Ok((status.state == DriverState::Failed).then(|| status.last_error.unwrap_or_else(|| "The driver stopped with a cleanup error.".into())));
    }
    let instance = status.instance.clone();
    let process = Watched::open(&instance);
    if cancelled.load(Ordering::Acquire) {
        return Err("Panel closed before input cleanup was confirmed.".into());
    }
    // A timeout can occur after Stop was accepted. Confirm status rather than
    // resending a potentially stale command.
    let stop_error = match close_call(Command::StopIf { expected: status.identity() }) {
        Ok(Reply::Stopped { .. }) | Ok(Reply::Status { .. }) => None,
        Ok(_) => return Err("Unexpected daemon stop response while closing.".into()),
        Err(error) => Some(error.to_string()),
    };
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while !cancelled.load(Ordering::Acquire) && std::time::Instant::now() < deadline {
        match close_call(Command::Status) {
            Ok(Reply::Status { status }) if status.instance == instance => {
                if !active(status.state) {
                    return Ok((status.state == DriverState::Failed).then(|| status.last_error.unwrap_or_else(|| "The driver stopped with a cleanup error.".into())));
                }
            }
            Ok(Reply::Status { .. }) => return Err("The daemon changed while closing. The panel stayed open; close it again to stop the current driver.".into()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                if process.as_ref().and_then(Watched::exit_code).is_some() {
                    return Ok(None);
                }
            }
            Err(_) => {}
            _ => return Err("Unexpected daemon status response while waiting for cleanup.".into()),
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Err(format!("Driver cleanup was not confirmed. The panel stayed open.{}", stop_error.map_or_else(String::new, |error| format!(" {error}"))))
}

fn publish(
    sender: &SyncSender<ClientEvent>,
    event: ClientEvent,
    window: isize,
    stop: &AtomicBool,
    reliable: bool,
) -> bool {
    let mut event = event;
    loop {
        if stop.load(Ordering::Acquire) {
            return false;
        }
        match sender.try_send(event) {
            Ok(()) => {
                unsafe {
                    PostMessageW(window as HWND, super::WM_DRIVER_STATUS, 0, 0);
                }
                return true;
            }
            Err(TrySendError::Full(returned)) if reliable => {
                event = returned;
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(_) => return false,
        }
    }
}

/// The daemon process this panel attached to. Its handle keeps the process ID
/// from being reused and yields the exit code, so a crash (a panic, or a
/// native fault inside a plugin) is told apart from an ordinary shutdown.
struct Watched {
    instance: String,
    pid: u32,
    /// When the daemon started, in seconds since the Unix epoch.
    started: u64,
    process: crate::hid::OwnedHandle,
}

impl Watched {
    /// Daemon instances are named `PID-NANOSECONDS`.
    fn open(instance: &str) -> Option<Self> {
        let (pid, started) = instance.split_once('-')?;
        let pid: u32 = pid.parse().ok()?;
        let started = started.parse::<u128>().ok()? / 1_000_000_000;
        let process = crate::hid::OwnedHandle::new(unsafe {
            OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
                0,
                pid,
            )
        })
        .ok()?;
        Some(Self {
            instance: instance.to_owned(),
            pid,
            started: u64::try_from(started).unwrap_or(u64::MAX),
            process,
        })
    }

    /// The exit code once the process has ended.
    fn exit_code(&self) -> Option<u32> {
        if unsafe { WaitForSingleObject(self.process.raw(), 0) } != WAIT_OBJECT_0 {
            return None;
        }
        let mut code = 0;
        (unsafe { GetExitCodeProcess(self.process.raw(), &mut code) } != 0).then_some(code)
    }
}

/// What the console says about a daemon that ended with `code`; nothing for
/// an ordinary shutdown.
pub(super) fn exit_message(
    code: u32,
    crash: Option<&otd_core::crash::CrashRecord>,
) -> Option<String> {
    if code == 0 && crash.is_none() {
        return None;
    }
    let meaning = match code {
        0 => "",
        1 => " (fatal error)",
        0xC000_0005 => " (access violation)",
        0xC000_00FD => " (stack overflow)",
        0xC000_0409 => " (fail-fast; a Rust panic ends the process this way)",
        0xE043_4352 => " (unhandled .NET exception)",
        _ => "",
    };
    let mut message =
        format!("The driver daemon stopped unexpectedly with exit code 0x{code:08X}{meaning}.");
    match crash {
        Some(crash) => {
            message.push(' ');
            message.push_str(&crash.summary());
            message.push_str(" The record is in crash.log in the settings directory.");
        }
        None => message.push_str(
            " It left no crash record; Windows Event Viewer (Windows Logs > Application) names the faulting module.",
        ),
    }
    message.push_str(" Start driver launches it again.");
    Some(message)
}

fn run(
    commands: Receiver<ClientCommand>,
    events: SyncSender<ClientEvent>,
    stop: Arc<AtomicBool>,
    window: isize,
) {
    let mut configured: Option<WorkerIdentity> = None;
    let mut previous_error: Option<String> = None;
    let mut was_online = true;
    // Retain across disconnections and daemon replacement: a queued launch
    // preference must not undo an explicit stop/shutdown from another client.
    let mut observed_active = false;
    let mut watched: Option<Watched> = None;
    let ready = crate::daemon::ensure_running(&stop).map(|status| {
        observed_active = active(status.state);
    });
    if !publish(&events, ClientEvent::DaemonReady(ready), window, &stop, true) {
        return;
    }
    while !stop.load(Ordering::Acquire) {
        let snapshot = (|| -> io::Result<(ControlStatus, Option<Box<Profile>>)> {
            let Reply::Status { status } = call(Command::Status)? else {
                return Err(io::Error::other("Unexpected daemon status response"));
            };
            observed_active |= active(status.state);
            let identity = status.identity();
            let profile = if active(status.state) && configured.as_ref() != Some(&identity) {
                match call(Command::GetConfiguration {
                    expected: identity.clone(),
                })? {
                    Reply::Configuration {
                        identity: returned,
                        profile_toml: Some(text),
                    } if returned == identity => Some(Box::new(
                        Profile::from_toml_text(&text, Path::new("daemon-active.toml"))
                            .map_err(io::Error::other)?,
                    )),
                    _ => {
                        return Err(io::Error::other(
                            "Active daemon configuration is unavailable; local editor was not replaced",
                        ));
                    }
                }
            } else {
                None
            };
            Ok((status, profile))
        })();
        match snapshot {
            Ok((status, profile)) => {
                if watched
                    .as_ref()
                    .is_none_or(|watched| watched.instance != status.instance)
                {
                    watched = Watched::open(&status.instance);
                }
                let identity = status.identity();
                let loaded = profile.is_some();
                if publish(
                    &events,
                    ClientEvent::Snapshot { status, profile },
                    window,
                    &stop,
                    false,
                ) {
                    if loaded {
                        configured = Some(identity);
                    }
                    was_online = true;
                    previous_error = None;
                }
            }
            Err(error) => {
                // Report a daemon that ended badly once, before going offline.
                if let Some(code) = watched.as_ref().and_then(Watched::exit_code) {
                    let ended = watched.take();
                    let crash = ended
                        .and_then(|ended| otd_core::crash::latest_for(ended.pid, ended.started));
                    if let Some(message) = exit_message(code, crash.as_ref()) {
                        publish(
                            &events,
                            ClientEvent::DaemonExited(message),
                            window,
                            &stop,
                            true,
                        );
                    }
                }
                let message = if error.kind() == io::ErrorKind::NotFound {
                    None
                } else {
                    Some(error.to_string())
                };
                if (was_online || previous_error != message)
                    && publish(
                        &events,
                        ClientEvent::Offline(message.clone()),
                        window,
                        &stop,
                        false,
                    )
                {
                    was_online = false;
                    previous_error = message;
                    configured = None;
                }
            }
        }
        match commands.recv_timeout(Duration::from_millis(500)) {
            Ok(command) => {
                let closing = matches!(&command, ClientCommand::Close);
                let event = if closing {
                    ClientEvent::CloseFinished(stop_for_close(&stop, watched.as_ref()))
                } else {
                    ClientEvent::ActionFinished(execute(command, &stop, &mut observed_active))
                };
                if !publish(
                    &events,
                    event,
                    window,
                    &stop,
                    true,
                ) {
                    break;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_failed_daemon_exits_are_reported() {
        assert_eq!(exit_message(0, None), None);
        let fault = exit_message(0xC000_0005, None).unwrap();
        assert!(fault.contains("0xC0000005 (access violation)"), "{fault}");
        assert!(fault.contains("Event Viewer"), "{fault}");
        let mut record = otd_core::crash::CrashRecord::new("0.14.0", "daemon", "panic", "boom");
        record.thread = Some("tablet-driver".into());
        record.location = Some("src/x.rs:1:1".into());
        let panic = exit_message(0xC000_0409, Some(&record)).unwrap();
        assert!(panic.contains("Rust panic"), "{panic}");
        assert!(
            panic
                .contains("daemon 0.14.0 panicked on thread 'tablet-driver' at src/x.rs:1:1: boom"),
            "{panic}"
        );
        // A clean exit with a recorded fatal error is still reported.
        assert!(exit_message(0, Some(&record)).is_some());
    }

    #[test]
    fn watched_instances_need_a_process_id_and_start_time() {
        assert!(Watched::open("not-an-instance").is_none());
        assert!(Watched::open("123").is_none());
        let own = format!("{}-{}", std::process::id(), 5_000_000_000u64);
        let watched = Watched::open(&own).expect("this process can be opened");
        assert_eq!((watched.pid, watched.started), (std::process::id(), 5));
        assert_eq!(
            watched.exit_code(),
            None,
            "a running process has no exit code"
        );
    }

    #[test]
    fn panel_launch_attaches_to_an_idle_daemon_without_starting_input() {
        use crate::control::{ControlError, ControlHandler};
        use std::sync::atomic::AtomicUsize;
        struct Fake { calls: Arc<AtomicUsize> }
        impl ControlHandler for Fake {
            fn poll(&mut self) {}
            fn handle(&mut self, command: Command) -> Result<Reply, ControlError> {
                assert!(matches!(command, Command::Status), "attachment must not start or replace input");
                self.calls.fetch_add(1, Ordering::SeqCst);
                Ok(Reply::Status { status: ControlStatus { instance: "fake-idle-daemon".into(),
                    state: DriverState::Stopped, generation: 0, profile: None, last_error: None,
                    logs: Vec::new(), log_sequence: 0 } })
            }
        }
        // cfg(test) endpoints include this test process ID, so this server and
        // client cannot connect to or replace the user's active daemon.
        let calls = Arc::new(AtomicUsize::new(0));
        let cancelled = Arc::new(AtomicBool::new(false));
        let server = {
            let (calls,cancelled) = (calls.clone(),cancelled.clone());
            std::thread::spawn(move || control::serve(&mut Fake { calls }, &cancelled))
        };
        for _ in 0..100 {
            if call(Command::Status).is_ok() { break; }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(calls.load(Ordering::SeqCst) > 0, "fake daemon did not become ready");
        let client = DaemonClient::new(std::ptr::null_mut()).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let mut ready = false;
        while std::time::Instant::now() < deadline {
            for event in client.drain() {
                if let ClientEvent::DaemonReady(result) = event { result.unwrap(); ready = true; }
            }
            if ready { break; }
            std::thread::sleep(Duration::from_millis(10));
        }
        drop(client);
        cancelled.store(true,Ordering::Release);
        control::wake();
        server.join().unwrap().unwrap();
        assert!(ready, "normal client creation must ensure daemon readiness even without AutoStart");
        assert!(calls.load(Ordering::SeqCst) >= 2);
    }
}
