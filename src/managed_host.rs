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
    observed_sessions: Arc<crate::device_sessions::SessionList>,
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
    native: Mutex<NativeState>, refreshing: Mutex<()>,
    observer_thread: OnceLock<std::thread::Thread>,
}
struct NativeState {
    identity: WorkerIdentity,
    devices: Option<crate::device_sessions::Handle>,
    log_sequence: u64,
    logs: Option<Arc<Value>>,
}
impl NativeState {
    fn update(&mut self, identity: WorkerIdentity, devices: Option<crate::device_sessions::Handle>,
        sequence: u64, logs: impl FnOnce() -> Value) -> bool {
        let changed = self.logs.is_none() || self.identity != identity || self.log_sequence != sequence
            || !same_devices(&self.devices, &devices);
        if self.log_sequence != sequence || self.logs.is_none() { self.logs = Some(Arc::new(logs())); }
        self.identity = identity;
        self.devices = devices;
        self.log_sequence = sequence;
        changed
    }
}
fn same_devices(a: &Option<crate::device_sessions::Handle>, b: &Option<crate::device_sessions::Handle>) -> bool {
    match (a, b) { (Some(a), Some(b)) => Arc::ptr_eq(&a.registry, &b.registry), (None, None) => true, _ => false }
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
        let snapshot = Snapshot { version: 1, daemon_identity: Some(identity.clone()),
            diagnostic_app_version: Some(env!("CARGO_PKG_VERSION").into()),
            diagnostic_build_date: Some(env!("OTD_BUILD_DATE").into()),
            settings: crate::upstream_rpc::initial_original_settings().ok(),
            application_info: crate::upstream_rpc::original_application_info().ok(),
            configurations, tablets: Some(json!([])), ..Snapshot::default() };
        let stop = Arc::new(AtomicBool::new(false));
        let backend = Arc::new(NativeBackend { stopped: Arc::clone(&stop), activated: AtomicBool::new(crate::dotnet::initialized()),
            native: Mutex::new(NativeState { identity, devices: None, log_sequence: 0, logs: None }), refreshing: Mutex::new(()),
            observer_thread: OnceLock::new(), published: Mutex::new(Published {
            publisher: None, snapshot: snapshot.clone(), last_devices: None, sessions: Vec::new(),
            observed_sessions: Arc::default(),
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
                    std::thread::park_timeout(Duration::from_millis(250));
                } else {
                    // The first Snapshot request explicitly wakes this owner;
                    // CLR initialization alone does not need recurring work.
                    std::thread::park();
                }
            }
        }).map_err(|error| error.to_string())?;
        let _ = backend.observer_thread.set(observer.thread().clone());
        // A prime/first request can race thread-handle installation. Its flag
        // is already visible; this token prevents losing that initial wake.
        observer.thread().unpark();
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
pub(crate) fn wake_observer() {
    let backend = CURRENT.get_or_init(|| Mutex::new(Weak::new())).lock().ok().and_then(|current| current.upgrade());
    if let Some(backend) = backend {
        if let Some(observer) = backend.observer_thread.get() { observer.unpark(); }
    }
}
fn call(command: Command) -> Result<Reply, String> {
    let response = control::request_owned(&control::Request::new(1, command), Duration::from_secs(2), std::process::id())
        .map_err(|error| format!("Managed native control: {error}; query state before retrying a mutation"))?;
    match response.reply { Reply::Error { error } => Err(error.message), reply => Ok(reply) }
}
/// Called by the native control owner after polling and before returning a
/// mutation receipt. This copies no device tree or log history when unchanged.
/// The observer never needs to send read-only requests to its own control pipe.
pub(crate) fn publish_native_state(identity: WorkerIdentity, devices: Option<crate::device_sessions::Handle>,
    sequence: u64, logs: impl FnOnce() -> Value) {
    let backend = CURRENT.get_or_init(|| Mutex::new(Weak::new())).lock().ok().and_then(|current| current.upgrade());
    if let Some(backend) = backend {
        let changed = backend.native.lock().ok().is_some_and(|mut state| state.update(identity, devices, sequence, logs));
        if changed { if let Some(observer) = backend.observer_thread.get() { observer.unpark(); } }
    }
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
        // Serialize observer/detect refreshes without holding the publication
        // lock across a settings projection's control transaction. Cold CLR
        // priming on that control thread must be able to publish metadata.
        let _refresh = self.refreshing.lock().map_err(|_| "Managed refresh lock poisoned")?;
        let (identity, devices, sequence, logs) = {
            let native = self.native.lock().map_err(|_| "Managed native state lock poisoned")?;
            (native.identity.clone(), native.devices.clone(), native.log_sequence, native.logs.clone())
        };
        let previous = self.published.lock().map_err(|_| "Managed snapshot lock poisoned")?.observed_sessions.clone();
        let observed = match devices {
            Some(handle) => handle.snapshot_cached(&previous)?,
            None if previous.sessions.is_empty() && previous.selected_id.is_none() => previous.clone(),
            None => Arc::default(),
        };
        let sessions_changed = !Arc::ptr_eq(&previous, &observed);
        let sessions = &observed.sessions;
        let session_generations = sessions_changed.then(|| sessions.iter()
            .map(|session| (session.id.clone(), session.device_generation, session.pending_generation)).collect::<Vec<_>>());
        let retained = crate::upstream_rpc::cached_original_settings();
        let (settings_changed, refresh_devices) = {
            let state = self.published.lock().map_err(|_| "Managed snapshot lock poisoned")?;
            let retained_changed = match (&state.retained, &retained) {
                (Some(old), Some(new)) => !Arc::ptr_eq(old, new), (None, None) => false, _ => true,
            };
            (full_settings || state.projection_pending || retained_changed || session_generations.as_ref().is_some_and(|next| state.sessions != *next)
                || state.snapshot.daemon_identity.as_ref() != Some(&identity),
                force_devices || state.last_devices.is_none_or(|last| last.elapsed() >= Duration::from_secs(3)))
        };
        let projected = (allow_projection && settings_changed).then(|| crate::upstream_rpc::get_original_settings().ok());
        let devices = refresh_devices.then(|| service_devices().ok().map(|devices| json!(devices)));
        let mut state = self.published.lock().map_err(|_| "Managed snapshot lock poisoned")?;
        let mut changed = false;
        if sessions_changed {
            changed |= replace_if_changed(&mut state.snapshot.source_sessions, sessions.iter().map(|session| SourceSession {
                id: session.id.clone(), tablet: session.tablet.clone(), device_generation: session.device_generation,
                pending_generation: session.pending_generation, connected: session.connected, state: session.state,
            }).collect());
            let tablets: Vec<_> = sessions.iter().filter(|session| session.connected).filter_map(|session| {
                session.opened_identifiers.as_ref().map(|identifiers| json!({"Properties": session.properties, "Identifiers": identifiers}))
            }).collect();
            changed |= replace_if_changed(&mut state.snapshot.tablets, Some(json!(tablets)));
        }
        if let Some(projected) = projected {
            changed |= replace_if_changed(&mut state.snapshot.settings, projected);
            state.projection_pending = false;
        }
        if let Some(devices) = devices {
            changed |= replace_if_changed(&mut state.snapshot.devices, devices);
            state.last_devices = Some(Instant::now());
        }
        if state.snapshot.log_sequence != sequence || (state.snapshot.logs.is_none() && logs.is_some()) {
            changed |= replace_if_changed(&mut state.snapshot.log_sequence, sequence);
            changed |= replace_if_changed(&mut state.snapshot.logs, logs.as_deref().cloned());
        }
        changed |= replace_if_changed(&mut state.snapshot.resynchronize, crate::upstream_rpc::original_resynchronize_epoch());
        changed |= replace_if_changed(&mut state.snapshot.daemon_identity, Some(identity));
        if let Some(generations) = session_generations { state.sessions = generations; }
        state.observed_sessions = observed;
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
    fn maintenance_deadline(&self,lane:usize)->Option<Instant>{if lane==4{crate::managed_inputs::maintenance_deadline()}else{None}}
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
    fn native_publication_reuses_logs_and_tracks_replacement_owner_and_registry() {
        let identity = WorkerIdentity { instance: "fixture".into(), generation: 1 };
        let mut state = NativeState { identity: identity.clone(), devices: None, log_sequence: 0, logs: None };
        assert!(state.update(identity.clone(), None, 0, || json!([])));
        let retained = state.logs.clone().unwrap();
        assert!(!state.update(identity.clone(), None, 0, || panic!("unchanged history cloned")));
        assert!(Arc::ptr_eq(&retained, state.logs.as_ref().unwrap()));
        let (first, _) = crate::device_sessions::Handle::channel(crate::hid::Event::create(true).unwrap());
        assert!(state.update(identity.clone(), Some(first.clone()), 0, || panic!("unchanged history cloned")));
        let candidate = crate::device_sessions::tests::candidate("managed-state-fixture", "managed-state-parent");
        let id = first.discover(&crate::device_sessions::tests::selected(&candidate)).unwrap();
        first.commit(&id, 7, crate::config::Profile::default());
        first.state(&id, crate::device_sessions::SessionState::Waiting, None);
        assert!(!state.update(identity.clone(), Some(first), 0, || panic!("unchanged history cloned")));
        // The retained handle sees changes even if identity, logical device
        // generation and log sequence are all unchanged.
        let current = state.devices.as_ref().unwrap().snapshot().unwrap();
        assert_eq!(current.sessions[0].device_generation, 7);
        assert_eq!(current.sessions[0].state, crate::device_sessions::SessionState::Waiting);
        let (replacement, _) = crate::device_sessions::Handle::channel(crate::hid::Event::create(true).unwrap());
        assert!(state.update(identity.clone(), Some(replacement), 0, || panic!("unchanged history cloned")));
        assert!(state.update(WorkerIdentity { instance: "replacement".into(), generation: 0 }, None, 1, || json!(["new log"])));
        assert!(!Arc::ptr_eq(&retained, state.logs.as_ref().unwrap()));
        assert_eq!(**state.logs.as_ref().unwrap(), json!(["new log"]));
    }

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
            last_devices: None, sessions: Vec::new(), observed_sessions: Arc::default(), retained: None, projection_pending: false, dirty: false };
        state.publish_if_changed().unwrap();
        assert_eq!(state.snapshot.version, 1);
        state.dirty = true;
        assert!(state.publish_if_changed().is_err());
        assert!(state.dirty);
        assert_eq!(state.snapshot.version, 1);
    }
}
