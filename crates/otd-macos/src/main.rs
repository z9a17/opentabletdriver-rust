//! Bounded native macOS CLI slice. Hardware validation is still pending.

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
mod descriptor;
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
mod usb;
#[cfg(target_os = "macos")]
mod ffi;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
mod keymap;
#[cfg(target_os = "macos")]
mod realtime;

const USAGE: &str = "OpenTabletDriver Rust macOS CLI\n\
Usage: opentabletdriver-rust-macos list\n\
       opentabletdriver-rust-macos device-strings VID PID INDEX [INDEX ...]\n\
       opentabletdriver-rust-macos displays\n\
       opentabletdriver-rust-macos run [--profile FILE] [--tablet NAME] [--screen WIDTHxHEIGHT]\n\
       opentabletdriver-rust-macos capture [--profile FILE] [--tablet NAME] [--seconds N] [--limit N]\n\
       opentabletdriver-rust-macos --version\n\n\
macOS 11+; grant Input Monitoring and Accessibility to your terminal.\n\
USB HID and absolute/relative mouse only; macOS hardware validation pending.";

fn main() {
    match std::env::args().nth(1).as_deref() {
        Some("--version" | "version" | "-V") => { println!("OpenTabletDriver Rust {} (macOS CLI)", env!("OTD_RELEASE_VERSION")); return; }
        Some("--help" | "help" | "-h") | None => { println!("{USAGE}"); return; }
        _ => {}
    }
    #[cfg(target_os = "macos")]
    if let Err(error) = app::main() {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
    #[cfg(not(target_os = "macos"))]
    {
        eprintln!("opentabletdriver-rust-macos runs on macOS only.");
        std::process::exit(1);
    }
}

#[cfg(target_os = "macos")]
mod app {
    use std::io;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};

    use otd_core::config::{OutputKind, Profile};
    use otd_core::decoders::TabletDecoder;
    use otd_core::endpoint_match;
    use otd_core::mapping::Rect;
    use otd_core::plugins::NoFilters;
    use otd_core::session::{self, Displays, Mode};
    use otd_core::spec::TabletSpec;
    use otd_core::tablets::{Database, DeviceIdentifier, ParserSupport, Role, TabletConfiguration, parser_support};

    use crate::macos::{self, Device, HidSource, Mouse, NativeDisplays};

    static STOP: AtomicBool = AtomicBool::new(false);

    struct Options {
        profile: Option<PathBuf>, tablet: Option<String>, screen: Option<Rect>,
        seconds: u64, limit: u32,
    }

    pub fn main() -> Result<(), String> {
        let mut args = std::env::args().skip(1);
        let command = args.next().ok_or(crate::USAGE)?;
        if command == "list" || command == "displays" {
            if args.next().is_some() { return Err(crate::USAGE.into()); }
            return if command == "list" { list() } else { display_list() };
        }
        if command == "device-strings" { return device_strings(args.collect()); }
        if command != "run" && command != "capture" { return Err(crate::USAGE.into()); }
        let capture = command == "capture";
        let mut options = Options { profile: None, tablet: None, screen: None, seconds: 10, limit: 2000 };
        while let Some(argument) = args.next() {
            if matches!(argument.as_str(), "--help" | "-h") { println!("{}", crate::USAGE); return Ok(()); }
            let value = args.next().ok_or_else(|| format!("missing value for {argument}\n{}", crate::USAGE))?;
            match argument.as_str() {
                "--profile" => options.profile = Some(value.into()),
                "--tablet" => options.tablet = Some(value),
                "--screen" => {
                    let (width, height) = value.split_once(['x', 'X']).ok_or("use --screen WIDTHxHEIGHT")?;
                    let width: i32 = width.parse().map_err(|_| "invalid screen width")?;
                    let height: i32 = height.parse().map_err(|_| "invalid screen height")?;
                    if !(1..=1_000_000).contains(&width) || !(1..=1_000_000).contains(&height) { return Err("screen dimensions must be between 1 and 1000000".into()); }
                    options.screen = Some(Rect { left: 0, top: 0, right: width, bottom: height });
                }
                "--seconds" if capture => {
                    options.seconds = value.parse().map_err(|_| "invalid capture seconds")?;
                    if !(1..=60).contains(&options.seconds) { return Err("capture seconds must be between 1 and 60".into()); }
                }
                "--limit" if capture => {
                    options.limit = value.parse().map_err(|_| "invalid capture limit")?;
                    if !(1..=1_000_000).contains(&options.limit) { return Err("capture limit must be between 1 and 1000000".into()); }
                }
                _ => return Err(crate::USAGE.into()),
            }
        }
        run(options, capture)
    }

    fn list() -> Result<(), String> {
        let database = otd_core::config::configured_tablets()?;
        let devices = macos::enumerate(&database).map_err(|error| error.to_string())?;
        if devices.is_empty() { println!("No USB HID endpoints."); }
        for device in &devices {
            let endpoint = &device.endpoint;
            let found = database.find(endpoint.vendor_id, endpoint.product_id)
                .find(|candidate| endpoint_match::matches(endpoint, candidate).is_ok());
            println!("{} {:04x}:{:04x} input {} output {} feature {}{}: {} [{}]",
                endpoint.path, endpoint.vendor_id, endpoint.product_id,
                endpoint.input_length, endpoint.output_length, endpoint.feature_length,
                if endpoint.can_open { "" } else { " (Input Monitoring denied)" },
                found.map_or_else(|| device.string_error.clone().unwrap_or_else(|| "no matching configuration".into()),
                    |found| format!("{} ({:?}, {:?})", found.configuration.name, found.role, found.parser)),
                device.name);
        }
        Ok(())
    }

    fn device_strings(arguments: Vec<String>) -> Result<(), String> {
        if arguments.len() < 3 { return Err(crate::USAGE.into()); }
        let id = |value: &str| -> Result<u16, String> {
            let value = value.trim_start_matches("0x").trim_start_matches("0X");
            u16::from_str_radix(value, 16).map_err(|_| "use hexadecimal VID and PID".into())
        };
        let (vendor, product) = (id(&arguments[0])?, id(&arguments[1])?);
        let indices: Vec<u8> = arguments[2..].iter().map(|value| value.parse::<u8>()
            .ok().filter(|index| *index != 0).ok_or_else(|| "string indices must be between 1 and 255".to_owned()))
            .collect::<Result<_, _>>()?;
        let database = otd_core::config::configured_tablets()?;
        let devices = macos::enumerate(&database).map_err(|error| error.to_string())?;
        let mut physical = std::collections::BTreeSet::new();
        for device in devices.iter().filter(|device| device.endpoint.vendor_id == vendor && device.endpoint.product_id == product) {
            if !physical.insert(&device.endpoint.physical_id) { continue; }
            for index in &indices {
                let value = device.indexed_string(*index).map_err(|error| format!("{}: {error}", device.endpoint.path))?;
                println!("{} string {index}: {value}", device.endpoint.path);
            }
        }
        if physical.is_empty() { return Err(format!("no USB device {vendor:04x}:{product:04x}")); }
        Ok(())
    }

    fn display_list() -> Result<(), String> {
        let mut displays = NativeDisplays::new(None).map_err(|error| error.to_string())?;
        let snapshot = displays.snapshot()?;
        println!("CoreGraphics logical coordinates (Retina points); virtual desktop {:?}", snapshot.virtual_screen);
        for (index, rectangle) in snapshot.monitors.iter().enumerate() {
            println!("monitor {index}: {}x{} at {},{}", rectangle.width(), rectangle.height(), rectangle.left, rectangle.top);
        }
        Ok(())
    }

    struct Selected<'a> {
        device: &'a Device, configuration: TabletConfiguration,
        identifier: DeviceIdentifier, spec: TabletSpec,
    }

    fn select<'a>(devices: &'a [Device], database: &Database, tablet: Option<&str>, path: Option<&str>) -> Result<Option<Selected<'a>>, String> {
        let mut unsupported = None;
        for device in devices.iter().filter(|device| path.is_none_or(|path| device.endpoint.path == path)) {
            for found in database.find(device.endpoint.vendor_id, device.endpoint.product_id)
                .filter(|found| found.role == Role::Digitizer && tablet.is_none_or(|name| found.configuration.name == name)) {
                match endpoint_match::matches(&device.endpoint, &found) {
                    Ok(()) => {}
                    Err(endpoint_match::Rejection::MissingString(index)) => {
                        unsupported = Some(format!("{} requires USB string descriptor {index} for matching: {}", found.configuration.name,
                            device.string_error.as_deref().unwrap_or("the descriptor is unavailable")));
                        continue;
                    }
                    Err(_) => continue,
                }
                if parser_support(found.identifier.parser()) == ParserSupport::Missing {
                    unsupported = Some(format!("{} uses unsupported parser {}", found.configuration.name, found.identifier.parser()));
                    continue;
                }
                match TabletSpec::from_configuration(found.configuration) {
                    Ok(spec) => return Ok(Some(Selected { device, configuration: found.configuration.clone(), identifier: found.identifier.clone(), spec })),
                    Err(error) => unsupported = Some(error),
                }
            }
        }
        unsupported.map_or(Ok(None), Err)
    }

    fn run(options: Options, capture: bool) -> Result<(), String> {
        install_signals()?;
        macos::input_permission().map_err(|error| error.to_string())?;
        let requested_profile = options.profile.as_deref().map(|path| Profile::load(Some(path))).transpose()?;
        let profile_tablet = requested_profile.as_ref().map(Profile::tablet_name).transpose()?.flatten();
        if let (Some(requested), Some(saved)) = (&options.tablet, &profile_tablet) {
            if requested != saved { return Err(format!("--tablet {requested} conflicts with the profile for {saved}")); }
        }
        let tablet = options.tablet.or(profile_tablet);
        let mut displays = NativeDisplays::new(options.screen).map_err(|error| error.to_string())?;
        let database = otd_core::config::configured_tablets()?;
        let deadline = capture.then(|| Instant::now() + Duration::from_secs(options.seconds));
        let mut waiting = false;
        while !STOP.load(Ordering::Acquire) {
            if deadline.is_some_and(|deadline| Instant::now() >= deadline) { return Err("capture deadline elapsed before a supported tablet was available".into()); }
            let devices = macos::enumerate(&database).map_err(|error| error.to_string())?;
            let device_path = requested_profile.as_ref().and_then(|profile| profile.device_path.as_deref());
            let Some(selected) = select(&devices, &database, tablet.as_deref(), device_path)? else {
                if !waiting { eprintln!("Waiting for {}.", tablet.as_deref().unwrap_or("a supported USB tablet")); waiting = true; }
                pause(deadline);
                continue;
            };
            waiting = false;
            let profile = match &requested_profile {
                Some(profile) => profile.clone(),
                None => Profile::load_otd_tablet(&selected.configuration.name)?.unwrap_or_default(),
            }.for_tablet(selected.spec)?;
            profile.validate_runtime_tablet_in(&database)?;
            if profile.output == OutputKind::Pen {
                return Err("macOS CLI supports mouse output only; pressure/tilt tablet output and Artist Mode are not implemented".into());
            }
            if profile.plugins.iter().any(|plugin| plugin.enabled) {
                return Err("external plugins are not supported by the macOS CLI runtime; disable them explicitly".into());
            }
            let _ = otd_core::pipeline::ReportPipeline::new(&profile)?;
            if profile.relative.is_none() { displays.snapshot()?.mapper(&profile)?; }
            let mode = deadline.map_or(Mode::Driver, |deadline| Mode::Capture { deadline, limit: options.limit });
            match run_session(&selected, &profile, &mut displays, mode) {
                Ok(()) if capture => return Ok(()),
                Ok(()) => {}
                Err(error) if STOP.load(Ordering::Acquire) => { let _ = error; break; }
                Err(error) => {
                    if capture || session::is_cleanup_failure(&error) || matches!(error.kind(),
                        io::ErrorKind::PermissionDenied | io::ErrorKind::InvalidData | io::ErrorKind::InvalidInput | io::ErrorKind::Unsupported) {
                        return Err(format!("{} stopped: {error}", selected.configuration.name));
                    }
                    eprintln!("{} stopped: {error}; rescanning USB tablets", selected.configuration.name);
                }
            }
            pause(deadline);
        }
        Ok(())
    }

    fn run_session(selected: &Selected<'_>, profile: &Profile, displays: &mut NativeDisplays, mode: Mode) -> io::Result<()> {
        let mut decoder = TabletDecoder::for_parser(selected.identifier.parser(), selected.spec)
            .ok_or_else(|| io::Error::new(io::ErrorKind::Unsupported, "unsupported tablet parser"))?;
        let mut source = HidSource::open(selected.device, format!("{} ({})", selected.configuration.name, selected.device.endpoint.path), &STOP)?;
        // Establish output permission/resources before any hardware init writes.
        let mouse = if matches!(mode, Mode::Driver) {
            Some(std::rc::Rc::new(std::cell::RefCell::new(Mouse::new(displays.geometry.clone())?)))
        } else { None };
        let actions = mouse.as_ref().map(|mouse| crate::macos::action_sink(std::rc::Rc::clone(mouse)));
        source.initialize(&selected.identifier, &selected.configuration,
            match mode { Mode::Capture { deadline, .. } => Some(deadline), Mode::Driver => None })?;
        if let ParserSupport::Partial(reason) = parser_support(selected.identifier.parser()) {
            eprintln!("Partial parser support: {reason}");
        }
        eprintln!("macOS native CLI: hardware validation pending; Ctrl+C stops and releases contact.");
        let _realtime = matches!(mode, Mode::Driver).then(crate::realtime::TimeConstraint::raise);
        session::run_gated_with_devices(&mut source, displays, profile, mode, &mut decoder, &mut NoFilters,
            |packet| match &mouse { Some(mouse) => mouse.borrow_mut().send(packet), None => Ok(()) },
            None, actions, &|line| eprintln!("{line}"), || Ok(true))
    }

    fn pause(deadline: Option<Instant>) {
        for _ in 0..20 {
            if STOP.load(Ordering::Acquire) || deadline.is_some_and(|deadline| Instant::now() >= deadline) { return; }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    extern "C" fn on_signal(_: libc::c_int) { STOP.store(true, Ordering::Release); }
    fn install_signals() -> Result<(), String> {
        for signal in [libc::SIGINT, libc::SIGTERM] {
            // SAFETY: handler only sets an atomic; no non-signal-safe APIs.
            let previous = unsafe { libc::signal(signal, on_signal as *const () as libc::sighandler_t) };
            if previous == libc::SIG_ERR { return Err(io::Error::last_os_error().to_string()); }
        }
        Ok(())
    }
}
