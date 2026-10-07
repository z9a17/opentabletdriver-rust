//! Trusted native and OpenTabletDriver .NET position filters. No DLL is loaded
//! by parsing or saving a profile. Loading occurs only for explicit inspection
//! or when starting output. Native processing allocates no report storage;
//! managed filters receive independently owned snapshots for safe retention.
use otd_core::tablets::{Database, DeviceIdentifier, Role, TabletConfiguration};
use otd_plugin_api::{ABI_VERSION, FilterApi, Header, Sample};
use std::ffi::{OsStr, c_void};
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::time::Instant;
use windows_sys::Win32::Foundation::{FreeLibrary, HMODULE};
use windows_sys::Win32::System::LibraryLoader::{
    GetProcAddress, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW,
};

pub use otd_core::plugins::{PipelineStage, PluginConfig, PluginKind};
#[path = "plugins/graph.rs"]
mod graph;

/// Explicit startup only: load installed parser metadata for a named tablet
/// whose configuration has a parser missing from the native implementation.
/// Enumeration and ordinary native startup never initialize CLR here.
pub fn prepare_parser_registry(profile: &crate::config::Profile, database: &Database) -> Result<(), String> {
    let Some(name) = profile.tablet_name()? else { return Ok(()); };
    let configuration = database.entries().iter().filter_map(otd_core::tablets::Entry::usable)
        .find(|configuration| configuration.name == name);
    if let Some(configuration) = configuration {
        let missing = configuration.digitizer_identifiers.iter().chain(configuration.auxiliary_identifiers())
            .any(|identifier| otd_core::tablets::parser_support(identifier.parser()) == otd_core::tablets::ParserSupport::Missing
                && !crate::dotnet::installed_report_parser(identifier.parser()));
        if missing { load_parser_registry()?; }
    }
    Ok(())
}
pub fn load_parser_registry() -> Result<(), String> {
    if !crate::plugin_catalog::recover_installations()? { return Err("Plugin installation is busy; retry parser startup.".into()); }
    let directory = crate::plugin_catalog::plugins_directory()?;
    std::fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
    crate::dotnet::reload_installed_plugins(&directory)?;
    Ok(())
}

/// Explicit original-settings import. Pure native profiles do not load CLR;
/// unresolved managed stores use one retained installed-registry snapshot.
pub fn import_otd_with_installed(text: &str, source: &Path, connected: &[String]) -> Result<crate::config::Profile, String> {
    let document = crate::config::OtdSettingsDocument::from_json(text, source)?;
    let profiles = document.profiles();
    let selected = connected.iter().find_map(|name| profiles.iter().find(|profile| &profile.tablet == name))
        .or_else(|| profiles.iter().find(|profile| profile.tablet == "Wacom PTH-660"))
        .or_else(|| profiles.iter().find(|profile| profile.runtime_tablet_supported))
        .ok_or("OpenTabletDriver settings have no profile for a tablet this driver supports")?.index;
    import_otd_selected_with_installed(text, source, selected)
}
fn import_otd_selected_with_installed(text: &str, source: &Path, selected: usize) -> Result<crate::config::Profile, String> {
    let pure = crate::config::Profile::from_otd_profile_text(text, source, selected, Default::default());
    if pure.as_ref().is_ok_and(|profile| !profile.diagnostics.iter().any(|item| item.kind == "unsupported_active")) {
        return pure;
    }
    let original: serde_json::Value = serde_json::from_str(text).map_err(|error| error.to_string())?;
    let output = &original["Profiles"][selected]["OutputMode"];
    if pure.is_err() && (!output["Enable"].as_bool().unwrap_or(false)
        || matches!(output["Path"].as_str(), Some("OpenTabletDriver.Desktop.Output.AbsoluteMode"
            | "OpenTabletDriver.Desktop.Output.RelativeMode" | crate::config::WINDOWS_INK_ABSOLUTE_MODE
            | crate::config::WINDOWS_PEN_POINTER_MODE | crate::config::LINUX_ARTIST_MODE))) {
        return pure;
    }
    let registry = match crate::dotnet::registry_snapshot() {
        Some(snapshot) => snapshot,
        None => {
            let directory = crate::plugin_catalog::plugins_directory()?;
            std::fs::create_dir_all(&directory).map_err(|error| format!("cannot prepare plugin registry: {error}"))?;
            std::sync::Arc::new(crate::dotnet::reload_installed_plugins(&directory)?)
        }
    };
    let mut profile = match pure {
        Ok(profile) => profile,
        Err(original_error) => {
            let name = output["Path"].as_str();
            let mut matches = registry.plugins.iter().filter(|entry| entry.metadata.category == "output"
                && entry.metadata.supported && Some(entry.config.type_name.as_str()) == name);
            let Some(entry) = matches.next() else { return Err(original_error); };
            if matches.any(|other| other.config.path != entry.config.path) {
                return Err("managed output class exists in multiple installed DLLs; select one explicitly".into());
            }
            crate::config::Profile::from_managed_output_store(text, source, selected, Default::default(),
                entry.config.clone(), entry.metadata.relative_output)?
        }
    };
    resolve_imported_stores(&mut profile, &registry.plugins)?;
    Ok(profile)
}
pub fn load_original_profile(connected: &[String]) -> Result<crate::config::Profile, String> {
    let path = crate::config::otd_settings_path().ok_or("OpenTabletDriver settings path is unavailable")?;
    let text = std::fs::read_to_string(&path).map_err(|error| format!("cannot read OpenTabletDriver settings {}: {error}", path.display()))?;
    import_otd_with_installed(&text, &path, connected)
}
pub fn load_original_tablet_profile(tablet: &str) -> Result<Option<crate::config::Profile>, String> {
    let Some(path) = crate::config::otd_settings_path() else { return Ok(None); };
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("cannot read {}: {error}", path.display())),
    };
    let document = crate::config::OtdSettingsDocument::from_json(&text, &path)?;
    let Some(selected) = document.profiles().into_iter().find(|profile| profile.tablet == tablet) else { return Ok(None); };
    import_otd_selected_with_installed(&text, &path, selected.index).map(Some)
}

/// Verify an unchanged output class without constructing it, then import its
/// original property store and geometry. Runtime construction verifies again.
pub fn import_managed_output(text: &str, source: &Path, selected: usize, mut config: PluginConfig) -> Result<crate::config::Profile, String> {
    let inspected = crate::dotnet::inspect_details(&config.path)?.into_iter().find(|entry| entry.config.type_name == config.type_name)
        .ok_or("managed output type was not discovered in the selected DLL")?;
    config.path = inspected.config.path;
    let metadata = inspected.metadata;
    if metadata.category != "output" || !metadata.supported { return Err("selected type is not a supported unchanged IOutputMode".into()); }
    crate::config::Profile::from_managed_output_store(text, source, selected, otd_core::config::ImportOptions::default(), config, metadata.relative_output)
}

/// Resolve preserved stores only from already inspected installed DLL classes.
/// A duplicate class is an explicit ambiguity; unknown stores remain archived.
pub fn resolve_imported_bindings(profile: &mut crate::config::Profile, inspected: &[crate::dotnet::InspectedFilter]) -> Result<usize, String> {
    let Some(imported) = &profile.imported_otd else { return Ok(0); };
    let mut resolved = profile.clone();
    let source: serde_json::Value = serde_json::from_str(&imported.settings_json).map_err(|error| error.to_string())?;
    let bindings = &source["Profiles"][imported.selected_profile]["Bindings"];
    let selected = imported.selected_profile;
    let original_profile = profile;
    let profile = &mut resolved;
    let mut locations = std::collections::BTreeSet::new();
    let mut count = 0;
    let mut resolve = |store: &serde_json::Value| -> Result<Option<PluginConfig>, String> {
        let Some(type_name) = store["Path"].as_str() else { return Ok(None); };
        let mut matches = inspected.iter().filter(|entry| entry.metadata.category == "binding" && entry.metadata.supported && entry.config.type_name == type_name);
        let Some(entry) = matches.next() else { return Ok(None); };
        if matches.any(|other| other.config.path != entry.config.path) { return Err(format!("unchanged binding {type_name} exists in multiple DLLs; select one explicitly")); }
        let mut config = entry.config.clone(); config.enabled = store["Enable"].as_bool().unwrap_or(false);
        config.settings_json = crate::config::Profile::managed_store_settings(store)?; config.validate()?;
        count += 1; Ok(Some(config))
    };
    for (field, destination) in [("TipButton", &mut profile.managed_tip_binding), ("EraserButton", &mut profile.managed_eraser_binding)] {
        if let Some(config) = resolve(&bindings[field])? { *destination = Some(config); locations.insert(format!("Bindings.{}", if field == "TipButton" { "Tip" } else { "Eraser" })); }
    }
    if let Some(config) = &profile.managed_tip_binding { profile.contact.tip_enabled = config.enabled; }
    if let Some(config) = &profile.managed_eraser_binding { profile.contact.eraser_enabled = config.enabled; }
    for (field, destination) in [("PenButtons", &mut profile.pen_buttons), ("AuxButtons", &mut profile.aux_buttons), ("MouseButtons", &mut profile.mouse_buttons)] {
        for (index, store) in bindings[field].as_array().into_iter().flatten().take(64).enumerate() {
            if let Some(config) = resolve(store)? { if destination.len() <= index { destination.resize_with(index + 1, || otd_core::output::buttons::ButtonAction::None); } destination[index] = otd_core::output::buttons::ButtonAction::Managed(config); locations.insert(format!("Bindings.{field}[{index}]")); }
        }
    }
    for (field, destination) in [("MouseScrollUp", &mut profile.mouse_scroll_up), ("MouseScrollDown", &mut profile.mouse_scroll_down)] {
        if let Some(config) = resolve(&bindings[field])? { *destination = otd_core::output::buttons::ButtonAction::Managed(config); locations.insert(format!("Bindings.{field}")); }
    }
    for (index, wheel) in bindings["WheelBindings"].as_array().into_iter().flatten().take(otd_core::reports::MAX_WHEELS).enumerate() {
        let clockwise = resolve(&wheel["ClockwiseRotation"])?; let counter_clockwise = resolve(&wheel["CounterClockwiseRotation"])?;
        let buttons = wheel["WheelButtons"].as_array().into_iter().flatten().take(64).map(&mut resolve).collect::<Result<Vec<_>, _>>()?;
        if clockwise.is_some() || counter_clockwise.is_some() || buttons.iter().any(Option::is_some) {
            if profile.wheels.len() <= index { profile.wheels.resize_with(index + 1, Default::default); }
            let destination = &mut profile.wheels[index];
            if let Some(config) = clockwise { destination.clockwise = otd_core::output::buttons::ButtonAction::Managed(config); locations.insert(format!("Bindings.WheelBindings[{index}].ClockwiseRotation")); }
            if let Some(config) = counter_clockwise { destination.counter_clockwise = otd_core::output::buttons::ButtonAction::Managed(config); locations.insert(format!("Bindings.WheelBindings[{index}].CounterClockwiseRotation")); }
            for (button, config) in buttons.into_iter().enumerate() { if let Some(config) = config { if destination.buttons.len() <= button { destination.buttons.resize_with(button + 1, || otd_core::output::buttons::ButtonAction::None); } destination.buttons[button] = otd_core::output::buttons::ButtonAction::Managed(config); locations.insert(format!("Bindings.WheelBindings[{index}].WheelButtons[{button}]")); } }
        }
    }
    profile.diagnostics.retain(|diagnostic| !locations.contains(&diagnostic.location) && !locations.contains(diagnostic.location.strip_prefix(&format!("Profiles[{selected}].")).unwrap_or(&diagnostic.location)));
    *original_profile = resolved;
    Ok(count)
}

/// Resolve original filter/tool/binding stores against an actual loaded registry.
/// Unknown enabled stores remain rejected; this never saves or starts a profile.
pub fn resolve_imported_stores(profile: &mut crate::config::Profile, inspected: &[crate::dotnet::InspectedFilter]) -> Result<usize, String> {
    let Some(imported) = &profile.imported_otd else { return Ok(0); };
    let source: serde_json::Value = serde_json::from_str(&imported.settings_json).map_err(|error| error.to_string())?;
    let selected = imported.selected_profile;
    let mut next = profile.clone();
    let saved_tip = next.managed_tip_binding.clone(); let saved_eraser = next.managed_eraser_binding.clone();
    let saved_pen = next.pen_buttons.clone(); let saved_aux = next.aux_buttons.clone(); let saved_mouse = next.mouse_buttons.clone();
    let saved_up = next.mouse_scroll_up.clone(); let saved_down = next.mouse_scroll_down.clone(); let saved_wheels = next.wheels.clone();
    let mut count = resolve_imported_bindings(&mut next, inspected)?;
    // Original stores seed missing slots. Existing managed slots are current
    // user choices and retain their own settings through subsequent reloads.
    if let Some(config) = saved_tip { next.contact.tip_enabled = config.enabled; next.managed_tip_binding = Some(config); }
    if let Some(config) = saved_eraser { next.contact.eraser_enabled = config.enabled; next.managed_eraser_binding = Some(config); }
    let preserve = |current: &mut otd_core::output::buttons::ButtonAction, saved: &otd_core::output::buttons::ButtonAction| { if matches!(saved, otd_core::output::buttons::ButtonAction::Managed(_)) { *current = saved.clone(); } };
    for (current, saved) in next.pen_buttons.iter_mut().zip(&saved_pen).chain(next.aux_buttons.iter_mut().zip(&saved_aux)).chain(next.mouse_buttons.iter_mut().zip(&saved_mouse)) { preserve(current, saved); }
    preserve(&mut next.mouse_scroll_up, &saved_up); preserve(&mut next.mouse_scroll_down, &saved_down);
    for (current, saved) in next.wheels.iter_mut().zip(&saved_wheels) { preserve(&mut current.clockwise, &saved.clockwise); preserve(&mut current.counter_clockwise, &saved.counter_clockwise); for (current, saved) in current.buttons.iter_mut().zip(&saved.buttons) { preserve(current, saved); } }
    let mut entries = Vec::new();
    let mut unresolved_active_filter = false;
    let mut native_radial = false;
    let mut installed_radial = false;
    let mut other_active_filter = false;
    let mut resolved_tools = std::collections::BTreeSet::new();
    let resolve = |store: &serde_json::Value, category: &str| -> Result<Option<PluginConfig>, String> {
        let Some(name) = store["Path"].as_str() else { return Ok(None); };
        let mut matches = inspected.iter().filter(|entry| entry.config.type_name == name && entry.metadata.category == category && entry.metadata.supported);
        let Some(entry) = matches.next() else { return Ok(None); };
        if matches.any(|other| other.config.path != entry.config.path) { return Err(format!("Unchanged {category} {name} occurs in multiple installed DLLs")); }
        let mut config = entry.config.clone(); config.enabled = store["Enable"].as_bool().unwrap_or(false);
        config.settings_json = crate::config::Profile::managed_store_settings(store)?; config.validate()?; Ok(Some(config))
    };
    for store in source["Profiles"][selected]["Filters"].as_array().into_iter().flatten() {
        let enabled = store["Enable"].as_bool().unwrap_or(false);
        let radial = store["Path"].as_str() == Some(otd_core::radial_follow::FILTER_PATH);
        if let Some(config) = resolve(store, "filter")? {
            installed_radial |= radial && enabled;
            other_active_filter |= enabled && !radial;
            entries.push(config); count += 1;
        } else if radial && enabled && !next.radial_follow.is_empty() {
            // Existing explicit native port is retained only where its startup
            // stage preserves the original chain order. It is not a DLL claim.
            if other_active_filter { return Err("Cannot resolve filter order: install the original Radial Follow DLL or explicitly select a native-only chain".into()); }
            native_radial = true;
        } else if enabled { unresolved_active_filter = true; other_active_filter = true; }
    }
    if native_radial && installed_radial { return Err("Cannot mix imported native and installed unchanged Radial Follow instances".into()); }
    if installed_radial { next.radial_follow.clear(); }
    for (index, store) in source["Tools"].as_array().into_iter().flatten().enumerate() {
        if let Some(config) = resolve(store, "tool")? { entries.push(config); count += 1; resolved_tools.insert(format!("Tools[{index}]")); }
    }
    // A repeated registry load must retain current property/enabled edits. A
    // different chain cannot be reconciled with source slot identity silently.
    if !next.plugins.is_empty() {
        if next.plugins.len() != entries.len() || next.plugins.iter().zip(&entries).any(|(current, original)| current.kind != original.kind || current.path != original.path || current.type_name != original.type_name) {
            return Err("Existing DLL chain differs from the imported store order; keep it explicitly or reimport before resolving installed stores".into());
        }
    } else { next.plugins = entries; }
    next.diagnostics.retain(|diagnostic| !resolved_tools.contains(&diagnostic.location)
        && (unresolved_active_filter || diagnostic.location != format!("Profiles[{selected}].Filters"))
        && !(installed_radial && diagnostic.location.starts_with(&format!("Profiles[{selected}].Filters."))));
    next.validate_filter_execution()?; next.validate_actions()?;
    *profile = next; Ok(count)
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
    pub stage: PipelineStage,
    disabled: bool,
    managed: bool,
    // Destroy the context before unloading its callbacks.
    _library: Option<Library>,
}

impl Plugin {
    #[cfg(test)]
    pub fn load(config: &PluginConfig) -> Result<Self, String> {
        Self::load_with_tablet(config, current_tablet())
    }

    pub fn load_with_tablet(
        config: &PluginConfig,
        tablet: &TabletConfiguration,
    ) -> Result<Self, String> {
        Self::load_with_identifiers(config, tablet, None)
    }

    fn load_with_identifiers(config: &PluginConfig, tablet: &TabletConfiguration,
        identifiers: Option<&[DeviceIdentifier]>) -> Result<Self, String> {
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
                "settings": serde_json::from_str::<serde_json::Value>(&config.settings_json).map_err(|e| e.to_string())?,
                "tablet": tablet,
                "identifiers": identifiers
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
        let stage = if config.kind == PluginKind::Dotnet {
            match crate::dotnet::position(context) {
                Ok(stage) => stage,
                Err(error) => {
                    unsafe { (api.destroy.unwrap())(context) };
                    return Err(error);
                }
            }
        } else {
            PipelineStage::PreTransform
        };
        Ok(Self {
            api,
            context,
            name,
            stage,
            disabled: false,
            managed: config.kind == PluginKind::Dotnet,
            _library: library,
        })
    }

    pub fn process(&mut self, sample: &mut Sample) -> bool {
        if self.disabled {
            return true;
        }
        let original = *sample;
        let status = unsafe { (self.api.process.unwrap())(self.context, sample) };
        self.finish_process(sample, original, status)
    }

    fn process_report(
        &mut self,
        sample: &mut Sample,
        pen: crate::protocol::PenReport,
        raw: Option<&[u8]>,
    ) -> bool {
        if !self.managed {
            return self.process(sample);
        }
        if self.disabled {
            return true;
        }
        let Some(raw) = raw else {
            eprintln!(
                "Disabled managed plugin {}: missing or invalid prepared raw pen packet",
                self.name
            );
            self.disabled = true;
            return false;
        };
        let original = *sample;
        let status = crate::dotnet::process_report(self.context, sample, pen, raw);
        self.finish_process(sample, original, status)
    }

    fn finish_process(&mut self, sample: &mut Sample, original: Sample, status: i32) -> bool {
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

    fn reset_report(&mut self, raw: &[u8]) -> bool {
        if self.disabled {
            return true;
        }
        if !self.managed {
            self.reset();
            return true;
        }
        if crate::dotnet::reset_report(self.context, raw) != 0 {
            self.disabled = true;
            return false;
        }
        true
    }
}

/// The current driver selects the built-in USB PTH-660 configuration. Keep
/// this choice in Rust so future device selection can supply its own entry.
fn current_tablet() -> &'static TabletConfiguration {
    Database::builtin()
        .find(crate::hid::WACOM_VENDOR, crate::hid::PTH660_USB)
        .find(|entry| entry.role == Role::Digitizer && entry.configuration.name == "Wacom PTH-660")
        .expect("the pinned database declares the PTH-660")
        .configuration
}

impl Drop for Plugin {
    fn drop(&mut self) {
        unsafe { (self.api.destroy.unwrap())(self.context) };
    }
}

pub struct PluginChain {
    // Drop the graph's managed references before disposing the plugin handles.
    graph: Option<crate::dotnet::Graph>,
    managed_output: Option<crate::dotnet::endpoints::OutputSession>,
    plugins: Vec<Plugin>,
    /// Matched, actually opened endpoints; captured only at session setup.
    identifiers: Option<Vec<DeviceIdentifier>>,
    has_pre: bool,
    has_pixels: bool,
    /// Runs the host's built-in filters before `plugins[slot]` instead of
    /// before every plugin; see `Profile::builtin_filter_slot`.
    builtin_slot: Option<usize>,
    epoch: Instant,
    failure: Option<usize>,
}

/// The profile's enabled tools, running until dropped. As upstream's
/// DriverDaemon.SetToolSettings does, a tool that fails to start is
/// reported and skipped.
pub struct Tools(Vec<*mut std::ffi::c_void>);

impl Tools {
    pub fn start(configs: &[PluginConfig], log: impl Fn(&str)) -> Self {
        let mut running = Vec::new();
        for config in configs
            .iter()
            .filter(|config| config.enabled && config.kind == PluginKind::DotnetTool)
        {
            match crate::dotnet::create_tool(config) {
                Ok(handle) => {
                    log(&format!("Started tool {}.", config.type_name));
                    running.push(handle);
                }
                Err(error) => log(&format!(
                    "Failed to start tool {}: {error}",
                    config.type_name
                )),
            }
        }
        Self(running)
    }
}

impl Drop for Tools {
    fn drop(&mut self) {
        for handle in self.0.drain(..).rev() {
            crate::dotnet::destroy_tool(handle);
        }
    }
}

impl PluginChain {
    pub fn load(configs: &[PluginConfig]) -> Result<Self, String> {
        Self::load_with_tablet(configs, current_tablet())
    }

    pub fn load_with_tablet(
        configs: &[PluginConfig],
        tablet: &TabletConfiguration,
    ) -> Result<Self, String> {
        Self::load_with_builtins(configs, tablet, None, None)
    }

    /// Loads the profile's filters with its built-in filters at the slot
    /// `Profile::builtin_filter_slot` gives, as OpenTabletDriver orders them.
    pub fn load_for_profile(
        profile: &crate::config::Profile,
        tablet: &TabletConfiguration,
    ) -> Result<Self, String> {
        Self::load_profile(profile, tablet, None)
    }

    pub fn load_for_profile_with_identifiers(profile: &crate::config::Profile,
        tablet: &TabletConfiguration, identifiers: &[DeviceIdentifier]) -> Result<Self, String> {
        Self::load_profile(profile, tablet, Some(identifiers))
    }

    fn load_profile(profile: &crate::config::Profile, tablet: &TabletConfiguration,
        identifiers: Option<&[DeviceIdentifier]>) -> Result<Self, String> {
        if profile.managed_output.as_ref().is_some_and(|mode| !mode.enabled) { return Err("The selected original managed output mode is disabled; select an enabled output before starting".into()); }
        let mut chain = Self::load_with_builtins(&profile.plugins, tablet, profile.builtin_filter_slot(), identifiers)?;
        if !otd_core::output::buttons::ButtonOutput::managed_slots(profile).is_empty() { chain.prepare_managed_decoder()?; }

        if let Some(config) = profile.managed_output.as_ref().filter(|config| config.enabled) {
            let output = crate::dotnet::endpoints::OutputSession::new_with_identifiers(config, profile, tablet, identifiers)?;
            if chain.graph.is_none() { chain.graph = graph::create_if(&chain.plugins, chain.builtin_slot, true)?; }
            chain.graph.as_mut().ok_or("managed output graph was not created")?.attach_output(&output)?;
            chain.managed_output = Some(output);
        }
        Ok(chain)
    }

    fn load_with_builtins(
        configs: &[PluginConfig],
        tablet: &TabletConfiguration,
        builtin_slot: Option<usize>,
        identifiers: Option<&[DeviceIdentifier]>,
    ) -> Result<Self, String> {
        let plugins = configs
            .iter()
            .filter(|p| p.enabled && p.kind != PluginKind::DotnetTool)
            .map(|config| Plugin::load_with_identifiers(config, tablet, identifiers))
            .collect::<Result<Vec<_>, _>>()?;
        let has_pre = plugins
            .iter()
            .any(|p| p.stage == PipelineStage::PreTransform);
        let has_pixels = plugins.iter().any(|p| p.stage == PipelineStage::Pixels);
        // Slot 0 is the default order: the host runs them first.
        let builtin_slot = builtin_slot
            .filter(|&slot| slot > 0)
            .map(|slot| slot.min(plugins.len()));
        let graph = graph::create(&plugins, builtin_slot)?;
        Ok(Self {
            graph,
            managed_output: None,
            identifiers: identifiers.map(<[DeviceIdentifier]>::to_vec),
            plugins,
            has_pre,
            has_pixels,
            builtin_slot,
            epoch: Instant::now(),
            failure: None,
        })
    }

    /// A newly opened/reopened endpoint set may differ after auxiliary failure.
    /// Reconstruct at this cold boundary before creating any host output sink.
    pub fn bind_identifiers(&mut self, profile: &crate::config::Profile,
        tablet: &TabletConfiguration, identifiers: &[DeviceIdentifier]) -> Result<bool, String> {
        if self.identifiers.as_deref() == Some(identifiers) { return Ok(false); }
        let replacement = Self::load_for_profile_with_identifiers(profile, tablet, identifiers)?;
        replacement.validate_output_mode(profile.relative.is_some())?;
        *self = replacement;
        Ok(true)
    }

    /// Original report consumers require an original parser object, including
    /// concrete type identity. Pure native profiles have no managed graph.
    pub fn needs_concrete_reports(&self) -> bool { self.graph.is_some() }
    pub fn prepare_managed_decoder(&mut self) -> Result<(), String> {
        if self.graph.is_none() { self.graph = graph::create_if(&self.plugins, self.builtin_slot, true)?; }
        Ok(())
    }

    /// Binds to this graph's exact output instance; third-party pointers retain
    /// their own real services/prerequisites rather than a native substitute.
    pub fn wrap_action_sink(&self, profile: &crate::config::Profile, tablet: &TabletConfiguration,
        native: Box<dyn otd_core::output::buttons::ActionSink>) -> Result<Box<dyn otd_core::output::buttons::ActionSink>, String> {
        crate::dotnet::endpoints::wrap_sink_with_identifiers(profile, tablet, native, self.managed_output.as_ref(), self.identifiers.as_deref())
    }

    /// The filters in the order they run and the settings they run with, for
    /// the session log, as upstream logs each filter's settings at startup.
    pub fn describe(&self, profile: &crate::config::Profile) -> String {
        let builtins: Vec<String> = profile
            .radial_follow
            .iter()
            .map(|s| {
                format!(
                    "built-in Radial Follow (outer {} mm, inner {} mm, smoothing {}, soft knee {}, leak {})",
                    s.outer_radius,
                    s.inner_radius,
                    s.smoothing_coefficient,
                    s.soft_knee_scale,
                    s.smoothing_leak_coefficient
                )
            })
            .collect();
        let configs = profile
            .plugins
            .iter()
            .filter(|p| p.enabled && p.kind != PluginKind::DotnetTool);
        let slot = self.builtin_slot.unwrap_or(0);
        let (mut before, mut after) = (Vec::new(), Vec::new());
        for (index, (plugin, config)) in self.plugins.iter().zip(configs).enumerate() {
            if index == slot {
                before.extend(builtins.iter().cloned());
            }
            let entry = format!("{} {}", plugin.name, config.settings_json);
            match plugin.stage {
                PipelineStage::PreTransform => before.push(entry),
                PipelineStage::Pixels => after.push(entry),
            }
        }
        if slot >= self.plugins.len() {
            before.extend(builtins);
        }
        let list = |items: Vec<String>| {
            if items.is_empty() {
                "none".to_owned()
            } else {
                items.join(" -> ")
            }
        };
        format!(
            "Filters before mapping: {}; after mapping: {}",
            list(before),
            list(after)
        )
    }

    pub fn process_pre(
        &mut self,
        position: (f32, f32),
        pen: crate::protocol::PenReport,
        now: Instant,
    ) -> (f32, f32) {
        self.process_stage(PipelineStage::PreTransform, position, pen, now)
    }

    pub fn process_pixels(
        &mut self,
        position: (f32, f32),
        pen: crate::protocol::PenReport,
        now: Instant,
    ) -> (f32, f32) {
        self.process_stage(PipelineStage::Pixels, position, pen, now)
    }

    fn process_stage(
        &mut self,
        stage: PipelineStage,
        position: (f32, f32),
        pen: crate::protocol::PenReport,
        now: Instant,
    ) -> (f32, f32) {
        if self.plugins.is_empty() {
            return position;
        }
        let mut sample = Sample {
            x: position.0,
            y: position.1,
            time_ns: now
                .saturating_duration_since(self.epoch)
                .as_nanos()
                .min(u128::from(u64::MAX)) as u64,
            pressure: u32::from(pen.pressure),
            flags: otd_plugin_api::PROXIMITY
                | if pen.eraser {
                    otd_plugin_api::ERASER
                } else {
                    0
                },
        };
        for (index, plugin) in self.plugins.iter_mut().enumerate() {
            if plugin.stage != stage {
                continue;
            }
            if !plugin.process_report(&mut sample, pen, None) {
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
        // Explicit resets between reads have no transport packet. Physical
        // range-loss reports carry their canonical raw bytes through dispatch.
        for (index, plugin) in self.plugins.iter_mut().enumerate() {
            if !plugin.reset_report(&[]) {
                self.failure = Some(index);
            }
        }
    }
    pub fn validate_output_mode(&self, _relative: bool) -> Result<(), String> {
        // PostTransform sees desktop pixels in absolute mode and motion deltas
        // in relative mode, matching the pinned OutputMode pipeline stages.
        Ok(())
    }

    pub fn take_failure(&mut self) -> Option<&str> {
        self.failure
            .take()
            .map(|index| self.plugins[index].name.as_str())
    }
}

impl otd_core::plugins::Filters for PluginChain {
    fn owns_mapping(&self) -> bool { self.managed_output.is_some() }
    fn uses_managed_graph(&self) -> bool {
        self.graph.is_some()
    }
    fn dispatch(
        &mut self,
        input: otd_core::plugins::DispatchInput<'_>,
        runtime: &mut dyn otd_core::plugins::PipelineRuntime,
    ) -> std::io::Result<()> {
        self.dispatch_graph(input, runtime)
    }
    fn next_tick(&mut self) -> Option<std::time::Duration> {
        self.next_tick_graph()
    }
    fn tick(
        &mut self,
        now: Instant,
        runtime: &mut dyn otd_core::plugins::PipelineRuntime,
    ) -> std::io::Result<()> {
        self.tick_graph(now, runtime)
    }
    fn has_pre(&self) -> bool {
        self.has_pre
    }

    fn process_pre(
        &mut self,
        position: (f32, f32),
        pen: crate::protocol::PenReport,
        now: Instant,
    ) -> (f32, f32) {
        PluginChain::process_pre(self, position, pen, now)
    }

    fn has_pixels(&self) -> bool {
        self.has_pixels
    }

    fn process_pixels(
        &mut self,
        position: (f32, f32),
        pen: crate::protocol::PenReport,
        now: Instant,
    ) -> (f32, f32) {
        PluginChain::process_pixels(self, position, pen, now)
    }

    fn reset(&mut self) {
        PluginChain::reset(self);
    }

    fn take_failure(&mut self) -> Option<&str> {
        PluginChain::take_failure(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Profile;
    use crate::mapping::{Crop, Mapper, Rect};
    use crate::protocol::PenReport;
    use otd_core::pipeline::ReportPipeline;

    #[test]
    fn runs_raw_filters_before_mapping_and_pixel_filters_after_mapping() {
        #[derive(Default)]
        struct Seen {
            x: f32,
        }
        unsafe extern "C" fn pre(context: *mut c_void, sample: *mut Sample) -> i32 {
            unsafe {
                (*context.cast::<Seen>()).x = (*sample).x;
                (*sample).x += 200.0;
            }
            0
        }
        unsafe extern "C" fn pixels(context: *mut c_void, sample: *mut Sample) -> i32 {
            unsafe {
                (*context.cast::<Seen>()).x = (*sample).x;
                (*sample).x += 10.0;
            }
            0
        }
        unsafe extern "C" fn reset(_: *mut c_void) {}
        unsafe extern "C" fn destroy(_: *mut c_void) {}
        fn fake(
            stage: PipelineStage,
            context: *mut c_void,
            process: unsafe extern "C" fn(*mut c_void, *mut Sample) -> i32,
        ) -> Plugin {
            Plugin {
                api: FilterApi {
                    header: Header::V1,
                    name: [0; 64],
                    create: None,
                    process: Some(process),
                    reset: Some(reset),
                    destroy: Some(destroy),
                },
                context,
                name: "stage test".into(),
                stage,
                disabled: false,
                managed: false,
                _library: None,
            }
        }
        let mut seen_pre = Seen::default();
        let mut seen_pixels = Seen::default();
        // Config order may interleave stages; execution order is by stage.
        let mut chain = PluginChain {
            graph: None,
            managed_output: None,
            identifiers: None,
            plugins: vec![
                fake(
                    PipelineStage::Pixels,
                    (&mut seen_pixels as *mut Seen).cast(),
                    pixels,
                ),
                fake(
                    PipelineStage::PreTransform,
                    (&mut seen_pre as *mut Seen).cast(),
                    pre,
                ),
            ],
            has_pre: true,
            has_pixels: true,
            builtin_slot: None,
            epoch: Instant::now(),
            failure: None,
        };
        let desktop = Rect {
            left: -1920,
            top: 0,
            right: 1920,
            bottom: 1080,
        };
        let mapper = Mapper::new(Crop::default(), 0, desktop, desktop).unwrap();
        let pen = PenReport {
            id: 0x10,
            x: 22_400,
            y: 14_800,
            pressure: 0,
            in_range: true,
            sense: true,
            tip_switch: false,
            eraser: false,
            tilt: [0; 2],
            rotation: Some(0),
            hover_distance: Some(0),
        };
        let mut pipeline = ReportPipeline::new(&Profile::default()).unwrap();
        let mut sent = None;
        crate::test_alloc::assert_no_allocations(|| {
            assert!(
                pipeline
                    .process(pen, Instant::now(), Some(mapper), &mut chain, |packet| {
                        sent = Some(packet);
                        Ok(())
                    })
                    .unwrap()
            );
        });
        assert_eq!(seen_pre.x, 22_400.0);
        let mapped = mapper.map_filtered_pixels(22_600.0, 14_800.0).unwrap();
        assert!((seen_pixels.x - mapped.0 as f32).abs() < 0.001);
        let expected = mapper
            .normalize_pixels(f64::from(seen_pixels.x + 10.0), f64::from(mapped.1 as f32))
            .unwrap();
        assert_eq!((sent.unwrap().dx, sent.unwrap().dy), expected);
    }

    /// The built-in Radial Follow runs at its slot among the DLL filters: a
    /// move inside its dead zone reaches a filter before it unchanged, and a
    /// filter after it sees the held position.
    #[test]
    fn built_in_radial_follow_runs_at_its_slot() {
        unsafe extern "C" fn record(context: *mut c_void, sample: *mut Sample) -> i32 {
            unsafe { *context.cast::<f32>() = (*sample).x };
            0
        }
        unsafe extern "C" fn ignore(_: *mut c_void) {}
        let profile = Profile {
            radial_follow: vec![crate::radial_follow::RadialFollowSettings {
                outer_radius: 0.7039,
                inner_radius: 0.302,
                smoothing_coefficient: 0.302,
                soft_knee_scale: 0.603,
                smoothing_leak_coefficient: 0.201,
            }],
            ..Profile::default()
        };
        let desktop = Rect {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1080,
        };
        let mapper = Mapper::new(Crop::default(), 0, desktop, desktop).unwrap();
        let start = Instant::now();
        for (slot, expected) in [(None, 22_400.0), (Some(1), 22_440.0)] {
            let mut seen = 0f32;
            let mut chain = PluginChain {
                graph: None,
                managed_output: None,
                identifiers: None,
                plugins: vec![Plugin {
                    api: FilterApi {
                        header: Header::V1,
                        name: [0; 64],
                        create: None,
                        process: Some(record),
                        reset: Some(ignore),
                        destroy: Some(ignore),
                    },
                    context: (&mut seen as *mut f32).cast(),
                    name: "recorder".into(),
                    stage: PipelineStage::PreTransform,
                    disabled: false,
                    managed: false,
                    _library: None,
                }],
                has_pre: true,
                has_pixels: false,
                builtin_slot: slot,
                epoch: start,
                failure: None,
            };
            let mut pipeline = ReportPipeline::new(&profile).unwrap();
            for (step, x) in [22_400u32, 22_440].into_iter().enumerate() {
                let pen = PenReport {
                    id: 0x10,
                    x,
                    y: 14_800,
                    pressure: 0,
                    in_range: true,
                    sense: true,
                    tip_switch: false,
                    eraser: false,
                    tilt: [0; 2],
                    rotation: Some(0),
                    hover_distance: Some(0),
                };
                let now = start + std::time::Duration::from_millis(100 + 2 * step as u64);
                pipeline
                    .process(pen, now, Some(mapper), &mut chain, |_| Ok(()))
                    .unwrap();
            }
            // 40 units are 0.2 mm, inside the 0.302 mm dead zone.
            assert!((seen - expected).abs() < 0.01, "slot {slot:?}: {seen}");
            assert!(chain.describe(&profile).contains("Radial Follow"));
        }
    }

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
            stage: PipelineStage::PreTransform,
            disabled: false,
            managed: false,
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

#[cfg(test)]
mod identifier_setup_tests {
    use super::*;
    #[test]
    fn unchanged_opened_endpoint_membership_retains_the_chain() {
        let tablet = current_tablet();
        let identifiers = vec![tablet.digitizer_identifiers[0].clone()];
        let profile = crate::config::Profile::default();
        let mut chain = PluginChain::load_for_profile_with_identifiers(&profile, tablet, &identifiers).unwrap();
        let epoch = chain.epoch;
        let storage = chain.identifiers.as_ref().unwrap().as_ptr();
        for _ in 0..4 {
            assert!(!chain.bind_identifiers(&profile, tablet, &identifiers).unwrap());
            assert_eq!(chain.epoch, epoch);
            assert_eq!(chain.identifiers.as_ref().unwrap().as_ptr(), storage);
        }
        let mut with_auxiliary = identifiers.clone();
        with_auxiliary.push(DeviceIdentifier { product_id: Some(999), ..Default::default() });
        assert!(chain.bind_identifiers(&profile, tablet, &with_auxiliary).unwrap());
        assert_eq!(chain.identifiers.as_deref(), Some(with_auxiliary.as_slice()));
    }
}
