pub mod action_output;
mod area_cli;
mod control;
mod daemon;
mod decode_cli;
mod diagnostics;
mod display;
mod dotnet;
mod hid;
mod original_driver;
mod output;
mod plugin_catalog;
mod plugins;
mod preset_cli;
mod priority;
mod profile_cli;
mod runtime;
mod session;
mod ui;
mod update;

// The portable core, at the crate paths the Windows modules use.
use otd_core::tablets::{self, Database, Origin, ParserSupport, Role, Severity};
#[cfg(test)]
use otd_core::test_alloc;
use otd_core::{config, mapping, protocol, radial_follow, relative};

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError, WAIT_OBJECT_0};
use windows_sys::Win32::System::Threading::{CreateMutexW, SetEvent, WaitForSingleObject};

use crate::config::Profile;
use crate::hid::{Event, Notification, OwnedHandle};
use crate::session::Mode;

fn usage() -> &'static str {
    "Usage:
  opentabletdriver-rust-ui.exe              Open the native control panel
  opentabletdriver-rust.exe ui [--tray]     Open the same control panel (--tray: in the tray)
  opentabletdriver-rust.exe                 Start the visible cursor daemon
  opentabletdriver-rust.exe run [--config driver.toml | --otd-settings settings.json]
  opentabletdriver-rust.exe daemon [--background]
  opentabletdriver-rust.exe start [--config driver.toml | --otd-settings settings.json]
  opentabletdriver-rust.exe restart [--config driver.toml | --otd-settings settings.json]
  opentabletdriver-rust.exe configuration
  opentabletdriver-rust.exe status | stop | shutdown | debug
  opentabletdriver-rust.exe profiles list|preview|import|export|select ...
  opentabletdriver-rust.exe presets list|show|save|export ...
  opentabletdriver-rust.exe area convert|full|fit ...
  opentabletdriver-rust.exe diagnostics --output NEW_FILE.json [--config PROFILE.toml]
  opentabletdriver-rust.exe decode --parser NAME --hex HEX_BYTES
  opentabletdriver-rust.exe settings [--config driver.toml | --otd-settings settings.json]
  opentabletdriver-rust.exe inspect-plugin path/to/managed-plugin.dll
  opentabletdriver-rust.exe check-plugins driver.toml
  opentabletdriver-rust.exe list [--paths]
  opentabletdriver-rust.exe displays
  opentabletdriver-rust.exe tablets [--list] [--configurations DIRECTORY]
  opentabletdriver-rust.exe capture [--config driver.toml | --otd-settings settings.json] [--seconds 1..60]
  opentabletdriver-rust.exe plugins catalog|installed|install NAME|remove NAME
  opentabletdriver-rust.exe device-strings VID PID [INDEX ...]
  opentabletdriver-rust.exe update [--check]
  opentabletdriver-rust.exe --version

Without a profile argument, use the saved Rust driver.toml, then OTD settings if present.
OTD_RUST_PORTABLE_DIR selects portable storage and disables automatic OTD import.
Capture does not inject cursor input. Plugin inspection/checks load trusted executable code."
}

enum Command {
    Decode(Vec<String>),
    Diagnostics(Vec<String>),
    Area(Vec<String>),
    Profiles(Vec<String>),
    Presets(Vec<String>),
    Daemon {
        background: bool,
    },
    Control(control::Command),
    Configuration,
    Restart {
        config: Option<PathBuf>,
        otd_settings: Option<PathBuf>,
    },
    Start {
        config: Option<PathBuf>,
        otd_settings: Option<PathBuf>,
    },
    Ui,
    Plugins(Vec<String>),
    DeviceStrings(Vec<String>),
    Update {
        check: bool,
    },
    Version,
    InspectPlugin(PathBuf),
    CheckPlugins(PathBuf),
    List {
        paths: bool,
    },
    Displays,
    Tablets {
        list: bool,
        directory: Option<PathBuf>,
    },
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
        "diagnostics" => Ok(Command::Diagnostics(args.collect())),
        "decode" => Ok(Command::Decode(args.collect())),
        "plugins" => Ok(Command::Plugins(args.collect())),
        "device-strings" => Ok(Command::DeviceStrings(args.collect())),
        "area" => Ok(Command::Area(args.collect())),
        "configuration" if args.next().is_none() => Ok(Command::Configuration),
        "profiles" => Ok(Command::Profiles(args.collect())),
        "presets" => Ok(Command::Presets(args.collect())),
        "daemon" => {
            let background = match args.next().as_deref() {
                None => false,
                Some("--background") => true,
                _ => return Err(usage().into()),
            };
            if args.next().is_some() {
                return Err(usage().into());
            }
            Ok(Command::Daemon { background })
        }
        "status" | "stop" | "shutdown" | "debug" => {
            if args.next().is_some() {
                return Err(usage().into());
            }
            Ok(Command::Control(match command.as_str() {
                "status" => control::Command::Status,
                "stop" => control::Command::Stop,
                "debug" => control::Command::Debug,
                _ => control::Command::Shutdown,
            }))
        }
        "ui" => match (args.next().as_deref(), args.next()) {
            (None, _) => Ok(Command::Ui),
            (Some("--tray"), None) => {
                ui::START_IN_TRAY.store(true, std::sync::atomic::Ordering::Relaxed);
                Ok(Command::Ui)
            }
            _ => Err(usage().into()),
        },
        "update" => match (args.next().as_deref(), args.next()) {
            (None, _) => Ok(Command::Update { check: false }),
            (Some("--check"), None) => Ok(Command::Update { check: true }),
            _ => Err(usage().into()),
        },
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
        "tablets" => {
            let (mut list, mut directory) = (false, None);
            while let Some(arg) = args.next() {
                match arg.as_str() {
                    "--list" => list = true,
                    "--configurations" => {
                        let value = args.next().ok_or("--configurations needs a directory")?;
                        directory = Some(PathBuf::from(value));
                    }
                    _ => {
                        return Err(format!(
                            "unknown tablets option: {arg}
{}",
                            usage()
                        ));
                    }
                }
            }
            Ok(Command::Tablets { list, directory })
        }
        "displays" => {
            if args.next().is_some() {
                Err(usage().into())
            } else {
                Ok(Command::Displays)
            }
        }
        "run" | "capture" | "settings" | "start" | "restart" => {
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
                "restart" => Ok(Command::Restart {
                    config,
                    otd_settings,
                }),
                "start" => Ok(Command::Start {
                    config,
                    otd_settings,
                }),
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

/// `device-strings VID PID [INDEX ...]`: USB strings of a device's HID
/// collections, for writing a configuration for an unsupported tablet. IDs
/// are hexadecimal (with or without 0x); indices default to 1 through 10.
fn device_strings(args: Vec<String>) -> Result<(), String> {
    let usage = "Usage: device-strings VID PID [INDEX ...]  (IDs in hex, e.g. 056a 0357)";
    let hex = |text: &str| {
        u16::from_str_radix(text.trim_start_matches("0x").trim_start_matches("0X"), 16).map_err(
            |_| {
                format!(
                    "{text} is not a hexadecimal ID
{usage}"
                )
            },
        )
    };
    let (vendor, product) = match args.as_slice() {
        [vendor, product, ..] => (hex(vendor)?, hex(product)?),
        _ => return Err(usage.into()),
    };
    let indices = if args.len() > 2 {
        args[2..]
            .iter()
            .map(|index| {
                index
                    .parse::<u8>()
                    .map_err(|_| format!("{index} is not a string index 0-255"))
            })
            .collect::<Result<Vec<_>, _>>()?
    } else {
        (1..=10).collect()
    };
    let collections = hid::read_strings(vendor, product, &indices)
        .map_err(|e| format!("HID discovery failed: {e}"))?;
    if collections.is_empty() {
        return Err(format!(
            "no HID collection with ID {vendor:04x}:{product:04x} is connected"
        ));
    }
    for (path, strings) in collections {
        println!("{path}");
        for (index, value) in strings {
            match value {
                Ok(text) => println!("  {index}: {text:?}"),
                Err(error) => println!("  {index}: (none: {error})"),
            }
        }
    }
    Ok(())
}

fn list(paths: bool) -> Result<(), String> {
    let devices = hid::enumerate().map_err(|e| format!("HID discovery failed: {e}"))?;
    if devices.is_empty() {
        println!("No HID collections matching known tablet vendor/product IDs found.");
    }
    for (index, device) in devices.iter().enumerate() {
        let role = match hid::identify(device, Database::builtin()) {
            Some((name, role, supported)) => format!(
                "{name} {}{}",
                if role == Role::Digitizer {
                    "pen"
                } else {
                    "auxiliary"
                },
                if supported {
                    ""
                } else {
                    " (unsupported parser)"
                }
            ),
            None => "unmatched collection".into(),
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

/// OpenTabletDriver's configuration directory. Its files override built-in
/// tablet configurations by name.
fn otd_configurations() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|directory| {
        PathBuf::from(directory)
            .join("OpenTabletDriver")
            .join("Configurations")
    })
}

/// The tablet configurations with the files in `directory` applied, or `None`
/// when there are none, and the number of files.
fn load_tablets(directory: Option<&Path>) -> Result<(Option<Database>, usize), String> {
    let files = match directory {
        Some(directory) => tablets::read_directory(directory).map_err(|e| e.to_string())?,
        None => Vec::new(),
    };
    let database = (!files.is_empty()).then(|| Database::with_overrides(&files));
    Ok((database, files.len()))
}

/// Says which configuration declares the USB PTH-660 pen interface and
/// whether an override file changes it. Overrides are used as upstream uses them.
fn report_pth_660(database: &Database) {
    let builtin = Database::builtin()
        .find(hid::WACOM_VENDOR, hid::PTH660_USB)
        .find(|m| m.role == Role::Digitizer)
        .expect("the pinned database declares the PTH-660");
    let mut declaring: Vec<&tablets::Entry> = Vec::new();
    for found in database.find(hid::WACOM_VENDOR, hid::PTH660_USB) {
        if found.role == Role::Digitizer && !declaring.iter().any(|e| std::ptr::eq(*e, found.entry))
        {
            declaring.push(found.entry);
        }
    }
    if declaring.is_empty() {
        eprintln!(
            "warning: no usable tablet configuration declares the USB PTH-660; it will not be selected"
        );
    }
    for entry in &declaring {
        let configuration = entry.configuration.as_ref().expect("usable entries parse");
        if entry.origin == Origin::BuiltIn {
            println!(
                "Tablet configuration: {}, built in from OpenTabletDriver {}",
                configuration.name,
                &tablets::source_revision()[..7]
            );
            continue;
        }
        let changed = builtin.configuration.changed_fields(configuration);
        if changed.is_empty() {
            println!(
                "Tablet configuration: {} from {}, the same as the built-in one",
                configuration.name, entry.path
            );
        } else {
            eprintln!(
                "Tablet override {} changes {} of {}; the override is used",
                entry.path,
                changed.join(", "),
                configuration.name
            );
        }
    }
    if declaring.len() > 1 {
        eprintln!(
            "warning: {} configurations declare the USB PTH-660 pen interface",
            declaring.len()
        );
    }
}

/// Loads the tablet configurations as OpenTabletDriver's daemon does when it
/// starts and reports what the USB PTH-660 resolves to. Nothing here runs per
/// report.
fn check_tablet_configurations() -> Result<Option<Database>, String> {
    let directory = otd_configurations().filter(|directory| directory.is_dir());
    let (custom, _) = load_tablets(directory.as_deref())?;
    let database = custom.as_ref().unwrap_or_else(|| Database::builtin());
    let errors = database
        .entries()
        .iter()
        .filter(|e| e.usable().is_none())
        .count();
    if errors > 0 {
        eprintln!(
            "warning: {errors} tablet configuration files have errors; `opentabletdriver-rust tablets` lists them"
        );
    }
    report_pth_660(database);
    Ok(custom)
}

/// Summarizes the tablet configuration database; with `list`, every tablet,
/// its interfaces and every diagnostic. Reads files only.
fn tablets(list: bool, directory: Option<PathBuf>) -> Result<(), String> {
    let explicit = directory.is_some();
    let directory = directory
        .or_else(otd_configurations)
        .filter(|directory| explicit || directory.is_dir());
    let (custom, files) = load_tablets(directory.as_deref())?;
    let database = custom.as_ref().unwrap_or_else(|| Database::builtin());
    let entries = database.entries();
    let built_in = Database::builtin().entries().len();
    println!(
        "{built_in} built-in tablet configurations from OpenTabletDriver {}",
        &tablets::source_revision()[..7]
    );
    if let Some(directory) = &directory {
        let kept = entries
            .iter()
            .filter(|e| e.origin == Origin::BuiltIn)
            .count();
        println!(
            "Configuration files in {}: {files} (replacing built-in ones: {})",
            directory.display(),
            built_in - kept
        );
    }
    let has = |severity| {
        entries
            .iter()
            .filter(|e| e.diagnostics.iter().any(|d| d.severity == severity))
            .count()
    };
    let errors = has(Severity::Error);
    println!(
        "{} usable, {errors} with errors, {} with warnings",
        entries.iter().filter(|e| e.usable().is_some()).count(),
        has(Severity::Warning)
    );
    let parsers = database.parsers();
    let missing: Vec<&str> = parsers
        .keys()
        .copied()
        .filter(|parser| tablets::parser_support(parser) == ParserSupport::Missing)
        .map(|parser| parser.rsplit('.').next().unwrap_or(parser))
        .collect();
    println!(
        "{} report parsers referenced; decoded by this driver: {}; not decoded: {}",
        parsers.len(),
        parsers.len() - missing.len(),
        if missing.is_empty() {
            "none".to_owned()
        } else {
            missing.join(", ")
        }
    );
    report_pth_660(database);
    for entry in entries {
        if list {
            match &entry.configuration {
                Some(configuration) => {
                    println!("{} [{}]", configuration.name, entry.path);
                    print_identifiers(configuration);
                }
                None => println!("[{}]", entry.path),
            }
        }
        for diagnostic in &entry.diagnostics {
            if list || diagnostic.severity == Severity::Error {
                println!("  {}: {diagnostic}", entry.path);
            }
        }
    }
    match errors {
        0 => Ok(()),
        1 => Err("1 configuration file has errors".into()),
        _ => Err(format!("{errors} configuration files have errors")),
    }
}

fn print_identifiers(configuration: &tablets::TabletConfiguration) {
    for (role, identifiers) in [
        ("digitizer", configuration.digitizer_identifiers.as_slice()),
        ("auxiliary", configuration.auxiliary_identifiers()),
    ] {
        for identifier in identifiers {
            let parser = identifier.parser();
            let support = match tablets::parser_support(parser) {
                ParserSupport::Partial(detail) => format!("partly decoded: {detail}"),
                ParserSupport::Missing => "not decoded".to_owned(),
            };
            println!(
                "  {role} {:04x}:{:04x}, {} report bytes, {}: {support}",
                identifier.vendor_id().unwrap_or(0),
                identifier.product_id().unwrap_or(0),
                identifier
                    .input_report_length
                    .map_or("any".into(), |n| n.to_string()),
                parser.rsplit('.').next().unwrap_or(parser),
            );
        }
    }
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

fn load_profile(
    config: Option<&PathBuf>,
    otd_settings: Option<&PathBuf>,
) -> Result<Profile, String> {
    if let Some(path) = otd_settings {
        Profile::load_otd(path)
    } else if let Some(path) = config {
        Profile::load(Some(path))
    } else {
        let path = otd_core::storage::data_directory()?.join("driver.toml");
        if path
            .try_exists()
            .map_err(|error| format!("cannot inspect saved profile {}: {error}", path.display()))?
        {
            Profile::load(Some(&path))
        } else if std::env::var_os("OTD_RUST_PORTABLE_DIR").is_some() {
            Ok(Profile::default())
        } else {
            Profile::load_connected(None, &hid::connected_tablets())
        }
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
    profile.validate_runtime_tablet()?;
    profile.validate_filter_execution()?;
    let tablet_name = profile.tablet_name()?;
    if profile.relative.is_none() {
        display::read_snapshot()?.mapper(&profile)?;
    }
    println!(
        "opentabletdriver-rust {} — Windows 11 tablet daemon",
        env!("CARGO_PKG_VERSION")
    );
    profile.print_summary();
    let custom_tablets = check_tablet_configurations()?;
    let database = custom_tablets
        .as_ref()
        .unwrap_or_else(|| Database::builtin());
    if capture_seconds.is_none() {
        println!("Reading pen input and moving the cursor. Press Ctrl+C to stop.");
    } else {
        println!("Read-only capture; no cursor input is injected.");
    }
    let _instance = single_instance()?;
    // Register before enumerating so an arrival between the two is not missed.
    let notification =
        Notification::register().map_err(|e| format!("PnP notification failed: {e}"))?;
    let mode = capture_seconds.map_or(Mode::Driver, |seconds| Mode::Capture {
        deadline: Instant::now() + Duration::from_secs(seconds),
        limit: 10_000,
    });
    let mut original_driver = if capture_seconds.is_none() {
        Some(
            original_driver::OriginalDriverGuard::pause().map_err(|error| {
                format!("could not pause the original OpenTabletDriver safely: {error}")
            })?,
        )
    } else {
        None
    };
    let outcome = (|| {
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
            let devices = hid::enumerate_with_database(database)
                .map_err(|e| format!("HID discovery failed: {e}"))?;
            let Some(selected) = hid::select_device(
                &devices,
                database,
                profile.device_path.as_deref(),
                tablet_name.as_deref(),
            )?
            else {
                if !waiting {
                    let waiting_for = format!(
                        "Waiting for {}",
                        tablet_name.as_deref().unwrap_or("a supported tablet")
                    );
                    eprintln!("{waiting_for}.");
                    status(&waiting_for);
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
            let mut plugins = plugins::PluginChain::load_with_tablet(
                if capture_seconds.is_none() {
                    &profile.plugins
                } else {
                    &[]
                },
                &selected.configuration,
            )?;
            plugins.validate_output_mode(profile.relative.is_some())?;
            if selected.auxiliary.is_some() {
                status(&format!(
                    "{} auxiliary collection paired; auxiliary output is not enabled yet",
                    selected.configuration.name
                ));
            }
            status(&format!(
                "{} found; opening pen input",
                selected.configuration.name
            ));
            match session::run(
                &selected,
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
                    if otd_core::session::is_cleanup_failure(&error) {
                        return Err(error.to_string());
                    }
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
    })();
    let restoration = original_driver.as_mut().map_or(Ok(()), |guard| {
        guard
            .restore()
            .map_err(|error| format!("could not restore the original driver: {error}"))
    });
    match (outcome, restoration) {
        (Err(error), Err(restore)) => Err(format!("{error}; {restore}")),
        (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
        (Ok(()), Ok(())) => Ok(()),
    }
}

fn main() {
    update::remove_leftovers();
    let result = match parse_args() {
        Ok(Command::Decode(args)) => decode_cli::run(args),
        Ok(Command::Plugins(args)) => plugin_catalog::run(args),
        Ok(Command::DeviceStrings(args)) => device_strings(args),
        Ok(Command::Diagnostics(args)) => diagnostics::run(args),
        Ok(Command::Area(args)) => area_cli::run(args),
        Ok(Command::Profiles(args)) => profile_cli::run(args),
        Ok(Command::Presets(args)) => preset_cli::run(args),
        Ok(Command::Daemon { background }) => {
            if background {
                daemon::background()
            } else {
                daemon::serve()
            }
        }
        Ok(Command::Control(command)) => {
            daemon::call(command).and_then(|reply| daemon::print_reply(&reply))
        }
        Ok(Command::Configuration) => {
            daemon::configuration().and_then(|reply| daemon::print_reply(&reply))
        }
        Ok(Command::Restart {
            config,
            otd_settings,
        }) => {
            let replacement = if config.is_some() || otd_settings.is_some() {
                load_profile(config.as_ref(), otd_settings.as_ref())
                    .and_then(|profile| profile.to_toml())
                    .map(Some)
            } else {
                Ok(None)
            };
            replacement
                .and_then(daemon::restart)
                .and_then(|reply| daemon::print_reply(&reply))
        }
        Ok(Command::Start {
            config,
            otd_settings,
        }) => load_profile(config.as_ref(), otd_settings.as_ref()).and_then(|profile| {
            profile.validate_runtime_tablet()?;
            daemon::call(control::Command::Start {
                profile_toml: Some(profile.to_toml()?),
            })
            .and_then(|reply| daemon::print_reply(&reply))
        }),
        Ok(Command::Ui) => ui::run(),
        Ok(Command::Update { check }) => update::run(check),
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
        Ok(Command::Tablets { list, directory }) => tablets(list, directory),
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
            println!("\n{}", profile_cli::usage());
            println!("\n{}", area_cli::usage());
            println!("\n{}", diagnostics::usage());
            Ok(())
        }
        Err(error) => Err(error),
    };
    if let Err(error) = result {
        eprintln!("{error}");
        std::process::exit(if error.starts_with("Usage:") { 2 } else { 1 });
    }
}
