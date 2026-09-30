//! Linux backend slice (X02/X03): runs a tablet through the portable core
//! with hidraw input and a uinput pointer, or with pen output a virtual
//! Artist Mode tablet. Desktop geometry is discovered at startup or supplied
//! with `--screen`. Capture mode initializes and reads the tablet without
//! creating an output device. There is no daemon, UI or plugin host yet.

// Portable parsing and event framing, unit-tested on every platform.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod artist;
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod descriptor;
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod sysfs;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
mod displays;

#[cfg(target_os = "linux")]
fn main() {
    if let Err(error) = app::main() {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("opentabletdriver-rust-linux runs on Linux only.");
    std::process::exit(1);
}

/// `WIDTHxHEIGHT`, e.g. `2560x1440`.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn parse_screen(text: &str) -> Result<(i32, i32), String> {
    let invalid = || format!("invalid screen size {text}; use WIDTHxHEIGHT");
    let (width, height) = text.split_once(['x', 'X']).ok_or_else(invalid)?;
    let width: i32 = width.trim().parse().map_err(|_| invalid())?;
    let height: i32 = height.trim().parse().map_err(|_| invalid())?;
    if width <= 0 || height <= 0 {
        return Err(invalid());
    }
    Ok((width, height))
}

#[cfg(target_os = "linux")]
mod app {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};

    use otd_core::config::Profile;
    use otd_core::decoders::TabletDecoder;
    use otd_core::display::{DisplayFingerprint, DisplaySnapshot};
    use otd_core::endpoint_match;
    use otd_core::plugins::NoFilters;
    use otd_core::session::{self, Displays, Mode};
    use otd_core::spec::TabletSpec;
    use otd_core::tablets::{Database, ParserSupport, Role, parser_support};

    use crate::linux::{self, Device, Hidraw, Uinput, VirtualTablet};
    use otd_core::config::OutputKind;

    static STOP: AtomicBool = AtomicBool::new(false);

    const USAGE: &str = "Usage: opentabletdriver-rust-linux list\n       opentabletdriver-rust-linux run [--profile FILE] [--screen WIDTHxHEIGHT] [--tablet NAME]\n       opentabletdriver-rust-linux capture [--seconds 1..60] [--tablet NAME]\n       opentabletdriver-rust-linux --version\n\nRun discovers Hyprland, Sway or X11 monitors at startup. Other desktops need --screen.\nCapture reads reports for up to 10 seconds by default and never creates uinput output.\nBoth commands may send the tablet's configured initialization reports.\nLinux release setup: sudo ./setup/install.sh install\nSource checkout setup: sudo ./packaging/linux/install.sh install";

    struct StaticDisplays(DisplaySnapshot);

    impl Displays for StaticDisplays {
        fn fingerprint(&mut self) -> DisplayFingerprint {
            self.0.fingerprint()
        }

        fn snapshot(&mut self) -> Result<DisplaySnapshot, String> {
            Ok(self.0.clone())
        }
    }

    pub fn main() -> Result<(), String> {
        let mut arguments = std::env::args().skip(1);
        let command = arguments.next();
        match command.as_deref() {
            None | Some("--help" | "-h" | "help") => { println!("{USAGE}"); Ok(()) }
            Some("--version" | "-V" | "version") => {
                println!("opentabletdriver-rust-linux {}", env!("OTD_RELEASE_VERSION"));
                Ok(())
            }
            Some("list") => {
                if arguments.next().is_some() { return Err(USAGE.into()); }
                list()
            }
            Some("run" | "capture") => {
                let capture = command.as_deref() == Some("capture");
                let (mut profile_path, mut screen, mut tablet, mut seconds) = (None, None, None, 10);
                while let Some(argument) = arguments.next() {
                    let mut value = || arguments.next().ok_or(USAGE);
                    match argument.as_str() {
                        "--help" | "-h" => { println!("{USAGE}"); return Ok(()); }
                        "--profile" if !capture => profile_path = Some(PathBuf::from(value()?)),
                        "--screen" if !capture => screen = Some(crate::parse_screen(&value()?)?),
                        "--tablet" => tablet = Some(value()?),
                        "--seconds" if capture => {
                            seconds = value()?.parse::<u64>().map_err(|_| "--seconds must be 1..60")?;
                            if !(1..=60).contains(&seconds) { return Err("--seconds must be 1..60".into()); }
                        }
                        _ => return Err(USAGE.into()),
                    }
                }
                if capture { capture_tablet(tablet, seconds) } else { run(profile_path, screen, tablet) }
            }
            _ => Err(USAGE.into()),
        }
    }

    fn list() -> Result<(), String> {
        let database = Database::builtin();
        let devices =
            linux::enumerate(database).map_err(|e| format!("hidraw discovery failed: {e}"))?;
        if devices.is_empty() {
            println!("No hidraw devices.");
        }
        for device in &devices {
            let endpoint = &device.endpoint;
            // Permission is an access diagnostic, not device identity. Only
            // this discovery copy bypasses the core's access prerequisite.
            let mut identity = endpoint.clone();
            identity.can_open = true;
            let found = database
                .find(endpoint.vendor_id, endpoint.product_id)
                .find(|found| endpoint_match::matches(&identity, found).is_ok());
            let description = found.map_or_else(|| {
                let candidates: Vec<_> = database.find(endpoint.vendor_id, endpoint.product_id)
                    .map(|found| found.configuration.name.as_str()).collect();
                if candidates.is_empty() { "no matching configuration".to_owned() }
                else { format!("unverified configuration candidates: {}", candidates.join(", ")) }
            }, |found| format!("{} ({:?})", found.configuration.name, found.role));
            println!(
                "{} {:04x}:{:04x} input {} output {} feature {}{}: {}{}",
                device.node.display(),
                endpoint.vendor_id,
                endpoint.product_id,
                endpoint.input_length,
                endpoint.output_length,
                endpoint.feature_length,
                if endpoint.can_open {
                    ""
                } else {
                    " (no access)"
                },
                description,
                device.kernel_driver.as_deref().map_or(String::new(), |driver| format!("; kernel driver {driver}")),
            );
            for (index, error) in &device.string_errors {
                eprintln!("{} USB string {index} could not be read: {error}", device.node.display());
            }
        }
        Ok(())
    }

    struct Selected<'a> {
        device: &'a Device,
        configuration: otd_core::tablets::TabletConfiguration,
        identifier: otd_core::tablets::DeviceIdentifier,
        spec: TabletSpec,
    }

    /// The first digitizer endpoint, by sysfs path, that matches a usable
    /// configuration whose parser this driver supports, as on Windows.
    fn select<'a>(
        devices: &'a [Device],
        database: &Database,
        tablet: Option<&str>,
    ) -> Result<Option<Selected<'a>>, String> {
        let mut unsupported = None;
        for device in devices {
            let mut identity = device.endpoint.clone();
            identity.can_open = true;
            for found in database
                .find(device.endpoint.vendor_id, device.endpoint.product_id)
                .filter(|found| {
                    found.role == Role::Digitizer
                        && tablet.is_none_or(|name| found.configuration.name == name)
                })
            {
                if !device.endpoint.can_open {
                    match endpoint_match::matches(&identity, &found) {
                        Ok(()) | Err(endpoint_match::Rejection::MissingString(_)) => {
                            return Err(format!("permission denied for {} ({}); run sudo ./setup/install.sh install in the extracted release (source checkout: packaging/linux/install.sh), then replug the tablet and run as your normal user", device.node.display(), found.configuration.name));
                        }
                        Err(_) => continue,
                    }
                }
                match endpoint_match::matches(&identity, &found) {
                    Ok(()) => {}
                    Err(endpoint_match::Rejection::MissingString(index)) => {
                        if let Some(error) = device.string_errors.get(&index) {
                            unsupported = Some(format!("{} cannot be identified: USB string {index} could not be read: {error}", found.configuration.name));
                        }
                        continue;
                    }
                    Err(_) => continue,
                }
                if parser_support(found.identifier.parser()) == ParserSupport::Missing {
                    unsupported = Some(format!(
                        "{} uses {}, which this driver cannot decode",
                        found.configuration.name,
                        found.identifier.parser()
                    ));
                    continue;
                }
                match TabletSpec::from_configuration(found.configuration) {
                    Ok(spec) => {
                        return Ok(Some(Selected {
                            device,
                            configuration: found.configuration.clone(),
                            identifier: found.identifier.clone(),
                            spec,
                        }));
                    }
                    Err(error) => unsupported = Some(error),
                }
            }
        }
        unsupported.map_or(Ok(None), Err)
    }

    fn run(
        profile_path: Option<PathBuf>,
        screen: Option<(i32, i32)>,
        tablet: Option<String>,
    ) -> Result<(), String> {
        std::panic::set_hook(Box::new(|info| {
            eprintln!("{info}");
            STOP.store(true, Ordering::Release);
        }));
        install_stop_handler()?;
        let profile = profile_path.as_deref().map(|path| Profile::load(Some(path))).transpose()?;
        let profile_tablet = profile.as_ref().map(Profile::tablet_name).transpose()?.flatten();
        if let (Some(requested), Some(saved)) = (&tablet, &profile_tablet) {
            if requested != saved {
                return Err(format!("--tablet {requested} conflicts with the profile for {saved}"));
            }
        }
        let tablet = tablet.or(profile_tablet);
        let mut displays = StaticDisplays(match screen {
            Some(screen) => crate::displays::explicit(screen),
            None => crate::displays::discover()?,
        });
        eprintln!("Desktop at startup: {:?}, {} monitor(s). Restart after display changes.", displays.0.virtual_screen, displays.0.monitors.len());
        let database = otd_core::config::configured_tablets()?;
        let mut waiting = false;
        while !STOP.load(Ordering::Acquire) {
            let devices =
                linux::enumerate(&database).map_err(|e| format!("hidraw discovery failed: {e}"))?;
            let Some(selected) = select(&devices, &database, tablet.as_deref())? else {
                if !waiting {
                    eprintln!(
                        "Waiting for {}.",
                        tablet.as_deref().unwrap_or("a supported tablet")
                    );
                    waiting = true;
                }
                pause();
                continue;
            };
            waiting = false;
            if let Some(driver) = &selected.device.kernel_driver {
                eprintln!("Tablet is bound to kernel driver {driver}; simultaneous native output can move the pointer twice. See ./setup/install.sh status in the extracted release (source: packaging/linux/install.sh) and the Linux README for scoped libinput input suppression.");
            }
            let profile = match &profile {
                Some(profile) => profile.clone(),
                None => Profile::load_otd_tablet(&selected.configuration.name)?.unwrap_or_default(),
            }.for_tablet(selected.spec)?;
            profile.validate_runtime_tablet_in(&database)?;
            if profile.plugins.iter().any(|plugin| plugin.enabled) {
                return Err("external plugins are not supported by the Linux runtime; disable them explicitly".into());
            }
            // Reject deterministic mapping/output errors before initialization
            // writes, and do not repeatedly reinitialize on an unchanged fault.
            let _ = otd_core::pipeline::ReportPipeline::new(&profile)?;
            if profile.relative.is_none() { displays.0.mapper(&profile)?; }
            if let Err(error) = run_session(&selected, &profile, &mut displays) {
                if STOP.load(Ordering::Acquire) { break; }
                match error {
                    SessionError::Fatal(error) => {
                        return Err(format!("{} stopped: {error}", selected.configuration.name));
                    }
                    SessionError::Retry(error) => {
                        eprintln!("{} stopped: {error}; retrying after a device scan", selected.configuration.name);
                    }
                }
            }
            pause();
        }
        Ok(())
    }

    fn capture_tablet(tablet: Option<String>, seconds: u64) -> Result<(), String> {
        install_stop_handler()?;
        let database = otd_core::config::configured_tablets()?;
        let devices = linux::enumerate(&database).map_err(|error| format!("hidraw discovery failed: {error}"))?;
        let selected = select(&devices, &database, tablet.as_deref())?
            .ok_or("no matching supported tablet connected")?;
        let profile = Profile::default().for_tablet(selected.spec)?;
        let mut decoder = TabletDecoder::for_parser(selected.identifier.parser(), selected.spec)
            .ok_or("unsupported parser")?;
        let label = format!("{} ({})", selected.configuration.name, selected.device.node.display());
        let mut source = Hidraw::open(selected.device, label, &STOP).map_err(|error| error.to_string())?;
        linux::initialize(selected.device, source.file(), &selected.identifier, &selected.configuration, &STOP)
            .map_err(|error| format!("tablet initialization failed: {error}"))?;
        eprintln!("Capture for up to {seconds} seconds; no uinput device or input injection. Move the pen to collect reports.");
        // Capture does not use a display mapper. A tiny placeholder keeps the
        // session interface usable in headless diagnostics without discovery.
        let mut displays = StaticDisplays(crate::displays::explicit((1, 1)));
        session::run(&mut source, &mut displays, &profile,
            Mode::Capture { deadline: Instant::now() + Duration::from_secs(seconds), limit: 100_000 },
            &mut decoder, &mut NoFilters, |_| Err(std::io::Error::other("capture attempted injection")),
            &|line| eprintln!("{line}"))
            .map_err(|error| format!("capture failed: {error}"))
    }

    enum SessionError {
        Fatal(std::io::Error),
        Retry(std::io::Error),
    }

    impl SessionError {
        fn hardware(error: std::io::Error) -> Self {
            // Hotplug can recover on a new scan. Permissions need user setup.
            // Invalid requests and unreleased output ownership cannot.
            if session::is_cleanup_failure(&error)
                || matches!(error.kind(),
                    std::io::ErrorKind::InvalidInput
                        | std::io::ErrorKind::PermissionDenied
                        | std::io::ErrorKind::InvalidData
                        | std::io::ErrorKind::Unsupported)
            {
                Self::Fatal(error)
            } else {
                Self::Retry(error)
            }
        }
    }

    fn run_session(
        selected: &Selected<'_>,
        profile: &Profile,
        displays: &mut StaticDisplays,
    ) -> Result<(), SessionError> {
        let mut decoder = TabletDecoder::for_parser(selected.identifier.parser(), selected.spec)
            .ok_or_else(|| SessionError::Fatal(std::io::Error::other("unsupported parser")))?;
        let label = format!(
            "{} ({})",
            selected.configuration.name,
            selected.device.node.display()
        );
        let mut source = Hidraw::open(selected.device, label, &STOP)
            .map_err(SessionError::hardware)?;
        // Output creation can fail for missing uinput permissions. Establish it
        // before performing any device initialization writes.
        let pen = if profile.output == OutputKind::Pen {
            Some(VirtualTablet::create(displays.0.virtual_screen).map_err(SessionError::Fatal)?)
        } else { None };
        let output = if pen.is_none() {
            Some(Uinput::create(profile.relative.is_some()).map_err(SessionError::Fatal)?)
        } else { None };
        linux::initialize(
            selected.device,
            source.file(),
            &selected.identifier,
            &selected.configuration,
            &STOP,
        ).map_err(SessionError::hardware)?;
        if let Some(tablet) = pen {
            // Artist Mode: the virtual tablet replaces the pointer.
            return session::run_gated_with_pen(
                &mut source,
                displays,
                profile,
                Mode::Driver,
                &mut decoder,
                &mut NoFilters,
                |_| Ok(()),
                Some(Box::new(tablet)),
                &|line| eprintln!("{line}"),
                || Ok(true),
            ).map_err(SessionError::hardware);
        }
        let output = output.ok_or_else(|| SessionError::Fatal(std::io::Error::other("mouse output was not created")))?;
        session::run(
            &mut source,
            displays,
            profile,
            Mode::Driver,
            &mut decoder,
            &mut NoFilters,
            |packet| output.send(packet),
            &|line| eprintln!("{line}"),
        ).map_err(SessionError::hardware)
    }

    /// Waits two seconds between device scans, waking early on a stop.
    fn pause() {
        for _ in 0..20 {
            if STOP.load(Ordering::Acquire) {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    extern "C" fn on_signal(_: libc::c_int) {
        STOP.store(true, Ordering::Release);
    }

    fn install_stop_handler() -> Result<(), String> {
        for signal in [libc::SIGINT, libc::SIGTERM] {
            // SAFETY: the handler only stores to an atomic, which is
            // async-signal-safe.
            let previous =
                unsafe { libc::signal(signal, on_signal as *const () as libc::sighandler_t) };
            if previous == libc::SIG_ERR {
                return Err(std::io::Error::last_os_error().to_string());
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn screen_sizes_parse() {
        assert_eq!(super::parse_screen("2560x1440"), Ok((2560, 1440)));
        assert!(super::parse_screen("0x1080").is_err());
        assert!(super::parse_screen("wide").is_err());
    }
}
