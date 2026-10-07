//! Retained original-format settings. Mutations are cold owner operations;
//! report/provider cached reads never wait for a settings apply to complete.
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, TryLockError};
use serde_json::{Value, json};
use otd_core::storage::{self, FileSnapshot, SaveMode};
use super::protocol::Error;

static STATE: OnceLock<Mutex<Option<Collection>>> = OnceLock::new();
static OWNER: Mutex<()> = Mutex::new(());
struct Collection {
    document: Value,
    cached: Arc<Value>,
    revision: u64,
    authoritative: bool,
    source: PathBuf,
    file: FileSnapshot,
}
#[derive(Clone)]
pub struct Snapshot { pub document: Value, pub revision: u64, pub authoritative: bool }

pub fn path() -> Result<PathBuf, String> {
    Ok(storage::data_directory()?.join("upstream-rpc-settings.json"))
}
pub fn owner() -> Result<MutexGuard<'static, ()>, Error> {
    OWNER.try_lock().map_err(|error| match error {
        TryLockError::WouldBlock => Error::failed("another settings operation owns the collection; refresh and retry"),
        TryLockError::Poisoned(_) => Error::failed("settings operation owner poisoned"),
    })
}
fn with<T>(f: impl FnOnce(&mut Collection) -> Result<T, Error>) -> Result<T, Error> {
    let slot = STATE.get_or_init(|| Mutex::new(None));
    let mut state = slot.lock()
        .map_err(|_| Error::failed("settings collection lock poisoned"))?;
    if state.is_none() {
        drop(state);
        let loaded = Collection::load()?;
        state = slot.lock().map_err(|_| Error::failed("settings collection lock poisoned"))?;
        if state.is_none() { *state = Some(loaded); }
    }
    f(state.as_mut().unwrap())
}
impl Collection {
    fn load() -> Result<Self, Error> {
        let destination = path()?;
        let initial = bounded_snapshot(&destination)?;
        let (document, source, authoritative, file) = if initial.exists() {
            let (document, file) = read_document(&destination)?;
            (document, destination.clone(), true, file)
        } else if let Some(original) = crate::config::otd_settings_path().filter(|path| path.exists()) {
            (read_document(&original)?.0, original, false, initial)
        } else { (empty(), destination, false, initial) };
        let cached = Arc::new(document.clone());
        Ok(Self { document, cached, revision: 1, authoritative, source, file })
    }
    fn replace(&mut self, document: Value, expected: u64, authoritative: bool) -> Result<u64, Error> {
        if self.revision != expected { return Err(Error::failed("settings collection changed during operation")); }
        if self.document != document || authoritative && !self.authoritative {
            self.revision = self.revision.checked_add(1).ok_or_else(|| Error::failed("settings collection revision exhausted"))?;
            self.document = document;
            self.cached = Arc::new(self.document.clone());
        }
        self.authoritative |= authoritative;
        Ok(self.revision)
    }
}
pub fn empty() -> Value {
    json!({"Revision":"0.6.7.0","Profiles":[],"Tools":[],
        "LockUsableAreaDisplay":true,"LockUsableAreaTablet":true})
}
/// Preserve all unknown fields and disconnected rows. Do not construct or
/// require installed plugins for a profile with no connected input device.
pub fn normalize(settings: &Value) -> Result<Value, Error> {
    let mut document = if settings.is_null() { empty() } else { settings.clone() };
    let object = document.as_object_mut().ok_or_else(|| Error::invalid("settings must be an object or null"))?;
    object.entry("Profiles").or_insert_with(|| json!([]));
    object.entry("Tools").or_insert_with(|| json!([]));
    let profiles = object["Profiles"].as_array().ok_or_else(|| Error::invalid("Profiles must be an array"))?;
    for profile in profiles {
        if !profile.is_object() || profile["Tablet"].as_str().is_none_or(str::is_empty) {
            return Err(Error::invalid("each settings profile needs a nonempty Tablet name"));
        }
    }
    let tools = object["Tools"].as_array().ok_or_else(|| Error::invalid("Tools must be an array"))?;
    for store in tools {
        if !store.is_null() && !store.is_object() { return Err(Error::invalid("Tools entries must be stores or null")); }
    }
    let encoded = serde_json::to_vec(&document).map_err(|error| Error::invalid(error.to_string()))?;
    if encoded.len() > crate::control::MAX_PROFILE_BYTES { return Err(Error::invalid("settings collection exceeds 128 KiB")); }
    Ok(document)
}
pub fn snapshot() -> Result<Snapshot, Error> {
    with(|state| Ok(Snapshot { document: state.document.clone(), revision: state.revision, authoritative: state.authoritative }))
}
pub fn cached() -> Option<Arc<Value>> {
    STATE.get()?.try_lock().ok()?.as_ref().map(|state| Arc::clone(&state.cached))
}
pub fn cached_revision() -> Option<u64> {
    STATE.get()?.try_lock().ok()?.as_ref().map(|state| state.revision)
}
pub fn check_revision(expected: Option<u64>) -> Result<Snapshot, Error> {
    let snapshot = snapshot()?;
    if snapshot.revision == u64::MAX { return Err(Error::failed("settings collection revision exhausted")); }
    if expected.is_some_and(|revision| revision != snapshot.revision) {
        return Err(Error::failed("settings collection changed since GetSettings; reload before replacing it"));
    }
    Ok(snapshot)
}
pub fn publish(document: Value, expected: u64, authoritative: bool) -> Result<u64, Error> {
    let document = normalize(&document)?;
    with(|state| state.replace(document,expected,authoritative))
}
pub fn read_saved() -> Result<(Value, FileSnapshot), Error> { read_document(&path()?) }
pub fn accept_loaded_file(file: FileSnapshot) -> Result<(), Error> {
    with(|state| { state.source = path()?; state.file = file; Ok(()) })
}
pub fn save(document: &Value, expected: u64) -> Result<(), Error> {
    let document = normalize(document)?;
    let bytes = serde_json::to_vec_pretty(&document).map_err(|error| Error::failed(error.to_string()))?;
    if bytes.len() > crate::control::MAX_PROFILE_BYTES { return Err(Error::invalid("serialized settings exceed 128 KiB")); }
    let previous = with(|state| {
        if state.revision != expected { return Err(Error::failed("settings changed before Save")); }
        Ok(state.file.clone())
    })?;
    let destination = path()?;
    let mode = if previous.exists() { SaveMode::Replace(&previous) } else { SaveMode::CreateNew };
    let saved = storage::save(&destination, &bytes, mode)?;
    with(|state| {
        if state.revision != expected { return Err(Error::failed("file saved, but settings changed before snapshot publication; reload")); }
        state.file = saved;
        state.source = destination;
        Ok(())
    })
}
fn bounded_snapshot(path: &Path) -> Result<FileSnapshot, Error> {
    if let Ok(metadata) = std::fs::metadata(path) {
        if metadata.len() > crate::control::MAX_PROFILE_BYTES as u64 { return Err(Error::failed("settings file exceeds 128 KiB")); }
    }
    Ok(storage::capture(path)?)
}
fn read_document(path: &Path) -> Result<(Value, FileSnapshot), Error> {
    let _ = bounded_snapshot(path)?;
    let loaded = storage::read_utf8(path)?;
    if loaded.text.len() > crate::control::MAX_PROFILE_BYTES { return Err(Error::failed("settings file exceeds 128 KiB")); }
    let document = serde_json::from_str(&loaded.text).map_err(|error| Error::invalid(format!("{}: {error}", path.display())))?;
    Ok((normalize(&document)?, loaded.snapshot))
}
/// Cold startup lookup. Explicit per-physical settings remain higher priority
/// at the caller. The operation owner is never acquired here: candidate setup
/// can safely read while a SetSettings transaction waits on native control.
pub fn profile_for_tablet(tablet: &otd_core::tablets::TabletConfiguration) -> Result<Option<crate::config::Profile>, String> {
    let retained = with(|state| {
        if !state.authoritative { return Ok(None); }
        Ok(Some((state.document.clone(), state.source.clone())))
    }).map_err(|error| error.message)?;
    let Some((document, source)) = retained else { return Ok(None); };
    // Missing names get pinned defaults only once an actual configuration is
    // available. Disconnected rows remain intact in the retained collection.
    let screen = crate::display::read_snapshot()?.virtual_screen;
    let projected = super::settings::for_detected(&document, std::slice::from_ref(tablet), screen)
        .map_err(|error| error.message)?;
    let text = serde_json::to_string(&projected).map_err(|error| error.to_string())?;
    let profile = crate::plugins::import_otd_with_installed(&text, &source, &[tablet.name.clone()])?;
    profile.for_tablet(otd_core::spec::TabletSpec::from_configuration(tablet)?).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn offline_collection_retains_unknown_disconnected_stores_and_accepts_empty_defaults() {
        let document = json!({"Profiles":[{"Tablet":"Unavailable future tablet","Filters":[
            {"Path":"Uninstalled.Filter","Enable":true,"Settings":[{"Property":"Opaque","Value":[null,17]}]}]}],
            "Tools":[],"OpaqueExtension":{"nested":true}});
        assert_eq!(normalize(&document).unwrap(), document);
        assert_eq!(normalize(&Value::Null).unwrap(), empty());
        assert!(normalize(&json!({"Profiles":null})).is_err());
        assert!(normalize(&json!({"Profiles":[{"Tablet":""}]})).is_err());
    }
    #[test]
    fn stale_idle_client_cannot_replace_a_newer_collection_or_its_disabled_stores() {
        let source = PathBuf::from("E:/AgentWork/tmp/otd-rpc-collection-readonly-fixture.json");
        let mut state = Collection { document:empty(), cached:Arc::new(empty()), revision:1, authoritative:false,
            file:storage::capture(&source).unwrap(), source };
        let replacement = normalize(&json!({"Profiles":[{"Tablet":"Disconnected","Opaque":17}],
            "Tools":[{"Path":"Unavailable.Tool","Settings":[]}]})).unwrap();
        assert_eq!(state.replace(replacement.clone(),1,true).unwrap(),2);
        assert!(state.replace(empty(),1,true).is_err());
        assert_eq!(state.document,replacement);
        assert!(state.authoritative);
        assert!(!state.document["Tools"][0]["Enable"].as_bool().unwrap_or(false),
            "pinned JSON constructor leaves omitted Enable false");
        assert_eq!(state.replace(replacement,2,true).unwrap(),2,"idempotent Set retains revision");
    }
}
