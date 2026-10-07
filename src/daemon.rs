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
    serve_with_rpc(None)
}
pub fn serve_with_rpc(upstream_pipe: Option<&str>) -> Result<(), String> {
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
    let rpc = upstream_pipe.map(crate::upstream_rpc::Listener::start).transpose()
        .map_err(|error| format!("upstream RPC endpoint: {error}"))?;
    if let Some(name) = upstream_pipe { println!("Upstream compatibility endpoint: {name}"); }
    let mut daemon = Daemon::new(Arc::clone(&cancelled));
    let managed_services = crate::managed_host::Owner::start(daemon.identity(), Arc::clone(&cancelled))
        .map_err(|error| format!("managed service owner: {error}"))?;
    if let Err(error) = crate::experimental::apply_saved(false) {
        daemon.scheduling_warning(error);
    }
    let result = control::serve(&mut daemon, &cancelled).map_err(|error| error.to_string());
    drop(managed_services);
    let cleanup = daemon.cleanup();
    drop(rpc);
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

/// A coherent active profile snapshot, with plugin paths already resolved by
/// the daemon's loader. Saving it relocates those paths to the destination.
pub fn active_profile() -> Result<crate::config::Profile, String> {
    match configuration()? {
        Reply::Configuration { profile_toml: Some(text), .. } => {
            let path = otd_core::storage::data_directory()?.join("driver.toml");
            crate::config::Profile::from_toml_text(&text, &path)
        }
        Reply::Configuration { profile_toml: None, .. } => Err("daemon has no active configuration".into()),
        _ => Err("unexpected daemon configuration response".into()),
    }
}

/// Save an active snapshot atomically. Replacement is explicit and guarded by
/// the destination bytes observed before contacting the daemon.
pub fn save_configuration(path: &std::path::Path, replace: bool) -> Result<(), String> {
    use otd_core::storage::{self, SaveMode};
    let previous = storage::capture(path)?;
    if previous.exists() && !replace { return Err("destination exists; use --replace to replace it with a backup".into()); }
    let profile = active_profile()?;
    let text = profile.to_toml_at(path)?;
    let mode = if previous.exists() { SaveMode::Replace(&previous) } else { SaveMode::CreateNew };
    storage::save(path, text.as_bytes(), mode)?;
    println!("{}", serde_json::json!({"output": path, "settings_revision": profile.settings_revision, "replaced": previous.exists()}));
    Ok(())
}

/// Newline-delimited native protocol-v2 requests and responses. This is the
/// Rust control contract, not StreamJsonRpc. No daemon is launched implicitly.
pub fn stdio() -> Result<(), String> {
    stdio_with(std::io::stdin().lock(), std::io::stdout().lock(), |request| {
        control::request(request, REQUEST_TIMEOUT)
    })
}

fn stdio_with(
    mut input: impl std::io::BufRead,
    mut output: impl std::io::Write,
    mut call: impl FnMut(&Request) -> std::io::Result<control::Response>,
) -> Result<(), String> {
    use std::io::{BufRead, Read};
    loop {
        let mut line = Vec::new();
        let count = input.by_ref().take((control::MAX_FRAME_BYTES + 1) as u64)
            .read_until(b'\n', &mut line).map_err(|error| error.to_string())?;
        if count == 0 { return Ok(()); }
        if line.len() > control::MAX_FRAME_BYTES {
            return Err("stdio request exceeds 256 KiB; input closed without dispatch".into());
        }
        let response = match serde_json::from_slice::<Request>(&line) {
            Ok(request) => {
                // Validate here as well as in request(), before handing anything
                // to the transport (including a fake endpoint in regression tests).
                if let Err(error) = request.validate() {
                    let response = control::Response { version: control::PROTOCOL_VERSION, id: request.id, reply: Reply::Error { error } };
                    serde_json::to_writer(&mut output, &response).map_err(|error| error.to_string())?;
                    output.write_all(b"\n").map_err(|error| error.to_string())?;
                    output.flush().map_err(|error| error.to_string())?;
                    continue;
                }
                match call(&request) {
                    Ok(response) => response,
                    Err(error) => control::Response { version: control::PROTOCOL_VERSION, id: request.id,
                        reply: Reply::Error { error: control::ControlError::new(
                            if error.kind() == std::io::ErrorKind::InvalidInput { control::ErrorCode::InvalidRequest } else { control::ErrorCode::Internal },
                            error.to_string()) } },
                }
            }
            Err(error) => control::Response { version: control::PROTOCOL_VERSION, id: 0,
                reply: Reply::Error { error: control::ControlError::new(control::ErrorCode::InvalidRequest, error.to_string()) } },
        };
        serde_json::to_writer(&mut output, &response).map_err(|error| error.to_string())?;
        output.write_all(b"\n").map_err(|error| error.to_string())?;
        output.flush().map_err(|error| error.to_string())?;
    }
}

/// Invoked only by an explicit CLI command. No console window is created.
pub fn background() -> Result<(), String> {
    background_with_rpc(None)
}
pub fn background_with_rpc(upstream_pipe: Option<&str>) -> Result<(), String> {
    let status = ensure_running_with_rpc(&AtomicBool::new(false), upstream_pipe)?;
    print_reply(&Reply::Status { status })
}

/// Ensure GUI and daemon run as separate processes. Reuse an existing service
/// without replacing its worker or profile; tablet input is a separate request.
pub fn ensure_running(cancelled: &AtomicBool) -> Result<ControlStatus, String> {
    ensure_running_with_rpc(cancelled, None)
}
fn ensure_running_with_rpc(cancelled: &AtomicBool, upstream_pipe: Option<&str>) -> Result<ControlStatus, String> {
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
        Ok(status) if upstream_pipe.is_none() => return Ok(status),
        Ok(_) => return Err("A Rust daemon is already running; compatibility options cannot be changed on an existing daemon. Start a new idle daemon with the explicit options after shutting this one down.".into()),
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
    let mut launch = ProcessCommand::new(executable);
    launch.arg("daemon");
    if let Some(name) = upstream_pipe { launch.args(["--upstream-rpc", "--upstream-pipe", name]); }
    let mut child = launch
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
            let owns_endpoints = upstream_pipe.is_none_or(|name| {
                let owner = crate::control::pipe::CompatPipe::server_process_id;
                let native = control::endpoint_name().ok().and_then(|endpoint| owner(&endpoint).ok());
                let compat = owner(&format!(r"\\.\pipe\{name}")).ok();
                native == Some(child.id()) && compat == Some(child.id())
            });
            if owns_endpoints { return Ok(status); }
        }
        if let Some(code) = child.try_wait().map_err(|error| error.to_string())? {
            // Another client may have won the launch race and owns the endpoint.
            if upstream_pipe.is_none() {
                if let Ok(status) = get_status() { return Ok(status); }
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

#[cfg(test)]
mod stdio_tests {
    use super::*;

    #[test]
    fn stdio_rejects_malformed_and_invalid_requests_before_dispatch() {
        let mut bytes = b"not json\n".to_vec();
        bytes.extend(serde_json::to_vec(&Request::new(0, Command::Stop)).unwrap());
        bytes.push(b'\n');
        bytes.extend(serde_json::to_vec(&Request::new(7, Command::Status)).unwrap());
        bytes.push(b'\n');
        let mut output = Vec::new();
        let mut calls = 0;
        stdio_with(std::io::Cursor::new(bytes), &mut output, |request| {
            calls += 1;
            assert_eq!(request.id, 7);
            Ok(control::Response { version: control::PROTOCOL_VERSION, id: request.id, reply: Reply::ShutdownAccepted })
        }).unwrap();
        assert_eq!(calls, 1);
        let replies = output.split(|byte| *byte == b'\n').filter(|line| !line.is_empty())
            .map(|line| serde_json::from_slice::<control::Response>(line).unwrap()).collect::<Vec<_>>();
        assert_eq!(replies.len(), 3);
        assert!(matches!(replies[0].reply, Reply::Error { .. }));
        assert!(matches!(replies[1].reply, Reply::Error { .. }));
        assert_eq!(replies[2].id, 7);
    }

    #[test]
    fn stdio_bounds_unterminated_lines_without_dispatch() {
        let mut output = Vec::new();
        let oversized = vec![b' '; control::MAX_FRAME_BYTES + 1];
        assert!(stdio_with(std::io::Cursor::new(oversized), &mut output, |_| panic!("must not dispatch")).is_err());
        assert!(output.is_empty());
    }
}
