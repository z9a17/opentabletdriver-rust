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
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW;

pub(super) enum ClientCommand {
    /// Attach if a worker already exists; otherwise start with the supplied profile.
    Start(Box<Profile>),
    Stop(WorkerIdentity),
    Restart {
        expected: WorkerIdentity,
        profile: Box<Profile>,
    },
}

pub(super) enum ClientEvent {
    Snapshot {
        status: ControlStatus,
        profile: Option<Box<Profile>>,
    },
    Offline(Option<String>),
    ActionFinished(Result<(), String>),
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
        // Detach only. Never stop input, join a pipe call on the UI thread, or
        // cancel a restart that the daemon has already accepted.
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

fn execute(command: ClientCommand, cancelled: &AtomicBool) -> Result<(), String> {
    let wire = match command {
        ClientCommand::Start(profile) => {
            let text = profile_text(&profile)?;
            let status = crate::daemon::ensure_running(cancelled)?;
            if active(status.state) {
                return Ok(());
            }
            Command::StartIf {
                expected: status.identity(),
                profile_toml: text,
            }
        }
        ClientCommand::Stop(expected) => Command::StopIf { expected },
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

fn run(
    commands: Receiver<ClientCommand>,
    events: SyncSender<ClientEvent>,
    stop: Arc<AtomicBool>,
    window: isize,
) {
    let mut configured: Option<WorkerIdentity> = None;
    let mut previous_error: Option<String> = None;
    let mut was_online = true;
    while !stop.load(Ordering::Acquire) {
        let snapshot = (|| -> io::Result<(ControlStatus, Option<Box<Profile>>)> {
            let Reply::Status { status } = call(Command::Status)? else {
                return Err(io::Error::other("Unexpected daemon status response"));
            };
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
                let result = execute(command, &stop);
                if !publish(
                    &events,
                    ClientEvent::ActionFinished(result),
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
