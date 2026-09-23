mod display;
mod dotnet;
mod hid;
mod original_driver;
mod output;
mod plugins;
mod priority;
mod session;
mod ui;

// The portable core, at the crate paths the Windows modules use.
#[cfg(test)]
use otd_core::test_alloc;
use otd_core::{config, mapping, protocol, radial_follow, relative};

use std::path::PathBuf;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError, WAIT_OBJECT_0};
use windows_sys::Win32::System::Threading::{CreateMutexW, SetEvent, WaitForSingleObject};

use crate::config::Profile;
use crate::hid::{Candidate, Event, Notification, OwnedHandle};
use crate::session::Mode;

fn usage() -> &'static str {
    "Usage:
  opentabletdriver-rust-ui.exe              Open the native control panel
  opentabletdriver-rust.exe ui              Open the same control panel
  opentabletdriver-rust.exe                 Start the visible cursor daemon
  opentabletdriver-rust.exe run [--config driver.toml | --otd-settings settings.json]
  opentabletdriver-rust.exe settings [--config driver.toml | --otd-settings settings.json]
  opentabletdriver-rust.exe inspect-plugin path/to/managed-plugin.dll
  opentabletdriver-rust.exe check-plugins driver.toml
  opentabletdriver-rust.exe list [--paths]
  opentabletdriver-rust.exe displays
  opentabletdriver-rust.exe capture [--config driver.toml | --otd-settings settings.json] [--seconds 1..60]
  opentabletdriver-rust.exe --version

Without a profile argument, the daemon reads your OpenTabletDriver PTH-660 settings.json if present.
Capture does not inject cursor input. Plugin inspection/checks load trusted executable code."
}

enum Command {
    Ui,
    Version,
    InspectPlugin(PathBuf),
    CheckPlugins(PathBuf),
    List {
        paths: bool,
    },
    Displays,
    Settings {
        config: Option<PathBuf>,
        otd_settings: Option<PathBuf>,
    },
    Run {
        config: Option<PathBuf>,
        otd_settings: Option<PathBuf>,
    },
    Capture {
        config: Option<PathBuf>,
        otd_settings: Option<PathBuf>,
        seconds: u64,
    },
    Help,
}

fn parse_args() -> Result<Command, String> {
    let mut args = std::env::args().skip(1);
    let Some(command) = args.next() else {
        return if env!("CARGO_BIN_NAME") == "opentabletdriver-rust-ui" {
            Ok(Command::Ui)
        } else {
            Ok(Command::Run {
                config: None,
                otd_settings: None,
            })
        };
    };
    match command.as_str() {
        "ui" if args.next().is_none() => Ok(Command::Ui),
        "--version" | "version" => Ok(Command::Version),
        "inspect-plugin" | "check-plugins" => {
            let path = PathBuf::from(args.next().ok_or("command needs a file path")?);
            if args.next().is_some() {
                return Err(usage().into());
            }
            if command == "inspect-plugin" {
                Ok(Command::InspectPlugin(path))
            } else {
                Ok(Command::CheckPlugins(path))
            }
        }
        "--help" | "-h" | "help" => Ok(Command::Help),
        "list" => {
            let mut paths = false;
            for arg in args {
                if arg == "--paths" {
                    paths = true;
                } else {
                    return Err(format!("unknown list option: {arg}\n{}", usage()));
                }
            }
            Ok(Command::List { paths })
        }
        "displays" => {
            if args.next().is_some() {
                Err(usage().into())
            } else {
                Ok(Command::Displays)
            }
        }
        "run" | "capture" | "settings" => {
            let mut config = None;
            let mut otd_settings = None;
            let mut seconds = 10;
            while let Some(arg) = args.next() {
                match arg.as_str() {
                    "--config" => {
                        let value = args.next().ok_or("--config needs a file path")?;
                        config = Some(PathBuf::from(value));
                    }
                    "--otd-settings" => {
                        let value = args.next().ok_or("--otd-settings needs a file path")?;
                        otd_settings = Some(PathBuf::from(value));
                    }
                    "--seconds" if command == "capture" => {
                        let value = args.next().ok_or("--seconds needs a number")?;
                        seconds = value
                            .parse::<u64>()
                            .map_err(|_| "--seconds must be an integer")?;
                        if !(1..=60).contains(&seconds) {
                            return Err("--seconds must be in 1..60".into());
                        }
                    }
                    _ => return Err(format!("unknown option: {arg}\n{}", usage())),
                }
            }
            if config.is_some() && otd_settings.is_some() {
                return Err("choose either --config or --otd-settings".into());
            }
            match command.as_str() {
                "run" => Ok(Command::Run {
                    config,
                    otd_settings,
                }),
                "settings" => Ok(Command::Settings {
                    config,
                    otd_settings,
                }),
                _ => Ok(Command::Capture {
                    config,
                    otd_settings,
                    seconds,
                }),
            }
        }
        _ => Err(usage().into()),
    }
}

fn list(paths: bool) -> Result<(), String> {
    let devices = hid::enumerate().map_err(|e| format!("HID discovery failed: {e}"))?;
    if devices.is_empty() {
        println!("No USB PTH-660 HID collections found.");
    }
    for (index, device) in devices.iter().enumerate() {
        let role = match device.input_length {
            hid::PEN_REPORT_LENGTH => "pen",
            hid::AUX_REPORT_LENGTH => "auxiliary",
            _ => "other",
        };
        let openable = device.open_read().is_ok();
        println!(
            "{index}: {role}, {:04x}:{:04x}, input={} bytes, usage={:04x}:{:04x}, readable={openable}",
            device.vendor, device.product, device.input_length, device.usage_page, device.usage
        );
        if paths {
            println!("    {}", device.path_text());
        }
    }
    Ok(())
}

fn displays() -> Result<(), String> {
    let snapshot = display::read_snapshot()?;
    let v = snapshot.virtual_screen;
    println!(
        "virtual desktop: ({}, {}) to ({}, {})",
        v.left, v.top, v.right, v.bottom
    );
    for (index, r) in snapshot.monitors.iter().enumerate() {
        println!(
            "monitor {index}: ({}, {}) to ({}, {})",
            r.left, r.top, r.right, r.bottom
        );
    }
    Ok(())
}

/// Held by whichever process is running the driver.
pub(crate) const INSTANCE_MUTEX: &str = "Local\\PTH660RustDriver";

fn single_instance() -> Result<OwnedHandle, String> {
    let name: Vec<u16> = INSTANCE_MUTEX.encode_utf16().chain(Some(0)).collect();
    let raw = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
    let handle = OwnedHandle::new(raw).map_err(|e| format!("instance guard failed: {e}"))?;
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        return Err("another PTH-660 Rust driver instance is already running".into());
    }
    Ok(handle)
}

fn choose<'a>(
    devices: &'a [Candidate],
    profile: &Profile,
) -> Result<Option<&'a Candidate>, String> {
    let mut matching = devices.iter().filter(|d| d.is_pen());
    if let Some(path) = &profile.device_path {
        return Ok(matching.find(|d| d.path_text().eq_ignore_ascii_case(path)));
    }
    let first = matching.next();
    if first.is_some() && matching.next().is_some() {
        return Err(
            "multiple PTH-660 pen collections found; set device_path in the profile".into(),
        );
    }
    Ok(first)
}

fn load_profile(
    config: Option<&PathBuf>,
    otd_settings: Option<&PathBuf>,
) -> Result<Profile, String> {
    if let Some(path) = otd_settings {
        Profile::load_otd(path)
    } else {
        Profile::load(config.map(PathBuf::as_path))
    }
}

fn show_settings(config: Option<PathBuf>, otd_settings: Option<PathBuf>) -> Result<(), String> {
    let profile = load_profile(config.as_ref(), otd_settings.as_ref())?;
    if profile.relative.is_none() {
        display::read_snapshot()?.mapper(&profile)?;
    }
    profile.print_summary();
    Ok(())
}

fn run(
    config: Option<PathBuf>,
    otd_settings: Option<PathBuf>,
    capture_seconds: Option<u64>,
) -> Result<(), String> {
    let profile = load_profile(config.as_ref(), otd_settings.as_ref())?;
    let stop_event = Event::create(true).map_err(|e| format!("stop event failed: {e}"))?;
    let stop_handle = stop_event.raw() as usize;
    ctrlc::set_handler(move || {
        unsafe { SetEvent(stop_handle as windows_sys::Win32::Foundation::HANDLE) };
    })
    .map_err(|e| format!("Ctrl+C handler failed: {e}"))?;
    drive(profile, &stop_event, capture_seconds, |_| {})
}

fn drive(
    profile: Profile,
    stop_event: &Event,
    capture_seconds: Option<u64>,
    status: impl Fn(&str),
) -> Result<(), String> {
    if profile.relative.is_none() {
        display::read_snapshot()?.mapper(&profile)?;
    }
    println!(
        "opentabletdriver-rust {} — Windows 11 USB PTH-660 daemon",
        env!("CARGO_PKG_VERSION")
    );
    profile.print_summary();
    if capture_seconds.is_none() {
        println!("Reading pen input and moving the cursor. Press Ctrl+C to stop.");
    } else {
        println!("Read-only capture; no cursor input is injected.");
    }
    let _instance = single_instance()?;
    let mut plugins = plugins::PluginChain::load(if capture_seconds.is_none() {
        &profile.plugins
    } else {
        &[]
    })?;
    plugins.validate_output_mode(profile.relative.is_some())?;
    // Register before enumerating so an arrival between the two is not missed.
    let notification =
        Notification::register().map_err(|e| format!("PnP notification failed: {e}"))?;
    let mode = capture_seconds.map_or(Mode::Driver, |seconds| Mode::Capture {
        deadline: Instant::now() + Duration::from_secs(seconds),
        limit: 10_000,
    });
    let _original_driver = if capture_seconds.is_none() {
        Some(
            original_driver::OriginalDriverGuard::pause().map_err(|error| {
                format!("could not pause the original OpenTabletDriver safely: {error}")
            })?,
        )
    } else {
        None
    };
    let mut waiting = false;
    loop {
        if unsafe { WaitForSingleObject(stop_event.raw(), 0) } == WAIT_OBJECT_0 {
            break;
        }
        if let Mode::Capture { deadline, .. } = mode
            && Instant::now() >= deadline
        {
            break;
        }
        let devices = hid::enumerate().map_err(|e| format!("HID discovery failed: {e}"))?;
        let Some(candidate) = choose(&devices, &profile)? else {
            if !waiting {
                eprintln!("Waiting for USB PTH-660.");
                status("Waiting for USB PTH-660");
                waiting = true;
            }
            if !session::wait_for_retry(&notification, stop_event)
                .map_err(|e| format!("wait failed: {e}"))?
            {
                break;
            }
            continue;
        };
        waiting = false;
        status("PTH-660 found; opening pen input");
        match session::run(
            candidate,
            &profile,
            &notification,
            stop_event,
            mode,
            &mut plugins,
            &status,
        ) {
            Ok(()) => {}
            Err(error) => {
                eprintln!("device session stopped: {error}");
                status(&format!("Device session stopped: {error}"));
            }
        }
        if matches!(mode, Mode::Capture { .. }) {
            break;
        }
        if !session::wait_for_retry(&notification, stop_event)
            .map_err(|e| format!("wait failed: {e}"))?
        {
            break;
        }
    }
    Ok(())
}

fn main() {
    let result = match parse_args() {
        Ok(Command::Ui) => ui::run(),
        Ok(Command::Version) => {
            println!("opentabletdriver-rust {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Ok(Command::InspectPlugin(path)) => dotnet::inspect(&path).and_then(|entries| {
            println!(
                "{}",
                serde_json::to_string_pretty(&entries).map_err(|e| e.to_string())?
            );
            Ok(())
        }),
        Ok(Command::CheckPlugins(path)) => Profile::load(Some(&path)).and_then(|profile| {
            let chain = plugins::PluginChain::load(&profile.plugins)?;
            chain.validate_output_mode(profile.relative.is_some())?;
            println!(
                "Loaded {} enabled plugin(s); no HID opened or input injected.",
                profile.plugins.iter().filter(|p| p.enabled).count()
            );
            Ok(())
        }),
        Ok(Command::List { paths }) => list(paths),
        Ok(Command::Displays) => displays(),
        Ok(Command::Settings {
            config,
            otd_settings,
        }) => show_settings(config, otd_settings),
        Ok(Command::Run {
            config,
            otd_settings,
        }) => run(config, otd_settings, None),
        Ok(Command::Capture {
            config,
            otd_settings,
            seconds,
        }) => run(config, otd_settings, Some(seconds)),
        Ok(Command::Help) => {
            println!("{}", usage());
            Ok(())
        }
        Err(error) => Err(error),
    };
    if let Err(error) = result {
        eprintln!("{error}");
        std::process::exit(if error.starts_with("Usage:") { 2 } else { 1 });
    }
}
