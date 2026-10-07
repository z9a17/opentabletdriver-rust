//! Cold-path services for unchanged managed plugins. No native report calls here.
//! Admission is not completion: a ticket becomes successful only after Backend returns.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

const MAX_TICKETS: usize = 64;
const MAX_BYTES: usize = 4 * 1024 * 1024;
const MAX_QUEUED_BYTES: usize = 8 * 1024 * 1024;
const TTL: Duration = Duration::from_secs(60);

/// Actual physical session state published by the native owner. Admission
/// resolves a binding's name here once; backend execution must not resolve it
/// again against whichever same-named tablet happens to be running later.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SourceSession {
    pub id: String,
    pub tablet: String,
    pub device_generation: u64,
    pub pending_generation: Option<u64>,
    pub connected: bool,
    pub state: crate::device_sessions::SessionState,
}

/// Native JSON uses upstream Newtonsoft property names inside these fields.
/// None means unavailable, not a successful fabricated empty value.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Snapshot {
    pub diagnostic_app_version: Option<String>,
    pub diagnostic_build_date: Option<String>,
    pub version: u64,
    pub daemon_identity: Option<crate::control::WorkerIdentity>,
    #[serde(default)]
    pub source_sessions: Vec<SourceSession>,
    pub settings: Option<Value>,
    pub application_info: Option<Value>,
    pub devices: Option<Value>,
    pub tablets: Option<Value>,
    pub configurations: Option<Value>,
    pub logs: Option<Value>,
    pub log_sequence: u64,
    /// Monotonic event epochs; the managed client raises Resynchronize only on change.
    pub resynchronize: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum Operation { Snapshot = 0, Daemon = 1, Detect = 2, DeviceString = 3,
    OpenStream = 4, ReadStream = 5, WriteStream = 6, GetFeature = 7,
    SetFeature = 8, CloseStream = 9, DeviceReports = 10, OutputOwner = 11, InputHold = 12, InputRelease = 13 }
impl Operation {
    fn parse(value: u32) -> Option<Self> { Some(match value {
        0 => Self::Snapshot, 1 => Self::Daemon, 2 => Self::Detect, 3 => Self::DeviceString,
        4 => Self::OpenStream, 5 => Self::ReadStream, 6 => Self::WriteStream,
        7 => Self::GetFeature, 8 => Self::SetFeature, 9 => Self::CloseStream,
        10 => Self::DeviceReports, 11 => Self::OutputOwner, 12=>Self::InputHold,13=>Self::InputRelease,_ => return None,
    }) }
}
#[derive(Clone, Debug)]
pub struct Request { pub operation: Operation, pub scope: u64, pub payload: Value,
    pub expected_daemon: Option<crate::control::WorkerIdentity>,
    /// Exact admitted physical source ID and generation, not a late name lookup.
    pub expected_source: Option<(String, u64)> }
/// Executed on the service owner thread, never on the HID/report thread.
/// Daemon payload is {"method":string,"params":[...]}; other payloads use
/// {"path":string,"index":u8} / {"stream":u64,"data":hex_string}.
/// Return Err when a real host capability is unavailable. Never acknowledge
/// mutations here before their transaction completed.
pub trait Backend: Send + Sync + 'static {
    fn execute(&self, request: Request) -> Result<Value, String>;
    fn maintain(&self,_lane:usize) {}
    fn shutdown(&self,_lane:usize) {}
}
struct Ticket { deadline: Instant, reply: Option<Vec<u8>> }
struct State {
    stopped: bool, snapshot: Vec<u8>, version: u64, daemon_identity: Option<crate::control::WorkerIdentity>,
    source_sessions: Vec<SourceSession>,
    tickets: HashMap<u64, Ticket>, queue: VecDeque<(u64, usize, Request)>, queued_bytes: usize,
}
struct Engine { state: Mutex<State>, ready: Condvar }
static HOST: OnceLock<Mutex<Option<Arc<Engine>>>> = OnceLock::new();
static NEXT_TICKET: AtomicU64 = AtomicU64::new(1);
static SNAPSHOTS_REQUESTED: AtomicBool = AtomicBool::new(false);
pub fn snapshots_requested() -> bool { SNAPSHOTS_REQUESTED.load(Ordering::Acquire) }
fn host() -> Option<Arc<Engine>> { HOST.get_or_init(|| Mutex::new(None)).lock().ok()?.clone() }
fn reply(result: Result<Value, String>) -> Vec<u8> {
    let value = match result { Ok(result) => serde_json::json!({"ok":true,"result":result}),
        Err(error) => serde_json::json!({"ok":false,"error":error}) };
    let bytes = serde_json::to_vec(&value).unwrap_or_default();
    if bytes.len() <= MAX_BYTES { bytes } else {
        br#"{"ok":false,"error":"Managed service reply exceeds 4 MiB."}"#.to_vec()
    }
}
/// Own this on the daemon/control lifetime. Starting it does not initialize CLR.
/// Drop stops admission and fails pending tickets. An executing backend cannot
/// be undone; backend teardown must wait for its own guarded transaction.
pub struct Host { engine: Arc<Engine>, workers: Vec<std::thread::JoinHandle<()>> }
#[derive(Clone)]
pub struct Publisher { engine: Arc<Engine> }
impl Host {
    pub fn start(snapshot: Snapshot, backend: Arc<dyn Backend>) -> Result<Self, String> {
        let value = serde_json::to_value(&snapshot).map_err(|e| e.to_string())?;
        if serde_json::to_vec(&value).map_err(|e| e.to_string())?.len() > MAX_BYTES - 64 {
            return Err("Managed snapshot exceeds 4 MiB.".into());
        }
        let bytes = reply(Ok(value));
        let mut slot = HOST.get_or_init(|| Mutex::new(None)).lock().map_err(|_| "Managed owner lock poisoned.")?;
        if slot.is_some() { return Err("Managed service owner already installed.".into()); }
        SNAPSHOTS_REQUESTED.store(false, Ordering::Release);
        let engine = Arc::new(Engine { state: Mutex::new(State { stopped: false,
            snapshot: bytes, version: snapshot.version, daemon_identity: snapshot.daemon_identity, tickets: HashMap::new(),
            source_sessions: snapshot.source_sessions,
            queue: VecDeque::new(), queued_bytes: 0 }), ready: Condvar::new() });
        let mut workers = Vec::new();
        for lane in 0..5 {
        let worker = engine.clone();
        let backend = Arc::clone(&backend);
        let join = std::thread::Builder::new().name(["managed-services","managed-device-io","managed-output-owner","managed-device-detect","managed-input-owner"][lane].into()).spawn(move || loop {
            backend.maintain(lane);
            let work = {
                let Ok(mut state) = worker.state.lock() else { return };
                // A real plugin constructor can ask for shared endpoint I/O
                // while a daemon operation is constructing it. Give these
                // requests their own serial owner rather than waiting behind
                // the constructor's occupied daemon lane.
                let index = loop {
                    if state.stopped {drop(state);backend.shutdown(lane);return;}
                    if let Some(index) = state.queue.iter().position(|(_, _, request)| {
                        let operation_lane=match request.operation {Operation::Daemon=>0,Operation::Detect=>3,Operation::OutputOwner=>2,Operation::InputHold|Operation::InputRelease=>4,_=>1};
                        operation_lane==lane
                    }) { break Some(index); }
                    let Ok((next,timeout)) = worker.ready.wait_timeout(state,Duration::from_millis(50)) else { return }; state = next;
                    if lane==4&&timeout.timed_out(){break None;}
                };
                let Some(index)=index else{continue};
                let (id, size, request) = state.queue.remove(index).unwrap();
                state.queued_bytes -= size;
                if !state.tickets.get(&id).is_some_and(|ticket| Instant::now() < ticket.deadline) {
                    state.tickets.remove(&id); continue;
                }
                (id, request)
            };
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| backend.execute(work.1)))
                .unwrap_or_else(|_| Err("Managed service backend panicked; application status is unknown.".into()));
            let bytes = reply(result);
            if let Ok(mut state) = worker.state.lock() {
                if !state.stopped { if let Some(ticket) = state.tickets.get_mut(&work.0) { ticket.reply = Some(bytes); } }
            }
        });
        match join {
            Ok(join) => workers.push(join),
            Err(error) => {
                if let Ok(mut state) = engine.state.lock() { state.stopped = true; engine.ready.notify_all(); }
                for worker in workers { let _ = worker.join(); }
                return Err(error.to_string());
            }
        }
        }
        *slot = Some(engine.clone());
        Ok(Self { engine, workers })
    }
    pub fn publisher(&self) -> Publisher { Publisher { engine: self.engine.clone() } }
    pub fn publish(&self, snapshot: Snapshot) -> Result<(), String> {
        self.publisher().publish(snapshot)
    }
}
impl Publisher {
    pub fn publish(&self, snapshot: Snapshot) -> Result<(), String> {
        let value = serde_json::to_value(&snapshot).map_err(|e| e.to_string())?;
        if serde_json::to_vec(&value).map_err(|e| e.to_string())?.len() > MAX_BYTES - 64 {
            return Err("Managed snapshot exceeds 4 MiB.".into());
        }
        let bytes = reply(Ok(value));
        let mut state = self.engine.state.lock().map_err(|_| "Managed owner lock poisoned.")?;
        if state.stopped { return Err("Managed service owner stopped.".into()); }
        if snapshot.version <= state.version { return Err("Managed snapshot version must increase.".into()); }
        state.version = snapshot.version; state.daemon_identity = snapshot.daemon_identity;
        state.source_sessions = snapshot.source_sessions; state.snapshot = bytes; Ok(())
    }
}
impl Drop for Host {
    fn drop(&mut self) {
        if let Ok(mut state) = self.engine.state.lock() {
            state.stopped = true; state.queue.clear(); state.queued_bytes = 0;
            for ticket in state.tickets.values_mut() {
                if ticket.reply.is_none() { ticket.reply = Some(reply(Err("Managed owner stopped; an executing transaction may have applied.".into()))); }
            }
            self.engine.ready.notify_all();
        }
        // Never drop this owner on its backend thread or while holding a
        // native transaction lock the backend needs. The daemon owns teardown.
        for worker in self.workers.drain(..) { let _ = worker.join(); }
        if let Ok(mut slot) = HOST.get_or_init(|| Mutex::new(None)).lock() {
            if slot.as_ref().is_some_and(|value| Arc::ptr_eq(value, &self.engine)) { *slot = None; }
        }
    }
}

/// ABI v1. Codes: -1 owner absent, -2 busy, -3 malformed, -4 size, -5 stopped,
/// -6 source absent/ambiguous/not running or currently being replaced.
#[repr(C)]
pub struct Callbacks { pub version: u32, pub size: u32,
    pub request: unsafe extern "C" fn(u32, u64, *const u8, u32, *mut u64) -> i32,
    pub poll: unsafe extern "C" fn(u64, *mut u8, u32) -> i32,
    pub release: unsafe extern "C" fn(u64), }
pub const CALLBACKS: Callbacks = Callbacks { version: 1, size: std::mem::size_of::<Callbacks>() as u32,
    request: request, poll: poll, release: release };
unsafe extern "C" fn request(op: u32, scope: u64, input: *const u8, len: u32, output: *mut u64) -> i32 {
    let Some(operation) = Operation::parse(op) else { return -3 };
    if output.is_null() || (input.is_null() && len != 0) { return -3; }
    if len as usize > MAX_BYTES { return -4; }
    let Some(engine) = host() else { return -1 };
    let payload = if len == 0 { Value::Null } else {
        let bytes = unsafe { std::slice::from_raw_parts(input, len as usize) };
        let Ok(value) = serde_json::from_slice(bytes) else { return -3 }; value
    };
    let Ok(mut state) = engine.state.lock() else { return -5 };
    if state.stopped { return -5; }
    let expected_source = if let Some(value) = payload.get("source_tablet_name") {
        let Some(name) = value.as_str().filter(|name| !name.is_empty() && name.len() <= 4096) else { return -3 };
        let supplied=payload.get("source_session");
        let identity=if let Some(supplied)=supplied {
            let Some(id)=supplied["id"].as_str().filter(|id|!id.is_empty()) else{return -3};
            let Some(generation)=supplied["device_generation"].as_u64().filter(|generation|*generation>0) else{return -3};
            Some((id,generation))
        }else{None};
        let mut matches = state.source_sessions.iter().filter(|source| source.tablet == name
            && identity.is_none_or(|(id,generation)|source.id==id && source.device_generation==generation)
            && source.connected && source.state == crate::device_sessions::SessionState::Running
            && source.pending_generation.is_none());
        let Some(source) = matches.next() else { return -6 };
        if source.id.is_empty() || source.device_generation == 0 || matches.next().is_some() { return -6; }
        Some((source.id.clone(), source.device_generation))
    } else { None };
    let now = Instant::now(); state.tickets.retain(|_, ticket| now < ticket.deadline);
    if state.tickets.len() >= MAX_TICKETS || state.queue.len() >= MAX_TICKETS
        || state.queued_bytes + len as usize > MAX_QUEUED_BYTES { return -2; }
    let Ok(id) = NEXT_TICKET.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| value.checked_add(1)) else { return -5 };
    let completed = (operation == Operation::Snapshot).then(|| state.snapshot.clone());
    if operation == Operation::Snapshot { SNAPSHOTS_REQUESTED.store(true, Ordering::Release); }
    state.tickets.insert(id, Ticket { deadline: now + TTL, reply: completed });
    if operation != Operation::Snapshot {
        let expected_daemon = state.daemon_identity.clone();
        state.queue.push_back((id, len as usize, Request { operation, scope, payload, expected_daemon, expected_source }));
        state.queued_bytes += len as usize; engine.ready.notify_all();
    }
    unsafe { *output = id; } 0
}
unsafe extern "C" fn poll(id: u64, output: *mut u8, capacity: u32) -> i32 {
    let Some(engine) = host() else { return -1 };
    let Ok(mut state) = engine.state.lock() else { return -5 };
    let Some(ticket) = state.tickets.get_mut(&id) else { return -3 };
    if Instant::now() >= ticket.deadline { state.tickets.remove(&id); return -3; }
    let Some(bytes) = ticket.reply.as_ref() else { return 0 };
    if !output.is_null() && capacity as usize >= bytes.len() {
        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), output, bytes.len()); }
    }
    bytes.len() as i32
}
unsafe extern "C" fn release(id: u64) {
    if let Some(engine) = host() { if let Ok(mut state) = engine.state.lock() { state.tickets.remove(&id); } }
}
