//! Guarded physical-device controls, separate from global driver shutdown.
use std::{path::Path, time::{Duration, Instant}};
use crate::{control::{self, Command, Reply, Request}, device_sessions::{SessionList, SessionState}};

fn call(command: Command) -> Result<Reply, String> {
    let response = control::request(&Request::new(1, command), Duration::from_secs(5))
        .map_err(|error| error.to_string())?;
    match response.reply { Reply::Error { error } => Err(format!("{:?}: {}", error.code, error.message)), reply => Ok(reply) }
}
fn sessions() -> Result<SessionList, String> {
    match call(Command::ListDeviceSessions)? {
        Reply::DeviceSessions { sessions, selected_id } => Ok(SessionList { sessions, selected_id }),
        _ => Err("unexpected device sessions response".into()),
    }
}
fn status() -> Result<control::ControlStatus, String> {
    match call(Command::Status)? { Reply::Status { status } => Ok(status), _ => Err("unexpected daemon status".into()) }
}

pub fn run(args: Vec<String>) -> Result<(), String> {
    let usage = "devices list | select ID | profile ID | apply ID PROFILE.toml | start ID | stop ID | save ID NEW_FILE.toml [--replace]";
    if args.len() == 1 && args[0] == "list" {
        println!("{}", serde_json::to_string_pretty(&sessions()?).map_err(|error| error.to_string())?);
        return Ok(());
    }
    if args.len() < 2 { return Err(usage.into()); }
    let action = args[0].as_str();
    let id = &args[1];
    let valid = match action {
        "select" | "profile" | "start" | "stop" => args.len() == 2,
        "apply" => args.len() == 3,
        "save" => args.len() == 3 || (args.len() == 4 && args[3] == "--replace"),
        _ => false,
    };
    if !valid { return Err(usage.into()); }
    let before = status()?;
    let list = sessions()?;
    let device = list.sessions.iter().find(|device| &device.id == id)
        .ok_or("unknown device ID; use devices list on the current daemon")?;
    if device.pending_generation.is_some() { return Err("device is changing state; refresh devices list".into()); }
    let expected = before.identity();
    let generation = device.device_generation;
    if action == "select" {
        match call(Command::SelectDeviceSession { expected, id: id.clone() })? {
            Reply::DeviceSessionSelected { id } => println!("Selected {id} for the tablet debugger."),
            _ => return Err("unexpected selection response".into()),
        }
        return Ok(());
    }
    if matches!(action, "profile" | "save") {
        let text = match call(Command::GetDeviceProfile { expected, id: id.clone(), device_generation: generation })? {
            Reply::DeviceProfile { identity, id: returned, device_generation, profile_toml }
                if identity == before.identity() && returned == *id && device_generation == generation => profile_toml,
            _ => return Err("device configuration changed while reading".into()),
        };
        if action == "profile" { print!("{text}"); }
        else {
            let destination = Path::new(&args[2]);
            let snapshot = otd_core::storage::capture(destination)?;
            let mode = if args.len() == 4 { otd_core::storage::SaveMode::Replace(&snapshot) }
                else { otd_core::storage::SaveMode::CreateNew };
            otd_core::storage::save(destination, text.as_bytes(), mode)?;
            println!("Saved {}. Active device settings were not changed.", destination.display());
        }
        return Ok(());
    }
    let command = match action {
        "start" => Command::StartDevice { expected, id: id.clone(), device_generation: generation },
        "stop" => Command::StopDevice { expected, id: id.clone(), device_generation: generation },
        "apply" => {
            let path = Path::new(&args[2]);
            let loaded = otd_core::storage::read_utf8(path)?;
            let profile = crate::config::Profile::from_toml_text(&loaded.text, path)?;
            Command::ApplyDeviceProfile { expected, id: id.clone(), device_generation: generation, profile_toml: profile.to_toml()? }
        },
        _ => unreachable!(),
    };
    let receipt = match call(command)? {
        Reply::DeviceOperationAccepted { receipt } if receipt.id == *id => receipt,
        _ => return Err("unexpected device operation response".into()),
    };
    if !receipt.accepted_pending {
        println!("Updated {id}, generation {}.", receipt.target_generation);
        return Ok(());
    }
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if status()?.instance != before.instance { return Err("daemon changed during operation; refresh devices list before retrying".into()); }
        let list = sessions()?;
        let current = list.sessions.iter().find(|device| &device.id == id).ok_or("device session was retired")?;
        if current.pending_generation.is_none() {
            if current.device_generation == receipt.target_generation {
                if current.state == SessionState::Failed {
                    return Err(current.last_error.clone().unwrap_or_else(|| "device operation failed".into()));
                }
                println!("{id}: {:?}, generation {}.", current.state, current.device_generation);
                return Ok(());
            }
            if current.device_generation > receipt.target_generation { return Err("a newer client changed this device; operation result is superseded".into()); }
            if let Some(error) = &current.last_error { return Err(error.clone()); }
        }
        if Instant::now() >= deadline { return Err("device operation remains pending; query devices list before retrying".into()); }
        std::thread::sleep(Duration::from_millis(50));
    }
}
