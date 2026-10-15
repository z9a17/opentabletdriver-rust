//! Daemon-owned backend for original managed services. Every operation here is
//! cold/background work; native report processing never calls into this module.
use std::sync::{Arc, Mutex, OnceLock, Weak, atomic::{AtomicBool, Ordering}};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use serde_json::{Value, json};
use crate::control::{self, Command, Reply, WorkerIdentity};
use crate::managed_services::{Backend, Host, Operation, Publisher, Request, Snapshot, SourceSession};

static CURRENT: OnceLock<Mutex<Weak<NativeBackend>>> = OnceLock::new();
struct Published {
    publisher: Option<Publisher>,
    snapshot: Snapshot,
    last_devices: Option<Instant>,
    sessions: Vec<(String, u64, Option<u64>)>,
    retained: Option<Arc<Value>>,
    projection_pending: bool,
    dirty: bool,
}
// Only compare newly observed dynamic fields. The static configuration catalog
// is never walked, cloned or serialized during an unchanged observer refresh.
fn replace_if_changed<T: PartialEq>(slot: &mut T, value: T) -> bool {
    if *slot == value { false } else { *slot = value; true }
}
impl Published {
    fn publish_if_changed(&mut self) -> Result<(), String> {
        if !self.dirty { return Ok(()); }
        let version = self.snapshot.version.checked_add(1).ok_or("Managed snapshot version exhausted")?;
        let publisher = self.publisher.as_ref().ok_or("Managed snapshot publisher unavailable")?;
        let previous = self.snapshot.version;
        self.snapshot.version = version;
        if let Err(error) = publisher.publish_ref(&self.snapshot) {
            self.snapshot.version = previous;
            return Err(error);
        }
        self.dirty = false;
        Ok(())
    }
}
struct NativeBackend {
    stopped: Arc<AtomicBool>, published: Mutex<Published>, activated: AtomicBool,
    observer_thread: OnceLock<std::thread::Thread>,
}

pub struct Owner {
    tools:Option<crate::tool_host::Owner>,
    host: Option<Host>,
    stop: Arc<AtomicBool>,
    observer: Option<JoinHandle<()>>,
    backend: Arc<NativeBackend>,
}
impl Owner {
    pub fn start(identity: WorkerIdentity, cancelled: Arc<AtomicBool>) -> Result<Self, String> {
        let configurations = crate::config::configured_tablets().ok().and_then(|database| {
            let values: Vec<_> = database.entries().iter().filter_map(|entry| entry.usable()).collect();
            serde_json::to_value(values).ok()
        });
        // Malformed optional compatibility settings must not stop a native-only
        // driver. Their unavailable provider returns an explicit error later.
        let snapshot = Snapshot { version: 1, daemon_identity: Some(identity),
            diagnostic_app_version: Some(env!("CARGO_PKG_VERSION").into()),
            diagnostic_build_date: Some(env!("OTD_BUILD_DATE").into()),
            settings: crate::upstream_rpc::initial_original_settings().ok(),
            application_info: crate::upstream_rpc::original_application_info().ok(),
            configurations, tablets: Some(json!([])), ..Snapshot::default() };
        let stop = Arc::new(AtomicBool::new(false));
        let backend = Arc::new(NativeBackend { stopped: Arc::clone(&stop), activated: AtomicBool::new(crate::dotnet::initialized()),
            observer_thread: OnceLock::new(), published: Mutex::new(Published {
            publisher: None, snapshot: snapshot.clone(), last_devices: None, sessions: Vec::new(),
            retained: crate::upstream_rpc::cached_original_settings(),
            projection_pending: true,
            dirty: false,
        }) });
        let host = Host::start(snapshot, backend.clone())?;
        backend.published.lock().map_err(|_| "Managed snapshot lock poisoned")?.publisher = Some(host.publisher());
        *CURRENT.get_or_init(|| Mutex::new(Weak::new())).lock().map_err(|_| "Managed backend lock poisoned")? = Arc::downgrade(&backend);
        let observer_backend = Arc::clone(&backend);
        let observer_stop = Arc::clone(&stop);
        let observer = std::thread::Builder::new().name("managed-service-state".into()).spawn(move || {
            while !observer_stop.load(Ordering::Acquire) && !cancelled.load(Ordering::Acquire) {
                if !observer_backend.activated.load(Ordering::Acquire) {
                    std::thread::park();
                    continue;
                }
                // Native-only profiles do not initialize CLR, enumerate devices
                // or send recurring control requests for managed services.
                if crate::dotnet::initialized() && crate::managed_services::snapshots_requested() {
                    let _ = observer_backend.refresh(false, false, true);
                }
                std::thread::park_timeout(Duration::from_millis(250));
            }
        }).map_err(|error| error.to_string())?;
        let _ = backend.observer_thread.set(observer.thread().clone());
        // Establish teardown ownership before any later startup step can fail.
        // Dropping a bare JoinHandle would leave the parked observer and its
        // retained backend alive after a failed tool-owner initialization.
        let mut owner = Self { tools: None, host: Some(host), stop, observer: Some(observer), backend };
        owner.tools = Some(crate::tool_host::Owner::start()?);
        if let Some(settings)=crate::upstream_rpc::cached_original_settings() {crate::tool_host::initialize_document((*settings).clone())?;}
        Ok(owner)
    }
}
impl Drop for Owner {
    fn drop(&mut self) {
        drop(self.tools.take());
        self.stop.store(true, Ordering::Release);
        if let Some(observer) = self.observer.take() {
            observer.thread().unpark();
            let _ = observer.join();
        }
        // The service queue joins its executing backend before daemon cleanup
        // can retire devices or a replacement service owner can be installed.
        drop(self.host.take());
        if let Ok(mut current) = CURRENT.get_or_init(|| Mutex::new(Weak::new())).lock() {
            if current.upgrade().is_some_and(|backend| Arc::ptr_eq(&backend, &self.backend)) { *current = Weak::new(); }
        }
    }
}
/// Cold CLR setup only. In a standalone metadata/UI process no native owner is
/// installed, so this does not enumerate hardware or contact another daemon.
pub fn prime() {
    let backend = CURRENT.get_or_init(|| Mutex::new(Weak::new())).lock().ok().and_then(|current| current.upgrade());
    // CLR can first initialize inside a daemon control handler. Sending an IPC
    // request back to that handler here would wait on its own control thread.
    // Seed only cold metadata; the background observer projects live settings
    // once CLR initialization has completed.
    if let Some(backend) = backend {
        let _ = backend.prime_metadata();
        backend.activated.store(true, Ordering::Release);
        if let Some(observer) = backend.observer_thread.get() { observer.unpark(); }
    }
}
fn call(command: Command) -> Result<Reply, String> {
    let response = control::request_owned(&control::Request::new(1, command), Duration::from_secs(2), std::process::id())
        .map_err(|error| format!("Managed native control: {error}; query state before retrying a mutation"))?;
    match response.reply { Reply::Error { error } => Err(error.message), reply => Ok(reply) }
}
impl NativeBackend {
    fn prime_metadata(&self) -> Result<(), String> {
        let mut state = self.published.lock().map_err(|_| "Managed snapshot lock poisoned")?;
        let mut changed = replace_if_changed(&mut state.snapshot.devices, service_devices().ok().map(|devices| json!(devices)));
        state.last_devices = Some(Instant::now());
        changed |= replace_if_changed(&mut state.snapshot.settings, crate::upstream_rpc::initial_original_settings().ok());
        changed |= replace_if_changed(&mut state.snapshot.resynchronize, crate::upstream_rpc::original_resynchronize_epoch());
        state.projection_pending = true;
        state.dirty |= changed;
        state.publish_if_changed()
    }
    fn refresh(&self, full_settings: bool, force_devices: bool, allow_projection: bool) -> Result<(), String> {
        if self.stopped.load(Ordering::Acquire) { return Err("Managed native owner is stopping".into()); }
        let mut state = self.published.lock().map_err(|_| "Managed snapshot lock poisoned")?;
        let status = match call(Command::Status)? { Reply::Status { status } => status, _ => return Err("Unexpected managed status reply".into()) };
        let sessions = match call(Command::ListDeviceSessions)? { Reply::DeviceSessions { sessions, .. } => sessions, _ => return Err("Unexpected managed device reply".into()) };
        let session_generations: Vec<_> = sessions.iter().map(|session| (session.id.clone(), session.device_generation, session.pending_generation)).collect();
        let mut changed = replace_if_changed(&mut state.snapshot.source_sessions, sessions.iter().map(|session| SourceSession {
            id: session.id.clone(), tablet: session.tablet.clone(), device_generation: session.device_generation,
            pending_generation: session.pending_generation, connected: session.connected, state: session.state,
        }).collect());
        let retained = crate::upstream_rpc::cached_original_settings();
        let retained_changed = match (&state.retained, &retained) {
            (Some(old), Some(new)) => !Arc::ptr_eq(old, new), (None, None) => false, _ => true,
        };
        let settings_changed = full_settings || state.projection_pending || retained_changed || state.sessions != session_generations
            || state.snapshot.daemon_identity.as_ref() != Some(&status.identity());
        let tablets: Vec<_> = sessions.iter().filter(|session| session.connected).filter_map(|session| {
            session.opened_identifiers.as_ref().map(|identifiers| json!({"Properties": session.properties, "Identifiers": identifiers}))
        }).collect();
        changed |= replace_if_changed(&mut state.snapshot.tablets, Some(json!(tablets)));
        if allow_projection && settings_changed {
            changed |= replace_if_changed(&mut state.snapshot.settings, crate::upstream_rpc::get_original_settings().ok());
            state.projection_pending = false;
        }
        if force_devices || state.last_devices.is_none_or(|last| last.elapsed() >= Duration::from_secs(3)) {
            changed |= replace_if_changed(&mut state.snapshot.devices, service_devices().ok().map(|devices| json!(devices)));
            state.last_devices = Some(Instant::now());
        }
        let logs = match call(Command::GetUpstreamLog) {
            Ok(Reply::UpstreamLog { messages, sequence, .. }) => {
                changed |= replace_if_changed(&mut state.snapshot.log_sequence, sequence);
                Some(json!(messages))
            }, _ => None,
        };
        changed |= replace_if_changed(&mut state.snapshot.logs, logs);
        changed |= replace_if_changed(&mut state.snapshot.resynchronize, crate::upstream_rpc::original_resynchronize_epoch());
        changed |= replace_if_changed(&mut state.snapshot.daemon_identity, Some(status.identity()));
        state.sessions = session_generations;
        state.retained = crate::upstream_rpc::cached_original_settings();
        state.dirty |= changed;
        state.publish_if_changed()
    }
    fn daemon(&self, request: &Request) -> Result<Value, String> {
        let method = request.payload["method"].as_str().ok_or("Managed daemon request needs method")?;
        let params = request.payload.get("params").cloned().unwrap_or_else(|| json!([]));
        if let Some(result) = crate::plugin_manager::invoke(method, &params, Some(&self.stopped))? { return Ok(result); }
        match method {
            "InstallUpdate"=>crate::upstream_rpc::install_hosted_update(&params),
            "FinishUpdate"=>crate::upstream_rpc::finish_hosted_update(),
            "ForceResynchronize" if request.expected_source.is_some()=>{
                let (id,generation)=request.expected_source.as_ref().unwrap();
                if !request.payload.get("source_session").is_some_and(crate::shared_devices::live_source){
                    return Err("Original source reader retired before resynchronization".into());
                }
                let sessions=match call(Command::ListDeviceSessions)?{Reply::DeviceSessions{sessions,..}=>sessions,_=>return Err("Unexpected source session reply".into())};
                if !sessions.iter().any(|session|session.id==*id&&session.device_generation==*generation
                    &&session.connected&&session.pending_generation.is_none()
                    &&session.state==crate::device_sessions::SessionState::Running){
                    return Err("Original source generation changed before resynchronization".into());
                }
                crate::upstream_rpc::invoke_original(method,&params)
            }
            "SetSettings" | "ResetSettings" => {
                let expected = request.expected_daemon.clone().ok_or("Native daemon identity unavailable; refresh before changing settings")?;
                let settings = if method == "ResetSettings" { Value::Null } else {
                    params.as_array().and_then(|values| values.first()).cloned().ok_or("SetSettings requires one settings collection")?
                };
                let source = match (request.payload.get("source_tablet_name"), request.payload.get("source_binding_owner")) {
                    (None, None) => None,
                    (Some(name), Some(owner)) => Some((name.as_str().ok_or("Invalid source tablet name")?.to_owned(),
                        owner.as_u64().and_then(|owner| u32::try_from(owner).ok()).ok_or("Invalid source binding owner")?)),
                    _ => return Err("Managed preset source needs both tablet name and binding owner".into()),
                };
                crate::upstream_rpc::set_original_settings_expected_with_source_generation(settings, expected, source,
                    request.expected_source.clone())?;
                Ok(Value::Null)
            }
            _ => crate::upstream_rpc::invoke_original(method, &params),
        }
    }
    fn device_string(&self, payload: &Value) -> Result<Value, String> {
        let path = payload["path"].as_str().ok_or("Device string request needs endpoint path")?;
        let index = payload["index"].as_u64().and_then(|index| u8::try_from(index).ok()).ok_or("Device string index must be 0..255")?;
        if let Some(result)=crate::shared_devices::device_string(path,index) { return result.map(|value|json!(value)); }
        let devices = crate::hid::enumerate_service_devices().map_err(|error| error.to_string())?;
        let device = devices.iter().find(|device| device["DevicePath"].as_str().is_some_and(|candidate| candidate.eq_ignore_ascii_case(path)))
            .ok_or("Device endpoint is no longer present")?;
        let vendor = device["VendorID"].as_u64().and_then(|value| u16::try_from(value).ok()).ok_or("Endpoint vendor ID unavailable")?;
        let product = device["ProductID"].as_u64().and_then(|value| u16::try_from(value).ok()).ok_or("Endpoint product ID unavailable")?;
        let endpoints = crate::hid::read_strings(vendor, product, &[index]).map_err(|error| error.to_string())?;
        let (_, strings) = endpoints.into_iter().find(|(candidate, _)| candidate.eq_ignore_ascii_case(path)).ok_or("Requested endpoint string unavailable")?;
        let (_, value) = strings.into_iter().next().ok_or("Requested device string unavailable")?;
        Ok(json!(value?))
    }
}
impl Backend for NativeBackend {
    fn maintain(&self,lane:usize){if lane==4{crate::managed_inputs::maintain();}}
    fn shutdown(&self,lane:usize){if lane==4{crate::managed_inputs::shutdown();}}
    fn execute(&self, request: Request) -> Result<Value, String> {
        if self.stopped.load(Ordering::Acquire) { return Err("Managed native owner is stopping".into()); }
        let result = match request.operation {
            Operation::InputHold|Operation::InputRelease=>return crate::managed_inputs::execute(request.operation,request.scope,&request.payload),
            Operation::Daemon => self.daemon(&request),
            Operation::Detect => crate::upstream_rpc::invoke_original("DetectTablets", &json!([])).and_then(|tablets| {
                let tablets = tablets.as_array().ok_or("Native detection did not return a tablet collection")?;
                Ok(json!(!tablets.is_empty()))
            }),
            Operation::DeviceString => self.device_string(&request.payload),
            Operation::Snapshot => Err("Snapshot requests are served by the managed service cache".into()),
            Operation::OpenStream | Operation::ReadStream | Operation::WriteStream | Operation::GetFeature
                | Operation::SetFeature | Operation::CloseStream | Operation::DeviceReports | Operation::OutputOwner =>
                    return crate::shared_devices::execute(request.operation,request.scope,&request.payload),
        };
        // Successful admission was never reported as application. Publish the
        // actual result state before the ticket becomes a completed response.
        if result.is_ok() && matches!(request.operation, Operation::Daemon | Operation::Detect) {
            self.refresh(true, matches!(request.operation, Operation::Detect), true)
                .map_err(|error| format!("Managed operation completed, but state refresh failed: {error}; query state before retrying"))?;
        }
        result
    }
}
pub fn publish_owned_devices() {
    if let Some(backend)=CURRENT.get_or_init(||Mutex::new(Weak::new())).lock().ok().and_then(|value|value.upgrade()) {
        if let Ok(devices)=service_devices() { if let Ok(mut state)=backend.published.lock() {
            let changed=replace_if_changed(&mut state.snapshot.devices,Some(json!(devices)));
            state.dirty |= changed;
            let _=state.publish_if_changed();
        } }
    }
}
fn service_devices()->Result<Vec<Value>,String> {
    let discovered=crate::hid::enumerate_service_devices().map_err(|error|error.to_string())?;
    let mut devices=Vec::new();
    let owned=crate::shared_devices::owned_metadata();
    // Current and prepared readers can share an endpoint path. Keep each
    // concrete reader generation available for exact scoped dependency lookup.
    for device in &discovered {
        if !owned.iter().any(|owned|device["DevicePath"].as_str().zip(owned["DevicePath"].as_str())
            .is_some_and(|(a,b)|a.eq_ignore_ascii_case(b))) {devices.push(device.clone());}
    }
    for owned in owned {
        if let Some(device)=discovered.iter().find(|device|device["DevicePath"].as_str().zip(owned["DevicePath"].as_str())
            .is_some_and(|(a,b)|a.eq_ignore_ascii_case(b))) {
            let mut device=device.clone();
            if let Some(object)=device.as_object_mut() { object.extend(owned.as_object().unwrap().clone()); object.insert("CanOpen".into(),json!(true)); }
            devices.push(device);
        } else {
            let mut device=owned;
            if let Some(object)=device.as_object_mut(){object.insert("CanOpen".into(),json!(true));}
            devices.push(device);
        }
    }
    Ok(devices)
}

#[cfg(test)]
mod snapshot_tests {
    use super::*;

    #[test]
    fn source_state_changes_are_observed_without_generation_changes() {
        let source = SourceSession { id: "offline-fixture".into(), tablet: "fixture".into(), device_generation: 7,
            pending_generation: None, connected: true, state: crate::device_sessions::SessionState::Running };
        let mut sessions = vec![source.clone()];
        assert!(!replace_if_changed(&mut sessions, vec![source.clone()]));
        let mut next = source.clone(); next.connected = false;
        assert!(replace_if_changed(&mut sessions, vec![next]));
        let mut next = source.clone(); next.state = crate::device_sessions::SessionState::Waiting;
        assert!(replace_if_changed(&mut sessions, vec![next]));
        let mut next = source; next.pending_generation = Some(8);
        assert!(replace_if_changed(&mut sessions, vec![next]));
    }

    #[test]
    fn unchanged_state_does_not_require_publication_and_failed_publication_retries() {
        let mut state = Published { publisher: None, snapshot: Snapshot { version: 1, ..Snapshot::default() },
            last_devices: None, sessions: Vec::new(), retained: None, projection_pending: false, dirty: false };
        state.publish_if_changed().unwrap();
        assert_eq!(state.snapshot.version, 1);
        state.dirty = true;
        assert!(state.publish_if_changed().is_err());
        assert!(state.dirty);
        assert_eq!(state.snapshot.version, 1);
    }
}
