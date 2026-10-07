// Reuse the exact Windows-shipped graph, native ABI validation, managed parser,
// registry lease and action ownership. Only host loading and owner adapters vary.
pub use otd_core::{config, mapping, protocol, radial_follow};
#[path = "../../../src/plugins.rs"]
pub mod plugins;
#[path = "../../../src/dotnet.rs"]
pub mod dotnet;
#[path = "../../../src/managed_services.rs"]
pub mod managed_services;

pub mod control {
    #[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct WorkerIdentity { pub instance: String, pub generation: u64 }
}
pub mod device_sessions {
    #[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub enum SessionState { Detected, Preparing, Starting, Running, Stopping, Stopped, Waiting, Failed }
    thread_local! { static SOURCE: std::cell::RefCell<Option<serde_json::Value>> = const { std::cell::RefCell::new(None) }; }
    pub struct SourceGuard(Option<serde_json::Value>);
    pub fn source_scope(id: &str, generation: u64, reader_generation: u64) -> SourceGuard {
        let value = serde_json::json!({"id":id,"device_generation":generation,"reader_generation":reader_generation});
        SourceGuard(SOURCE.with(|slot| slot.replace(Some(value))))
    }
    pub fn source_session_json() -> Option<serde_json::Value> { SOURCE.with(|slot| slot.borrow().clone()) }
    pub fn source_generation() -> u64 { SOURCE.with(|slot|slot.borrow().as_ref().and_then(|value|value["device_generation"].as_u64()).unwrap_or(0)) }
    pub fn debug_key() -> Option<String> { SOURCE.with(|slot|slot.borrow().as_ref().and_then(|value|value["id"].as_str()).map(str::to_owned)) }
    impl Drop for SourceGuard { fn drop(&mut self) { SOURCE.with(|slot| { slot.replace(self.0.take()); }); } }
}
pub mod display {
    thread_local! { static SNAPSHOT: std::cell::RefCell<Option<otd_core::display::DisplaySnapshot>> = const { std::cell::RefCell::new(None) }; }
    /// Construction metadata belongs to this worker; another device's mapper
    /// must never replace its snapshot during managed endpoint construction.
    pub fn set_snapshot(snapshot: otd_core::display::DisplaySnapshot) { SNAPSHOT.with(|slot| *slot.borrow_mut() = Some(snapshot)); }
    pub fn read_snapshot() -> Result<otd_core::display::DisplaySnapshot, String> {
        SNAPSHOT.with(|slot| slot.borrow().clone().ok_or_else(|| "native display snapshot has not been supplied by this worker".into()))
    }
}
pub mod action_output {
    thread_local! { static SUPPORTS: std::cell::Cell<Option<fn(otd_core::actions::Action)->bool>> = const { std::cell::Cell::new(None) }; }
    pub fn set_supports(supports: fn(otd_core::actions::Action)->bool) { SUPPORTS.with(|slot| slot.set(Some(supports))); }
    pub fn supports(action: otd_core::actions::Action) -> bool { SUPPORTS.with(|slot| slot.get().is_some_and(|supports| supports(action))) }
}
pub mod managed_host {
    use std::sync::{Mutex,OnceLock};
    use super::managed_services::{Publisher,Snapshot};
    struct Cached {publisher:Publisher,snapshot:Snapshot,inventory:Option<serde_json::Value>}
    static CURRENT:OnceLock<Mutex<Option<Cached>>>=OnceLock::new();
    pub fn install(publisher:Publisher,snapshot:Snapshot)->Result<(),String>{
        let mut current=CURRENT.get_or_init(||Mutex::new(None)).lock().map_err(|_|"Managed publication poisoned")?;
        if current.is_some(){return Err("Managed publication owner already installed".into());}let inventory=snapshot.devices.clone();*current=Some(Cached{publisher,snapshot,inventory});Ok(())
    }
    pub fn publish(mut snapshot:Snapshot)->Result<(),String>{
        let mut current=CURRENT.get_or_init(||Mutex::new(None)).lock().map_err(|_|"Managed publication poisoned")?;
        let current=current.as_mut().ok_or("Managed native publication owner unavailable")?;
        snapshot.version=current.snapshot.version.checked_add(1).ok_or("Managed snapshot generation exhausted")?;
        current.inventory=snapshot.devices.clone();snapshot.devices=merge_owned(current.inventory.as_ref());
        current.publisher.publish(snapshot.clone())?;current.snapshot=snapshot;Ok(())
    }
    pub fn clear(){if let Ok(mut current)=CURRENT.get_or_init(||Mutex::new(None)).lock(){*current=None;}}
    fn merge_owned(inventory:Option<&serde_json::Value>)->Option<serde_json::Value>{
        let discovered=inventory?.as_array()?;let owned=crate::shared_devices::owned_metadata();let mut result=Vec::new();
        for device in discovered{if !owned.iter().any(|owner|owner["DevicePath"]==device["DevicePath"]){result.push(device.clone());}}
        for owner in owned{let mut device=discovered.iter().find(|device|device["DevicePath"]==owner["DevicePath"]).cloned().unwrap_or_else(||serde_json::json!({}));
            if let Some(value)=device.as_object_mut(){value.extend(owner.as_object()?.clone());value.insert("CanOpen".into(),true.into());}result.push(device);}
        Some(serde_json::json!(result))
    }
    pub fn publish_owned_devices(){
        if let Ok(mut current)=CURRENT.get_or_init(||Mutex::new(None)).lock(){if let Some(current)=current.as_mut(){
            // Do not collapse current and prepared readers sharing one path.
            // Scoped constructors select their concrete reader_generation.
            current.snapshot.devices=merge_owned(current.inventory.as_ref());
            if let Some(version)=current.snapshot.version.checked_add(1){current.snapshot.version=version;let _=current.publisher.publish(current.snapshot.clone());}
        }}
    }
    // Cold setup publishes exact prepared reader metadata without sending a
    // recursive control call to the transaction waiting for this constructor.
    pub fn prime(){publish_owned_devices();}
}
pub mod upstream_rpc {
    pub use crate::original_rpc_protocol as protocol;
    pub mod collection { pub fn empty() -> serde_json::Value { serde_json::json!({"Revision":"0.6.7.0","Profiles":[],"Tools":[],"LockUsableAreaDisplay":true,"LockUsableAreaTablet":true}) } }
    pub fn original_application_info() -> Result<serde_json::Value, String> {
        let data = otd_core::storage::data_directory()?;
        Ok(serde_json::json!({"AppDataDirectory":data,"SettingsFile":data.join("settings.json"),
            "PluginDirectory":crate::plugin_catalog::plugins_directory()?,"PresetDirectory":data.join("presets"),
            "LogDirectory":data.join("logs"),"TemporaryDirectory":data.join("compat-temp"),"CacheDirectory":data.join("cache"),
            "BackupDirectory":data.join("backup"),"TrashDirectory":data.join("trash"),
            "ConfigurationDirectory":otd_core::config::configurations_directory()}))
    }
}
