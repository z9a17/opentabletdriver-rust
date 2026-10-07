//! Pinned IDriverDaemon methods. Missing providers return errors, never a
//! successful empty/default result. This executes only on compatibility workers.
use std::path::Path;
use std::sync::{Arc, atomic::{AtomicBool, AtomicU64, Ordering}};
use std::time::{Duration, Instant};
use serde_json::{Value, json};
use crate::control::{self, Command, ControlStatus, Reply, Request, UpstreamLogMessage};
use super::protocol::{self, Error, Service};

pub const METHODS: &[&str] = &[
    "WriteMessage", "LoadPlugins", "InstallPlugin", "UninstallPlugin", "DownloadPlugin",
    "GetDevices", "GetTablets", "DetectTablets", "SetSettings", "GetSettings", "ResetSettings",
    "GetApplicationInfo", "SetTabletDebug", "RequestDeviceString", "GetCurrentLog",
    "GetDiagnosticInfo", "CheckForUpdates", "InstallUpdate", "ForceResynchronize",
];
pub struct Shared { pub resynchronize: AtomicU64, pub update: std::sync::Mutex<Option<crate::update::Release>> }
impl Default for Shared { fn default() -> Self { Self { resynchronize: AtomicU64::new(0), update: std::sync::Mutex::new(None) } } }
pub struct Connection {
    shared: Arc<Shared>,
    stop: Arc<AtomicBool>,
    next_poll: Instant,
    log_cursor: Option<(String, u64)>,
    tablet_cursor: Option<Value>,
    resync_cursor: u64,
    update: Option<UpdateOwnership>,
}
struct UpdateOwnership { token: String, exit: bool }
impl Connection {
    pub fn new(shared: Arc<Shared>, stop: Arc<AtomicBool>) -> Self {
        let resync_cursor = shared.resynchronize.load(Ordering::Acquire);
        Self { shared, stop, next_poll: Instant::now(), log_cursor: None,
            tablet_cursor: None, resync_cursor, update:None }
    }
    fn call(&self, command: Command) -> Result<Reply, Error> {
        if self.stop.load(Ordering::Acquire) { return Err(Error::failed("daemon is shutting down")); }
        let response = control::request_owned(&Request::new(1, command), Duration::from_secs(5), std::process::id())
            .map_err(|error| Error::failed(format!("native control: {error}; query state before retrying a mutation")))?;
        match response.reply {
            Reply::Error { error } => Err(Error::failed(format!("{:?}: {}", error.code, error.message))),
            reply => Ok(reply),
        }
    }
    fn status(&self) -> Result<ControlStatus, Error> {
        match self.call(Command::Status)? { Reply::Status { status } => Ok(status), _ => Err(Error::failed("unexpected native status")) }
    }
    fn finish_update(&mut self) -> Result<bool, Error> {
        let Some(update) = &self.update else { return Ok(false); };
        // Ownership cleanup must run even after this connection/daemon's local
        // listener cancellation was requested. PID/token guards prevent it
        // affecting a replacement daemon or another updater.
        let response = control::request_owned(&Request::new(1,Command::FinishUpdate {
            token:update.token.clone(),success:update.exit }),Duration::from_secs(5),std::process::id())
            .map_err(|error| Error::failed(format!("update ownership cleanup: {error}")))?;
        let exit = update.exit;
        match response.reply {
            Reply::ShutdownAccepted if exit => {},
            Reply::UpdateCancelled if !exit => {},
            Reply::Error { error } => return Err(Error::failed(error.message)),
            _ => return Err(Error::failed("unexpected update cleanup reply")),
        }
        self.update = None;
        Ok(exit)
    }
    /// Called only after the compatibility response has finished writing.
    pub fn after_reply(&mut self) -> bool {
        let exit = self.update.as_ref().is_some_and(|update| update.exit);
        if exit { let _ = self.finish_update(); }
        exit
    }
    fn install_update(&mut self) -> Result<Value, Error> {
        let release = self.shared.update.lock().map_err(|_| Error::failed("update state lock poisoned"))?
            .take().ok_or_else(|| Error::failed("No checked update is available; call CheckForUpdates first"))?;
        let expected = self.status()?.identity();
        let token = match self.call(Command::BeginUpdate { expected })? {
            Reply::UpdateAccepted { token } => token,
            _ => return Err(Error::failed("unexpected update reservation reply")),
        };
        self.update = Some(UpdateOwnership { token:token.clone(),exit:false });
        let deadline = Instant::now() + Duration::from_secs(30);
        let result = (|| -> Result<Value, Error> {
            loop {
                match self.call(Command::UpdateStatus { token:token.clone() })? {
                    Reply::UpdateState { token:reply_token,ready,error } if reply_token == token => {
                        if let Some(error) = error { return Err(Error::failed(error)); }
                        if ready { break; }
                    },
                    _ => return Err(Error::failed("unexpected update reservation state")),
                }
                if Instant::now() >= deadline { return Err(Error::failed("update resource drain timed out")); }
                std::thread::sleep(Duration::from_millis(25));
            }
            let executable = std::env::current_exe().map_err(|error| Error::failed(error.to_string()))?;
            let folder = executable.parent().ok_or_else(|| Error::failed("install directory is unavailable"))?;
            match crate::update::install(&release,folder,&|_| {}) {
                Ok(()) => { self.update.as_mut().unwrap().exit = true; Ok(Value::Null) },
                Err(error) => {
                    // Download/hash failures have no transaction. A journaled
                    // replacement failure must recover before permitting new
                    // workers; any recovered/uncertain installed files require
                    // this old daemon to exit instead of loading mixed versions.
                    match crate::update::recover_for_daemon(folder) {
                        Ok(false) => Err(Error::failed(error)),
                        Ok(true) => { self.update.as_mut().unwrap().exit = true;
                            Err(Error::failed(format!("{error}; interrupted update recovered; daemon will exit"))) },
                        Err(recovery) => { self.update.as_mut().unwrap().exit = true;
                            Err(Error::failed(format!("{error}; update recovery failed: {recovery}; daemon will exit and preserve recovery files"))) },
                    }
                }
            }
        })();
        if !self.update.as_ref().is_some_and(|update| update.exit) {
            if let Err(cleanup) = self.finish_update() {
                return Err(Error::failed(format!("{}; {}",result.err().map_or_else(|| "update failed".into(),|error| error.message),cleanup.message)));
            }
        }
        result
    }
    fn tablets(&self) -> Result<Value, Error> {
        self.tablet_reply(Command::ListDeviceSessions)
    }
    fn tablet_reply(&self, command: Command) -> Result<Value, Error> {
        match self.call(command)? {
            Reply::DeviceSessions { sessions, .. } => {
                let values: Vec<_> = sessions.iter().filter(|session| session.connected).map(|session| {
                    let mut identifiers = vec![session.digitizer.clone()];
                    if let Some(auxiliary) = &session.auxiliary { identifiers.push(auxiliary.clone()); }
                    json!({"Properties":session.properties,"Identifiers":identifiers})
                }).collect();
                Ok(json!(values))
            }
            _ => Err(Error::failed("unexpected device session snapshot")),
        }
    }
    fn settings(&self) -> Result<Value, Error> {
        let expected = self.status()?.identity();
        let text = match self.call(Command::GetConfiguration { expected: expected.clone() })? {
            Reply::Configuration { identity, profile_toml: Some(text) } if identity == expected => text,
            Reply::Configuration { profile_toml: None, .. } => return Err(Error::failed("daemon has no settings configuration")),
            _ => return Err(Error::failed("configuration changed while reading settings")),
        };
        let path = otd_core::storage::data_directory()?.join("driver.toml");
        let profile = crate::config::Profile::from_toml_text(&text, &path)?;
        // Strict export retains original store identities. Standalone native
        // profiles/unreconciled DLL paths explicitly fail instead of inventing
        // OTD stores or returning stale archived settings as active settings.
        let sessions = match self.call(Command::ListDeviceSessions)? {
            Reply::DeviceSessions { sessions, .. } => sessions,
            _ => return Err(Error::failed("unexpected device session snapshot")),
        };
        let screen = crate::display::read_snapshot()?.virtual_screen;
        let profile = if profile.imported_otd.is_none() {
            let tablet = sessions.iter().find(|session| session.primary && session.connected)
                .ok_or_else(|| Error::failed("native settings export needs the actual primary tablet"))?;
            super::settings::canonical_copy(&profile, &tablet.properties, screen)?
        } else { profile };
        let exported = export_settings(&profile)?;
        let mut document: Value = serde_json::from_str(&exported).map_err(|error| Error::failed(error.to_string()))?;
        let mut active = std::collections::BTreeMap::<String,Value>::new();
        for session in sessions.iter().filter(|session| session.connected) {
            let per_device = if session.primary { profile.clone() } else {
                let text = match self.call(Command::GetDeviceProfile { expected:expected.clone(), id:session.id.clone(),
                    device_generation:session.device_generation })? {
                    Reply::DeviceProfile { identity, id, device_generation, profile_toml }
                        if identity == expected && id == session.id && device_generation == session.device_generation => profile_toml,
                    _ => return Err(Error::failed("device configuration changed while reading settings")),
                };
                let device_profile = crate::config::Profile::from_toml_text(&text,&path)?;
                super::settings::canonical_copy(&device_profile,&session.properties,screen)?
            };
            let device_document: Value = serde_json::from_str(&export_settings(&per_device)?).map_err(|error| Error::failed(error.to_string()))?;
            if device_document["Tools"] != document["Tools"] {
                return Err(Error::unsupported("GetSettings", "per-device tool collections cannot be represented by one upstream global Tools collection"));
            }
            let index = per_device.imported_otd.as_ref().ok_or_else(|| Error::failed("export projection has no selected profile"))?.selected_profile;
            let selected = device_document["Profiles"].get(index).ok_or_else(|| Error::failed("selected exported profile is missing"))?.clone();
            if let Some(previous) = active.insert(session.tablet.clone(),selected.clone()) {
                if previous != selected { return Err(Error::unsupported("GetSettings", "same-model physical tablets have differing native profiles; upstream profiles are keyed by tablet name")); }
            }
        }
        let profiles = document["Profiles"].as_array_mut().ok_or_else(|| Error::failed("exported settings have no Profiles collection"))?;
        for (name,selected) in active {
            let matches: Vec<_> = profiles.iter().enumerate().filter(|(_,profile)| profile["Tablet"] == name).map(|(index,_)| index).collect();
            match matches.as_slice() {
                [] => profiles.push(selected),
                [index,..] => profiles[*index] = selected,
            }
        }
        if self.status()?.identity() != expected { return Err(Error::failed("daemon configuration changed while reading settings")); }
        Ok(document)
    }
    fn session_snapshots(&self) -> Result<Vec<crate::device_sessions::SessionSnapshot>,Error> {
        match self.call(Command::ListDeviceSessions)? {
            Reply::DeviceSessions { sessions,.. } => Ok(sessions),
            _ => Err(Error::failed("unexpected device session snapshot")),
        }
    }
    fn device_profile(&self,id:&str,generation:u64,instance:&str) -> Result<String,Error> {
        let status = self.status()?;
        if status.instance != instance { return Err(Error::failed("daemon identity changed during settings operation")); }
        let expected = status.identity();
        match self.call(Command::GetDeviceProfile { expected:expected.clone(),id:id.into(),device_generation:generation })? {
            Reply::DeviceProfile { identity,id:reply_id,device_generation,profile_toml }
                if identity == expected && reply_id == id && device_generation == generation => Ok(profile_toml),
            _ => Err(Error::failed("unexpected guarded device profile reply")),
        }
    }
    fn set_settings(&mut self,settings:&Value) -> Result<Value,Error> {
        let status = self.status()?;
        let sessions = self.session_snapshots()?;
        let sessions:Vec<_> = sessions.into_iter().filter(|session| session.connected).collect();
        if sessions.is_empty() { return Err(Error::unsupported("SetSettings","idle settings collection storage without detected sessions is not implemented")); }
        if sessions.iter().any(|session| session.pending_generation.is_some()
            || !matches!(session.state,crate::device_sessions::SessionState::Running | crate::device_sessions::SessionState::Stopped)) {
            return Err(Error::failed("device sessions are transitioning/unavailable; refresh and retry without replacing pending operations"));
        }
        let tablets:Vec<_> = sessions.iter().map(|session| session.properties.clone()).collect();
        let displays = crate::display::read_snapshot()?;
        let settings = super::settings::for_detected(settings,&tablets,displays.virtual_screen)?;
        let profiles = settings["Profiles"].as_array().ok_or_else(|| Error::invalid("settings must contain Profiles"))?;
        let text = serde_json::to_string(&settings).map_err(|error| Error::invalid(error.to_string()))?;
        if text.len() > control::MAX_PROFILE_BYTES { return Err(Error::invalid("settings exceed 128 KiB")); }
        let path = otd_core::storage::data_directory()?.join("upstream-rpc-settings.json");
        let mut plans = Vec::new();
        // Validate every applicable profile and capture every guarded prior
        // configuration before accepting the first mutation.
        for session in &sessions {
            let index = profiles.iter().position(|profile| profile["Tablet"] == session.tablet)
                .ok_or_else(|| Error::invalid("detected tablet profile is missing"))?;
            let profile = crate::config::Profile::from_otd_profile_text(&text,&path,index,Default::default())?;
            if let Some(diagnostic) = profile.diagnostics.iter().find(|diagnostic| diagnostic.kind == "unsupported_active") {
                return Err(Error::unsupported("SetSettings",&diagnostic.message));
            }
            let profile = profile.for_tablet(otd_core::spec::TabletSpec::from_configuration(&session.properties)?)?;
            profile.validate_runtime_tablet()?;
            if profile.relative.is_none() { displays.mapper(&profile)?; }
            let replacement = profile.to_toml()?;
            let encoded = serde_json::to_vec(&replacement).map_err(|error| Error::invalid(error.to_string()))?;
            if replacement.len() > control::MAX_PROFILE_BYTES || encoded.len() > control::MAX_FRAME_BYTES - 1024 {
                return Err(Error::invalid("imported profile exceeds native control limits"));
            }
            let before = self.device_profile(&session.id,session.device_generation,&status.instance)?;
            plans.push(super::apply::Plan { id:session.id.clone(),generation:session.device_generation,before,replacement });
        }
        if self.status()?.identity() != status.identity() { return Err(Error::failed("daemon configuration changed during settings preflight")); }
        let mut backend = DeviceApply { connection:self,instance:status.instance };
        let result = super::apply::run(&plans,&mut backend,Instant::now()+Duration::from_secs(40),
            || Instant::now()+Duration::from_secs(40));
        if let Err(error) = result {
            self.shared.resynchronize.fetch_add(1,Ordering::AcqRel);
            return Err(error);
        }
        Ok(Value::Null)
    }
    pub fn events(&mut self) -> Vec<Value> {
        if Instant::now() < self.next_poll || self.stop.load(Ordering::Acquire) { return Vec::new(); }
        self.next_poll = Instant::now() + Duration::from_millis(250);
        let mut events = Vec::new();
        if let Ok(status) = self.status() {
            if let Some((instance, sequence)) = &self.log_cursor {
                if instance == &status.instance {
                    let available_start = status.log_sequence.saturating_sub(status.logs.len() as u64);
                    let start = sequence.saturating_sub(available_start).min(status.logs.len() as u64) as usize;
                    // A lagging reader sees retained logs only. Retention is
                    // bounded and timestamps below are observation timestamps.
                    for line in &status.logs[start..] {
                        events.push(protocol::event("Message", native_log(line)));
                    }
                }
            }
            self.log_cursor = Some((status.instance, status.log_sequence));
        }
        if let Ok(tablets) = self.tablets() {
            if self.tablet_cursor.as_ref().is_some_and(|previous| previous != &tablets) {
                events.push(protocol::event("TabletsChanged", tablets.clone()));
            }
            self.tablet_cursor = Some(tablets);
        }
        let resync = self.shared.resynchronize.load(Ordering::Acquire);
        if self.resync_cursor != resync {
            self.resync_cursor = resync;
            events.push(protocol::event("Resynchronize", json!({})));
        }
        events
    }
}
impl Drop for Connection {
    fn drop(&mut self) {
        // A failed response write still exits after committed replacement;
        // an abandoned uncommitted reservation is cancelled, never forgotten.
        let _ = self.finish_update();
    }
}
struct DeviceApply<'a> { connection:&'a mut Connection,instance:String }
impl super::apply::Backend for DeviceApply<'_> {
    fn apply(&mut self,id:&str,generation:u64,text:&str) -> Result<crate::device_sessions::SessionReceipt,Error> {
        let status = self.connection.status()?;
        if status.instance != self.instance { return Err(Error::failed("daemon identity changed during device apply")); }
        match self.connection.call(Command::ApplyDeviceProfile { expected:status.identity(),id:id.into(),
            device_generation:generation,profile_toml:text.into() })? {
            Reply::DeviceOperationAccepted { receipt } => Ok(receipt),
            _ => Err(Error::failed("unexpected device operation receipt")),
        }
    }
    fn wait(&mut self,receipt:&crate::device_sessions::SessionReceipt,_text:&str,deadline:Instant) -> Result<(),Error> {
        loop {
            let status = self.connection.status()?;
            if status.instance != self.instance { return Err(Error::failed("daemon identity changed during device apply")); }
            let sessions = self.connection.session_snapshots()?;
            let session = sessions.iter().find(|session| session.id == receipt.id)
                .ok_or_else(|| Error::failed("device session disappeared during settings apply"))?;
            if session.device_generation > receipt.target_generation
                || session.pending_generation.is_some_and(|generation| generation != receipt.target_generation) {
                return Err(Error::failed("another client replaced this device operation"));
            }
            if session.device_generation == receipt.target_generation && session.pending_generation.is_none() {
                if matches!(session.state,crate::device_sessions::SessionState::Running
                    | crate::device_sessions::SessionState::Stopped | crate::device_sessions::SessionState::Waiting) {
                    // This generation is committed only after actual Run or an
                    // explicit stopped-profile commit. Verified native ports
                    // may rewrite runtime TOML, so byte identity is not the
                    // operation receipt's completion contract.
                    let _ = self.connection.device_profile(&receipt.id,receipt.target_generation,&self.instance)?;
                    return Ok(());
                }
            }
            if session.pending_generation.is_none() || session.state == crate::device_sessions::SessionState::Failed {
                return Err(Error::failed(session.last_error.clone().unwrap_or_else(|| "device settings apply failed".into())));
            }
            if Instant::now() >= deadline { return Err(Error::failed("device settings apply remains pending; query sessions/settings before retrying")); }
            std::thread::sleep(Duration::from_millis(25));
        }
    }
    fn restore(&mut self,plan:&super::apply::Plan,receipt:&crate::device_sessions::SessionReceipt,deadline:Instant) -> Result<(),Error> {
        let sessions = self.connection.session_snapshots()?;
        let session = sessions.iter().find(|session| session.id == plan.id).ok_or_else(|| Error::failed("device session disappeared"))?;
        if session.pending_generation.is_some() { return Err(Error::failed("device operation is still pending; it was not replaced")); }
        if session.device_generation == plan.generation {
            let retained = self.connection.device_profile(&plan.id,plan.generation,&self.instance)?;
            if retained == plan.before { return Ok(()); }
            return Err(Error::failed("prior device generation has unexpected settings"));
        }
        if session.device_generation != receipt.target_generation {
            return Err(Error::failed("newer device generation preserved"));
        }
        if Instant::now() >= deadline { return Err(Error::failed("settings rollback deadline expired")); }
        let restored = self.apply(&plan.id,receipt.target_generation,&plan.before)?;
        self.wait(&restored,&plan.before,deadline)
    }
}
/// Resolved Windows DLL paths are host details, not OTD store identities. Only
/// reconcile an existing one-to-one store in the same group/order. Extra DLLs,
/// native ABI entries, mixed native/managed Radial Follow or ambiguous stores
/// fail rather than returning stale source filters as if they were active.
fn export_settings(profile: &crate::config::Profile) -> Result<String, Error> {
    if profile.plugins.is_empty() { return Ok(profile.to_otd_json()?); }
    use otd_core::plugins::PluginKind;
    let mut copy = profile.clone();
    let imported = copy.imported_otd.as_mut().ok_or_else(|| Error::unsupported("GetSettings", "native DLL entries have no original OTD stores"))?;
    let mut document: Value = serde_json::from_str(&imported.settings_json).map_err(|error| Error::failed(error.to_string()))?;
    let filters: Vec<_> = profile.plugins.iter().filter(|plugin| plugin.kind != PluginKind::DotnetTool).collect();
    if filters.iter().any(|plugin| plugin.kind != PluginKind::Dotnet) {
        return Err(Error::unsupported("GetSettings", "native ABI DLL filters cannot be represented as unchanged OTD stores"));
    }
    if !profile.radial_follow.is_empty() && filters.iter().any(|plugin| plugin.type_name == otd_core::radial_follow::FILTER_PATH) {
        return Err(Error::unsupported("GetSettings", "mixed native/managed Radial Follow source reconciliation is ambiguous"));
    }
    fn reconcile(stores: &mut Value, plugins: &[&otd_core::plugins::PluginConfig], native_radial: bool) -> Result<(), Error> {
        let stores = stores.as_array_mut().ok_or_else(|| Error::unsupported("GetSettings", "original plugin collection is missing"))?;
        let relevant: Vec<_> = stores.iter().enumerate().filter(|(_, store)| !(native_radial && store["Path"] == otd_core::radial_follow::FILTER_PATH)).map(|(index, _)| index).collect();
        if relevant.len() != plugins.len() { return Err(Error::unsupported("GetSettings", "original and runtime plugin collections differ in length")); }
        for (index, plugin) in relevant.into_iter().zip(plugins) {
            let store = &mut stores[index];
            if store["Path"] != plugin.type_name { return Err(Error::unsupported("GetSettings", "original and runtime plugin order/type differs")); }
            let settings: serde_json::Map<String, Value> = serde_json::from_str(&plugin.settings_json)
                .map_err(|error| Error::failed(error.to_string()))?;
            store["Enable"] = json!(plugin.enabled);
            store["Settings"] = json!(settings.into_iter().map(|(property,value)| json!({"Property":property,"Value":value})).collect::<Vec<_>>());
        }
        Ok(())
    }
    let selected = document.get_mut("Profiles").and_then(Value::as_array_mut)
        .and_then(|profiles| profiles.get_mut(imported.selected_profile))
        .ok_or_else(|| Error::failed("original selected OTD profile is missing"))?;
    reconcile(&mut selected["Filters"], &filters, !profile.radial_follow.is_empty())?;
    let tools: Vec<_> = profile.plugins.iter().filter(|plugin| plugin.kind == PluginKind::DotnetTool).collect();
    // Missing Tools is a legitimate empty collection in older imported documents.
    if document.get("Tools").is_none() && tools.is_empty() { document["Tools"] = json!([]); }
    reconcile(&mut document["Tools"], &tools, false)?;
    imported.settings_json = serde_json::to_string(&document).map_err(|error| Error::failed(error.to_string()))?;
    // Strict core export interprets a source RF store as its native import.
    // For a managed RF store, mirror that import only in the disposable export
    // copy so export retains the reconciled source store, including Enable.
    // The live profile/filter chain is never changed by this projection.
    if profile.radial_follow.is_empty() && filters.iter().any(|plugin| plugin.type_name == otd_core::radial_follow::FILTER_PATH) {
        copy.radial_follow = crate::config::Profile::from_otd_profile_text(&imported.settings_json,
            Path::new(&imported.source_path), imported.selected_profile, Default::default())?.radial_follow;
    }
    copy.plugins.clear();
    Ok(copy.to_otd_json()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn active_managed_stores_export_without_runtime_paths_or_dropping_settings() {
        let source = json!({"Profiles":[{"Tablet":"Wacom PTH-660",
            "OutputMode":{"Path":"OpenTabletDriver.Desktop.Output.AbsoluteMode","Enable":true},
            "AbsoluteModeSettings":{"Display":{"Width":2560,"Height":1440,"X":1280,"Y":720,"Rotation":0},
                "Tablet":{"Width":224,"Height":148,"X":112,"Y":74,"Rotation":0},"EnableClipping":true},
            "Filters":[{"Path":"Example.Filter","Enable":true,"Settings":[{"Property":"Amount","Value":1}]}],
            "Bindings":{}}],"Tools":[]});
        let mut profile = crate::config::Profile::from_otd_text(&source.to_string(), Path::new("source.json")).unwrap();
        profile.plugins.push(otd_core::plugins::PluginConfig {
            path: Path::new("E:/resolved/Example.dll").into(),
            kind: otd_core::plugins::PluginKind::Dotnet, enabled: true,
            type_name: "Example.Filter".into(), settings_json: r#"{"Amount":2,"Extra":null}"#.into(),
        });
        let original = profile.to_toml().unwrap();
        let exported: Value = serde_json::from_str(&export_settings(&profile).unwrap()).unwrap();
        assert_eq!(exported["Profiles"][0]["Filters"][0]["Path"], "Example.Filter");
        assert_eq!(exported["Profiles"][0]["Filters"][0]["Settings"], json!([
            {"Property":"Amount","Value":2},{"Property":"Extra","Value":null}]));
        assert!(!exported.to_string().contains("resolved"));
        assert_eq!(profile.to_toml().unwrap(), original);
        profile.plugins[0].type_name = "Another.Filter".into();
        assert!(export_settings(&profile).is_err());
    }
}
fn native_log(line: &str) -> Value {
    let mut now: windows_sys::Win32::Foundation::SYSTEMTIME = unsafe { std::mem::zeroed() };
    unsafe { windows_sys::Win32::System::SystemInformation::GetSystemTime(&mut now); }
    json!({"Time":format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",now.wYear,now.wMonth,now.wDay,now.wHour,now.wMinute,now.wSecond,now.wMilliseconds),
        "Group":"RustDaemon (observed)","Message":line,"StackTrace":null,"Level":1,"Notification":false})
}
fn text_argument<'a>(params: &'a Value, name: &str) -> Result<&'a str, Error> {
    protocol::argument(params, name)?.as_str().ok_or_else(|| Error::invalid(format!("{name} must be a string")))
}
fn aliased_argument<'a>(params: &'a Value, name: &str, implementation_name: &str) -> Result<&'a Value, Error> {
    if let Value::Object(values) = params {
        if values.len() == 1 && values.contains_key(implementation_name) {
            return protocol::argument(params, implementation_name);
        }
    }
    protocol::argument(params, name)
}
fn operating_system() -> Result<Value, Error> {
    use windows_sys::Win32::System::SystemInformation::{GetVersionExW, OSVERSIONINFOW};
    let mut version: OSVERSIONINFOW = unsafe { std::mem::zeroed() };
    version.dwOSVersionInfoSize = std::mem::size_of::<OSVERSIONINFOW>() as u32;
    if unsafe { GetVersionExW(&mut version) } == 0 {
        return Err(Error::failed(format!("Windows version: {}", std::io::Error::last_os_error())));
    }
    let end = version.szCSDVersion.iter().position(|value| *value == 0).unwrap_or(version.szCSDVersion.len());
    Ok(json!({"Name":"Win32NT","Version":format!("{}.{}.{}",version.dwMajorVersion,version.dwMinorVersion,version.dwBuildNumber),
        "Attributes":{"ServicePack":String::from_utf16_lossy(&version.szCSDVersion[..end]),
            "Architecture":std::env::consts::ARCH,"VersionProvider":"GetVersionExW (manifest-dependent)"}}))
}
impl Service for Connection {
    fn invoke(&mut self, method: &str, params: &Value) -> Result<Value, Error> {
        match method {
            "GetTablets" => { protocol::no_arguments(params)?; self.tablets() },
            "GetSettings" => { protocol::no_arguments(params)?; self.settings() },
            "SetSettings" => self.set_settings(protocol::argument(params, "settings")?),
            "GetCurrentLog" => {
                protocol::no_arguments(params)?;
                Ok(json!(self.status()?.logs.iter().map(|line| native_log(line)).collect::<Vec<_>>()))
            }
            "WriteMessage" => {
                let message: UpstreamLogMessage = serde_json::from_value(protocol::argument(params, "message")?.clone())
                    .map_err(|error| Error::invalid(error.to_string()))?;
                match self.call(Command::WriteMessage { message })? {
                    Reply::MessageWritten => Ok(Value::Null), _ => Err(Error::failed("unexpected log-write response")),
                }
            }
            "InstallPlugin" => {
                crate::plugin_catalog::install_file(Path::new(text_argument(params, "filePath")?))?;
                Ok(json!(true))
            }
            "UninstallPlugin" => {
                // Pinned implementation actually accepts a directory path,
                // while IDriverDaemon names it friendlyName. Accept either only
                // after matching one real installed identity; never arbitrary rm.
                let name = aliased_argument(params, "friendlyName", "directoryPath")?.as_str()
                    .ok_or_else(|| Error::invalid("friendlyName/directoryPath must be a string"))?;
                let matches: Vec<_> = crate::plugin_catalog::installed()?.into_iter()
                    .filter(|(folder, metadata)| folder.to_string_lossy().eq_ignore_ascii_case(name) || metadata.name == name).collect();
                if matches.len() != 1 { return Err(Error::invalid("plugin must uniquely identify an installed name or directory")); }
                crate::plugin_catalog::uninstall(&matches[0].0)?; Ok(json!(true))
            }
            "DownloadPlugin" => {
                let metadata: crate::plugin_catalog::PluginMetadata = serde_json::from_value(protocol::argument(params, "metadata")?.clone())
                    .map_err(|error| Error::invalid(error.to_string()))?;
                if !metadata.supports_driver() { return Err(Error::invalid("plugin does not support pinned OpenTabletDriver 0.6.7")); }
                crate::plugin_catalog::install(&metadata)?; Ok(json!(true))
            }
            "RequestDeviceString" => {
                let values = match params {
                    Value::Array(values) if values.len() == 3 => [values[0].as_i64(),values[1].as_i64(),values[2].as_i64()],
                    Value::Object(values) if values.len() == 3 => [values.get("vendorID").or_else(|| values.get("vid")).and_then(Value::as_i64),values.get("productID").or_else(|| values.get("pid")).and_then(Value::as_i64),values.get("index").and_then(Value::as_i64)],
                    _ => return Err(Error::invalid("expected vendorID, productID, index")),
                };
                let vendor = values[0].and_then(|value| u16::try_from(value).ok()).ok_or_else(|| Error::invalid("vendorID must be 0..65535"))?;
                let product = values[1].and_then(|value| u16::try_from(value).ok()).ok_or_else(|| Error::invalid("productID must be 0..65535"))?;
                let index = values[2].and_then(|value| u8::try_from(value).ok()).ok_or_else(|| Error::invalid("index must be 0..255"))?;
                let devices = crate::hid::read_strings(vendor, product, &[index]).map_err(|error| Error::failed(error.to_string()))?;
                let (_, strings) = devices.into_iter().next().ok_or_else(|| Error::failed("device not found"))?;
                let (_, value) = strings.into_iter().next().ok_or_else(|| Error::failed("device string unavailable"))?;
                Ok(json!(value?))
            }
            "GetApplicationInfo" => {
                protocol::no_arguments(params)?;
                let data = otd_core::storage::data_directory()?;
                Ok(json!({"AppDataDirectory":data,"SettingsFile":data.join("driver.toml"),"PluginDirectory":crate::plugin_catalog::plugins_directory()?,
                    "PresetDirectory":otd_core::presets::PresetStore::user()?.directory(),"LogDirectory":data,
                    "TemporaryDirectory":std::env::temp_dir(),"CacheDirectory":null,"BackupDirectory":null,"TrashDirectory":null,
                    "ConfigurationDirectory":otd_core::config::configurations_directory()}))
            }
            "ForceResynchronize" => {
                protocol::no_arguments(params)?;
                self.shared.resynchronize.fetch_add(1, Ordering::AcqRel); Ok(Value::Null)
            }
            "DetectTablets" => { protocol::no_arguments(params)?; self.tablet_reply(Command::DetectDeviceSessions) },
            "LoadPlugins" => { protocol::no_arguments(params)?; Err(Error::unsupported(method, "dynamic plugin-manager reload is not implemented; installing a DLL does not imply loading it")) },
            "ResetSettings" => {
                protocol::no_arguments(params)?;
                let sessions = match self.call(Command::ListDeviceSessions)? {
                    Reply::DeviceSessions { sessions, .. } => sessions,
                    _ => return Err(Error::failed("unexpected device session snapshot")),
                };
                let tablets: Vec<_> = sessions.into_iter().filter(|session| session.connected).map(|session| session.properties).collect();
                self.set_settings(&super::settings::defaults(&tablets,crate::display::read_snapshot()?.virtual_screen)?)
            },
            "SetTabletDebug" => {
                if !aliased_argument(params, "isEnabled", "enabled")?.is_boolean() { return Err(Error::invalid("isEnabled must be bool")); }
                Err(Error::unsupported(method, "full-rate multi-tablet DeviceReport events are not implemented; use native bounded capture explicitly"))
            }
            "GetDevices" => {
                protocol::no_arguments(params)?;
                Ok(json!(crate::hid::enumerate_rpc_devices().map_err(|error| Error::failed(error.to_string()))?))
            }
            "GetDiagnosticInfo" => {
                protocol::no_arguments(params)?;
                let status = self.status()?;
                let devices = crate::hid::enumerate_rpc_devices().map_err(|error| Error::failed(error.to_string()))?;
                Ok(json!({"App Version":format!("OpenTabletDriver Rust v{}",env!("CARGO_PKG_VERSION")),
                    "Build Date":null,"Operating System":operating_system()?,
                    "Environment Variables":std::env::vars().collect::<std::collections::BTreeMap<_,_>>(),
                    "HID Devices":devices,"Console Log":status.logs.iter().map(|line| native_log(line)).collect::<Vec<_>>(),
                    "Rust Native State":{"instance":status.instance,"generation":status.generation,"state":status.state,"profile":status.profile}}))
            }
            "CheckForUpdates" => {
                protocol::no_arguments(params)?;
                let release = crate::update::latest()?;
                let available = release.version > crate::update::current_version();
                let result = if available { json!({"Version":format!("{}.{}.{}",release.version.0,release.version.1,release.version.2)}) } else { Value::Null };
                *self.shared.update.lock().map_err(|_| Error::failed("update state lock poisoned"))? = available.then_some(release);
                Ok(result)
            },
            "InstallUpdate" => { protocol::no_arguments(params)?; self.install_update() },
            _ => Err(Error { code: -32601, message: format!("Method not found: {method}") }),
        }
    }
}
