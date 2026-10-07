//! Owned managed output/binding endpoints. CLR code never holds native scopes.
use std::collections::VecDeque;
use std::ffi::c_void;
use std::io;
use std::time::Duration;
use otd_core::actions::{Action, KeyboardUsage, MouseButton};
use otd_core::config::{OutputKind, Profile};
use otd_core::output::buttons::{ActionSink, ButtonOutput, ScrollAxis, ScrollPulse};
use otd_core::plugins::{ManagedCommand, PluginConfig};
use otd_core::reports::{ReportKind, ReportValues};
use otd_core::tablets::{DeviceIdentifier, TabletConfiguration};
use super::GraphReport;

type Create = unsafe extern "C" fn(*const u8, usize) -> *mut c_void;
type Reports = unsafe extern "C" fn(*const *mut c_void, u32, *const GraphReport) -> i32;
type Set = unsafe extern "C" fn(*mut c_void, u32, i32) -> i32;
type Release = unsafe extern "C" fn(*mut c_void) -> i32;
type Drain = unsafe extern "C" fn(*mut c_void, *mut ManagedCommand, u32) -> i32;
type Next = unsafe extern "C" fn(*mut c_void) -> i64;
type Tick = unsafe extern "C" fn(*mut c_void) -> i32;
type Destroy = unsafe extern "C" fn(*mut c_void);
type Attach = unsafe extern "C" fn(*mut c_void, *mut c_void) -> i32;
pub(super) struct Api {
    create_binding: Create, create_output: Create, reports: Reports, set: Set,
    release: Release, drain: Drain, next: Next, tick: Tick, destroy: Destroy, attach: Attach,
}
impl Api {
    pub(super) fn load(entry: &impl Fn(&str) -> Result<*mut c_void, String>) -> Result<Self, String> {
        Ok(Self { create_binding: unsafe { std::mem::transmute::<*mut c_void, Create>(entry("CreateBinding")?) },
            create_output: unsafe { std::mem::transmute::<*mut c_void, Create>(entry("CreateOutput")?) },
            reports: unsafe { std::mem::transmute::<*mut c_void, Reports>(entry("BindingsReport")?) },
            set: unsafe { std::mem::transmute::<*mut c_void, Set>(entry("BindingSet")?) },
            release: unsafe { std::mem::transmute::<*mut c_void, Release>(entry("BindingRelease")?) },
            drain: unsafe { std::mem::transmute::<*mut c_void, Drain>(entry("EndpointDrain")?) },
            next: unsafe { std::mem::transmute::<*mut c_void, Next>(entry("EndpointNextTick")?) },
            tick: unsafe { std::mem::transmute::<*mut c_void, Tick>(entry("EndpointTick")?) },
            destroy: unsafe { std::mem::transmute::<*mut c_void, Destroy>(entry("DestroyEndpoint")?) },
            attach: unsafe { std::mem::transmute::<*mut c_void, Attach>(entry("AttachOutput")?) } })
    }
}
fn api() -> Result<&'static Api, String> { super::bridge()?.endpoints.as_ref().ok_or_else(|| "The installed .NET bridge lacks unchanged output/binding support; replace data/compat with this release's files.".into()) }
fn envelope(config: &PluginConfig, profile: &Profile, tablet: &TabletConfiguration, owner: u32, identifiers: Option<&[DeviceIdentifier]>) -> Result<serde_json::Value, String> {
    config.validate()?;
    let keys: serde_json::Map<String, serde_json::Value> = if cfg!(windows) {
        otd_core::keys::windows_names().map(|(name, code)| (name.into(),
            serde_json::json!(KeyboardUsage::windows_virtual_key(code).map_or(0, KeyboardUsage::usage)))).collect()
    } else { otd_core::keys::names()
        .filter(|(_, key)| crate::action_output::supports(Action::Key(*key)))
        .map(|(name, usage)| (name.into(), serde_json::json!(usage.usage()))).collect() };
    Ok(serde_json::json!({ "assembly_path": config.path.canonicalize().map_err(|error| format!("{}: {error}", config.path.display()))?,
        "type_name": config.type_name, "settings": serde_json::from_str::<serde_json::Value>(&config.settings_json).map_err(|error| error.to_string())?,
        "tablet": tablet, "identifiers": identifiers, "source_session":super::source_session_json(), "pen": profile.output == OutputKind::Pen, "relative": profile.relative.is_some(), "owner": owner, "keys": keys }))
}
struct Endpoint { context: *mut c_void }
impl Endpoint {
    fn create(config: &serde_json::Value, output: bool) -> Result<Self, String> {
        let encoded = config.to_string();
        if encoded.len() > 262144 { return Err("managed endpoint configuration exceeds 256 KiB".into()); }
        let api = api()?;
        let create = if output { api.create_output } else { api.create_binding };
        let context = unsafe { create(encoded.as_ptr(), encoded.len()) };
        if context.is_null() { return Err(super::last_error()); }
        Ok(Self { context })
    }
    fn status(status: i32) -> io::Result<()> { if status == 0 { Ok(()) } else { Err(io::Error::other(super::last_error())) } }
    fn next(&self) -> io::Result<Duration> { let next = unsafe { (api().map_err(io::Error::other)?.next)(self.context) }; u64::try_from(next).map(Duration::from_micros).map_err(|_| io::Error::other(super::last_error())) }
    fn drain(&self, buffer: &mut [ManagedCommand; 256]) -> io::Result<usize> {
        let count = unsafe { (api().map_err(io::Error::other)?.drain)(self.context, buffer.as_mut_ptr(), 256) };
        if !(0..=256).contains(&count) { return Err(io::Error::other(super::last_error())); }
        Ok(count as usize)
    }
}
impl Drop for Endpoint { fn drop(&mut self) { if let Ok(api) = api() { unsafe { (api.destroy)(self.context) }; } } }

pub struct OutputSession { endpoint: Endpoint }
impl OutputSession {
    pub fn new(config: &PluginConfig, profile: &Profile, tablet: &TabletConfiguration) -> Result<Self, String> {
        Self::new_with_identifiers(config, profile, tablet, None)
    }
    pub(crate) fn new_with_identifiers(config: &PluginConfig, profile: &Profile, tablet: &TabletConfiguration,
        identifiers: Option<&[DeviceIdentifier]>) -> Result<Self, String> {
        Self::new_for_graph(config, profile, tablet, identifiers, None)
    }
    pub(crate) fn new_for_graph(config: &PluginConfig, profile: &Profile, tablet: &TabletConfiguration,
        identifiers: Option<&[DeviceIdentifier]>, graph: Option<&super::Graph>) -> Result<Self, String> {
        let mut value = envelope(config, profile, tablet, 4096, identifiers)?;
        if let Some(graph) = graph { value["graph_context"] = serde_json::json!(graph.context_handle() as usize); }
        value["disable_pressure"] = profile.contact.disable_pressure.into(); value["disable_tilt"] = profile.contact.disable_tilt.into();
        if let Some(relative) = profile.relative {
            value["sensitivity_x"] = serde_json::json!(relative.sensitivity.0); value["sensitivity_y"] = serde_json::json!(relative.sensitivity.1);
            value["rotation"] = serde_json::json!(relative.rotation); value["reset_ms"] = serde_json::json!(relative.reset_delay.as_secs_f64() * 1000.0);
        } else {
            let mapping = if let Some(mapping) = profile.otd_mapping { mapping } else {
                let display = crate::display::read_snapshot()?;
                let area = profile.monitor.map(|index| display.monitors.get(index).copied().ok_or_else(|| format!("monitor {index} is unavailable"))).transpose()?.unwrap_or(display.virtual_screen);
                let crop = if profile.crop == otd_core::mapping::Crop::default() { otd_core::mapping::Crop::full(profile.tablet) } else { profile.crop };
                let (sx, sy) = profile.tablet.mm_per_unit();
                otd_core::mapping::OtdMapping { tablet: otd_core::mapping::OtdArea { width: f64::from(crop.width) * sx, height: f64::from(crop.height) * sy,
                    x: (f64::from(crop.x) + f64::from(crop.width) / 2.0) * sx, y: (f64::from(crop.y) + f64::from(crop.height) / 2.0) * sy, rotation: f64::from(profile.rotation) },
                    display: otd_core::mapping::OtdArea { width: f64::from(area.width()), height: f64::from(area.height()), x: f64::from(area.left) + f64::from(area.width()) / 2.0,
                        y: f64::from(area.top) + f64::from(area.height()) / 2.0, rotation: 0.0 }, clipping: true, limiting: false }
            };
            value["input"] = serde_json::to_value(mapping.tablet).map_err(|error| error.to_string())?;
            value["output"] = serde_json::to_value(mapping.display).map_err(|error| error.to_string())?;
            value["clipping"] = mapping.clipping.into(); value["limiting"] = mapping.limiting.into();
        }
        Endpoint::create(&value, true).map(|endpoint| Self { endpoint })
    }
    pub(super) fn attach(&self, graph: *mut c_void) -> Result<(), String> {
        if unsafe { (api()?.attach)(graph, self.endpoint.context) } != 0 { return Err(super::last_error()); }
        Ok(())
    }
}

struct Binding { owner: u32, config: PluginConfig, endpoint: Endpoint, failed: bool }
struct BindingSink {
    bindings: Vec<Binding>, contexts: Vec<*mut c_void>, native: Box<dyn ActionSink>,
    buffer: Box<[ManagedCommand; 256]>, pending: VecDeque<ManagedCommand>, failed: bool,
}
/// Constructs each unchanged binding once. Native-only profiles return the
/// original sink without creating CLR state, report storage, or wrapper queues.
pub fn wrap_sink(profile: &Profile, tablet: &TabletConfiguration, native: Box<dyn ActionSink>) -> Result<Box<dyn ActionSink>, String> {
    if profile.managed_output.as_ref().is_some_and(|config| config.enabled) { return Err("managed output bindings require PluginChain::wrap_action_sink so the mode's actual pointer services can be shared".into()); }
    wrap_sink_with_output(profile, tablet, native, None)
}
pub(crate) fn wrap_sink_with_output(profile: &Profile, tablet: &TabletConfiguration, native: Box<dyn ActionSink>, output: Option<&OutputSession>) -> Result<Box<dyn ActionSink>, String> {
    wrap_sink_with_identifiers(profile, tablet, native, output, None)
}
pub(crate) fn wrap_sink_with_identifiers(profile: &Profile, tablet: &TabletConfiguration, native: Box<dyn ActionSink>,
    output: Option<&OutputSession>, identifiers: Option<&[DeviceIdentifier]>) -> Result<Box<dyn ActionSink>, String> {
    wrap_sink_for_graph(profile, tablet, native, output, identifiers, None)
}
pub(crate) fn wrap_sink_for_graph(profile: &Profile, tablet: &TabletConfiguration, native: Box<dyn ActionSink>,
    output: Option<&OutputSession>, identifiers: Option<&[DeviceIdentifier]>, graph: Option<&super::Graph>) -> Result<Box<dyn ActionSink>, String> {
    let slots = ButtonOutput::managed_slots(profile);
    if slots.is_empty() { return Ok(native); }
    let bindings = slots.into_iter().map(|(owner, config)| {
        let mut value = envelope(&config, profile, tablet, owner, identifiers)?;
        if let Some(graph) = graph { value["graph_context"] = serde_json::json!(graph.context_handle() as usize); }
        if let Some(output) = output { value["output_context"] = serde_json::json!(output.endpoint.context as usize); }
        let endpoint = Endpoint::create(&value, false)?;
        Ok(Binding { owner, config, endpoint, failed: false })
    }).collect::<Result<Vec<_>, String>>()?;
    let contexts = bindings.iter().map(|binding| binding.endpoint.context).collect();
    Ok(Box::new(BindingSink { bindings, contexts, native, buffer: Box::new([ManagedCommand::default(); 256]), pending: VecDeque::with_capacity(256), failed: false }))
}
fn action(command: ManagedCommand) -> io::Result<Action> {
    match command.kind {
        1 => match command.value { 1 => Some(MouseButton::Left), 2 => Some(MouseButton::Middle), 3 => Some(MouseButton::Right), 4 => Some(MouseButton::Backward), 5 => Some(MouseButton::Forward), _ => None }.map(Action::Mouse),
        2 => u16::try_from(command.value).ok().and_then(KeyboardUsage::new).map(Action::Key),
        _ => None,
    }.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid managed input action"))
}
impl BindingSink {
    fn drain(&mut self, index: usize) -> io::Result<()> {
        let count = self.bindings[index].endpoint.drain(&mut self.buffer)?;
        for command in self.buffer[..count].iter().copied() {
            match command.kind {
                1 | 2 => { let action = action(command)?; if !self.native.supports(action) { return Err(io::Error::new(io::ErrorKind::Unsupported, "managed binding requested unsupported action")); } self.native.hold(command.owner + 8192, action, command.flags & 1 != 0)?; }
                3 => { self.native.flush()?; self.native.scroll(ScrollPulse { axis: if command.flags & 1 == 0 { ScrollAxis::Vertical } else { ScrollAxis::Horizontal }, delta: command.value as i32 })?; }
                0 | 4 => { if self.pending.len() == 256 { return Err(io::Error::other("managed binding pointer queue exceeded 256 commands")); } self.pending.push_back(command); }
                _ => return Err(io::Error::new(io::ErrorKind::InvalidData, "unknown managed input service command")),
            }
        }
        self.native.flush()?;
        Ok(())
    }
}
impl ActionSink for BindingSink {
    fn has_managed(&self) -> bool { true }
    fn supports(&self, action: Action) -> bool { self.native.supports(action) }
    fn supports_scroll(&self) -> bool { self.native.supports_scroll() }
    fn supports_managed(&self, config: &PluginConfig) -> bool { self.bindings.iter().any(|binding| &binding.config == config && !binding.failed) }
    fn set_report(&mut self, kind: ReportKind, values: &ReportValues, raw: &[u8]) -> io::Result<()> {
        let frame = GraphReport::new(kind, values, raw)?;
        Endpoint::status(unsafe { (api().map_err(io::Error::other)?.reports)(self.contexts.as_ptr(), self.contexts.len() as u32, &frame) })
    }
    fn managed_binding(&mut self, owner: u32, config: &PluginConfig, pressed: bool) -> io::Result<()> {
        let index = self.bindings.iter().position(|binding| binding.owner == owner && &binding.config == config).ok_or_else(|| io::Error::other("managed binding was not prepared for this slot"))?;
        if self.bindings[index].failed { return Ok(()); }
        let result = Endpoint::status(unsafe { (api().map_err(io::Error::other)?.set)(self.bindings[index].endpoint.context, owner, i32::from(pressed)) }).and_then(|()| self.drain(index));
        if result.is_err() {
            self.bindings[index].failed = true;
            let _ = unsafe { (api().map_err(io::Error::other)?.release)(self.bindings[index].endpoint.context) };
            // The failure is reported once. Host cleanup owns any acknowledged
            // prefix; later transitions skip the disabled plugin.
        }
        result
    }
    fn next_managed_command(&mut self) -> Option<ManagedCommand> { self.pending.pop_front() }
    fn hold(&mut self, binding: u32, action: Action, held: bool) -> io::Result<()> { self.native.hold(binding, action, held) }
    fn scroll(&mut self, pulse: ScrollPulse) -> io::Result<()> { self.native.scroll(pulse) }
    fn flush(&mut self) -> io::Result<usize> { self.native.flush() }
    fn release_all(&mut self) -> io::Result<usize> {
        let mut error = None;
        for index in 0..self.bindings.len() {
            if self.bindings[index].failed { continue; }
            let release = Endpoint::status(unsafe { (api().map_err(io::Error::other)?.release)(self.bindings[index].endpoint.context) }).and_then(|()| self.drain(index));
            if let Err(failure) = release { if error.is_none() { error = Some(failure); } }
        }
        // Host ownership cleanup must still happen after a plugin Release error.
        let released = self.native.release_all();
        self.pending.clear();
        if let Some(error) = error { return Err(error); }
        released
    }
    fn managed_next_tick(&self) -> Option<Duration> {
        if self.failed { return None; }
        self.bindings.iter().filter(|binding| !binding.failed).filter_map(|binding| binding.endpoint.next().ok()).min()
    }
    fn managed_tick(&mut self) -> io::Result<()> {
        for index in 0..self.bindings.len() {
            if self.bindings[index].failed { continue; }
            let result = Endpoint::status(unsafe { (api().map_err(io::Error::other)?.tick)(self.bindings[index].endpoint.context) }).and_then(|()| self.drain(index));
            if result.is_err() { self.bindings[index].failed = true; }
            result?;
        }
        Ok(())
    }
}
impl Drop for BindingSink { fn drop(&mut self) { let _ = self.release_all(); } }
