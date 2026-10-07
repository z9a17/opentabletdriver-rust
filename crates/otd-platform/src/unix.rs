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
    // The shared native owner publishes its metadata before any CLR constructor.
    // This bootstrap hook therefore performs no I/O or recursive control call.
    pub fn prime() {}
}
pub mod plugin_catalog {
    pub fn plugins_directory() -> Result<std::path::PathBuf, String> { Ok(otd_core::storage::data_directory()?.join("plugins")) }
    pub fn recover_installations() -> Result<bool, String> {
        // Portable mutations acquire the owner transaction; never initialize a
        // plugin while a Windows-shipped catalog transaction is being staged.
        Ok(!plugins_directory()?.join(".install.lock").exists())
    }
}
pub mod upstream_rpc {
    pub fn original_application_info() -> Result<serde_json::Value, String> {
        let data = otd_core::storage::data_directory()?;
        Ok(serde_json::json!({"AppDataDirectory":data,"SettingsFile":data.join("settings.json"),
            "PluginDirectory":super::plugin_catalog::plugins_directory()?,"PresetDirectory":data.join("presets"),
            "LogDirectory":data.join("logs"),"TemporaryDirectory":data.join("compat-temp"),"CacheDirectory":data.join("cache"),
            "BackupDirectory":data.join("backup"),"TrashDirectory":data.join("trash"),
            "ConfigurationDirectory":otd_core::config::configurations_directory()}))
    }
}
