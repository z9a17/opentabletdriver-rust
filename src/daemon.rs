//! Headless driver ownership. Control traffic never runs on the report thread.
use std::collections::VecDeque;
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::{Command as ProcessCommand, Stdio};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver},
};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::config::Profile;
use crate::control::{
    self, Command, ControlError, ControlHandler, ControlStatus, DriverState, ErrorCode, Reply,
    Request,
};
use crate::hid::Event;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

struct Worker {
    stop: Event,
    thread: JoinHandle<Result<(), String>>,
    messages: Receiver<String>,
}

struct Daemon {
    worker: Option<Worker>,
    state: DriverState,
    generation: u64,
    profile: Option<String>,
    last_error: Option<String>,
    logs: VecDeque<String>,
}

impl Daemon {
    fn new() -> Self {
        Self {
            worker: None,
            state: DriverState::Stopped,
            generation: 0,
            profile: None,
            last_error: None,
            logs: VecDeque::new(),
        }
    }

    fn log(&mut self, mut line: String) {
        if line.len() > control::MAX_LOG_LINE_BYTES {
            let mut end = control::MAX_LOG_LINE_BYTES;
            while !line.is_char_boundary(end) {
                end -= 1;
            }
            line.truncate(end);
        }
        if self.logs.len() == control::MAX_LOG_LINES {
            self.logs.pop_front();
        }
        self.logs.push_back(line);
    }

    fn status(&self) -> Reply {
        Reply::Status {
            status: ControlStatus {
                state: self.state,
                generation: self.generation,
                profile: self.profile.clone(),
                last_error: self.last_error.clone(),
                logs: self.logs.iter().cloned().collect(),
            },
        }
    }

    fn signal_stop(&mut self) -> Result<(), ControlError> {
        if let Some(worker) = &self.worker {
            worker
                .stop
                .signal()
                .map_err(|error| ControlError::new(ErrorCode::StopFailed, error.to_string()))?;
            self.state = DriverState::Stopping;
        }
        Ok(())
    }

    fn finished(&mut self, result: std::thread::Result<Result<(), String>>) {
        let result = result.unwrap_or_else(|_| Err("driver worker panicked".into()));
        match result {
            Ok(()) => {
                self.state = DriverState::Stopped;
                self.log("Driver stopped; cleanup completed.".into());
            }
            Err(error) => {
                self.state = DriverState::Failed;
                self.last_error = Some(error.clone());
                self.log(error);
            }
        }
    }

    fn cleanup(&mut self) -> Result<(), String> {
        self.signal_stop().map_err(|error| error.message)?;
        if let Some(worker) = self.worker.take() {
            self.finished(worker.thread.join());
        }
        if matches!(self.state, DriverState::Failed) {
            Err(self
                .last_error
                .clone()
                .unwrap_or_else(|| "driver cleanup failed".into()))
        } else {
            Ok(())
        }
    }
}

impl ControlHandler for Daemon {
    fn poll(&mut self) {
        // This queue is bounded and status-only. Pen reports never enter it.
        let messages: Vec<_> = self
            .worker
            .as_ref()
            .map(|worker| worker.messages.try_iter().collect())
            .unwrap_or_default();
        for message in messages {
            if matches!(self.state, DriverState::Starting) {
                self.state = DriverState::Running;
            }
            self.log(message);
        }
        if self
            .worker
            .as_ref()
            .is_some_and(|worker| worker.thread.is_finished())
        {
            let worker = self.worker.take().expect("finished worker exists");
            self.finished(worker.thread.join());
        }
    }

    fn handle(&mut self, command: Command) -> Result<Reply, ControlError> {
        self.poll();
        match command {
            Command::Status => Ok(self.status()),
            Command::Start { profile_toml } => {
                if self.worker.is_some() {
                    return Err(ControlError::new(
                        ErrorCode::Busy,
                        "driver is already active; stop it before starting another profile",
                    ));
                }
                let profile = match profile_toml {
                    Some(text) => Profile::from_toml_text(&text, Path::new("daemon-request.toml")),
                    None => Profile::load(None),
                }
                .and_then(|profile| {
                    profile.validate_runtime_tablet("Wacom PTH-660")?;
                    profile.validate_filter_execution()?;
                    Ok(profile)
                })
                .map_err(|error| ControlError::new(ErrorCode::InvalidProfile, error))?;
                let generation = self.generation.checked_add(1).ok_or_else(|| {
                    ControlError::new(ErrorCode::Internal, "generation counter exhausted")
                })?;
                let stop = Event::create(true).map_err(|error| {
                    ControlError::new(ErrorCode::StartFailed, error.to_string())
                })?;
                let worker_stop = stop.duplicate().map_err(|error| {
                    ControlError::new(ErrorCode::StartFailed, error.to_string())
                })?;
                let profile_name = profile.source.clone();
                let (sender, messages) = mpsc::sync_channel(control::MAX_LOG_LINES);
                let thread = std::thread::Builder::new()
                    .name("tablet-driver".into())
                    .spawn(move || {
                        crate::drive(profile, &worker_stop, None, |message| {
                            let _ = sender.try_send(message.to_owned());
                        })
                    })
                    .map_err(|error| {
                        ControlError::new(ErrorCode::StartFailed, error.to_string())
                    })?;
                self.generation = generation;
                self.profile = Some(profile_name);
                self.last_error = None;
                self.logs.clear();
                self.state = DriverState::Starting;
                self.worker = Some(Worker {
                    stop,
                    thread,
                    messages,
                });
                self.log("Driver start accepted; query status for startup failures.".into());
                Ok(Reply::Started { generation })
            }
            Command::Stop => {
                self.signal_stop()?;
                self.poll();
                if self.worker.is_some() || matches!(self.state, DriverState::Failed) {
                    Ok(self.status())
                } else {
                    Ok(Reply::Stopped {
                        generation: self.generation,
                    })
                }
            }
            Command::Shutdown => {
                self.signal_stop()?;
                Ok(Reply::ShutdownAccepted)
            }
        }
    }
}

/// Own resources until driver cleanup and original-driver restoration finish.
pub fn serve() -> Result<(), String> {
    println!(
        "Daemon control endpoint: {}",
        control::endpoint_name().map_err(|error| error.to_string())?
    );
    let cancelled = Arc::new(AtomicBool::new(false));
    let signal = Arc::clone(&cancelled);
    ctrlc::set_handler(move || signal.store(true, Ordering::Release))
        .map_err(|error| error.to_string())?;
    let mut daemon = Daemon::new();
    let result = control::serve(&mut daemon, &cancelled).map_err(|error| error.to_string());
    let cleanup = daemon.cleanup();
    result.and(cleanup)
}

pub fn call(command: Command) -> Result<Reply, String> {
    let response =
        control::request(&Request::new(1, command), REQUEST_TIMEOUT).map_err(|error| {
            format!("daemon control failed: {error}; start it with 'daemon --background'")
        })?;
    checked_reply(response.reply)
}

fn checked_reply(reply: Reply) -> Result<Reply, String> {
    match reply {
        Reply::Error { error } => Err(format!("{:?}: {}", error.code, error.message)),
        reply => Ok(reply),
    }
}

pub fn print_reply(reply: &Reply) -> Result<(), String> {
    println!(
        "{}",
        serde_json::to_string_pretty(reply).map_err(|error| error.to_string())?
    );
    Ok(())
}

/// Invoked only by an explicit CLI command. No console window is created.
pub fn background() -> Result<(), String> {
    if let Ok(response) = control::request(
        &Request::new(1, Command::Status),
        Duration::from_millis(500),
    ) {
        return checked_reply(response.reply).and_then(|reply| print_reply(&reply));
    }
    let executable = std::env::current_exe()
        .map_err(|error| error.to_string())?
        .with_file_name("opentabletdriver-rust.exe");
    let mut child = ProcessCommand::new(executable)
        .arg("daemon")
        .creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("could not launch daemon: {error}"))?;
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(response) = control::request(
            &Request::new(1, Command::Status),
            Duration::from_millis(250),
        ) {
            return checked_reply(response.reply).and_then(|reply| print_reply(&reply));
        }
        if let Some(code) = child.try_wait().map_err(|error| error.to_string())? {
            return Err(format!(
                "daemon exited with {code}; run 'daemon' from a terminal to see its error"
            ));
        }
        if Instant::now() >= deadline {
            return Err(
                "daemon did not become ready within 10 seconds; query status before retrying"
                    .into(),
            );
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}
