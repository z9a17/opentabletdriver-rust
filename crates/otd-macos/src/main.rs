//! Native macOS runtime, multi-device control and optional original Eto services.
//! Hardware validation remains deferred.

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
mod descriptor;
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
mod usb;
#[cfg(target_os = "macos")]
mod ffi;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
mod pointer;
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
Daemon: daemon [--upstream-rpc | --upstream-pipe NAME]\n\
Original frontend: ui; native control: status/start/stop/shutdown/detect/request/console\n\
macOS 11+; grant Input Monitoring and Accessibility to your terminal.\n\
USB/Bluetooth HID and absolute/relative tablet mouse output; macOS hardware validation pending.";

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
    use otd_core::endpoint_match;
    use otd_core::mapping::Rect;
    use otd_platform::plugins::PluginChain;
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
        if command == "daemon" {
            install_signals()?;
            let options = otd_platform::cli::Options::parse(args)?;
            return otd_platform::cli::daemon(std::sync::Arc::new(NativePlatform::default()), options, &STOP);
        }
        if command == "update" {return otd_platform::cli::update(std::sync::Arc::new(NativePlatform::default()),args.collect());}
        if command == "plugins" { return otd_platform::plugin_catalog::run(args.collect()); }
        if command == "ui" { return otd_platform::cli::ui(args.collect()); }
        if matches!(command.as_str(),"original-console"|"otd") { return otd_platform::cli::original_console(args.collect()); }
        if matches!(command.as_str(), "status" | "start" | "stop" | "shutdown" | "detect" | "request" | "console") {
            return otd_platform::cli::control(&command, args.collect());
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
        let devices = macos::enumerate(&database, None, None).map_err(|error| error.to_string())?;
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
        let devices = macos::enumerate(&database, None, None).map_err(|error| error.to_string())?;
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
        identifier: DeviceIdentifier, spec: TabletSpec,auxiliary:Option<(&'a Device,DeviceIdentifier)>,
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
                if parser_support(found.identifier.parser()) == ParserSupport::Missing
                    && !otd_platform::dotnet::installed_report_parser(found.identifier.parser()) {
                    unsupported = Some(format!("{} uses unsupported parser {}", found.configuration.name, found.identifier.parser()));
                    continue;
                }
                match TabletSpec::from_configuration(found.configuration) {
                    Ok(spec) => return Ok(Some(Selected { device, configuration: found.configuration.clone(), identifier: found.identifier.clone(), spec,auxiliary:None })),
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
        if !capture{return otd_platform::cli::daemon(std::sync::Arc::new(NativePlatform{profile:requested_profile,tablet,screen:options.screen}),otd_platform::cli::Options::default(),&STOP);}
        let mut displays = NativeDisplays::new(options.screen).map_err(|error| error.to_string())?;
        let database = otd_core::config::configured_tablets()?;
        let deadline = capture.then(|| Instant::now() + Duration::from_secs(options.seconds));
        let mut waiting = false;
        while !STOP.load(Ordering::Acquire) {
            if deadline.is_some_and(|deadline| Instant::now() >= deadline) { return Err("capture deadline elapsed before a supported tablet was available".into()); }
            let devices = match macos::enumerate(&database, Some(&STOP), deadline) {
                Ok(devices) => devices,
                Err(error) if error.kind() == io::ErrorKind::Interrupted && STOP.load(Ordering::Acquire) => break,
                Err(error) => return Err(error.to_string()),
            };
            let device_path = requested_profile.as_ref().and_then(|profile| profile.device_path.as_deref());
            let Some(selected) = select(&devices, &database, tablet.as_deref(), device_path)? else {
                if !waiting { eprintln!("Waiting for {}.", tablet.as_deref().unwrap_or("a supported USB tablet")); waiting = true; }
                pause(deadline);
                continue;
            };
            waiting = false;
            let profile = match &requested_profile {
                Some(profile) => profile.clone(),
                None => otd_platform::plugins::load_original_tablet_profile(&selected.configuration.name)?.unwrap_or_default(),
            }.for_tablet(selected.spec)?;
            otd_platform::plugins::validate_runtime_profile(&profile)?;
            if profile.output == OutputKind::Pen {
                return Err("the pinned macOS output contract is mouse/keyboard; Windows Ink and Linux Artist Mode pen output are unavailable on macOS".into());
            }
            let _ = otd_core::pipeline::ReportPipeline::new(&profile)?;
            if profile.relative.is_none() { displays.snapshot()?.mapper(&profile)?; }
            let mode = deadline.map_or(Mode::Driver, |deadline| Mode::Capture { deadline, limit: options.limit });
            match run_session(&selected, &profile, &mut displays, mode, &STOP, None) {
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

    struct SourceEndpoint<'a>{endpoint:&'a otd_core::endpoint_match::Endpoint,native:Option<&'a Device>,custom:Option<u64>}
    impl<'a> SourceEndpoint<'a>{
        fn open(&self,configuration:&TabletConfiguration,identifier:&DeviceIdentifier,auxiliary:bool,gate:std::sync::Arc<otd_platform::shared_devices::OutputGate>,epoch:Option<u64>,stop:&'a AtomicBool)->io::Result<otd_platform::managed_source::Input<'a,HidSource<'a>>>{
            if let Some(token)=self.custom{return otd_platform::managed_source::Source::open(token,self.endpoint,configuration,identifier,auxiliary,gate,epoch,stop).map(otd_platform::managed_source::Input::Managed);}
            let device=self.native.ok_or_else(||io::Error::new(io::ErrorKind::NotFound,"Exact native endpoint disappeared"))?;
            let mut source=HidSource::open(device,format!("{} ({})",configuration.name,self.endpoint.path),stop)?;source.attach(configuration,identifier,auxiliary,gate,epoch)?;Ok(otd_platform::managed_source::Input::Native(source))
        }
        fn initialize(&self,source:&mut otd_platform::managed_source::Input<'_,HidSource<'_>>,identifier:&DeviceIdentifier,configuration:&TabletConfiguration,deadline:Option<Instant>)->io::Result<()>{match source{
            otd_platform::managed_source::Input::Native(source)=>source.initialize(identifier,configuration,deadline),
            otd_platform::managed_source::Input::Managed(source)=>source.initialize(identifier,configuration,self.endpoint),
        }}
    }
    struct RunSelection<'a>{device:SourceEndpoint<'a>,configuration:TabletConfiguration,identifier:DeviceIdentifier,spec:TabletSpec,auxiliary:Option<(SourceEndpoint<'a>,DeviceIdentifier)>}
    fn run_session(selected:&Selected<'_>,profile:&Profile,displays:&mut NativeDisplays,mode:Mode,stop:&AtomicBool,context:Option<&otd_platform::daemon::WorkerContext>)->io::Result<()>{
        let selected=RunSelection{device:SourceEndpoint{endpoint:&selected.device.endpoint,native:Some(selected.device),custom:None},configuration:selected.configuration.clone(),identifier:selected.identifier.clone(),spec:selected.spec,
            auxiliary:selected.auxiliary.as_ref().map(|(device,identifier)|(SourceEndpoint{endpoint:&device.endpoint,native:Some(device),custom:None},identifier.clone()))};
        run_selection(&selected,profile,displays,mode,stop,context)
    }
    fn run_selection(selected: &RunSelection<'_>, profile: &Profile, displays: &mut NativeDisplays, mode: Mode, stop: &AtomicBool, context: Option<&otd_platform::daemon::WorkerContext>) -> io::Result<()> {
        otd_platform::display::set_snapshot(displays.snapshot().map_err(io::Error::other)?);
        otd_platform::action_output::set_supports(|action| match action {
            otd_core::actions::Action::Mouse(_) => true,
            otd_core::actions::Action::Key(key) => crate::keymap::key_code(key).is_some(),
        });
        // Establish output permission/resources before any hardware init writes.
        let mouse = if matches!(mode, Mode::Driver) {
            Some(std::rc::Rc::new(std::cell::RefCell::new(Mouse::new(displays.geometry.clone())?)))
        } else { None };
        if let Some(context) = context { context.activate().map_err(io::Error::other)?; }
        let gate=otd_platform::shared_devices::OutputGate::new()?;
        let mut source=selected.device.open(&selected.configuration,&selected.identifier,false,gate.clone(),context.map(|context|context.reader_generation),stop)?;
        let mut auxiliary=selected.auxiliary.as_ref().and_then(|(device,identifier)|{
            let opened=(||{let mut auxiliary=device.open(&selected.configuration,identifier,true,gate.clone(),context.map(|context|context.reader_generation),stop)?;
                device.initialize(&mut auxiliary,identifier,&selected.configuration,match mode{Mode::Capture{deadline,..}=>Some(deadline),Mode::Driver=>None})?;auxiliary.initialized();Ok::<_,io::Error>(auxiliary)})();
            match opened{Ok(source)=>Some((source,identifier.clone())),Err(error)=>{eprintln!("Auxiliary endpoint unavailable: {error}");None}}
        });
        let mut identifiers=vec![selected.identifier.clone()];if let Some((_,identifier))=&auxiliary{identifiers.push(identifier.clone());}
        let tablet=serde_json::json!({"Properties":selected.configuration,"Identifiers":identifiers});source.tablet(tablet.clone());if let Some((source,_))=&auxiliary{source.tablet(tablet);}
        otd_platform::managed_host::publish_owned_devices();
        let mut plugins = if matches!(mode, Mode::Driver) {
            PluginChain::load_for_profile_with_identifiers(profile, &selected.configuration, &identifiers)
        } else { PluginChain::load_with_tablet(&[], &selected.configuration) }.map_err(io::Error::other)?;
        let mut decoder = plugins.source_decoder(selected.identifier.parser(), selected.spec).map_err(io::Error::other)?;

        let mut auxiliary_decoder=auxiliary.as_ref().map(|(_,identifier)|plugins.source_decoder_for_endpoint(identifier.parser(),selected.spec,true)).transpose().map_err(io::Error::other)?;
        let actions = mouse.as_ref().map(|mouse| plugins.wrap_action_sink(profile, &selected.configuration,
            crate::macos::action_sink(std::rc::Rc::clone(mouse)).map_err(|error|error.to_string())?)).transpose().map_err(io::Error::other)?;
        let actions=actions.map(|sink|match context{Some(context)=>context.actions(sink,profile.binding_inhibit),None=>sink});
        selected.device.initialize(&mut source,&selected.identifier, &selected.configuration,
            match mode { Mode::Capture { deadline, .. } => Some(deadline), Mode::Driver => None })?;
        let _debug = otd_core::debug::Registration::with_selection_key(
            otd_core::debug::Device { name: selected.configuration.name.clone(), parser: selected.identifier.parser().into() },
            selected.device.endpoint.input_length.max(selected.auxiliary.as_ref().map_or(0,|(device,_)|device.endpoint.input_length)) as usize, auxiliary.as_ref().map(|(_,identifier)|identifier.parser().to_owned()), context.map(|context| context.id.clone()));
        source.initialized();
        let source=otd_platform::paired_source::PairedSource::new(source,auxiliary.take().map(|(source,_)|source),otd_platform::managed_source::Input::wait_pair);
        let mut source=otd_platform::daemon::LifecycleSource::new(source,context);
        if let ParserSupport::Partial(reason) = parser_support(selected.identifier.parser()) {
            eprintln!("Partial parser support: {reason}");
        }
        eprintln!("macOS native CLI: hardware validation pending; Ctrl+C stops and releases contact.");
        let _realtime = matches!(mode, Mode::Driver).then(crate::realtime::TimeConstraint::raise);
        let _tools = (matches!(mode, Mode::Driver) && context.is_none()).then(|| otd_platform::plugins::Tools::start(&profile.plugins, |line| eprintln!("{line}")));
        let result=session::run_gated_with_endpoints(&mut source, displays, profile, mode, &mut decoder, auxiliary_decoder.as_mut().map(|decoder|decoder as &mut dyn otd_core::decoders::PenDecoder), &mut plugins,
            |packet| match &mouse { Some(mouse) => mouse.borrow_mut().send(packet), None => Ok(()) },
            None, actions, &|line| eprintln!("{line}"), || Ok(true));
        source.source_mut().close(|source|source.close()).and(result)
    }

    #[derive(Default)]
    struct NativePlatform{profile:Option<Profile>,screen:Option<Rect>,tablet:Option<String>}
    impl otd_platform::daemon::Platform for NativePlatform {
        fn default_profile(&self,tablet:&str)->Result<Option<Profile>,String>{if self.tablet.as_deref().is_none_or(|name|name==tablet){Ok(self.profile.clone())}else{Ok(None)}}
        fn startup_tools(&self)->Option<Vec<otd_core::plugins::PluginConfig>>{self.profile.as_ref().map(|profile|profile.plugins.iter().filter(|config|config.kind==otd_core::plugins::PluginKind::DotnetTool).cloned().collect())}
        fn prepare_start(&self) -> Result<(),String> {
            let database=otd_core::config::configured_tablets()?;
            let endpoints=macos::enumerate(&database,None,None).map_err(|error|error.to_string())?.into_iter().map(|device|device.endpoint.clone()).collect::<Vec<_>>();
            otd_platform::daemon::prepare_connected(&database,&endpoints)
        }
        fn discover(&self) -> Result<Vec<otd_platform::daemon::Device>,String> {
            let database=otd_core::config::configured_tablets()?;
            let mut endpoints=macos::enumerate(&database,None,None).map_err(|error|error.to_string())?.into_iter().map(|device|device.endpoint.clone()).collect::<Vec<_>>();
            let custom=otd_platform::managed_source::endpoints(&database)?;endpoints.extend(custom.iter().map(|(endpoint,_)|endpoint.clone()));
            otd_platform::daemon::discover(&database,&endpoints).map(|devices|devices.into_iter().filter(|device|self.tablet.as_deref().is_none_or(|name|device.configuration.name==name)&&self.profile.as_ref().and_then(|profile|profile.device_path.as_deref()).is_none_or(|path|device.endpoint.path==path)).map(|mut device|{
                device.custom_endpoint=custom.iter().find(|(endpoint,_)|endpoint.path==device.endpoint.path&&endpoint.physical_id==device.endpoint.physical_id).map(|(_,token)|*token);
                device.auxiliary_custom_endpoint=device.auxiliary.as_ref().and_then(|(aux,_)|custom.iter().find(|(endpoint,_)|endpoint.path==aux.path&&endpoint.physical_id==aux.physical_id).map(|(_,token)|*token));device
            }).collect())
        }
        fn screen(&self) -> Result<Rect,String> {
            let mut displays=NativeDisplays::new(self.screen).map_err(|error|error.to_string())?;
            Ok(displays.snapshot()?.virtual_screen)
        }
        fn inventory(&self) -> Result<serde_json::Value,String> {
            let database=otd_core::config::configured_tablets()?;
            let devices=macos::enumerate(&database,None,None).map_err(|error|error.to_string())?;
            let mut inventory=macos::rpc_inventory(&devices).as_array().cloned().ok_or("Native inventory is not an array")?;inventory.extend(otd_platform::managed_source::inventory()?);Ok(serde_json::json!(inventory))
        }
        fn run(&self,device:otd_platform::daemon::Device,profile:Profile,context:otd_platform::daemon::WorkerContext) -> Result<(),String> {
            if profile.output==OutputKind::Pen{return Err("Pinned macOS output is mouse/keyboard; Artist Mode/Ink output is unavailable".into());}
            let database=otd_core::config::configured_tablets()?;
            let devices=macos::enumerate(&database,Some(&context.stop),None).map_err(|error|error.to_string())?;
            let actual=devices.iter().find(|actual|actual.endpoint.path==device.endpoint.path&&actual.endpoint.physical_id==device.endpoint.physical_id);
            if actual.is_none()&&device.custom_endpoint.is_none(){return Err("Exact physical endpoint disconnected before preparation".into());}
            let auxiliary=device.auxiliary.as_ref().and_then(|(endpoint,identifier)|{let native=devices.iter().find(|candidate|candidate.endpoint.path==endpoint.path&&candidate.endpoint.physical_id==endpoint.physical_id);
                (native.is_some()||device.auxiliary_custom_endpoint.is_some()).then_some((SourceEndpoint{endpoint,native,custom:device.auxiliary_custom_endpoint},identifier.clone()))});
            let selected=RunSelection {device:SourceEndpoint{endpoint:&device.endpoint,native:actual,custom:device.custom_endpoint},spec:TabletSpec::from_configuration(&device.configuration)?,configuration:device.configuration,identifier:device.identifier,auxiliary};
            let mut displays=NativeDisplays::new(self.screen).map_err(|error|error.to_string())?;
            run_selection(&selected,&profile,&mut displays,Mode::Driver,&context.stop,Some(&context)).map_err(|error|error.to_string())
        }
        fn device_string(&self,vendor:u16,product:u16,index:u8)->Result<String,String>{
            let database=otd_core::config::configured_tablets()?;
            for device in macos::enumerate(&database,None,None).map_err(|e|e.to_string())?.iter().filter(|device|device.endpoint.vendor_id==vendor&&device.endpoint.product_id==product){
                if let Some(value)=otd_platform::shared_devices::device_string(&device.endpoint.path,index){return value;}
                return device.indexed_string(index).map_err(|e|e.to_string());
            }
            for device in otd_platform::dotnet::custom_devices::snapshot()?.iter().filter(|device|device.vendor==vendor&&device.product==product){return otd_platform::dotnet::custom_devices::device_string(device.endpoint,index);}
            Err("No endpoint matches the requested vendor/product".into())
        }
        fn service_io(&self,request:otd_platform::managed_services::Request) -> Result<serde_json::Value,String> {
            if matches!(request.operation,otd_platform::managed_services::Operation::InputHold|otd_platform::managed_services::Operation::InputRelease){
                if request.operation==otd_platform::managed_services::Operation::InputHold&&request.payload["type"]!="renew"{macos::ensure_shared_inputs().map_err(|error|error.to_string())?;}
                return otd_platform::input_owner::execute(request.operation,request.scope,&request.payload);
            }
            otd_platform::shared_devices::execute(request.operation,request.scope,&request.payload)
        }
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
