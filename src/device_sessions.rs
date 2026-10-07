//! Control-plane identity and profiles for independently owned Windows tablets.
//! Physical keys never leave this module. No registry operation runs per report.
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, mpsc};

use serde::{Deserialize, Serialize};
use crate::config::Profile;
use crate::hid::{self, Candidate, Event, SelectedDevice};
use otd_core::tablets::{Database, DeviceIdentifier, TabletConfiguration};
mod profile_store;
use profile_store::ProfileFile;

pub const MAX_SESSIONS: usize = 32;
/// Capture only the exact file the daemon observed. A new snapshot must never
/// silently bless same-revision edits made by another program.
pub fn observed_profile_snapshot(device: &SessionSnapshot) -> Result<otd_core::storage::FileSnapshot, String> {
    let path = std::path::Path::new(&device.profile_path);
    if let Some(expected) = &device.persisted_digest {
        let loaded = otd_core::storage::read_utf8(path)?;
        if profile_store::sha256(loaded.text.as_bytes())? != *expected {
            return Err("The saved tablet profile changed outside this panel; reload it or use Save As.".into());
        }
        Ok(loaded.snapshot)
    } else {
        let snapshot = otd_core::storage::capture(path)?;
        if snapshot.exists() { return Err("A saved tablet profile appeared outside this panel; reload it or use Save As.".into()); }
        Ok(snapshot)
    }
}
fn runtime_edit_pending(previous: bool, explicitly_applied: bool, persisted_match: bool) -> bool {
    !persisted_match && (previous || explicitly_applied)
}

pub type SessionId = String;
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionState { Detected, Preparing, Starting, Running, Waiting, Stopping, Stopped, Failed }
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentityStability { PhysicalParent, PathFallback }
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionSnapshot {
    pub id: SessionId,
    pub device_generation: u64,
    pub pending_generation: Option<u64>,
    pub tablet: String,
    pub state: SessionState,
    pub primary: bool,
    pub selected: bool,
    pub connected: bool,
    pub identity_stability: IdentityStability,
    pub properties: TabletConfiguration,
    pub digitizer: DeviceIdentifier,
    pub auxiliary: Option<DeviceIdentifier>,
    /// Discovery metadata above can outlive open handles. Only this list
    /// describes endpoints owned by a current activated session.
    #[serde(default)]
    pub opened_identifiers: Option<Vec<DeviceIdentifier>>,
    pub profile_source: Option<String>,
    pub profile_path: String,
    pub persisted_revision: Option<u64>,
    pub persisted_digest: Option<String>,
    /// The active profile equals its observed file, including revision. A UI
    /// verifies the digest before treating a fresh disk capture as clean.
    pub profile_saved: bool,
    /// Initial defaults/imports are intentionally clean in the panel. Only a
    /// committed explicit Apply that differs from disk sets this flag.
    #[serde(default)]
    pub has_unsaved_runtime_edits: bool,
    pub last_error: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionList { pub sessions: Vec<SessionSnapshot>, pub selected_id: Option<SessionId> }
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionReceipt {
    pub id: SessionId,
    pub device_generation: u64,
    pub target_generation: u64,
    pub accepted_pending: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SavedDeviceProfile {
    pub id: SessionId,
    pub device_generation: u64,
    pub profile_path: String,
    pub settings_revision: u64,
    pub persisted_digest: String,
    pub profile_toml: String,
}
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct DeviceKey { tablet: String, parent: String, fallback: bool }
impl DeviceKey {
    pub(crate) fn of(selected: &SelectedDevice<'_>) -> Self {
        let parent = &selected.pen.endpoint.physical_id;
        Self { tablet: selected.configuration.name.clone(),
            parent: if parent.is_empty() { selected.pen.path_text().to_uppercase() } else { parent.to_uppercase() },
            fallback: parent.is_empty() }
    }
}
pub(crate) struct Entry {
    pub snapshot: SessionSnapshot,
    key: DeviceKey,
    pub path: String,
    pub profile: Option<Profile>,
    pub enabled: bool,
    file: ProfileFile,
    pending_edit_generation: Option<u64>,
    opened_epoch: u64,
}
#[derive(Default)]
pub(crate) struct Registry {
    pub entries: BTreeMap<SessionId, Entry>,
    pub primary: Option<SessionId>,
    pub selected: Option<SessionId>,
    next: u64,
    next_opened_epoch: u64,
    pub shutting_down: bool,
}
pub(crate) enum Request {
    Refresh(mpsc::SyncSender<Result<SessionList, String>>),
    Apply { id: SessionId, profile: Profile, generation: u64 },
    Stop { id: SessionId, generation: u64 },
    Start { id: SessionId, generation: u64 },
}
#[derive(Clone)]
pub struct Handle {
    pub(crate) registry: Arc<Mutex<Registry>>,
    requests: mpsc::SyncSender<Request>,
    wake: Arc<Mutex<Event>>,
}
impl Handle {
    pub(crate) fn channel(wake: Event) -> (Self, mpsc::Receiver<Request>) {
        let (requests, receiver) = mpsc::sync_channel(16);
        (Self { registry: Arc::default(), requests, wake: Arc::new(Mutex::new(wake)) }, receiver)
    }
    pub fn snapshot(&self) -> Result<SessionList, String> {
        let registry = self.registry.lock().map_err(|_| "device registry poisoned")?;
        Ok(SessionList { sessions: registry.entries.values().map(|entry| entry.snapshot.clone()).collect(),
            selected_id: registry.selected.clone() })
    }
    /// Explicit discovery completes on the supervisor, independently of live
    /// readers. New devices can still be preparing when metadata is returned.
    pub fn refresh(&self) -> Result<SessionList, String> {
        {
            let registry = self.registry.lock().map_err(|_| "device registry poisoned")?;
            if registry.shutting_down { return Err("device supervisor is stopping".into()); }
        }
        let (reply, response) = mpsc::sync_channel(1);
        self.requests.try_send(Request::Refresh(reply)).map_err(|error| format!("device discovery was not accepted: {error}"))?;
        if let Ok(wake) = self.wake.lock() { let _ = wake.signal(); }
        response.recv_timeout(std::time::Duration::from_secs(2))
            .map_err(|error| format!("device discovery did not complete within its control deadline: {error}"))?
    }
    pub fn select(&self, id: &str) -> Result<(), String> {
        let mut registry = self.registry.lock().map_err(|_| "device registry poisoned")?;
        if registry.shutting_down { return Err("device supervisor is stopping".into()); }
        let entry = registry.entries.get(id).ok_or("unknown device session")?;
        if entry.snapshot.state != SessionState::Running { return Err("device session is not running".into()); }
        otd_core::debug::select_device_key(id).map_err(|error| format!("debugger selection failed: {error:?}"))?;
        registry.selected = Some(id.to_owned());
        for entry in registry.entries.values_mut() { entry.snapshot.selected = entry.snapshot.id == id; }
        Ok(())
    }
    pub fn profile(&self, id: &str, generation: u64) -> Result<String, String> {
        let registry = self.registry.lock().map_err(|_| "device registry poisoned")?;
        let entry = checked(&registry, id, generation)?;
        if let Some(profile) = entry.profile.as_ref().or(entry.file.profile.as_ref()) { return profile.to_toml(); }
        if let Some(error) = &entry.file.error { return Err(error.clone()); }
        Profile { target_tablet: Some(entry.snapshot.tablet.clone()),
            tablet: otd_core::spec::TabletSpec::from_configuration(&entry.snapshot.properties)?,
            source: format!("unsaved full-area defaults for {}", entry.snapshot.tablet), ..Profile::default() }.to_toml()
    }
    /// Explicit Save leaves the active profile unchanged until Apply.
    pub fn save_profile(&self, id: &str, generation: u64, expected_revision: Option<u64>, expected_digest: Option<&str>, profile: &Profile) -> Result<SavedDeviceProfile, String> {
        let mut registry = self.registry.lock().map_err(|_| "device registry poisoned")?;
        let entry = checked(&registry, id, generation)?;
        if entry.snapshot.pending_generation.is_some() { return Err("device session is transitioning".into()); }
        if profile.tablet_name()?.is_some_and(|name| name != entry.snapshot.tablet) { return Err("profile belongs to another tablet".into()); }
        let entry = registry.entries.get_mut(id).unwrap();
        let saved = entry.file.save(profile, expected_revision, expected_digest)?;
        entry.snapshot.persisted_revision = entry.file.revision();
        entry.snapshot.persisted_digest = entry.file.digest.clone();
        entry.snapshot.profile_saved = entry.profile.as_ref().is_some_and(|active| entry.file.matches(active));
        if entry.snapshot.profile_saved { entry.snapshot.has_unsaved_runtime_edits = false; }
        Ok(SavedDeviceProfile { id: id.to_owned(), device_generation: generation,
            profile_path: entry.file.path.to_string_lossy().into_owned(), settings_revision: saved.settings_revision,
            persisted_digest: entry.file.digest.clone().unwrap(), profile_toml: saved.to_toml()? })
    }
    pub fn apply(&self, id: &str, generation: u64, profile: Profile) -> Result<SessionReceipt, String> {
        crate::plugins::validate_runtime_profile(&profile)?;
        profile.validate_filter_execution()?;
        let text = profile.to_toml()?;
        if text.len() > crate::control::MAX_PROFILE_BYTES || serde_json::to_vec(&text).map_err(|error| error.to_string())?.len() > crate::control::MAX_FRAME_BYTES - 1024 {
            return Err("device profile exceeds control frame limits".into());
        }
        let mut registry = self.registry.lock().map_err(|_| "device registry poisoned")?;
        let entry = checked(&registry, id, generation)?;
        if profile.tablet_name()?.is_some_and(|name| name != entry.snapshot.tablet) { return Err("profile belongs to another tablet".into()); }
        // This explicit session ID owns the physical target. A legacy saved
        // collection selector can become stale across reconnects; it cannot
        // retarget the bound worker and does not invalidate a profile edit.
        self.submit(&mut registry, id, generation, |next| Request::Apply { id: id.to_owned(), profile, generation: next })
    }
    pub fn stop_device(&self, id: &str, generation: u64) -> Result<SessionReceipt, String> {
        let mut registry = self.registry.lock().map_err(|_| "device registry poisoned")?;
        if !checked(&registry, id, generation)?.enabled { return Err("device session is already stopped".into()); }
        self.submit(&mut registry, id, generation, |next| Request::Stop { id: id.to_owned(), generation: next })
    }
    pub fn start_device(&self, id: &str, generation: u64) -> Result<SessionReceipt, String> {
        let mut registry = self.registry.lock().map_err(|_| "device registry poisoned")?;
        if checked(&registry, id, generation)?.enabled { return Err("device session is already active".into()); }
        self.submit(&mut registry, id, generation, |next| Request::Start { id: id.to_owned(), generation: next })
    }
    fn submit(&self, registry: &mut Registry, id: &str, generation: u64, request: impl FnOnce(u64) -> Request) -> Result<SessionReceipt, String> {
        let entry = checked(registry, id, generation)?;
        if entry.snapshot.primary { return Err("primary device lifecycle requires the daemon transaction".into()); }
        if entry.snapshot.pending_generation.is_some() { return Err("device session is transitioning".into()); }
        let next = generation.checked_add(1).ok_or("device generation exhausted")?;
        let request = request(next);
        let explicit_edit = matches!(&request, Request::Apply { .. });
        self.requests.try_send(request).map_err(|error| format!("device command was not accepted: {error}"))?;
        let entry = registry.entries.get_mut(id).unwrap();
        entry.snapshot.pending_generation = Some(next);
        entry.pending_edit_generation = explicit_edit.then_some(next);
        // Acceptance is durable in the bounded command queue. A failed wake is
        // harmless: the supervisor polls it within 20ms and never duplicates it.
        if let Ok(wake) = self.wake.lock() { let _ = wake.signal(); }
        Ok(SessionReceipt { id: id.to_owned(), device_generation: generation, target_generation: next, accepted_pending: true })
    }
    pub(crate) fn discover(&self, selected: &SelectedDevice<'_>) -> Result<SessionId, String> {
        let mut registry = self.registry.lock().map_err(|_| "device registry poisoned")?;
        let key = DeviceKey::of(selected);
        if let Some(entry) = registry.entries.values_mut().find(|entry| entry.key == key) {
            entry.path = selected.pen.path_text();
            entry.snapshot.connected = true;
            if !matches!(entry.snapshot.state, SessionState::Running | SessionState::Starting | SessionState::Stopping) {
                entry.snapshot.properties = selected.configuration.clone();
                entry.snapshot.digitizer = selected.identifier.clone();
                entry.snapshot.auxiliary = selected.auxiliary.as_ref().map(|(_, identifier)| identifier.clone());
            }
            return Ok(entry.snapshot.id.clone());
        }
        if registry.entries.len() == MAX_SESSIONS { return Err("device identity registry is full (32 retained tablets)".into()); }
        registry.next = registry.next.checked_add(1).ok_or("device identity counter exhausted")?;
        let id = format!("device-{}", registry.next);
        let file = ProfileFile::for_device(&key)?;
        let profile_path = file.path.to_string_lossy().into_owned();
        let file_error = file.error.clone();
        registry.entries.insert(id.clone(), Entry { key: key.clone(), path: selected.pen.path_text(), profile: None, enabled: file_error.is_none(),
            snapshot: SessionSnapshot { id: id.clone(), device_generation: 0, pending_generation: None,
                tablet: selected.configuration.name.clone(), state: if file_error.is_some() { SessionState::Failed } else { SessionState::Detected }, primary: false, selected: false,
                connected: true, identity_stability: if key.fallback { IdentityStability::PathFallback } else { IdentityStability::PhysicalParent },
                properties: selected.configuration.clone(), digitizer: selected.identifier.clone(),
                auxiliary: selected.auxiliary.as_ref().map(|(_, identifier)| identifier.clone()), opened_identifiers: None, profile_source: None,
                profile_path, persisted_revision: file.revision(), persisted_digest: file.digest.clone(), profile_saved: false, has_unsaved_runtime_edits: false,
                last_error: file_error }, file, pending_edit_generation: None, opened_epoch: 0 });
        Ok(id)
    }
    pub(crate) fn primary_id(&self) -> Result<Option<SessionId>, String> {
        Ok(self.registry.lock().map_err(|_| "device registry poisoned")?.primary.clone())
    }
    pub(crate) fn reserve_primary(&self, selected: &SelectedDevice<'_>) -> Result<SessionId, String> {
        let id = self.discover(selected)?;
        let mut registry = self.registry.lock().map_err(|_| "device registry poisoned")?;
        if registry.primary.as_ref().is_some_and(|primary| *primary != id) { return Err("primary profile cannot switch physical tablets; use per-device controls".into()); }
        registry.primary = Some(id.clone());
        registry.entries.get_mut(&id).unwrap().snapshot.primary = true;
        if registry.selected.is_none() {
            // A previous frozen capture can still be owned while a new driver
            // generation starts. It must drain its old token independently;
            // capture ownership must not prevent new tablet processing.
            let _ = otd_core::debug::prefer_device_key(&id);
            registry.selected = Some(id.clone());
            registry.entries.get_mut(&id).unwrap().snapshot.selected = true;
        }
        Ok(id)
    }
    pub(crate) fn find<'a>(&self, id: &str, devices: &'a [Candidate], database: &Database) -> Result<Option<SelectedDevice<'a>>, String> {
        let key = self.registry.lock().map_err(|_| "device registry poisoned")?.entries.get(id).ok_or("unknown device session")?.key.clone();
        for device in devices {
            if let Ok(Some(selected)) = hid::select_device(devices, database, Some(&device.path_text()), Some(&key.tablet)) {
                if DeviceKey::of(&selected) == key { return Ok(Some(selected)); }
            }
        }
        Ok(None)
    }
    pub(crate) fn validate_profile(&self, id: &str, profile: &Profile) -> Result<(), String> {
        let registry = self.registry.lock().map_err(|_| "device registry poisoned")?;
        let entry = registry.entries.get(id).ok_or("unknown device session")?;
        if profile.tablet_name()?.is_some_and(|name| name != entry.snapshot.tablet) { return Err("profile belongs to another physical tablet; use per-device controls".into()); }
        // Once bound, the private physical key governs reconnects. A saved
        // collection path may change; it must not retarget this worker.
        Ok(())
    }
    pub(crate) fn state(&self, id: &str, state: SessionState, error: Option<String>) {
        if let Ok(mut registry) = self.registry.lock() {
            if let Some(entry) = registry.entries.get_mut(id) { entry.snapshot.state = state; entry.snapshot.last_error = error.map(bounded);
                if matches!(state, SessionState::Stopped | SessionState::Failed | SessionState::Waiting) { entry.snapshot.opened_identifiers = None; } }
        }
    }
    pub(crate) fn commit(&self, id: &str, generation: u64, profile: Profile) {
        self.commit_origin(id, generation, profile, false);
    }
    pub(crate) fn commit_applied(&self, id: &str, generation: u64, profile: Profile) {
        self.commit_origin(id, generation, profile, true);
    }
    fn commit_origin(&self, id: &str, generation: u64, profile: Profile, explicitly_applied: bool) {
        if let Ok(mut registry) = self.registry.lock() {
            if let Some(entry) = registry.entries.get_mut(id) {
                // Ignore an older transaction's completion after a new owner.
                if generation < entry.snapshot.device_generation
                    || entry.snapshot.pending_generation.is_some_and(|target| target != generation) { return; }
                let changed = entry.profile.as_ref().and_then(|previous| previous.to_toml().ok()) != profile.to_toml().ok();
                let explicit_edit = changed && (explicitly_applied || entry.pending_edit_generation == Some(generation));
                if let Ok(file) = ProfileFile::read(entry.file.path.clone()) { entry.file = file; }
                entry.snapshot.device_generation = generation;
                entry.snapshot.pending_generation = None;
                entry.snapshot.profile_source = Some(bounded(profile.source.clone()));
                entry.snapshot.persisted_revision = entry.file.revision();
                entry.snapshot.persisted_digest = entry.file.digest.clone();
                entry.snapshot.profile_saved = entry.file.matches(&profile);
                entry.snapshot.has_unsaved_runtime_edits = runtime_edit_pending(
                    entry.snapshot.has_unsaved_runtime_edits, explicit_edit, entry.snapshot.profile_saved);
                entry.pending_edit_generation = None;
                entry.snapshot.last_error = None;
                entry.profile = Some(profile);
                entry.enabled = true;
            }
        }
    }
    pub(crate) fn reject(&self, id: &str, error: String) {
        if let Ok(mut registry) = self.registry.lock() {
            if let Some(entry) = registry.entries.get_mut(id) { entry.snapshot.pending_generation = None; entry.pending_edit_generation = None; entry.snapshot.last_error = Some(bounded(error)); }
        }
    }
    pub(crate) fn primary_commit(&self, profile: Profile) -> Result<(), String> {
        self.primary_commit_with_origin(profile, false)
    }
    pub(crate) fn primary_commit_with_origin(&self, profile: Profile, explicitly_applied: bool) -> Result<(), String> {
        let Some(id) = self.primary_id()? else { return Err("primary device is not yet detected".into()); };
        let generation = self.registry.lock().map_err(|_| "device registry poisoned")?.entries[&id].snapshot.device_generation.checked_add(1).ok_or("device generation exhausted")?;
        self.commit_origin(&id, generation, profile, explicitly_applied);
        Ok(())
    }
    pub(crate) fn primary_state(&self, state: SessionState, error: Option<String>) {
        if let Ok(Some(id)) = self.primary_id() { self.state(&id, state, error); }
    }
    pub(crate) fn activated(&self, id: &str, selected: &SelectedDevice<'_>) {
        if let Ok(mut registry) = self.registry.lock() {
            if let Some(entry) = registry.entries.get_mut(id) {
                entry.snapshot.properties = selected.configuration.clone();
                entry.snapshot.digitizer = selected.identifier.clone();
                entry.snapshot.auxiliary = selected.auxiliary.as_ref().map(|(_, identifier)| identifier.clone());
            }
        }
    }
    /// Setup-only epoch prevents old session cleanup from erasing a replacement's
    /// live identifiers. The caller clears its exact epoch after read/output cleanup.
    pub(crate) fn activated_with_identifiers(&self, id: &str, selected: &SelectedDevice<'_>,
        identifiers: &[DeviceIdentifier]) -> u64 {
        if identifiers.is_empty() || identifiers.len() > 2 || identifiers.first() != Some(&selected.identifier)
            || identifiers.get(1).is_some_and(|auxiliary| selected.auxiliary.as_ref().map(|(_, id)| id) != Some(auxiliary)) {
            return 0;
        }
        let Ok(mut registry) = self.registry.lock() else { return 0; };
        if !registry.entries.contains_key(id) { return 0; }
        let Some(epoch) = registry.next_opened_epoch.checked_add(1) else { return 0; };
        registry.next_opened_epoch = epoch;
        let entry = registry.entries.get_mut(id).unwrap();
        entry.snapshot.properties = selected.configuration.clone();
        entry.snapshot.digitizer = selected.identifier.clone();
        entry.snapshot.auxiliary = selected.auxiliary.as_ref().map(|(_, identifier)| identifier.clone());
        entry.snapshot.opened_identifiers = Some(identifiers.to_vec());
        entry.opened_epoch = epoch;
        epoch
    }
    pub(crate) fn clear_opened_identifiers(&self, id: &str, epoch: u64) {
        if epoch == 0 { return; }
        if let Ok(mut registry) = self.registry.lock() {
            if let Some(entry) = registry.entries.get_mut(id) {
                if entry.opened_epoch == epoch { entry.snapshot.opened_identifiers = None; }
            }
        }
    }
    pub(crate) fn error(&self, id: &str, error: String) {
        if let Ok(mut registry) = self.registry.lock() {
            if let Some(entry) = registry.entries.get_mut(id) { entry.snapshot.last_error = Some(bounded(error)); }
        }
    }
    pub(crate) fn saved_profile(&self, id: &str) -> Result<Option<Profile>, String> {
        let (path, tablet) = {
            let registry = self.registry.lock().map_err(|_| "device registry poisoned")?;
            let entry = registry.entries.get(id).ok_or("unknown device session")?;
            (entry.file.path.clone(), entry.snapshot.tablet.clone())
        };
        // Explicit Save may have used the UI's core snapshot writer. Re-read
        // here rather than requiring a daemon restart to observe that file.
        let file = ProfileFile::read(path)?;
        if let Some(error) = &file.error { return Err(error.clone()); }
        if file.profile.as_ref().map(Profile::tablet_name).transpose()?.flatten().is_some_and(|name| name != tablet) {
            return Err("saved physical profile belongs to another tablet".into());
        }
        let profile = file.profile.clone();
        self.registry.lock().map_err(|_| "device registry poisoned")?.entries.get_mut(id).ok_or("unknown device session")?.file = file;
        Ok(profile)
    }
    /// Runs once before pipeline/handle preparation. Saved defaults apply to
    /// the initial generation; an explicit Apply wins unless the current file
    /// is its exact semantic source, in which case use that actual parsed file.
    pub(crate) fn effective_profile(&self, id: &str, incoming: &Profile, prefer_saved: bool) -> Result<Profile, String> {
        let (path, tablet) = {
            let registry = self.registry.lock().map_err(|_| "device registry poisoned")?;
            let entry = registry.entries.get(id).ok_or("unknown device session")?;
            (entry.file.path.clone(), entry.snapshot.tablet.clone())
        };
        let file = ProfileFile::read(path)?;
        let effective = if prefer_saved || file.matches(incoming) {
            if let Some(error) = &file.error { return Err(error.clone()); }
            file.profile.as_ref().unwrap_or(incoming).clone()
        } else { incoming.clone() };
        if effective.tablet_name()?.is_some_and(|name| name != tablet) { return Err("saved physical profile belongs to another tablet".into()); }
        let mut registry = self.registry.lock().map_err(|_| "device registry poisoned")?;
        registry.entries.get_mut(id).ok_or("unknown device session")?.file = file;
        Ok(effective)
    }
    pub(crate) fn primary_pending(&self, generation: u64, state: SessionState) {
        if let Ok(mut registry) = self.registry.lock() {
            if let Some(id) = registry.primary.clone() {
                let entry = registry.entries.get_mut(&id).unwrap();
                entry.snapshot.pending_generation = Some(generation);
                entry.pending_edit_generation = None;
                entry.snapshot.state = state;
            }
        }
    }
    pub(crate) fn primary_stopped(&self, generation: u64, error: Option<String>) {
        if let Ok(mut registry) = self.registry.lock() {
            if let Some(id) = registry.primary.clone() {
                let entry = registry.entries.get_mut(&id).unwrap();
                entry.snapshot.device_generation = generation;
                entry.snapshot.pending_generation = None;
                entry.pending_edit_generation = None;
                entry.snapshot.opened_identifiers = None;
                entry.snapshot.last_error = error.map(bounded);
                entry.snapshot.state = if entry.snapshot.last_error.is_some() { SessionState::Failed } else { SessionState::Stopped };
                entry.enabled = false;
            }
        }
    }
}
fn checked<'a>(registry: &'a Registry, id: &str, generation: u64) -> Result<&'a Entry, String> {
    if registry.shutting_down { return Err("device supervisor is stopping".into()); }
    let entry = registry.entries.get(id).ok_or("unknown device session")?;
    if entry.snapshot.device_generation != generation { return Err("device generation changed; refresh device sessions".into()); }
    Ok(entry)
}
fn bounded(mut value: String) -> String {
    if value.len() > crate::control::MAX_LOG_LINE_BYTES {
        let mut end = crate::control::MAX_LOG_LINE_BYTES;
        while !value.is_char_boundary(end) { end -= 1; }
        value.truncate(end);
    }
    value
}

thread_local! { static DEBUG_KEY: RefCell<Option<String>> = const { RefCell::new(None) }; }
pub(crate) fn set_debug_key(id: &str) { DEBUG_KEY.with(|key| *key.borrow_mut() = Some(id.to_owned())); }
pub(crate) fn debug_key() -> Option<String> { DEBUG_KEY.with(|key| key.borrow().clone()) }

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use otd_core::endpoint_match::{Endpoint, Transport};
    pub(crate) fn candidate(path: &str, parent: &str) -> Candidate {
        Candidate { path: path.encode_utf16().chain([0]).collect(), vendor: 1, product: 2,
            input_length: 10, usage_page: 13, usage: 2,
            endpoint: Endpoint { path: path.into(), physical_id: parent.into(), transport: Transport::UsbHid,
                vendor_id: 1, product_id: 2, can_open: true, input_length: 10, output_length: 0,
                feature_length: 0, strings: BTreeMap::new(), attributes: None } }
    }
    pub(crate) fn selected(candidate: &Candidate) -> SelectedDevice<'_> {
        SelectedDevice { pen: candidate, configuration: TabletConfiguration { name: "same model".into(), ..Default::default() },
            identifier: DeviceIdentifier::default(), auxiliary: None, spec: Profile::default().tablet }
    }
    #[test]
    fn same_model_devices_collections_and_reconnects_keep_independent_profiles() {
        let (handle, _requests) = Handle::channel(Event::create(true).unwrap());
        let a = candidate("collection-a", "parent-a");
        let b = candidate("collection-b", "parent-b");
        let a2 = candidate("another-collection-a", "parent-a");
        let id_a = handle.discover(&selected(&a)).unwrap();
        let id_b = handle.discover(&selected(&b)).unwrap();
        assert_ne!(id_a, id_b);
        assert_eq!(handle.discover(&selected(&a2)).unwrap(), id_a);
        let reconnect = candidate("changed-path-a", "PARENT-A");
        assert_eq!(handle.discover(&selected(&reconnect)).unwrap(), id_a);
        handle.commit(&id_a, 1, Profile { rotation: 90, ..Profile::default() });
        handle.commit(&id_b, 1, Profile { rotation: 180, ..Profile::default() });
        let profile_a = Profile::from_toml_text(&handle.profile(&id_a, 1).unwrap(), std::path::Path::new("a.toml")).unwrap();
        let profile_b = Profile::from_toml_text(&handle.profile(&id_b, 1).unwrap(), std::path::Path::new("b.toml")).unwrap();
        assert_eq!((profile_a.rotation, profile_b.rotation), (90, 180));
        assert!(handle.profile(&id_a, 0).is_err());
        let json = serde_json::to_string(&handle.snapshot().unwrap()).unwrap();
        assert!(!json.contains("parent-a"));
        assert!(!json.contains("changed-path"));
        assert_eq!(handle.snapshot().unwrap().sessions.len(), 2);
    }
    #[test]
    fn missing_parent_is_an_explicit_path_fallback_not_model_identity() {
        let (handle, _requests) = Handle::channel(Event::create(true).unwrap());
        let a = candidate("path-a", "");
        let b = candidate("path-b", "");
        assert_ne!(handle.discover(&selected(&a)).unwrap(), handle.discover(&selected(&b)).unwrap());
        assert!(handle.snapshot().unwrap().sessions.iter().all(|entry| entry.identity_stability == IdentityStability::PathFallback));
    }
}

#[cfg(test)]
mod runtime_origin_and_opened_tests {
    use super::*;
    #[test]
    fn defaults_noop_apply_edits_and_old_activation_cleanup_remain_distinct() {
        let (handle, _requests) = Handle::channel(Event::create(true).unwrap());
        let candidate = tests::candidate("origin-only-fixture", "origin-only-parent");
        let selected = tests::selected(&candidate);
        let id = handle.discover(&selected).unwrap();
        let initial = Profile::default();
        handle.commit(&id, 1, initial.clone());
        assert!(!handle.snapshot().unwrap().sessions[0].has_unsaved_runtime_edits);
        handle.commit_applied(&id, 2, initial.clone());
        assert!(!handle.snapshot().unwrap().sessions[0].has_unsaved_runtime_edits, "no-op Apply is not an edit");
        handle.commit_applied(&id, 3, Profile { rotation: 90, ..initial.clone() });
        assert!(handle.snapshot().unwrap().sessions[0].has_unsaved_runtime_edits);
        handle.commit(&id, 2, initial);
        assert_eq!(handle.snapshot().unwrap().sessions[0].device_generation, 3, "stale commit cannot erase edit origin");
        assert!(runtime_edit_pending(true, false, false));
        assert!(!runtime_edit_pending(true, false, true), "an actual persisted match clears origin");
        let first = handle.activated_with_identifiers(&id, &selected, &[selected.identifier.clone()]);
        let replacement = handle.activated_with_identifiers(&id, &selected, &[selected.identifier.clone()]);
        assert!(replacement > first && first > 0);
        handle.clear_opened_identifiers(&id, first);
        assert!(handle.snapshot().unwrap().sessions[0].opened_identifiers.is_some(), "retired cleanup cannot clear replacement");
        handle.clear_opened_identifiers(&id, replacement);
        assert!(handle.snapshot().unwrap().sessions[0].opened_identifiers.is_none());
        assert!(handle.snapshot().unwrap().sessions[0].connected, "discovery survives closed handles");
    }
}
