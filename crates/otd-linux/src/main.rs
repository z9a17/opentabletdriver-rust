//! Linux native hidraw/uinput runtime with multi-device control and optional
//! original Eto/StreamJsonRpc services. Artist Mode stays in the portable core.
//! Runtime and hardware validation remain deferred to the owner.

// Portable parsing and event framing, unit-tested on every platform.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod artist;
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod descriptor;
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod keymap;
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod sysfs;

#[cfg(target_os = "linux")]
mod displays;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
mod realtime;

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
    use otd_platform::plugins::PluginChain;
    use otd_core::session::{self, Displays, Mode};
    use otd_core::spec::TabletSpec;
    use otd_core::tablets::{Database, ParserSupport, Role, parser_support};

    use crate::linux::{self, Device, Hidraw, Uinput, VirtualKeyboard, VirtualTablet};
    use otd_core::config::OutputKind;
    use otd_core::output::buttons::ButtonAction;

    static STOP: AtomicBool = AtomicBool::new(false);

    const USAGE: &str = "Usage: opentabletdriver-rust-linux list\n       opentabletdriver-rust-linux run [--profile FILE] [--screen WIDTHxHEIGHT] [--tablet NAME]\n       opentabletdriver-rust-linux capture [--seconds 1..60] [--tablet NAME]\n       opentabletdriver-rust-linux --version\n\nRun discovers Hyprland, Sway or X11 monitors at startup. Other desktops need --screen.\nCapture reads reports for up to 10 seconds by default and never creates uinput output.\nBoth commands may send the tablet's configured initialization reports.\nDaemon: daemon [--upstream-rpc | --upstream-pipe NAME]\nOriginal frontend: ui\nNative control: status/start/stop/shutdown/detect/request/console\nLinux release setup: sudo ./setup/install.sh install\nSource checkout setup: sudo ./packaging/linux/install.sh install";

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
            Some("daemon") => {
                install_stop_handler()?;
                let options = otd_platform::cli::Options::parse(arguments)?;
                otd_platform::cli::daemon(std::sync::Arc::new(NativePlatform::default()), options, &STOP)
            }
            Some("ui") => otd_platform::cli::ui(arguments.collect()),
            Some("original-console" | "otd") => otd_platform::cli::original_console(arguments.collect()),
            Some("plugins") => otd_platform::plugin_catalog::run(arguments.collect()),
            Some("status" | "start" | "stop" | "shutdown" | "detect" | "request" | "console") =>
                otd_platform::cli::control(command.as_deref().unwrap(), arguments.collect()),
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
        auxiliary:Option<(&'a Device,otd_core::tablets::DeviceIdentifier)>,
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
                if parser_support(found.identifier.parser()) == ParserSupport::Missing
                    && !otd_platform::dotnet::installed_report_parser(found.identifier.parser()) {
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
                            spec,auxiliary:None,
                        }));
                    }
                    Err(error) => unsupported = Some(error),
                }
            }
        }
        unsupported.map_or(Ok(None), Err)
    }

    fn run(profile_path:Option<PathBuf>,screen:Option<(i32,i32)>,tablet:Option<String>)->Result<(),String>{
        install_stop_handler()?;
        let profile=profile_path.as_deref().map(|path|Profile::load(Some(path))).transpose()?;
        let profile_tablet=profile.as_ref().map(Profile::tablet_name).transpose()?.flatten();
        if let(Some(requested),Some(saved))=(&tablet,&profile_tablet){if requested!=saved{return Err(format!("--tablet {requested} conflicts with profile for {saved}"));}}
        otd_platform::cli::daemon(std::sync::Arc::new(NativePlatform{profile,screen,tablet:tablet.or(profile_tablet)}),otd_platform::cli::Options::default(),&STOP)
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

    struct SourceEndpoint<'a>{endpoint:&'a otd_core::endpoint_match::Endpoint,native:Option<&'a Device>,custom:Option<u64>}
    impl<'a> SourceEndpoint<'a>{
        fn open(&self,configuration:&otd_core::tablets::TabletConfiguration,identifier:&otd_core::tablets::DeviceIdentifier,auxiliary:bool,gate:std::sync::Arc<otd_platform::shared_devices::OutputGate>,epoch:Option<u64>,stop:&'a AtomicBool)->std::io::Result<otd_platform::managed_source::Input<'a,Hidraw<'a>>>{
            if let Some(token)=self.custom{return otd_platform::managed_source::Source::open(token,self.endpoint,configuration,identifier,auxiliary,gate,epoch,stop).map(otd_platform::managed_source::Input::Managed);}
            let device=self.native.ok_or_else(||std::io::Error::new(std::io::ErrorKind::NotFound,"Exact native endpoint disappeared"))?;
            let mut source=Hidraw::open(device,format!("{} ({})",configuration.name,self.endpoint.path),stop)?;source.attach(device,configuration,identifier,auxiliary,gate,epoch)?;Ok(otd_platform::managed_source::Input::Native(source))
        }
        fn initialize(&self,source:&mut otd_platform::managed_source::Input<'_,Hidraw<'_>>,identifier:&otd_core::tablets::DeviceIdentifier,configuration:&otd_core::tablets::TabletConfiguration,stop:&AtomicBool)->std::io::Result<()>{match source{
            otd_platform::managed_source::Input::Native(source)=>linux::initialize(self.native.ok_or_else(||std::io::Error::other("Native endpoint metadata missing"))?,source.file(),identifier,configuration,stop),
            otd_platform::managed_source::Input::Managed(source)=>source.initialize(identifier,configuration,self.endpoint),
        }}
    }
    struct RunSelection<'a>{device:SourceEndpoint<'a>,configuration:otd_core::tablets::TabletConfiguration,identifier:otd_core::tablets::DeviceIdentifier,spec:TabletSpec,auxiliary:Option<(SourceEndpoint<'a>,otd_core::tablets::DeviceIdentifier)>}
    fn run_session(
        selected: &RunSelection<'_>,
        profile: &Profile,
        displays: &mut StaticDisplays,
        stop: &AtomicBool,
        context: Option<&otd_platform::daemon::WorkerContext>,
    ) -> Result<(), SessionError> {
        otd_platform::display::set_snapshot(displays.0.clone());
        otd_platform::action_output::set_supports(|action| match action {
            otd_core::actions::Action::Mouse(_) => true,
            otd_core::actions::Action::Key(key) => crate::keymap::key_code(key).is_some(),
        });

        // Establish every output resource before initialization writes. Permission
        // failures are fatal; reconnect only retries actual hardware loss.
        // Resource discovery covers every binding group, including Artist Mode
        // profiles whose pointer is otherwise absent.
        let non_pen_actions = || profile.aux_buttons.iter().chain(&profile.mouse_buttons)
            .chain([&profile.mouse_scroll_up, &profile.mouse_scroll_down])
            .chain(profile.wheels.iter().flat_map(|wheel| [&wheel.clockwise, &wheel.counter_clockwise]
                .into_iter().chain(wheel.buttons.iter())));
        let actions = || profile.pen_buttons.iter().chain(non_pen_actions());
        fn inner(action: &ButtonAction) -> &ButtonAction {
            match action { ButtonAction::Toggle(inner) => inner.as_ref(), action => action }
        }
        let managed = !otd_core::output::buttons::ButtonOutput::managed_slots(profile).is_empty() || profile.managed_output.is_some();
        let keys = managed || actions().any(|action| matches!(inner(action), ButtonAction::Keys(_)));
        let clicks = managed || actions().any(|action| matches!(inner(action), ButtonAction::Mouse(_) | ButtonAction::Scroll(_)))
            || non_pen_actions().any(|action| matches!(inner(action), ButtonAction::Barrel(1 | 2)));
        let keyboard = keys.then(VirtualKeyboard::create).transpose().map_err(SessionError::Fatal)?;
        if keys{linux::ensure_shared_inputs().map_err(SessionError::Fatal)?;}
        let pen = if profile.output == OutputKind::Pen {
            Some(VirtualTablet::create(displays.0.virtual_screen).map_err(SessionError::Fatal)?)
        } else { None };
        let pointer = if pen.is_none() || clicks {
            Some(std::rc::Rc::new(Uinput::create(pen.is_some() || profile.relative.is_some()).map_err(SessionError::Fatal)?))
        } else { None };
        if let Some(context) = context { context.activate().map_err(|error| SessionError::Fatal(std::io::Error::other(error)))?; }
        let gate=otd_platform::shared_devices::OutputGate::new().map_err(SessionError::Fatal)?;
        let mut source=selected.device.open(&selected.configuration,&selected.identifier,false,gate.clone(),context.map(|context|context.reader_generation),stop).map_err(SessionError::hardware)?;
        let mut auxiliary=selected.auxiliary.as_ref().and_then(|(device,identifier)|{
            let opened=(||{let mut auxiliary=device.open(&selected.configuration,identifier,true,gate.clone(),context.map(|context|context.reader_generation),stop)?;
                device.initialize(&mut auxiliary,identifier,&selected.configuration,stop)?;auxiliary.initialized();Ok::<_,std::io::Error>(auxiliary)})();
            match opened{Ok(source)=>Some((source,identifier.clone())),Err(error)=>{eprintln!("Auxiliary endpoint unavailable: {error}");None}}
        });
        let mut identifiers=vec![selected.identifier.clone()];if let Some((_,identifier))=&auxiliary{identifiers.push(identifier.clone());}
        let tablet=serde_json::json!({"Properties":selected.configuration,"Identifiers":identifiers});source.tablet(tablet.clone());if let Some((source,_))=&auxiliary{source.tablet(tablet);}
        otd_platform::managed_host::publish_owned_devices();
        let mut plugins = PluginChain::load_for_profile_with_identifiers(profile, &selected.configuration,
            &identifiers).map_err(|error| SessionError::Fatal(std::io::Error::other(error)))?;
        let mut decoder = plugins.source_decoder(selected.identifier.parser(), selected.spec)
            .map_err(|error| SessionError::Fatal(std::io::Error::other(error)))?;
        let mut auxiliary_decoder=auxiliary.as_ref().map(|(_,identifier)|plugins.source_decoder_for_endpoint(identifier.parser(),selected.spec,true)).transpose()
            .map_err(|error|SessionError::Fatal(std::io::Error::other(error)))?;
        let actions = plugins.wrap_action_sink(profile, &selected.configuration, linux::action_sink(pointer.clone(), keyboard).map_err(SessionError::Fatal)?)
            .map_err(|error| SessionError::Fatal(std::io::Error::other(error)))?;
        let actions=match context{Some(context)=>context.actions(actions,profile.binding_inhibit),None=>actions};
        selected.device.initialize(&mut source,&selected.identifier,&selected.configuration,stop).map_err(SessionError::hardware)?;
        source.initialized();
        let _debug = otd_core::debug::Registration::with_selection_key(
            otd_core::debug::Device { name: selected.configuration.name.clone(), parser: selected.identifier.parser().into() },
            selected.device.endpoint.input_length.max(selected.auxiliary.as_ref().map_or(0,|(device,_)|device.endpoint.input_length)) as usize, auxiliary.as_ref().map(|(_,identifier)|identifier.parser().to_owned()), context.map(|context| context.id.clone()));
        let source=otd_platform::paired_source::PairedSource::new(source,auxiliary.take().map(|(source,_)|source),otd_platform::managed_source::Input::wait_pair);
        let mut source=otd_platform::daemon::LifecycleSource::new(source,context);
        let _realtime = crate::realtime::RealtimePriority::raise();
        let _tools = context.is_none().then(|| otd_platform::plugins::Tools::start(&profile.plugins, |line| eprintln!("{line}")));
        let result=if let Some(tablet) = pen {
            session::run_gated_with_endpoints(
                &mut source, displays, profile, Mode::Driver,
                &mut decoder, auxiliary_decoder.as_mut().map(|decoder|decoder as &mut dyn otd_core::decoders::PenDecoder), &mut plugins, |_| Ok(()), Some(Box::new(tablet)),
                Some(actions), &|line| eprintln!("{line}"), || Ok(true),
            )
        }else{
        let output = pointer.ok_or_else(|| SessionError::Fatal(std::io::Error::other("mouse output was not created")))?;
        let sender = std::rc::Rc::clone(&output);
        session::run_gated_with_endpoints(
            &mut source, displays, profile, Mode::Driver, &mut decoder, auxiliary_decoder.as_mut().map(|decoder|decoder as &mut dyn otd_core::decoders::PenDecoder),
            &mut plugins, move |packet| sender.send(packet), None,
            Some(actions),
            &|line| eprintln!("{line}"), || Ok(true),
        )};
        let cleanup=source.source_mut().close(|source|source.close());
        cleanup.and(result).map_err(SessionError::hardware)
    }

    #[derive(Default)]
    struct NativePlatform{profile:Option<Profile>,screen:Option<(i32,i32)>,tablet:Option<String>}
    impl otd_platform::daemon::Platform for NativePlatform {
        fn default_profile(&self,tablet:&str)->Result<Option<Profile>,String>{if self.tablet.as_deref().is_none_or(|name|name==tablet){Ok(self.profile.clone())}else{Ok(None)}}
        fn startup_tools(&self)->Option<Vec<otd_core::plugins::PluginConfig>>{self.profile.as_ref().map(|profile|profile.plugins.iter().filter(|config|config.kind==otd_core::plugins::PluginKind::DotnetTool).cloned().collect())}
        fn prepare_start(&self) -> Result<(),String> {
            let database=otd_core::config::configured_tablets()?;
            let endpoints=linux::enumerate(&database).map_err(|error|error.to_string())?.into_iter().map(|device|device.endpoint).collect::<Vec<_>>();
            otd_platform::daemon::prepare_connected(&database,&endpoints)
        }
        fn discover(&self) -> Result<Vec<otd_platform::daemon::Device>,String> {
            let database=otd_core::config::configured_tablets()?;
            let mut endpoints=linux::enumerate(&database).map_err(|error|error.to_string())?.into_iter().map(|device|device.endpoint).collect::<Vec<_>>();
            let custom=otd_platform::managed_source::endpoints(&database)?;endpoints.extend(custom.iter().map(|(endpoint,_)|endpoint.clone()));
            otd_platform::daemon::discover(&database,&endpoints).map(|devices|devices.into_iter().filter(|device|self.tablet.as_deref().is_none_or(|name|device.configuration.name==name)&&self.profile.as_ref().and_then(|profile|profile.device_path.as_deref()).is_none_or(|path|device.endpoint.path==path)).map(|mut device|{
                device.custom_endpoint=custom.iter().find(|(endpoint,_)|endpoint.path==device.endpoint.path&&endpoint.physical_id==device.endpoint.physical_id).map(|(_,token)|*token);
                device.auxiliary_custom_endpoint=device.auxiliary.as_ref().and_then(|(aux,_)|custom.iter().find(|(endpoint,_)|endpoint.path==aux.path&&endpoint.physical_id==aux.physical_id).map(|(_,token)|*token));device
            }).collect())
        }
        fn screen(&self) -> Result<otd_core::mapping::Rect,String> { Ok(match self.screen{Some(screen)=>crate::displays::explicit(screen),None=>crate::displays::discover()?}.virtual_screen) }
        fn inventory(&self) -> Result<serde_json::Value,String> {
            let database=otd_core::config::configured_tablets()?;
            let endpoints=linux::enumerate(&database).map_err(|error|error.to_string())?.into_iter().map(|device|device.endpoint).collect::<Vec<_>>();
            let mut inventory=otd_platform::daemon::inventory(&endpoints).as_array().cloned().ok_or("Native inventory is not an array")?;inventory.extend(otd_platform::managed_source::inventory()?);Ok(serde_json::json!(inventory))
        }
        fn run(&self,device:otd_platform::daemon::Device,profile:Profile,context:otd_platform::daemon::WorkerContext) -> Result<(),String> {
            let database=otd_core::config::configured_tablets()?;
            let devices=linux::enumerate(&database).map_err(|error|error.to_string())?;
            let actual=devices.iter().find(|actual|actual.endpoint.path==device.endpoint.path&&actual.endpoint.physical_id==device.endpoint.physical_id);
            if actual.is_none()&&device.custom_endpoint.is_none(){return Err("Exact physical endpoint disconnected before preparation".into());}
            let auxiliary=device.auxiliary.as_ref().and_then(|(endpoint,identifier)|{let native=devices.iter().find(|candidate|candidate.endpoint.path==endpoint.path&&candidate.endpoint.physical_id==endpoint.physical_id);
                (native.is_some()||device.auxiliary_custom_endpoint.is_some()).then_some((SourceEndpoint{endpoint,native,custom:device.auxiliary_custom_endpoint},identifier.clone()))});
            let selected=RunSelection {device:SourceEndpoint{endpoint:&device.endpoint,native:actual,custom:device.custom_endpoint},spec:TabletSpec::from_configuration(&device.configuration)?,configuration:device.configuration,identifier:device.identifier,auxiliary};
            let mut displays=StaticDisplays(match self.screen{Some(screen)=>crate::displays::explicit(screen),None=>crate::displays::discover()?});
            run_session(&selected,&profile,&mut displays,&context.stop,Some(&context)).map_err(|error|match error{SessionError::Fatal(error)|SessionError::Retry(error)=>error.to_string()})
        }
        fn service_io(&self,request:otd_platform::managed_services::Request) -> Result<serde_json::Value,String> {
            if matches!(request.operation,otd_platform::managed_services::Operation::InputHold|otd_platform::managed_services::Operation::InputRelease){
                if request.operation==otd_platform::managed_services::Operation::InputHold&&request.payload["type"]!="renew"{linux::ensure_shared_inputs().map_err(|error|error.to_string())?;}
                return otd_platform::input_owner::execute(request.operation,request.scope,&request.payload);
            }
            otd_platform::shared_devices::execute(request.operation,request.scope,&request.payload)
        }
    }

    /// Waits two seconds between device scans. A stop signal ends the wait:
    /// poll is never restarted after a signal handler, while
    /// `std::thread::sleep` would sleep on.
    fn pause() {
        if !STOP.load(Ordering::Acquire) {
            // SAFETY: no descriptors, only the timeout.
            unsafe { libc::poll(std::ptr::null_mut(), 0, 2_000) };
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
