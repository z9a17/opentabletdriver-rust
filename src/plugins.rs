//! Trusted native and OpenTabletDriver .NET position filters. No DLL is loaded
//! by parsing or saving a profile. Loading occurs only for explicit inspection
//! or when starting output. Native processing allocates no report storage;
//! managed filters receive independently owned snapshots for safe retention.
use otd_core::tablets::{Database, Role, TabletConfiguration};
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
                "tablet": tablet
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
    plugins: Vec<Plugin>,
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
        Self::load_with_builtins(configs, tablet, None)
    }

    /// Loads the profile's filters with its built-in filters at the slot
    /// `Profile::builtin_filter_slot` gives, as OpenTabletDriver orders them.
    pub fn load_for_profile(
        profile: &crate::config::Profile,
        tablet: &TabletConfiguration,
    ) -> Result<Self, String> {
        Self::load_with_builtins(&profile.plugins, tablet, profile.builtin_filter_slot())
    }

    fn load_with_builtins(
        configs: &[PluginConfig],
        tablet: &TabletConfiguration,
        builtin_slot: Option<usize>,
    ) -> Result<Self, String> {
        let plugins = configs
            .iter()
            .filter(|p| p.enabled && p.kind != PluginKind::DotnetTool)
            .map(|config| Plugin::load_with_tablet(config, tablet))
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
            plugins,
            has_pre,
            has_pixels,
            builtin_slot,
            epoch: Instant::now(),
            failure: None,
        })
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
