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
    debug: Option<super::debug::Capture>,
    debug_sessions: Vec<crate::device_sessions::SessionSnapshot>,
    settings_revision: Option<u64>,
    settings_native: Option<(control::WorkerIdentity, Vec<(String,u64,Option<u64>)>)>,
}
struct UpdateOwnership { token: String, exit: bool }
impl Connection {
    pub fn new(shared: Arc<Shared>, stop: Arc<AtomicBool>) -> Self {
        let resync_cursor = shared.resynchronize.load(Ordering::Acquire);
        let mut connection = Self { shared, stop, next_poll: Instant::now(), log_cursor: None,
            tablet_cursor: None, resync_cursor, update:None, debug:None, debug_sessions:Vec::new(), settings_revision:None, settings_native:None };
        // Establish the retained-history boundary before reading a request.
        // A complete/coalesced first frame may never enter an IO wait callback.
        let _ = connection.establish_log_cursor();
        connection
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
    fn logs(&self) -> Result<(String, u64, Vec<UpstreamLogMessage>), Error> {
        match self.call(Command::GetUpstreamLog)? {
            Reply::UpstreamLog { instance, sequence, messages } => Ok((instance, sequence, messages)),
            _ => Err(Error::failed("unexpected retained log response")),
        }
    }
    fn establish_log_cursor(&mut self) -> Result<(),Error> {
        if self.log_cursor.is_none() {
            let (instance,sequence,_) = self.logs()?;
            self.log_cursor = Some((instance,sequence));
        }
        Ok(())
    }
    fn finish_update(&mut self) -> Result<bool, Error> {
        let Some(update) = &self.update else { return Ok(false); };
        // Ownership cleanup must run even after this connection/daemon's local
        // listener cancellation was requested. PID/token guards prevent it
        // affecting a replacement daemon or another updater.
        let exit = update.exit;
        let deadline=Instant::now()+Duration::from_secs(40);
        loop {
            let response = control::request_owned(&Request::new(1,Command::FinishUpdate {
                token:update.token.clone(),success:exit }),Duration::from_secs(5),std::process::id())
                .map_err(|error| Error::failed(format!("update ownership cleanup: {error}")))?;
            match response.reply {
                Reply::ShutdownAccepted if exit => break,
                Reply::UpdateCancelled if !exit => break,
                Reply::Error { error } if !exit && matches!(error.code,control::ErrorCode::Busy) && Instant::now()<deadline=>
                    std::thread::sleep(Duration::from_millis(25)),
                Reply::Error { error } => return Err(Error::failed(error.message)),
                _ => return Err(Error::failed("unexpected update cleanup reply")),
            }
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
            match crate::update::install_with_cancel(&release,folder,&|_| {}, Some(&self.stop)) {
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
                let values: Vec<_> = sessions.iter().filter(|session| session.connected).filter_map(|session| {
                    // Original TabletReference describes an open InputDeviceTree,
                    // not every candidate discovered by the native registry.
                    let identifiers = session.opened_identifiers.as_ref()?;
                    Some(json!({"Properties":session.properties,"Identifiers":identifiers}))
                }).collect();
                Ok(json!(values))
            }
            _ => Err(Error::failed("unexpected device session snapshot")),
        }
    }
    pub(super) fn settings(&mut self) -> Result<Value, Error> {
        let _owner = super::collection::owner()?;
        self.settings_owned()
    }
    fn settings_owned(&mut self) -> Result<Value, Error> {
        let retained = super::collection::snapshot()?;
        let expected = self.status()?.identity();
        let text = match self.call(Command::GetConfiguration { expected: expected.clone() })? {
            Reply::Configuration { identity, profile_toml: Some(text) } if identity == expected => text,
            Reply::Configuration { identity, profile_toml: None } if identity == expected => {
                let sessions = self.session_snapshots()?;
                if self.status()?.identity() != expected { return Err(Error::failed("daemon changed while reading idle settings")); }
                self.settings_revision = Some(retained.revision);
                self.settings_native = Some((expected,device_generations(&sessions)));
                return Ok(retained.document);
            }
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
        if sessions.is_empty() && retained.authoritative {
            if self.status()?.identity() != expected { return Err(Error::failed("daemon changed while reading idle settings")); }
            self.settings_revision = Some(retained.revision);
            self.settings_native = Some((expected,device_generations(&sessions)));
            return Ok(retained.document);
        }
        let screen = crate::display::read_snapshot()?.virtual_screen;
        let primary_stale = retained.authoritative && sessions.iter().any(|session|
            session.primary && !session.connected && session.use_settings_collection);
        let profile = if !primary_stale && profile.imported_otd.is_none() {
            let tablet = sessions.iter().find(|session| session.primary && session.profile_source.is_some())
                .ok_or_else(|| Error::failed("native settings export needs a known primary tablet configuration"))?;
            super::settings::canonical_copy(&profile, &tablet.properties, screen)?
        } else { profile };
        let exported: Value = if primary_stale { retained.document.clone() } else {
            serde_json::from_str(&export_settings(&profile)?).map_err(|error| Error::failed(error.to_string()))?
        };
        let mut document = if retained.authoritative || retained.document["Profiles"].as_array().is_some_and(|rows| !rows.is_empty()) {
            retained.document
        } else { exported.clone() };
        document["Tools"] = exported["Tools"].clone();
        let mut active = std::collections::BTreeMap::<String,Value>::new();
        // Registry-owned profiles survive disconnect. Export them too, using
        // frozen configuration metadata without claiming an open tablet.
        for session in sessions.iter().filter(|session| session.profile_source.is_some()
            && (session.connected || !session.use_settings_collection)) {
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
        let after = self.session_snapshots()?;
        if device_generations(&after) != device_generations(&sessions) {
            return Err(Error::failed("device settings changed while reading the collection; retry"));
        }
        self.settings_revision = Some(super::collection::publish(document.clone(), retained.revision, false)?);
        self.settings_native = Some((expected,device_generations(&after)));
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
    pub(super) fn set_settings(&mut self,settings:&Value) -> Result<Value,Error> {
        let _owner = super::collection::owner()?;
        self.set_settings_owned(settings,None,None,None)
    }
    pub(super) fn set_settings_expected(&mut self,settings:&Value,expected:control::WorkerIdentity,
        source:Option<(String,u32)>,expected_source:Option<(String,u64)>) -> Result<Value,Error> {
        let _owner = super::collection::owner()?;
        self.set_settings_owned(settings,Some(expected),source,expected_source)
    }
    fn set_settings_owned(&mut self,settings:&Value,expected:Option<control::WorkerIdentity>,
        source:Option<(String,u32)>,expected_source:Option<(String,u64)>) -> Result<Value,Error> {
        let retained = super::collection::check_revision(self.settings_revision)?;
        let settings = super::collection::normalize(settings)?;
        let status = self.status()?;
        if expected.is_some_and(|expected| expected != status.identity()) {
            return Err(Error::failed("driver generation changed before original settings apply"));
        }
        let sessions = self.session_snapshots()?;
        if self.settings_native.as_ref().is_some_and(|(identity,generations)|
            *identity != status.identity() || *generations != device_generations(&sessions)) {
            return Err(Error::failed("native settings changed since GetSettings; reload before applying the collection"));
        }
        let sessions:Vec<_> = sessions.into_iter().filter(|session| session.connected).collect();
        if let Some((id,generation)) = &expected_source {
            if !sessions.iter().any(|session| &session.id == id && session.device_generation == *generation
                && session.pending_generation.is_none() && session.state == crate::device_sessions::SessionState::Running) {
                return Err(Error::failed("source settings request no longer owns its admitted physical device generation"));
            }
        }
        let source = if let Some((tablet,owner)) = source {
            let mut matches = sessions.iter().filter(|session| session.tablet == tablet
                && session.state == crate::device_sessions::SessionState::Running);
            let session = matches.next().ok_or_else(|| Error::failed("source preset tablet is no longer running"))?;
            if matches.next().is_some() { return Err(Error::failed("source preset tablet name is ambiguous across active physical sessions")); }
            if expected_source.as_ref().is_some_and(|(id,generation)| id != &session.id || *generation != session.device_generation) {
                return Err(Error::failed("source preset name resolved to another physical device generation"));
            }
            Some((session.id.clone(),owner))
        } else { None };
        if sessions.is_empty() {
            if self.status()?.identity() != status.identity() || self.session_snapshots()?.iter().any(|session| session.connected) {
                return Err(Error::failed("device lifecycle changed during idle settings apply; refresh and retry"));
            }
            let settings_json = serde_json::to_string(&settings).map_err(|error| Error::invalid(error.to_string()))?;
            let revision = match self.call(Command::SetIdleOriginalSettings { expected:status.identity(),
                expected_revision:retained.revision,settings_json })? {
                Reply::OriginalSettingsCommitted { revision } => revision,
                _ => return Err(Error::failed("unexpected idle collection publication receipt")),
            };
            self.settings_revision = Some(revision);
            self.settings_native = None;
            crate::tool_host::apply_document(&settings).map_err(Error::failed)?;
            return Ok(Value::Null);
        }
        if sessions.iter().any(|session| session.pending_generation.is_some()
            || !matches!(session.state,crate::device_sessions::SessionState::Running | crate::device_sessions::SessionState::Stopped)) {
            return Err(Error::failed("device sessions are transitioning/unavailable; refresh and retry without replacing pending operations"));
        }
        let tablets:Vec<_> = sessions.iter().map(|session| session.properties.clone()).collect();
        let displays = crate::display::read_snapshot()?;
        let settings = super::settings::for_detected(&settings,&tablets,displays.virtual_screen)?;
        let profiles = settings["Profiles"].as_array().ok_or_else(|| Error::invalid("settings must contain Profiles"))?;
        let text = serde_json::to_string(&settings).map_err(|error| Error::invalid(error.to_string()))?;
        if text.len() > control::MAX_PROFILE_BYTES { return Err(Error::invalid("settings exceed 128 KiB")); }
        let path = otd_core::storage::data_directory()?.join("upstream-rpc-settings.json");
        let registry = crate::dotnet::registry_snapshot();
        let mut plans = Vec::new();
        // Validate every applicable profile and capture every guarded prior
        // configuration before accepting the first mutation.
        for session in &sessions {
            let index = profiles.iter().position(|profile| profile["Tablet"] == session.tablet)
                .ok_or_else(|| Error::invalid("detected tablet profile is missing"))?;
            let mut profile = import_profile(&text,&path,index,registry.as_deref())?;
            if let Some(registry) = &registry {
                crate::plugins::resolve_imported_stores(&mut profile,&registry.plugins)?;
            }
            if let Some(diagnostic) = profile.diagnostics.iter().find(|diagnostic| diagnostic.kind == "unsupported_active") {
                return Err(Error::unsupported("SetSettings",&diagnostic.message));
            }
            let profile = profile.for_tablet(otd_core::spec::TabletSpec::from_configuration(&session.properties)?)?;
            crate::plugins::validate_runtime_profile(&profile)?;
            if profile.relative.is_none() { displays.mapper(&profile)?; }
            let replacement = profile.to_toml()?;
            let encoded = serde_json::to_vec(&replacement).map_err(|error| Error::invalid(error.to_string()))?;
            if replacement.len() > control::MAX_PROFILE_BYTES || encoded.len() > control::MAX_FRAME_BYTES - 1024 {
                return Err(Error::invalid("imported profile exceeds native control limits"));
            }
            let before = self.device_profile(&session.id,session.device_generation,&status.instance)?;
            let previous = crate::config::Profile::from_toml_text(&before,&path)?;
            if !previous.preserved_fields.is_empty() || previous.plugins.iter().any(|plugin|
                plugin.kind == otd_core::plugins::PluginKind::Native) {
                return Err(Error::unsupported("SetSettings", "native extensions/ABI plugin settings cannot be replaced through an original-format collection; edit the native profile explicitly"));
            }
            plans.push(super::apply::Plan { id:session.id.clone(),generation:session.device_generation,before,replacement,
                before_collection:session.use_settings_collection });
        }
        if self.status()?.identity() != status.identity() { return Err(Error::failed("daemon configuration changed during settings preflight")); }
        let mut backend = DeviceApply { connection:self,instance:status.instance,source };
        let result = super::apply::run(&plans,&mut backend,Instant::now()+Duration::from_secs(40),
            || Instant::now()+Duration::from_secs(40));
        if let Err(error) = result {
            self.shared.resynchronize.fetch_add(1,Ordering::AcqRel);
            return Err(error);
        }
        self.settings_revision = Some(super::collection::publish(settings.clone(), retained.revision, true)?);
        crate::tool_host::apply_document(&settings).map_err(Error::failed)?;
        self.settings_native = None;
        self.shared.resynchronize.fetch_add(1,Ordering::AcqRel);
        Ok(Value::Null)
    }
    pub(super) fn load_settings(&mut self) -> Result<Value, Error> {
        let _owner = super::collection::owner()?;
        let (document, file) = super::collection::read_saved()?;
        self.set_settings_owned(&document,None,None,None)?;
        super::collection::accept_loaded_file(file)?;
        Ok(Value::Null)
    }
    pub(super) fn save_settings(&mut self) -> Result<Value, Error> {
        let _owner = super::collection::owner()?;
        super::collection::check_revision(self.settings_revision)?;
        let document = self.settings_owned()?;
        super::collection::save(&document,self.settings_revision.unwrap())?;
        Ok(Value::Null)
    }
    pub fn events(&mut self) -> Vec<Value> {
        if self.stop.load(Ordering::Acquire) { return Vec::new(); }
        let mut events = Vec::new();
        if let Some(capture) = &mut self.debug {
            match capture.poll(Some(&self.debug_sessions)) {
                Ok((reports, diagnostics)) => {
                    events.extend(reports);
                    events.extend(diagnostics.iter().map(|message| protocol::event("Message", native_log(message))));
                }
                Err(error) => {
                    self.debug = None;
                    events.push(protocol::event("Message", native_log(&format!("Compatibility report capture stopped: {error}"))));
                }
            }
        }
        if Instant::now() < self.next_poll { return events; }
        self.next_poll = Instant::now() + Duration::from_millis(250);
        if self.debug.is_some() {
            if let Ok(sessions) = self.session_snapshots() { self.debug_sessions = sessions; }
        }
        if let Ok((instance, sequence, messages)) = self.logs() {
            events.extend(message_events(&mut self.log_cursor,instance,sequence,&messages));
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
struct DeviceApply<'a> { connection:&'a mut Connection,instance:String,source:Option<(String,u32)> }
impl DeviceApply<'_> {
    fn apply_owned(&mut self,id:&str,generation:u64,text:&str,collection:bool,
        binding_inhibit:Option<u32>) -> Result<crate::device_sessions::SessionReceipt,Error> {
        let status = self.connection.status()?;
        if status.instance != self.instance { return Err(Error::failed("daemon identity changed during device apply")); }
        let command = if collection {
            Command::ApplyOriginalDeviceProfile { expected:status.identity(),id:id.into(),
                device_generation:generation,profile_toml:text.into(),binding_inhibit }
        } else { Command::ApplyDeviceProfile { expected:status.identity(),id:id.into(),
            device_generation:generation,profile_toml:text.into() } };
        match self.connection.call(command)? {
            Reply::DeviceOperationAccepted { receipt } => Ok(receipt),
            _ => Err(Error::failed("unexpected device operation receipt")),
        }
    }
}
impl super::apply::Backend for DeviceApply<'_> {
    fn apply(&mut self,id:&str,generation:u64,text:&str) -> Result<crate::device_sessions::SessionReceipt,Error> {
        let inhibition = if self.source.as_ref().is_some_and(|(source,_)| source == id) {
            Some(self.source.take().unwrap().1)
        } else { None };
        self.apply_owned(id,generation,text,true,inhibition)
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
        let restored = self.apply_owned(&plan.id,receipt.target_generation,&plan.before,plan.before_collection,None)?;
        self.wait(&restored,&plan.before,deadline)
    }
}
/// Import a verified installed output only when ordinary import cannot represent it.
fn import_profile(text:&str,path:&Path,index:usize,registry:Option<&crate::dotnet::ManagedRegistryInfo>) -> Result<crate::config::Profile,Error> {
    match crate::config::Profile::from_otd_profile_text(text,path,index,Default::default()) {
        Ok(profile) => Ok(profile),
        Err(original) => {
            // Pure cached registry lookup never starts CLR for ordinary/native
            // profiles. An unknown output is accepted only by its actual loaded
            // unchanged IOutputMode identity; malformed native stores still fail.
            let Some(registry) = registry else { return Err(original.into()); };
            let settings:Value = serde_json::from_str(text).map_err(|error| Error::invalid(error.to_string()))?;
            let name = settings["Profiles"][index]["OutputMode"]["Path"].as_str();
            let mut matches = registry.plugins.iter().filter(|entry| entry.metadata.category == "output"
                && entry.metadata.supported && Some(entry.config.type_name.as_str()) == name);
            let Some(entry) = matches.next() else { return Err(original.into()); };
            if matches.any(|other| other.config.path != entry.config.path) {
                return Err(Error::failed("managed output class occurs in multiple installed DLLs; select its identity explicitly"));
            }
            Ok(crate::config::Profile::from_managed_output_store(text,path,index,Default::default(),
                entry.config.clone(),entry.metadata.relative_output)?)
        }
    }
}
/// Reconcile an existing one-to-one store in the same group/order. Extra DLLs,
/// native ABI entries, mixed native/managed Radial Follow or ambiguous stores
/// fail rather than returning stale source filters as if they were active.
fn export_settings(profile: &crate::config::Profile) -> Result<String, Error> {
    Ok(profile.to_otd_json()?)
}
fn device_generations(sessions:&[crate::device_sessions::SessionSnapshot]) -> Vec<(String,u64,Option<u64>)> {
    sessions.iter().map(|session| (session.id.clone(),session.device_generation,session.pending_generation)).collect()
}
fn message_events(cursor:&mut Option<(String,u64)>,instance:String,sequence:u64,messages:&[UpstreamLogMessage]) -> Vec<Value> {
    let mut events = Vec::new();
    if let Some((previous_instance,previous_sequence)) = cursor.as_ref() {
        if previous_instance == &instance {
            let available_start = sequence.saturating_sub(messages.len() as u64);
            let start = previous_sequence.saturating_sub(available_start).min(messages.len() as u64) as usize;
            events.extend(messages[start..].iter().map(|message| protocol::event("Message",json!(message))));
        }
    }
    *cursor = Some((instance,sequence));
    events
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn first_coalesced_write_and_lagging_retained_tail_preserve_original_typed_messages() {
        let before = UpstreamLogMessage::native("preconnection history".into());
        let written:UpstreamLogMessage = serde_json::from_value(json!({"Time":"2026-10-07T09:08:07.654+02:00",
            "Group":null,"Message":"first immediate request","StackTrace":"original trace","Level":3,"Notification":true})).unwrap();
        // Cursor comes from the read-only construction snapshot, not the first
        // post-request IO-wait callback. No wait callback is needed to emit it.
        let mut cursor = Some(("owned daemon".into(),8));
        let events = message_events(&mut cursor,"owned daemon".into(),9,&[before,written.clone()]);
        assert_eq!(events,vec![protocol::event("Message",json!(written))]);
        assert!(message_events(&mut cursor,"owned daemon".into(),9,&[written.clone()]).is_empty());
        let events = message_events(&mut cursor,"owned daemon".into(),80,&[written.clone()]);
        assert_eq!(events,vec![protocol::event("Message",json!(written))]);
        assert!(message_events(&mut cursor,"replacement daemon".into(),1,&[written]).is_empty());
    }
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
    json!(UpstreamLogMessage::native(line.into()))
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
        // If the constructor raced initial native-pipe startup, establish the
        // baseline before any first method can append logs or mutate state.
        if METHODS.contains(&method) { self.establish_log_cursor()?; }
        match method {
            "GetPluginTypes" => {
                protocol::no_arguments(params)?;
                crate::plugins::load_parser_registry()?;
                Ok(crate::dotnet::get_plugin_types()?)
            },
            "ConstructPluginStore" => {
                let (path,category)=match params {
                    Value::Array(values) if values.len()==2=>(values[0].as_str(),values[1].as_str()),
                    Value::Object(values) if values.len()==2=>(values.get("path").and_then(Value::as_str),values.get("category").and_then(Value::as_str)),
                    _=>return Err(Error::invalid("expected plugin path and category")),
                };
                let path=path.ok_or_else(||Error::invalid("plugin path must be a string"))?;
                let category=category.ok_or_else(||Error::invalid("plugin category must be a string"))?;
                crate::plugins::load_parser_registry()?;
                Ok(crate::dotnet::construct_plugin_store(path,category)?)
            },
            "GetTablets" => { protocol::no_arguments(params)?; self.tablets() },
            "GetSettings" => { protocol::no_arguments(params)?; self.settings() },
            "SetSettings" => self.set_settings(protocol::argument(params, "settings")?),
            // Explicit Rust extensions; the pinned 19-method interface itself
            // leaves serialization to the caller's Settings.Serialize method.
            "LoadSettings" => { protocol::no_arguments(params)?; self.load_settings() },
            "SaveSettings" => { protocol::no_arguments(params)?; self.save_settings() },
            "GetCurrentLog" => {
                protocol::no_arguments(params)?;
                Ok(json!(self.logs()?.2))
            }
            "WriteMessage" => {
                let message: UpstreamLogMessage = serde_json::from_value(protocol::argument(params, "message")?.clone())
                    .map_err(|error| Error::invalid(error.to_string()))?;
                Request::new(1,Command::WriteMessage { message:message.clone() }).validate()
                    .map_err(|error| Error::invalid(error.message))?;
                match self.call(Command::WriteMessage { message })? {
                    Reply::MessageWritten => Ok(Value::Null), _ => Err(Error::failed("unexpected log-write response")),
                }
            }
            "InstallPlugin" => {
                crate::plugin_catalog::install_file_with_cancel(Path::new(text_argument(params, "filePath")?), Some(&self.stop))?;
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
                crate::plugin_catalog::install_with_cancel(&metadata, Some(&self.stop))?; Ok(json!(true))
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
                Ok(super::original_application_info()?)
            }
            "ForceResynchronize" => {
                protocol::no_arguments(params)?;
                self.shared.resynchronize.fetch_add(1, Ordering::AcqRel); Ok(Value::Null)
            }
            "DetectTablets" => { protocol::no_arguments(params)?; self.tablet_reply(Command::DetectDeviceSessions) },
            "LoadPlugins" => {
                protocol::no_arguments(params)?;
                crate::download::cancelled(Some(&self.stop))?;
                let directory = crate::plugin_catalog::plugins_directory()?;
                std::fs::create_dir_all(&directory).map_err(|error| Error::failed(error.to_string()))?;
                let registry = crate::dotnet::reload_installed_plugins(&directory)?;
                crate::download::cancelled(Some(&self.stop))?;
                self.shared.resynchronize.fetch_add(1, Ordering::AcqRel);
                // Original contract is Task, not a fabricated inventory result.
                // Loading is real; unchanged constructors execute only when a
                // guarded device profile actually starts through its owner.
                let _ = registry;
                Ok(Value::Null)
            },
            "ResetSettings" => {
                protocol::no_arguments(params)?;
                self.set_settings(&Value::Null)
            },
            "SetTabletDebug" => {
                let enabled = aliased_argument(params, "isEnabled", "enabled")?.as_bool()
                    .ok_or_else(|| Error::invalid("isEnabled must be bool"))?;
                if enabled && self.debug.is_none() {
                    let sessions = self.session_snapshots()?;
                    let capture = super::debug::Capture::new(&sessions)?;
                    self.debug_sessions = sessions;
                    self.debug = Some(capture);
                } else if !enabled { self.debug = None; self.debug_sessions.clear(); }
                Ok(Value::Null)
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
                    "Build Date":env!("OTD_BUILD_DATE"),"Operating System":operating_system()?,
                    "Environment Variables":std::env::vars().collect::<std::collections::BTreeMap<_,_>>(),
                    "HID Devices":devices,"Console Log":self.logs()?.2,
                    "Rust Native State":{"instance":status.instance,"generation":status.generation,"state":status.state,"profile":status.profile}}))
            }
            "CheckForUpdates" => {
                protocol::no_arguments(params)?;
                let release = crate::update::latest_with_cancel(Some(&self.stop))?;
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
