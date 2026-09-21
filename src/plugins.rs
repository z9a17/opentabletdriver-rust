//! Trusted native and OpenTabletDriver .NET position filters. No DLL is loaded
//! by parsing or saving a profile. Loading occurs only for explicit inspection
//! or when starting output; the successful per-report path never allocates.
use otd_plugin_api::{ABI_VERSION, FilterApi, Header, Sample};
use serde::{Deserialize, Serialize};
use std::ffi::{OsStr, c_void};
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::time::Instant;
use windows_sys::Win32::Foundation::{FreeLibrary, HMODULE};
use windows_sys::Win32::System::LibraryLoader::{
    GetProcAddress, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW,
};

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PluginKind {
    #[default]
    Native,
    Dotnet,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PluginConfig {
    pub path: PathBuf,
    #[serde(default)]
    pub kind: PluginKind,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub type_name: String,
    #[serde(default = "empty_settings")]
    pub settings_json: String,
}

fn empty_settings() -> String {
    "{}".into()
}

impl PluginConfig {
    pub fn validate(&self) -> Result<(), String> {
        if self.path.as_os_str().is_empty() {
            return Err("plugin DLL path is empty".into());
        }
        if self.settings_json.len() > 65_536 {
            return Err("plugin settings exceed 64 KiB".into());
        }
        let value: serde_json::Value = serde_json::from_str(&self.settings_json)
            .map_err(|e| format!("invalid plugin settings JSON: {e}"))?;
        if !value.is_object() {
            return Err("plugin settings must be a JSON object".into());
        }
        if self.kind == PluginKind::Dotnet && self.type_name.trim().is_empty() {
            return Err(".NET plugin requires a type_name".into());
        }
        Ok(())
    }
}

pub struct Library(HMODULE);

pub fn wide(value: &OsStr) -> Result<Vec<u16>, String> {
    let mut result: Vec<_> = value.encode_wide().collect();
    if result.contains(&0) {
        return Err("path contains a NUL character".into());
    }
    result.push(0);
    Ok(result)
}

impl Library {
    pub fn load(path: &Path) -> Result<Self, String> {
        let path = path
            .canonicalize()
            .map_err(|e| format!("cannot open {}: {e}", path.display()))?;
        let name = wide(path.as_os_str())?;
        let handle = unsafe {
            LoadLibraryExW(
                name.as_ptr(),
                std::ptr::null_mut(),
                LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32,
            )
        };
        if handle.is_null() {
            return Err(format!(
                "cannot load {}: {}",
                path.display(),
                std::io::Error::last_os_error()
            ));
        }
        Ok(Self(handle))
    }

    pub fn symbol(
        &self,
        name: &'static [u8],
    ) -> Result<unsafe extern "system" fn() -> isize, String> {
        unsafe { GetProcAddress(self.0, name.as_ptr()) }.ok_or_else(|| {
            format!(
                "DLL is missing export {}",
                String::from_utf8_lossy(&name[..name.len() - 1])
            )
        })
    }
}

impl Drop for Library {
    fn drop(&mut self) {
        unsafe { FreeLibrary(self.0) };
    }
}

fn validate_header(header: Header) -> Result<(), String> {
    if header.abi_version != ABI_VERSION
        || header.struct_size != std::mem::size_of::<FilterApi>() as u32
    {
        return Err("incompatible filter ABI (expected version 1)".into());
    }
    Ok(())
}

pub struct Plugin {
    api: FilterApi,
    context: *mut c_void,
    pub name: String,
    disabled: bool,
    // Destroy the context before unloading its callbacks.
    _library: Option<Library>,
}

impl Plugin {
    pub fn load(config: &PluginConfig) -> Result<Self, String> {
        config.validate()?;
        let (api, library) = if config.kind == PluginKind::Dotnet {
            (crate::dotnet::filter_api()?, None)
        } else {
            let library = Library::load(&config.path)?;
            let entry: unsafe extern "C" fn() -> *const FilterApi =
                unsafe { std::mem::transmute(library.symbol(b"otd_filter_v1\0")?) };
            let ptr = unsafe { entry() };
            if ptr.is_null() {
                return Err("plugin returned a null API".into());
            }
            // Native DLLs are trusted executable code. The ABI guarantees a
            // readable header even when versions differ.
            validate_header(unsafe { ptr.cast::<Header>().read() })?;
            (unsafe { ptr.read() }, Some(library))
        };
        validate_header(api.header)?;
        let (Some(create), Some(_), Some(_), Some(_)) =
            (api.create, api.process, api.reset, api.destroy)
        else {
            return Err("plugin has missing callbacks".into());
        };
        let length = api
            .name
            .iter()
            .position(|b| *b == 0)
            .ok_or("plugin name is not terminated")?;
        let name = std::str::from_utf8(&api.name[..length])
            .map_err(|_| "invalid plugin name")?
            .to_owned();
        let name = if config.kind == PluginKind::Dotnet {
            config.type_name.clone()
        } else {
            name
        };
        let settings = if config.kind == PluginKind::Dotnet {
            serde_json::json!({
                "assembly_path": config.path.canonicalize().map_err(|e| e.to_string())?,
                "type_name": config.type_name,
                "settings": serde_json::from_str::<serde_json::Value>(&config.settings_json).map_err(|e| e.to_string())?
            }).to_string()
        } else {
            config.settings_json.clone()
        };
        let context = unsafe { create(settings.as_ptr(), settings.len()) };
        if context.is_null() {
            if config.kind == PluginKind::Dotnet {
                return Err(crate::dotnet::last_error());
            }
            return Err(format!(
                "{name}: initialization failed; check plugin type/settings and diagnostics"
            ));
        }
        Ok(Self {
            api,
            context,
            name,
            disabled: false,
            _library: library,
        })
    }

    pub fn process(&mut self, sample: &mut Sample) -> bool {
        if self.disabled {
            return true;
        }
        let original = *sample;
        let status = unsafe { (self.api.process.unwrap())(self.context, sample) };
        if status != 0 || !sample.x.is_finite() || !sample.y.is_finite() {
            *sample = original;
            self.disabled = true;
            return false;
        }
        // V1 exposes only positional changes; report/contact metadata stays raw.
        let position = (sample.x, sample.y);
        *sample = original;
        sample.x = position.0;
        sample.y = position.1;
        true
    }

    pub fn reset(&mut self) {
        if !self.disabled {
            unsafe { (self.api.reset.unwrap())(self.context) };
        }
    }
}

impl Drop for Plugin {
    fn drop(&mut self) {
        unsafe { (self.api.destroy.unwrap())(self.context) };
    }
}

pub struct PluginChain {
    plugins: Vec<Plugin>,
    epoch: Instant,
    failure: Option<usize>,
}

impl PluginChain {
    pub fn load(configs: &[PluginConfig]) -> Result<Self, String> {
        let plugins = configs
            .iter()
            .filter(|p| p.enabled)
            .map(Plugin::load)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            plugins,
            epoch: Instant::now(),
            failure: None,
        })
    }

    pub fn process(&mut self, position: (f32, f32), pen: crate::protocol::PenReport) -> (f32, f32) {
        if self.plugins.is_empty() {
            return position;
        }
        let mut sample = Sample {
            x: position.0,
            y: position.1,
            time_ns: self.epoch.elapsed().as_nanos().min(u128::from(u64::MAX)) as u64,
            pressure: u32::from(pen.pressure),
            flags: otd_plugin_api::PROXIMITY
                | if pen.eraser {
                    otd_plugin_api::ERASER
                } else {
                    0
                },
        };
        for (index, plugin) in self.plugins.iter_mut().enumerate() {
            if !plugin.process(&mut sample) {
                self.failure = Some(index);
                eprintln!(
                    "Disabled failing plugin: {} (error or nonfinite position)",
                    plugin.name
                );
            }
        }
        (sample.x, sample.y)
    }

    pub fn reset(&mut self) {
        for plugin in &mut self.plugins {
            plugin.reset();
        }
    }
    pub fn is_empty(&self) -> bool {
        self.plugins.is_empty()
    }

    pub fn take_failure(&mut self) -> Option<&str> {
        self.failure
            .take()
            .map(|index| self.plugins[index].name.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_output_disables_once_and_context_is_destroyed() {
        unsafe extern "C" fn bad(context: *mut c_void, sample: *mut Sample) -> i32 {
            unsafe {
                *context.cast::<u32>() += 1;
                (*sample).x = f32::NAN;
            }
            0
        }
        unsafe extern "C" fn reset(context: *mut c_void) {
            unsafe {
                *context.cast::<u32>() += 100;
            }
        }
        unsafe extern "C" fn destroy(context: *mut c_void) {
            unsafe {
                *context.cast::<u32>() += 10;
            }
        }
        let mut calls = 0u32;
        let mut plugin = Plugin {
            api: FilterApi {
                header: Header::V1,
                name: [0; 64],
                create: None,
                process: Some(bad),
                reset: Some(reset),
                destroy: Some(destroy),
            },
            context: (&mut calls as *mut u32).cast(),
            name: "test".into(),
            disabled: false,
            _library: None,
        };
        let mut sample = Sample {
            x: 123.0,
            y: 456.0,
            ..Sample::default()
        };
        crate::test_alloc::assert_no_allocations(|| {
            assert!(!plugin.process(&mut sample));
            assert_eq!((sample.x, sample.y), (123.0, 456.0));
            assert!(plugin.process(&mut sample));
            plugin.reset();
        });
        drop(plugin);
        assert_eq!(calls, 11);
    }
    #[test]
    fn rejects_bad_abi_and_invalid_settings_without_loading_code() {
        assert!(
            validate_header(Header {
                abi_version: 999,
                struct_size: 0
            })
            .is_err()
        );
        let config = PluginConfig {
            path: "missing.dll".into(),
            kind: PluginKind::Native,
            enabled: true,
            type_name: String::new(),
            settings_json: "[]".into(),
        };
        assert!(config.validate().is_err());
    }

    #[test]
    #[ignore = "requires built sample DLL; set OTD_TEST_PLUGIN"]
    fn native_plugin_round_trip() {
        let path = std::env::var_os("OTD_TEST_PLUGIN").expect("set OTD_TEST_PLUGIN");
        let mut plugin = Plugin::load(&PluginConfig {
            path: path.into(),
            kind: PluginKind::Native,
            enabled: true,
            type_name: String::new(),
            settings_json: r#"{"alpha":0.5}"#.into(),
        })
        .unwrap();
        let mut sample = Sample {
            x: 100.0,
            y: 200.0,
            ..Sample::default()
        };
        assert!(plugin.process(&mut sample));
        sample.x = 200.0;
        sample.y = 400.0;
        crate::test_alloc::assert_no_allocations(|| assert!(plugin.process(&mut sample)));
        assert_eq!((sample.x, sample.y), (150.0, 300.0));
        plugin.reset();
        sample.x = 600.0;
        assert!(plugin.process(&mut sample));
        assert_eq!(sample.x, 600.0);
    }
}
