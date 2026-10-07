//! Single native installation/discovery owner for direct original Desktop calls.
//! Called on service worker threads; native report paths never enter this module.
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::Mutex;
use serde_json::{Value, json};

#[derive(Default)]
struct AdmissionState { calls: usize, update: bool }
static ADMISSION: Mutex<AdmissionState> = Mutex::new(AdmissionState { calls: 0, update: false });

/// Cold service admission, never held as a mutex across a plugin constructor.
/// The update owner fails Busy rather than waiting on a callback that may itself
/// need the native control thread. Accepted operations finish before reservation.
pub(crate) struct Admission;
impl Drop for Admission {
    fn drop(&mut self) { if let Ok(mut state) = ADMISSION.lock() { state.calls -= 1; } }
}
pub(crate) fn admit() -> Result<Admission, String> {
    let mut state = ADMISSION.lock().map_err(|_| "Plugin admission lock poisoned")?;
    if state.update { return Err("Plugin operations are reserved for update".into()); }
    state.calls = state.calls.checked_add(1).ok_or("Plugin admission count exhausted")?;
    Ok(Admission)
}
pub(crate) struct UpdateReservation;
impl Drop for UpdateReservation {
    fn drop(&mut self) { if let Ok(mut state) = ADMISSION.lock() { state.update = false; } }
}
pub(crate) fn reserve_update() -> Result<UpdateReservation, String> {
    let mut state = ADMISSION.lock().map_err(|_| "Plugin admission lock poisoned")?;
    if state.update || state.calls != 0 { return Err("Plugin operations are active or already reserved; retry update after they finish".into()); }
    state.update = true;
    Ok(UpdateReservation)
}
pub(crate) fn is_operation(method: &str) -> bool {
    matches!(method,"LoadPlugins"|"InstallPlugin"|"DownloadPlugin"|"UninstallPlugin"|"InstallPluginDirectory"|"UpdatePluginDirectory"|"UnloadPluginContext"|"RemovePluginAssembly")
}

pub fn invoke(method: &str, params: &Value, cancel: Option<&AtomicBool>) -> Result<Option<Value>, String> {
    if !is_operation(method) { return Ok(None); }
    let _admission = admit()?;
    crate::download::cancelled(cancel)?;
    let args = params.as_array().ok_or("Plugin manager parameters must be an array")?;
    let text = |index:usize| args.get(index).and_then(Value::as_str).ok_or("Plugin manager argument must be a string");
    let arity = |count:usize| if args.len() == count {Ok(())} else {Err("Invalid plugin manager argument count".to_owned())};
    let root = crate::plugin_catalog::plugins_directory()?;
    std::fs::create_dir_all(&root).map_err(|error|error.to_string())?;
    match method {
        "UnloadPluginContext" => {
            arity(1)?;
            crate::dotnet::mutate_installed_plugins(&json!({"operation":"unload_context","path":text(0)?}))?;
            return Ok(Some(json!(true)));
        }
        "RemovePluginAssembly" => {
            arity(2)?;
            let context = args[1].as_str();
            if !args[1].is_null() && context.is_none() {return Err("Assembly context directory must be string or null".into());}
            crate::dotnet::mutate_installed_plugins(&json!({"operation":"remove_assembly","identity":text(0)?,"context_directory":context}))?;
            return Ok(Some(json!(true)));
        }
        "LoadPlugins" => {arity(0)?;}
        "InstallPlugin" => {arity(1)?;crate::plugin_catalog::install_file_with_cancel(Path::new(text(0)?),cancel)?;}
        "DownloadPlugin" => {
            arity(1)?;
            let metadata:crate::plugin_catalog::PluginMetadata=serde_json::from_value(args[0].clone()).map_err(|error|error.to_string())?;
            if !metadata.supports_driver(){return Err("Plugin does not support original OpenTabletDriver 0.6.7".into());}
            crate::plugin_catalog::install_with_cancel(&metadata,cancel)?;
        }
        "UninstallPlugin" => {
            arity(1)?;let name=text(0)?;
            let matches:Vec<_>=crate::plugin_catalog::installed()?.into_iter().filter(|(path,metadata)|
                path.to_string_lossy()==name || metadata.name==name).collect();
            if matches.len()!=1 {return Err("Plugin must uniquely identify an installed directory or name".into());}
            crate::plugin_catalog::uninstall(&matches[0].0)?;
        }
        "InstallPluginDirectory"|"UpdatePluginDirectory" => {
            arity(2)?;crate::plugin_catalog::install_directory(Path::new(text(0)?),Path::new(text(1)?),method=="UpdatePluginDirectory",cancel)?;
        }
        _=>unreachable!(),
    }
    // A completed file mutation must publish actual CLR and Rust discovery
    // before reporting success. Failure explicitly distinguishes applied files.
    crate::dotnet::reload_installed_plugins(&root).map_err(|error|format!("Plugin files changed or Load attempted, but registry publication failed: {error}; inspect installed files before retrying"))?;
    Ok(Some(if method=="LoadPlugins" {Value::Null}else{json!(true)}))
}
