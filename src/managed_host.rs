//! Daemon-owned backend for original managed services. Every operation here is
//! cold/background work; native report processing never calls into this module.
use std::sync::{Arc, Mutex, OnceLock, Weak, atomic::{AtomicBool, Ordering}};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use serde_json::{Value, json};
use crate::control::{self, Command, Reply, WorkerIdentity};
use crate::managed_services::{Backend, Host, Operation, Publisher, Request, Snapshot};

static CURRENT: OnceLock<Mutex<Weak<NativeBackend>>> = OnceLock::new();
struct Published {
    publisher: Option<Publisher>,
    snapshot: Snapshot,
    last_devices: Option<Instant>,
    sessions: Vec<(String, u64, Option<u64>)>,
    retained: Option<Arc<Value>>,
    projection_pending: bool,
}
struct NativeBackend { stopped: Arc<AtomicBool>, published: Mutex<Published> }

pub struct Owner {
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
            settings: crate::upstream_rpc::initial_original_settings().ok(),
            application_info: crate::upstream_rpc::original_application_info().ok(),
            configurations, tablets: Some(json!([])), ..Snapshot::default() };
        let stop = Arc::new(AtomicBool::new(false));
        let backend = Arc::new(NativeBackend { stopped: Arc::clone(&stop), published: Mutex::new(Published {
            publisher: None, snapshot: snapshot.clone(), last_devices: None, sessions: Vec::new(),
            retained: crate::upstream_rpc::cached_original_settings(),
            projection_pending: true,
        }) });
        let host = Host::start(snapshot, backend.clone())?;
        backend.published.lock().map_err(|_| "Managed snapshot lock poisoned")?.publisher = Some(host.publisher());
        *CURRENT.get_or_init(|| Mutex::new(Weak::new())).lock().map_err(|_| "Managed backend lock poisoned")? = Arc::downgrade(&backend);
        let observer_backend = Arc::clone(&backend);
        let observer_stop = Arc::clone(&stop);
        let observer = std::thread::Builder::new().name("managed-service-state".into()).spawn(move || {
            while !observer_stop.load(Ordering::Acquire) && !cancelled.load(Ordering::Acquire) {
                // Native-only profiles do not initialize CLR, enumerate devices
                // or send recurring control requests for managed services.
                if crate::dotnet::initialized() { let _ = observer_backend.refresh(false, false, true); }
                std::thread::park_timeout(Duration::from_millis(250));
            }
        }).map_err(|error| error.to_string())?;
        Ok(Self { host: Some(host), stop, observer: Some(observer), backend })
    }
}
impl Drop for Owner {
    fn drop(&mut self) {
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
    if let Some(backend) = backend { let _ = backend.prime_metadata(); }
}
fn call(command: Command) -> Result<Reply, String> {
    let response = control::request_owned(&control::Request::new(1, command), Duration::from_secs(2), std::process::id())
        .map_err(|error| format!("Managed native control: {error}; query state before retrying a mutation"))?;
    match response.reply { Reply::Error { error } => Err(error.message), reply => Ok(reply) }
}
impl NativeBackend {
    fn prime_metadata(&self) -> Result<(), String> {
        let mut state = self.published.lock().map_err(|_| "Managed snapshot lock poisoned")?;
        state.snapshot.devices = crate::hid::enumerate_service_devices().ok().map(|devices| json!(devices));
        state.last_devices = Some(Instant::now());
        state.snapshot.settings = crate::upstream_rpc::initial_original_settings().ok();
        state.snapshot.resynchronize = crate::upstream_rpc::original_resynchronize_epoch();
        state.projection_pending = true;
        state.snapshot.version = state.snapshot.version.checked_add(1).ok_or("Managed snapshot version exhausted")?;
        state.publisher.as_ref().ok_or("Managed snapshot publisher unavailable")?.publish(state.snapshot.clone())
    }
    fn refresh(&self, full_settings: bool, force_devices: bool, allow_projection: bool) -> Result<(), String> {
        if self.stopped.load(Ordering::Acquire) { return Err("Managed native owner is stopping".into()); }
        let mut state = self.published.lock().map_err(|_| "Managed snapshot lock poisoned")?;
        let status = match call(Command::Status)? { Reply::Status { status } => status, _ => return Err("Unexpected managed status reply".into()) };
        let sessions = match call(Command::ListDeviceSessions)? { Reply::DeviceSessions { sessions, .. } => sessions, _ => return Err("Unexpected managed device reply".into()) };
        let session_generations: Vec<_> = sessions.iter().map(|session| (session.id.clone(), session.device_generation, session.pending_generation)).collect();
        let retained = crate::upstream_rpc::cached_original_settings();
        let retained_changed = match (&state.retained, &retained) {
            (Some(old), Some(new)) => !Arc::ptr_eq(old, new), (None, None) => false, _ => true,
        };
        let settings_changed = full_settings || state.projection_pending || retained_changed || state.sessions != session_generations
            || state.snapshot.daemon_identity.as_ref() != Some(&status.identity());
        let tablets: Vec<_> = sessions.iter().filter(|session| session.connected).filter_map(|session| {
            session.opened_identifiers.as_ref().map(|identifiers| json!({"Properties": session.properties, "Identifiers": identifiers}))
        }).collect();
        state.snapshot.tablets = Some(json!(tablets));
        if allow_projection && settings_changed {
            state.snapshot.settings = crate::upstream_rpc::get_original_settings().ok();
            state.projection_pending = false;
        }
        if force_devices || state.last_devices.is_none_or(|last| last.elapsed() >= Duration::from_secs(3)) {
            state.snapshot.devices = crate::hid::enumerate_service_devices().ok().map(|devices| json!(devices));
            state.last_devices = Some(Instant::now());
        }
        state.snapshot.logs = match call(Command::GetUpstreamLog) {
            Ok(Reply::UpstreamLog { messages, sequence, .. }) => {
                state.snapshot.log_sequence = sequence;
                Some(json!(messages))
            }, _ => None,
        };
        state.snapshot.resynchronize = crate::upstream_rpc::original_resynchronize_epoch();
        state.snapshot.daemon_identity = Some(status.identity());
        state.snapshot.version = state.snapshot.version.checked_add(1).ok_or("Managed snapshot version exhausted")?;
        state.sessions = session_generations;
        state.retained = crate::upstream_rpc::cached_original_settings();
        state.publisher.as_ref().ok_or("Managed snapshot publisher unavailable")?.publish(state.snapshot.clone())
    }
    fn daemon(&self, request: &Request) -> Result<Value, String> {
        let method = request.payload["method"].as_str().ok_or("Managed daemon request needs method")?;
        let params = request.payload.get("params").cloned().unwrap_or_else(|| json!([]));
        match method {
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
                crate::upstream_rpc::set_original_settings_expected_with_source(settings, expected, source)?;
                Ok(Value::Null)
            }
            "SetTabletDebug" => Err("Managed IDriverDaemon.DeviceReport subscriptions are not supplied yet; use the native/full-rate RPC recording endpoint".into()),
            "InstallUpdate" => Err("Managed update installation requires a persistent update owner; use the native panel update workflow".into()),
            _ => crate::upstream_rpc::invoke_original(method, &params),
        }
    }
    fn device_string(&self, payload: &Value) -> Result<Value, String> {
        let path = payload["path"].as_str().ok_or("Device string request needs endpoint path")?;
        let index = payload["index"].as_u64().and_then(|index| u8::try_from(index).ok()).ok_or("Device string index must be 0..255")?;
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
    fn execute(&self, request: Request) -> Result<Value, String> {
        if self.stopped.load(Ordering::Acquire) { return Err("Managed native owner is stopping".into()); }
        let result = match request.operation {
            Operation::Daemon => self.daemon(&request),
            Operation::Detect => crate::upstream_rpc::invoke_original("DetectTablets", &json!([])).and_then(|tablets| {
                let tablets = tablets.as_array().ok_or("Native detection did not return a tablet collection")?;
                Ok(json!(!tablets.is_empty()))
            }),
            Operation::DeviceString => self.device_string(&request.payload),
            Operation::Snapshot => Err("Snapshot requests are served by the managed service cache".into()),
            Operation::OpenStream | Operation::ReadStream | Operation::WriteStream | Operation::GetFeature
                | Operation::SetFeature | Operation::CloseStream => Err("Plugin-owned endpoint streams and feature/write access are not supplied by this native owner; native session readers retain endpoint ownership".into()),
        };
        // Successful admission was never reported as application. Publish the
        // actual result state before the ticket becomes a completed response.
        if result.is_ok() {
            self.refresh(true, matches!(request.operation, Operation::Detect), true)
                .map_err(|error| format!("Managed operation completed, but state refresh failed: {error}; query state before retrying"))?;
        }
        result
    }
}
