//! Headless driver ownership. Control traffic never runs on the report thread.
use crate::control::{self, Command, ControlStatus, Reply, Request, WorkerIdentity};
use std::os::windows::process::CommandExt;
use std::process::{Command as ProcessCommand, Stdio};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};
mod state;
use state::Daemon;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

/// Own resources until all driver output cleanup finishes.
pub fn serve() -> Result<(), String> {
    println!(
        "Daemon control endpoint: {}",
        control::endpoint_name().map_err(|error| error.to_string())?
    );
    let cancelled = Arc::new(AtomicBool::new(false));
    let signal = Arc::clone(&cancelled);
    ctrlc::set_handler(move || {
        signal.store(true, Ordering::Release);
        control::wake();
    })
    .map_err(|error| error.to_string())?;
    let mut daemon = Daemon::new(Arc::clone(&cancelled));
    if let Err(error) = crate::experimental::apply_saved(false) {
        daemon.scheduling_warning(error);
    }
    let result = control::serve(&mut daemon, &cancelled).map_err(|error| error.to_string());
    let cleanup = daemon.cleanup();
    match (result, cleanup) {
        (Err(error), Err(cleanup)) => Err(format!("{error}; cleanup also failed: {cleanup}")),
        (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
        (Ok(()), Ok(())) => Ok(()),
    }
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

fn current_identity() -> Result<WorkerIdentity, String> {
    match call(Command::Status)? {
        Reply::Status { status } => Ok(status.identity()),
        _ => Err("unexpected daemon status response".into()),
    }
}

/// Fetch a coherent configuration snapshot without starting a daemon or worker.
pub fn configuration() -> Result<Reply, String> {
    call(Command::GetConfiguration {
        expected: current_identity()?,
    })
}

/// Guard the whole read/replace sequence against another client's lifecycle
/// changes. A conflict is returned to the caller instead of retried implicitly.
pub fn restart(replacement: Option<String>) -> Result<Reply, String> {
    let expected = current_identity()?;
    let profile_toml = match replacement {
        Some(profile) => profile,
        None => match call(Command::GetConfiguration {
            expected: expected.clone(),
        })? {
            Reply::Configuration {
                identity,
                profile_toml,
            } if identity == expected => {
                profile_toml.ok_or("daemon has no saved worker configuration; specify --config")?
            }
            _ => return Err("unexpected daemon configuration response".into()),
        },
    };
    call(Command::Restart {
        expected,
        profile_toml,
    })
}

/// Invoked only by an explicit CLI command. No console window is created.
pub fn background() -> Result<(), String> {
    let status = ensure_running(&AtomicBool::new(false))?;
    print_reply(&Reply::Status { status })
}

/// Ensure GUI and daemon run as separate processes. Reuse an existing service
/// without replacing its worker or profile; tablet input is a separate request.
pub fn ensure_running(cancelled: &AtomicBool) -> Result<ControlStatus, String> {
    let get_status = || -> Result<ControlStatus, std::io::Error> {
        let response = control::request(
            &Request::new(1, Command::Status),
            Duration::from_millis(750),
        )?;
        match response.reply {
            Reply::Status { status } => Ok(status),
            Reply::Error { error } => Err(std::io::Error::other(error.message)),
            _ => Err(std::io::Error::other("unexpected daemon status response")),
        }
    };
    match get_status() {
        Ok(status) => return Ok(status),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(format!(
                "Cannot connect to existing daemon: {error}; query status before retrying."
            ));
        }
    }
    if control::legacy_service_present().map_err(|error| error.to_string())? {
        return Err("A protocol-v1 daemon is present. Stop and shut it down with its original executable before starting this version; it was left untouched.".into());
    }
    if cancelled.load(Ordering::Acquire) {
        return Err("daemon launch cancelled".into());
    }
    // Unit-test pipe names are isolated, but the sibling release executable
    // would own the real user's endpoint. Never launch it from a test binary.
    if cfg!(test) {
        return Err("Unit tests cannot launch a live driver daemon; provide a fake test endpoint.".into());
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
        if cancelled.load(Ordering::Acquire) {
            return Err(
                "UI detached while daemon was starting; query status before another start.".into(),
            );
        }
        if let Ok(status) = get_status() {
            return Ok(status);
        }
        if let Some(code) = child.try_wait().map_err(|error| error.to_string())? {
            // Another client may have won the launch race and owns the endpoint.
            if let Ok(status) = get_status() {
                return Ok(status);
            }
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
