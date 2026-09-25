//! On-demand CoreCLR hosting using Microsoft's nethost/hostfxr API. Rust-only
//! profiles never initialize .NET. Managed calls are direct function pointers;
//! there is no JSON serialization or interprocess message per pen report.
use crate::plugins::{Library, wide};
use otd_plugin_api::FilterApi;
use std::ffi::{OsStr, OsString, c_void};
use std::os::windows::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

type GetApi = unsafe extern "C" fn() -> *const FilterApi;
type GetPosition = unsafe extern "C" fn(*mut c_void) -> i32;
type Inspect = unsafe extern "C" fn(*const u8, i32, *mut u8, i32) -> i32;
type GetError = unsafe extern "C" fn(*mut u8, i32) -> i32;
struct Bridge {
    get_api: GetApi,
    get_position: GetPosition,
    inspect: Inspect,
    get_error: GetError,
}
static BRIDGE: OnceLock<Result<Bridge, String>> = OnceLock::new();

fn bridge_dir() -> Result<PathBuf, String> {
    if let Some(path) = std::env::var_os("OTD_COMPAT_DIR") {
        return Ok(path.into());
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let parent = exe.parent().ok_or("executable has no parent directory")?;
    Ok(parent.join("compat"))
}

fn load_bridge() -> Result<Bridge, String> {
    type GetHostPath = unsafe extern "system" fn(*mut u16, *mut usize, *const c_void) -> i32;
    type Initialize = unsafe extern "C" fn(*const u16, *const c_void, *mut *mut c_void) -> i32;
    type GetDelegate = unsafe extern "C" fn(*mut c_void, i32, *mut *mut c_void) -> i32;
    type Close = unsafe extern "C" fn(*mut c_void) -> i32;
    type LoadAssembly = unsafe extern "system" fn(
        *const u16,
        *const u16,
        *const u16,
        *const u16,
        *mut c_void,
        *mut *mut c_void,
    ) -> i32;
    let directory = bridge_dir()?;
    let nethost = Library::load(&directory.join("nethost.dll"))
        .map_err(|e| format!(".NET bridge is not installed beside the driver: {e}"))?;
    let get_path: GetHostPath =
        unsafe { std::mem::transmute(nethost.symbol(b"get_hostfxr_path\0")?) };
    let mut path = vec![0u16; 32_768];
    let mut size = path.len();
    if unsafe { get_path(path.as_mut_ptr(), &mut size, std::ptr::null()) } != 0 {
        return Err(
            ".NET hosting runtime was not found; install the x64 .NET 8 runtime or newer".into(),
        );
    }
    let length = path
        .iter()
        .position(|c| *c == 0)
        .ok_or("invalid hostfxr path")?;
    let host = Library::load(Path::new(&OsString::from_wide(&path[..length])))?;
    let initialize: Initialize =
        unsafe { std::mem::transmute(host.symbol(b"hostfxr_initialize_for_runtime_config\0")?) };
    let get_delegate: GetDelegate =
        unsafe { std::mem::transmute(host.symbol(b"hostfxr_get_runtime_delegate\0")?) };
    let close: Close = unsafe { std::mem::transmute(host.symbol(b"hostfxr_close\0")?) };
    let config = wide(directory.join("OtdCompat.runtimeconfig.json").as_os_str())?;
    let mut context = std::ptr::null_mut();
    let code = unsafe { initialize(config.as_ptr(), std::ptr::null(), &mut context) };
    if code < 0 || context.is_null() {
        return Err(format!(
            "cannot initialize .NET compatibility runtime (0x{code:08x})"
        ));
    }
    let mut delegate = std::ptr::null_mut();
    let code = unsafe { get_delegate(context, 5, &mut delegate) };
    unsafe { close(context) };
    if code != 0 || delegate.is_null() {
        return Err(format!("cannot get .NET assembly loader (0x{code:08x})"));
    }
    let load: LoadAssembly = unsafe { std::mem::transmute(delegate) };
    let assembly = wide(
        directory
            .join("OtdCompat.dll")
            .canonicalize()
            .map_err(|e| e.to_string())?
            .as_os_str(),
    )?;
    let type_name = wide(OsStr::new("OtdCompat.EntryPoints, OtdCompat"))?;
    let entry = |name: &str| -> Result<*mut c_void, String> {
        let method = wide(OsStr::new(name))?;
        let mut entry = std::ptr::null_mut();
        let code = unsafe {
            load(
                assembly.as_ptr(),
                type_name.as_ptr(),
                method.as_ptr(),
                usize::MAX as *const u16,
                std::ptr::null_mut(),
                &mut entry,
            )
        };
        if code != 0 || entry.is_null() {
            return Err(format!("cannot load .NET entry {name} (0x{code:08x})"));
        }
        Ok(entry)
    };
    let bridge = Bridge {
        get_api: unsafe { std::mem::transmute::<*mut c_void, GetApi>(entry("GetApi")?) },
        get_position: unsafe {
            std::mem::transmute::<*mut c_void, GetPosition>(entry("GetPosition")?)
        },
        inspect: unsafe { std::mem::transmute::<*mut c_void, Inspect>(entry("Inspect")?) },
        get_error: unsafe { std::mem::transmute::<*mut c_void, GetError>(entry("GetError")?) },
    };
    // The runtime and managed entry points live for this process. Retain the
    // hosting module as well; do not attempt to unload CoreCLR under plugins.
    std::mem::forget(host);
    Ok(bridge)
}

fn bridge() -> Result<&'static Bridge, String> {
    BRIDGE
        .get_or_init(load_bridge)
        .as_ref()
        .map_err(Clone::clone)
}

pub fn filter_api() -> Result<FilterApi, String> {
    let ptr = unsafe { (bridge()?.get_api)() };
    if ptr.is_null() {
        return Err(".NET bridge returned a null API".into());
    }
    Ok(unsafe { ptr.read() })
}

pub fn position(context: *mut c_void) -> Result<crate::plugins::PipelineStage, String> {
    match unsafe { (bridge()?.get_position)(context) } {
        1 => Ok(crate::plugins::PipelineStage::PreTransform),
        2 => Ok(crate::plugins::PipelineStage::Pixels),
        -1 => Err(last_error()),
        other => Err(format!("unsupported .NET pipeline position {other}")),
    }
}

pub fn last_error() -> String {
    let Ok(bridge) = bridge() else {
        return ".NET compatibility bridge unavailable".into();
    };
    let mut buffer = [0u8; 4096];
    let length = unsafe { (bridge.get_error)(buffer.as_mut_ptr(), buffer.len() as i32) };
    if length < 0 || length as usize > buffer.len() {
        return "Invalid .NET error response".into();
    }
    String::from_utf8_lossy(&buffer[..length as usize]).into_owned()
}

#[derive(Clone, Debug, serde::Deserialize)]
pub struct PropertyMetadata {
    pub name: String,
    pub display_name: Option<String>,
    pub unit: Option<String>,
    pub tooltip: Option<String>,
}

#[derive(Clone, Debug, serde::Deserialize)]
pub struct FilterMetadata {
    pub type_name: String,
    pub display_name: Option<String>,
    pub properties: Vec<PropertyMetadata>,
    /// Attribute defaults from inspection; omitted properties retain constructor defaults.
    pub default_settings_json: String,
}

pub struct InspectedFilter {
    pub config: crate::plugins::PluginConfig,
    pub metadata: FilterMetadata,
}

pub fn inspect_details(path: &Path) -> Result<Vec<InspectedFilter>, String> {
    let path = path.canonicalize().map_err(|e| e.to_string())?;
    let path_text = path.to_str().ok_or(".NET DLL path is not Unicode")?;
    let mut output = vec![0; 65_536];
    let size = loop {
        let size = unsafe {
            (bridge()?.inspect)(
                path_text.as_ptr(),
                path_text.len() as i32,
                output.as_mut_ptr(),
                output.len() as i32,
            )
        };
        if size < 0 {
            return Err(last_error());
        }
        let size = size as usize;
        if size <= output.len() {
            break size;
        }
        if size > 1_048_576 {
            return Err(".NET plugin metadata exceeds 1 MiB".into());
        }
        output.resize(size, 0);
    };
    #[derive(serde::Deserialize)]
    struct Entry {
        type_name: String,
        display_name: Option<String>,
        settings: serde_json::Value,
        properties: Vec<PropertyMetadata>,
    }
    let entries: Vec<Entry> = serde_json::from_slice(&output[..size]).map_err(|e| e.to_string())?;
    Ok(entries
        .into_iter()
        .map(|entry| InspectedFilter {
            config: crate::plugins::PluginConfig {
                path: path.clone(),
                kind: crate::plugins::PluginKind::Dotnet,
                enabled: false,
                type_name: entry.type_name.clone(),
                settings_json: entry.settings.to_string(),
            },
            metadata: FilterMetadata {
                type_name: entry.type_name,
                display_name: entry.display_name,
                properties: entry.properties,
                default_settings_json: entry.settings.to_string(),
            },
        })
        .collect())
}

pub fn inspect(path: &Path) -> Result<Vec<crate::plugins::PluginConfig>, String> {
    inspect_details(path).map(|entries| entries.into_iter().map(|entry| entry.config).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Profile;
    use crate::mapping::{Crop, Mapper, Rect};
    use crate::plugins::{PipelineStage, Plugin, PluginChain, PluginConfig, PluginKind};
    use crate::protocol::PenReport;
    use otd_core::pipeline::ReportPipeline;
    use otd_plugin_api::Sample;
    use std::time::{Duration, Instant};

    #[test]
    #[ignore = "requires bridge and SettingsFixture DLL; set OTD_COMPAT_DIR and OTD_TEST_SETTINGS_PLUGIN"]
    fn dotnet_settings_match_upstream_null_and_default_rules() {
        let path: PathBuf = std::env::var_os("OTD_TEST_SETTINGS_PLUGIN")
            .expect("set OTD_TEST_SETTINGS_PLUGIN")
            .into();
        let entries = inspect_details(&path).unwrap();
        let entry = entries
            .iter()
            .find(|entry| entry.config.type_name == "SettingsFixture.DefaultsFilter")
            .unwrap();
        let defaults: serde_json::Value =
            serde_json::from_str(&entry.config.settings_json).unwrap();
        assert_eq!(defaults, serde_json::json!({"AttributeOffset": 7.0}));
        assert_eq!(entry.metadata.properties.len(), 3);
        assert!(
            entry
                .metadata
                .properties
                .iter()
                .any(|property| property.name == "InheritedOffset")
        );

        // The fixture's OnDependencyLoad snapshots the three properties. The
        // emitted X therefore verifies settings were applied before that hook.
        for (settings, expected_x) in [
            ("{}", 19.5),
            (r#"{"AttributeOffset":null}"#, 24.5),
            (r#"{"ConstructorOffset":null}"#, 19.5),
            (r#"{"InheritedOffset":10.0}"#, 26.5),
            (r#"{"AttributeOffset":11.0}"#, 28.5),
        ] {
            let config = PluginConfig {
                path: path.clone(),
                kind: PluginKind::Dotnet,
                enabled: true,
                type_name: "SettingsFixture.DefaultsFilter".into(),
                settings_json: settings.into(),
            };
            let mut plugin = Plugin::load(&config).unwrap();
            let mut sample = Sample {
                x: 10.0,
                y: 20.0,
                ..Sample::default()
            };
            assert!(plugin.process(&mut sample), "settings: {settings}");
            assert_eq!(sample.x, expected_x, "settings: {settings}");
            assert_eq!(sample.y, 20.0);
        }
    }

    #[test]
    #[ignore = "requires bridge and SettingsFixture DLL; set OTD_COMPAT_DIR and OTD_TEST_SETTINGS_PLUGIN"]
    fn dotnet_tablet_reference_fields_and_properties_precede_load_callback() {
        let path: PathBuf = std::env::var_os("OTD_TEST_SETTINGS_PLUGIN")
            .expect("set OTD_TEST_SETTINGS_PLUGIN")
            .into();
        let config = PluginConfig {
            path,
            kind: PluginKind::Dotnet,
            enabled: true,
            type_name: "SettingsFixture.ReferenceFilter".into(),
            settings_json: "{}".into(),
        };
        let mut plugin = Plugin::load(&config).unwrap();
        let mut sample = Sample {
            x: 10.0,
            y: 20.0,
            ..Sample::default()
        };
        assert!(plugin.process(&mut sample));
        assert_eq!(sample.x, 17.0);
        assert_eq!(sample.y, 20.0);
    }

    #[test]
    #[ignore = "requires bridge and SettingsFixture DLL; set OTD_COMPAT_DIR and OTD_TEST_SETTINGS_PLUGIN"]
    fn dotnet_tablet_reference_uses_selected_database_specification() {
        let path: PathBuf = std::env::var_os("OTD_TEST_SETTINGS_PLUGIN")
            .expect("set OTD_TEST_SETTINGS_PLUGIN")
            .into();
        let tablet = otd_core::tablets::Database::builtin()
            .entries()
            .iter()
            .filter_map(|entry| entry.usable())
            .find(|tablet| tablet.name == "Wacom PTH-860")
            .unwrap();
        let config = PluginConfig {
            path,
            kind: PluginKind::Dotnet,
            enabled: true,
            type_name: "SettingsFixture.SelectedSpecificationFilter".into(),
            settings_json: "{}".into(),
        };
        let mut plugin = Plugin::load_with_tablet(&config, tablet).unwrap();
        let mut sample = Sample {
            x: 10.0,
            y: 20.0,
            ..Sample::default()
        };
        assert!(plugin.process(&mut sample));
        assert_eq!((sample.x, sample.y), (321.0, 20.0));
    }

    #[test]
    #[ignore = "requires bridge and DiscoveryFixture DLL; set OTD_COMPAT_DIR and OTD_TEST_DISCOVERY_PLUGIN"]
    fn dotnet_discovery_respects_platform_and_ignore_attributes() {
        let path: PathBuf = std::env::var_os("OTD_TEST_DISCOVERY_PLUGIN")
            .expect("set OTD_TEST_DISCOVERY_PLUGIN")
            .into();
        let names: std::collections::BTreeSet<_> = inspect_details(&path)
            .unwrap()
            .into_iter()
            .map(|entry| entry.config.type_name)
            .collect();
        assert_eq!(
            names,
            [
                "DiscoveryFixture.UnrestrictedFilter",
                "DiscoveryFixture.WindowsAndLinuxFilter",
                "DiscoveryFixture.WindowsFilter",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect()
        );

        let mut config = PluginConfig {
            path,
            kind: PluginKind::Dotnet,
            enabled: true,
            type_name: "DiscoveryFixture.UnrestrictedFilter".into(),
            settings_json: "{}".into(),
        };
        let mut plugin = Plugin::load(&config).unwrap();
        let mut sample = Sample {
            x: 42.0,
            y: 24.0,
            ..Sample::default()
        };
        assert!(plugin.process(&mut sample));
        assert_eq!((sample.x, sample.y), (42.0, 24.0));
        drop(plugin);

        for (name, diagnostic) in [
            ("LinuxFilter", "[SupportedPlatform]"),
            ("UnknownPlatformFilter", "[SupportedPlatform]"),
            ("IgnoredFilter", "[PluginIgnore]"),
        ] {
            config.type_name = format!("DiscoveryFixture.{name}");
            let error = Plugin::load(&config).err().expect("type must be rejected");
            assert!(error.contains(diagnostic), "{name}: {error}");
        }
    }

    #[test]
    #[ignore = "requires bridge and unchanged RadialFollow DLL; set OTD_COMPAT_DIR and OTD_TEST_DOTNET_PLUGIN"]
    fn dotnet_plugin_round_trip() {
        let path: PathBuf = std::env::var_os("OTD_TEST_DOTNET_PLUGIN")
            .expect("set OTD_TEST_DOTNET_PLUGIN")
            .into();
        let entries = inspect_details(&path).unwrap();
        assert!(
            entries
                .iter()
                .any(|e| e.config.type_name == "RadialFollow.RadialFollowSmoothingTabletSpace")
        );
        let screen = &entries
            .iter()
            .find(|entry| entry.config.type_name == "RadialFollow.RadialFollowSmoothingScreenSpace")
            .unwrap()
            .metadata;
        assert_eq!(
            screen.display_name.as_deref(),
            Some("AbstractQbit's Radial Follow Smoothing (Screen coordinates)")
        );
        let outer = screen
            .properties
            .iter()
            .find(|property| property.name == "OuterRadius")
            .unwrap();
        assert_eq!(outer.display_name.as_deref(), Some("Outer Radius"));
        assert_eq!(outer.unit.as_deref(), Some("px"));
        assert!(outer.tooltip.as_deref().unwrap().contains("pixels"));
        let tablet = &entries
            .iter()
            .find(|entry| entry.config.type_name == "RadialFollow.RadialFollowSmoothingTabletSpace")
            .unwrap()
            .metadata;
        assert_eq!(
            tablet
                .properties
                .iter()
                .find(|property| property.name == "OuterRadius")
                .unwrap()
                .unit
                .as_deref(),
            Some("mm")
        );
        let mut config = PluginConfig {
            path,
            kind: PluginKind::Dotnet,
            enabled: true,
            type_name: "RadialFollow.RadialFollowSmoothingTabletSpace".into(),
            settings_json: r#"{"InnerRadius":0.5,"OuterRadius":1.0}"#.into(),
        };
        let mut plugin = Plugin::load(&config).unwrap();
        assert_eq!(plugin.stage, PipelineStage::PreTransform);
        let mut sample = Sample {
            x: 20_000.0,
            y: 10_000.0,
            flags: 1,
            ..Sample::default()
        };
        // Let the original plugin's real 50 ms stopwatch reset to the baseline.
        std::thread::sleep(Duration::from_millis(55));
        assert!(plugin.process(&mut sample));
        assert_eq!((sample.x, sample.y), (20_000.0, 10_000.0));
        sample.x += 20.0; // 0.1 mm is inside its configured 0.5 mm dead zone.
        assert!(plugin.process(&mut sample));
        assert_eq!(sample.x, 20_000.0);
        plugin.reset();
        drop(plugin);
        config.type_name = "RadialFollow.RadialFollowSmoothingScreenSpace".into();
        config.settings_json = r#"{"InnerRadius":5.0,"OuterRadius":10.0}"#.into();
        let mut screen_plugin = Plugin::load(&config).unwrap();
        assert_eq!(screen_plugin.stage, PipelineStage::Pixels);
        let mut pixels = Sample {
            x: 100.0,
            y: 200.0,
            ..Sample::default()
        };
        std::thread::sleep(Duration::from_millis(55));
        assert!(screen_plugin.process(&mut pixels));
        assert_eq!((pixels.x, pixels.y), (100.0, 200.0));
        pixels.x += 2.0;
        assert!(screen_plugin.process(&mut pixels));
        assert_eq!(pixels.x, 100.0, "the screen-space dead zone is in pixels");
        screen_plugin.reset();
        drop(screen_plugin);

        let mut chain = PluginChain::load(&[config.clone()]).unwrap();
        assert!(chain.validate_output_mode(false).is_ok());
        assert!(chain.validate_output_mode(true).is_err());
        let desktop = Rect {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1080,
        };
        let mapper = Mapper::new(Crop::default(), 0, desktop, desktop).unwrap();
        let mut pipeline = ReportPipeline::new(&Profile::default()).unwrap();
        let mut pen = PenReport {
            id: 0x10,
            x: 20_000,
            y: 10_000,
            pressure: 0,
            in_range: true,
            sense: true,
            tip_switch: false,
            eraser: false,
            tilt: [0; 2],
            rotation: Some(0),
            hover_distance: Some(0),
        };
        std::thread::sleep(Duration::from_millis(55));
        let mut first = None;
        assert!(
            pipeline
                .process(pen, Instant::now(), Some(mapper), &mut chain, |packet| {
                    first = Some(packet);
                    Ok(())
                })
                .unwrap()
        );
        assert!(first.is_some());
        pen.x += 20; // Under one screen pixel, inside the configured 5 px dead zone.
        let mut second = None;
        assert!(
            !pipeline
                .process(pen, Instant::now(), Some(mapper), &mut chain, |packet| {
                    second = Some(packet);
                    Ok(())
                })
                .unwrap()
        );
        assert!(
            second.is_none(),
            "screen-space smoothing suppresses the move"
        );

        config.type_name = "RadialFollow.RadialFollowSmoothingTabletSpace".into();
        config.settings_json = r#"{"Typo":0.5}"#.into();
        assert!(Plugin::load(&config).is_err());
    }

    #[test]
    #[ignore = "manual release-mode benchmark; requires bridge and unchanged RadialFollow DLL; no HID or SendInput"]
    fn benchmark_dotnet_post_transform_pipeline() {
        use std::hint::black_box;

        let path: PathBuf = std::env::var_os("OTD_TEST_DOTNET_PLUGIN")
            .expect("set OTD_TEST_DOTNET_PLUGIN")
            .into();
        let config = PluginConfig {
            path,
            kind: PluginKind::Dotnet,
            enabled: true,
            type_name: "RadialFollow.RadialFollowSmoothingScreenSpace".into(),
            settings_json: r#"{"InnerRadius":5.0,"OuterRadius":10.0}"#.into(),
        };
        let desktop = Rect {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1080,
        };
        let mapper = Mapper::new(Crop::default(), 0, desktop, desktop).unwrap();
        let mut pen = PenReport {
            id: 0x10,
            x: 20_000,
            y: 10_000,
            pressure: 0,
            in_range: true,
            sense: true,
            tip_switch: false,
            eraser: false,
            tilt: [0; 2],
            rotation: Some(0),
            hover_distance: Some(0),
        };
        const REPORTS: u32 = 100_000;
        for with_plugin in [false, true] {
            let mut chain = PluginChain::load(if with_plugin {
                std::slice::from_ref(&config)
            } else {
                &[]
            })
            .unwrap();
            let mut pipeline = ReportPipeline::new(&Profile::default()).unwrap();
            let mut run = |reports: u32| {
                for index in 0..reports {
                    pen.x = 20_000 + index % 1_000;
                    pipeline
                        .process(pen, Instant::now(), Some(mapper), &mut chain, |packet| {
                            black_box(packet);
                            Ok(())
                        })
                        .unwrap();
                }
            };
            run(1_000);
            let start = Instant::now();
            run(REPORTS);
            println!(
                "absolute replay: pixel_plugin={with_plugin}, reports={REPORTS}, {:.1} ns/report; simulated output, no HID or SendInput",
                start.elapsed().as_nanos() as f64 / f64::from(REPORTS)
            );
        }
    }
}
