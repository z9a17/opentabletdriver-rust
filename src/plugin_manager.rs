//! Single native installation/discovery owner for direct original Desktop calls.
//! Called on service worker threads; native report paths never enter this module.
use std::path::Path;
use std::sync::atomic::AtomicBool;
use serde_json::{Value, json};

pub fn invoke(method: &str, params: &Value, cancel: Option<&AtomicBool>) -> Result<Option<Value>, String> {
    if !matches!(method,"LoadPlugins"|"InstallPlugin"|"DownloadPlugin"|"UninstallPlugin"|"InstallPluginDirectory"|"UpdatePluginDirectory"|"UnloadPluginContext"|"RemovePluginAssembly") { return Ok(None); }
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
