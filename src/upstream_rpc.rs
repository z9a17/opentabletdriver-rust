//! Opt-in upstream Windows RPC. Native v2 framing and daemon ownership remain
//! separate. Never start the original daemon or a tablet worker for RPC.
//! Only explicit managed provider/debug requests initialize .NET.
pub(crate) mod protocol;
mod service;
mod settings;
mod apply;
mod debug;
mod collection;

use std::io;
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use crate::control::pipe::CompatPipe;

pub const DEFAULT_PIPE: &str = "OpenTabletDriver.Daemon";
/// Private original-console connection; never takes the original daemon name.
pub const CONSOLE_PIPE: &str = "OpenTabletDriverRust.Console";
const MAX_CLIENTS: u32 = 4;
const IDLE_BUDGET: Duration = Duration::from_secs(120);
const FRAME_BUDGET: Duration = Duration::from_secs(5);

fn shared() -> Arc<service::Shared> {
    static SHARED: std::sync::OnceLock<Arc<service::Shared>> = std::sync::OnceLock::new();
    Arc::clone(SHARED.get_or_init(|| Arc::new(service::Shared::default())))
}
fn provider_connection() -> service::Connection {
    service::Connection::new(shared(), Arc::new(AtomicBool::new(false)))
}
static HOSTED_UPDATE:std::sync::Mutex<Option<String>>=std::sync::Mutex::new(None);
/// Retain update ownership until the original RpcHost confirms response flush.
pub fn install_hosted_update(params:&serde_json::Value)->Result<serde_json::Value,String>{
    use protocol::Service;
    let mut connection=provider_connection();
    let result=connection.invoke("InstallUpdate",params).map_err(|error|error.message)?;
    let token=connection.detach_installed_update().ok_or("Installed update has no retirement token")?;
    *HOSTED_UPDATE.lock().map_err(|_|"Hosted update ownership poisoned")?=Some(token);
    Ok(result)
}
pub fn finish_hosted_update()->Result<serde_json::Value,String>{
    let mut update=HOSTED_UPDATE.lock().map_err(|_|"Hosted update ownership poisoned")?;
    let token=update.as_ref().ok_or("No installed original RPC update is pending")?;
    let response=crate::control::request_owned(&crate::control::Request::new(1,crate::control::Command::FinishUpdate{token:token.clone(),success:true}),Duration::from_secs(5),std::process::id()).map_err(|error|error.to_string())?;
    match response.reply{
        crate::control::Reply::ShutdownAccepted=>{*update=None;Ok(serde_json::Value::Null)},
        crate::control::Reply::Error{error}=>Err(error.message),_=>Err("Unexpected original update retirement receipt".into())
    }
}
/// Background/control worker only; active settings projection uses native IPC.
pub fn get_original_settings() -> Result<serde_json::Value, String> {
    provider_connection().settings().map_err(|error| error.message)
}
pub fn initial_original_settings() -> Result<serde_json::Value, String> {
    collection::snapshot().map(|snapshot| snapshot.document).map_err(|error| error.message)
}
pub fn settings_collection_revision() -> Result<u64,String> {
    collection::snapshot().map(|snapshot| snapshot.revision).map_err(|error| error.message)
}
pub fn cached_settings_collection_revision() -> Option<u64> { collection::cached_revision() }
/// Native daemon-owner admission only: caller must guard lifecycle and worker
/// identity. Does not reserve OWNER because the originating RPC already owns it.
pub fn publish_idle_original_settings(settings:serde_json::Value, expected_revision:u64) -> Result<u64,String> {
    let document = collection::normalize(&settings).map_err(|error| error.message)?;
    let revision = collection::publish(document,expected_revision,true).map_err(|error| error.message)?;
    shared().resynchronize.fetch_add(1,Ordering::AcqRel);
    Ok(revision)
}
/// Report callbacks may only use this nonblocking, already initialized snapshot.
/// None means unavailable/busy; no disk, CLR, native IPC or mutation is performed.
pub fn cached_original_settings() -> Option<Arc<serde_json::Value>> { collection::cached() }
pub fn set_original_settings(settings: serde_json::Value) -> Result<(), String> {
    provider_connection().set_settings(&settings).map(|_| ()).map_err(|error| error.message)
}
pub fn set_original_settings_expected(settings: serde_json::Value,
    expected: crate::control::WorkerIdentity) -> Result<(), String> {
    set_original_settings_expected_with_source(settings,expected,None)
}
pub fn set_original_settings_expected_with_source(settings:serde_json::Value,
    expected:crate::control::WorkerIdentity,source:Option<(String,u32)>) -> Result<(),String> {
    set_original_settings_expected_with_source_generation(settings,expected,source,None)
}
pub fn set_original_settings_expected_with_source_generation(settings:serde_json::Value,
    expected:crate::control::WorkerIdentity,source:Option<(String,u32)>,expected_source:Option<(String,u64)>) -> Result<(),String> {
    provider_connection().set_settings_expected(&settings,expected,source,expected_source).map(|_| ()).map_err(|error| error.message)
}
pub fn reset_original_settings() -> Result<(), String> { set_original_settings(serde_json::Value::Null) }
pub fn load_original_settings() -> Result<(), String> {
    provider_connection().load_settings().map(|_| ()).map_err(|error| error.message)
}
pub fn save_original_settings() -> Result<(), String> {
    provider_connection().save_settings().map(|_| ()).map_err(|error| error.message)
}
pub fn original_settings_path() -> Result<std::path::PathBuf, String> { collection::path() }
/// Path metadata only, available before the native control pipe is serving.
pub fn original_application_info() -> Result<serde_json::Value, String> {
    let data = otd_core::storage::data_directory()?;
    Ok(serde_json::json!({"AppDataDirectory":data,"SettingsFile":collection::path()?,
        "PluginDirectory":crate::plugin_catalog::plugins_directory()?,
        "PresetDirectory":otd_core::presets::PresetStore::user()?.directory(),"LogDirectory":data,
        // DesktopPluginManager.Clean recursively removes its temporary/trash
        // folders. Never expose the process-wide system temp directory as one.
        "TemporaryDirectory":data.join("compat-temp"),"CacheDirectory":data.join("cache"),
        "BackupDirectory":data.join("backup"),"TrashDirectory":data.join("trash"),
        "ConfigurationDirectory":otd_core::config::configurations_directory()}))
}
pub fn invoke_original(method: &str, params: &serde_json::Value) -> Result<serde_json::Value, String> {
    use protocol::Service;
    provider_connection().invoke(method,params).map_err(|error| error.message)
}
pub fn original_resynchronize_epoch() -> u64 { shared().resynchronize.load(Ordering::Acquire) }
pub fn profile_for_tablet(tablet: &otd_core::tablets::TabletConfiguration)
    -> Result<Option<crate::config::Profile>, String> { collection::profile_for_tablet(tablet) }

pub struct Listener { stop: Arc<AtomicBool>, workers: Vec<JoinHandle<()>> }
impl Listener {
    pub fn start(name: &str) -> io::Result<Self> {
        let pipes = CompatPipe::instances(name, MAX_CLIENTS)?;
        let shared = shared();
        let mut listener = Self { stop: Arc::new(AtomicBool::new(false)), workers: Vec::new() };
        for (index, pipe) in pipes.into_iter().enumerate() {
            let stop = Arc::clone(&listener.stop);
            let shared = Arc::clone(&shared);
            listener.workers.push(std::thread::Builder::new().name(format!("upstream-rpc-{index}"))
                .spawn(move || {
                    while !stop.load(Ordering::Acquire) {
                        if pipe.connect(&stop).is_err() {
                            pipe.disconnect();
                            if !stop.load(Ordering::Acquire) { std::thread::sleep(Duration::from_millis(25)); }
                            continue;
                        }
                        let mut service = service::Connection::new(Arc::clone(&shared), Arc::clone(&stop));
                        let close_after_reply = serve_connection(&pipe, &stop, &mut service).unwrap_or(false);
                        if close_after_reply { break; }
                        // Do not FlushFileBuffers: a stalled client could block
                        // daemon cleanup. All pending IO has completed/drained.
                        pipe.disconnect();
                    }
                })?);
        }
        Ok(listener)
    }
}
impl Drop for Listener {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        for worker in self.workers.drain(..) { let _ = worker.join(); }
    }
}
fn write(pipe: &CompatPipe, stop: &AtomicBool, value: &serde_json::Value) -> io::Result<()> {
    pipe.write(&protocol::encode(value)?, Instant::now() + FRAME_BUDGET, stop)
}
fn serve_connection(pipe: &CompatPipe, stop: &AtomicBool, service: &mut service::Connection) -> io::Result<bool> {
    while !stop.load(Ordering::Acquire) {
        let mut failure = None;
        let body = read_message(|buffer, deadline| pipe.read(buffer, deadline, stop, &mut || {
            if failure.is_none() {
                for event in service.events() {
                    if let Err(error) = write(pipe, stop, &event) {
                        failure = Some(error);
                        pipe.cancel_pending();
                        break;
                    }
                }
            }
        }))?;
        if let Some(error) = failure { return Err(error); }
        if let Some(response) = protocol::response(&body, service) {
            pipe.write(&protocol::encode_response(&response)?, Instant::now() + FRAME_BUDGET, stop)?;
        }
        if service.after_reply() {
            // CloseHandle preserves normal pipe EOF semantics; explicitly
            // disconnecting here would discard an unread final update reply.
            return Ok(true);
        }
        // Busy clients must receive events too, rather than depending on IO wait.
        for event in service.events() { write(pipe, stop, &event)?; }
    }
    Ok(false)
}

/// Exact reads preserve coalesced frames and fragmented headers without an
/// unbounded receive buffer. Idle budget starts per message; after the first
/// byte, the complete header/body share a five-second budget.
fn read_message(mut read: impl FnMut(&mut [u8], Instant) -> io::Result<usize>) -> io::Result<Vec<u8>> {
    let mut header = Vec::with_capacity(128);
    let mut deadline = Instant::now() + IDLE_BUDGET;
    loop {
        let mut byte = [0];
        if read(&mut byte, deadline)? != 1 { return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "RPC connection closed")); }
        if header.is_empty() { deadline = Instant::now() + FRAME_BUDGET; }
        header.push(byte[0]);
        if header.len() > protocol::MAX_HEADER { return Err(io::Error::new(io::ErrorKind::InvalidData, "RPC headers exceed 4 KiB")); }
        if header.ends_with(b"\r\n\r\n") { break; }
    }
    let size = protocol::body_length(&header)?;
    let mut body = vec![0; size];
    let mut filled = 0;
    while filled < size {
        let bytes = read(&mut body[filled..], deadline)?;
        if bytes == 0 || bytes > size - filled { return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "RPC body truncated")); }
        filled += bytes;
    }
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    #[test]
    fn persistent_coalesced_and_fragmented_streamjsonrpc_frames() {
        let value = serde_json::json!({"jsonrpc":"2.0","method":"GetTablets","id":1});
        let encoded = protocol::encode(&value).unwrap();
        let mut stream = std::io::Cursor::new([encoded.clone(),encoded].concat());
        for _ in 0..2 {
            let body = read_message(|buffer, _| { let size = buffer.len().min(3); stream.read(&mut buffer[..size]) }).unwrap();
            assert_eq!(serde_json::from_slice::<serde_json::Value>(&body).unwrap(), value);
        }
        assert!(read_message(|buffer, _| stream.read(buffer)).is_err());
    }
    #[test]
    fn truncated_body_and_oversized_headers_never_dispatch() {
        for bytes in [b"Content-Length: 4\r\n\r\n{}".to_vec(), vec![b'x';protocol::MAX_HEADER + 1]] {
            let mut stream = std::io::Cursor::new(bytes);
            assert!(read_message(|buffer, _| stream.read(buffer)).is_err());
        }
        assert_eq!(service::METHODS.len(), 19);
    }
}
