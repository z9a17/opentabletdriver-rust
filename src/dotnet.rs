//! On-demand CoreCLR hosting using Microsoft's nethost/hostfxr API. Rust-only
//! profiles never initialize .NET. Managed calls are direct function pointers;
//! there is no JSON serialization or interprocess message per pen report.
use crate::plugins::{Library, wide};
use otd_plugin_api::{FilterApi, Sample};
use std::ffi::{OsStr, OsString, c_void};
use std::os::windows::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

#[path = "dotnet/graph.rs"]
mod graph;
#[path = "dotnet/endpoints.rs"]
pub mod endpoints;
#[path = "dotnet/registry.rs"]
mod registry;
#[path = "dotnet/parser.rs"]
mod parser;
pub use parser::{RuntimeDecoder, ManagedReportParser, installed_report_parser};
pub use registry::{ManagedDebugDecoder, ManagedDebugReport, ManagedRegistryInfo, known_report_parser, registry_snapshot, reload_installed_plugins};
pub use graph::{Graph, GraphNode, GraphReport};

type GetApi = unsafe extern "C" fn() -> *const FilterApi;
type GetPosition = unsafe extern "C" fn(*mut c_void) -> i32;
type Inspect = unsafe extern "C" fn(*const u8, i32, *mut u8, i32) -> i32;
type GetError = unsafe extern "C" fn(*mut u8, i32) -> i32;
type ProcessReport = unsafe extern "C" fn(*mut c_void, *mut Sample, *const NativePenReport) -> i32;
type ResetReport = unsafe extern "C" fn(*mut c_void, *const u8, u32) -> i32;

/// Managed bridge extension only. Native filter ABI v1 remains unchanged.
/// Layout matches NativePenReport in compat/OtdCompat/EntryPoints.cs.
#[repr(C)]
struct NativePenReport {
    version: u32,
    size: u32,
    raw: *const u8,
    raw_length: u32,
    pen_buttons: u32,
    pen_button_count: u32,
    tilt_x: f32,
    tilt_y: f32,
    near_proximity: u32,
    hover_distance: u32,
    capabilities: u32,
}

struct Bridge {
    endpoints: Option<endpoints::Api>,
    registry: Option<registry::Api>,
    parser: Option<parser::Api>,
    get_api: GetApi,
    get_position: GetPosition,
    inspect: Inspect,
    get_error: GetError,
    process_report: ProcessReport,
    reset_report: ResetReport,
    create_graph: graph::CreateGraph,
    dispatch_graph: graph::DispatchGraph,
    graph_failure: graph::GraphFailure,
    destroy_graph: graph::DestroyGraph,
    graph_next_tick: Option<graph::GraphNextTick>,
    tick_graph: Option<graph::TickGraph>,
    /// Fused continuations (0.15.3): the host runs its built-in filters
    /// before dispatch, and transform plus output cross into Rust once.
    dispatch_graph2: Option<graph::DispatchGraph>,
    tick_graph2: Option<graph::TickGraph>,
    create_tool: Option<CreateTool>,
    destroy_tool: Option<DestroyTool>,
}
type CreateTool = unsafe extern "C" fn(*const u8, usize) -> *mut c_void;
type DestroyTool = unsafe extern "C" fn(*mut c_void);
static BRIDGE: OnceLock<Result<Bridge, String>> = OnceLock::new();

fn bridge_dir() -> Result<PathBuf, String> {
    if let Some(path) = std::env::var_os("OTD_COMPAT_DIR") {
        return Ok(path.into());
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let parent = exe.parent().ok_or("executable has no parent directory")?;
    Ok(packaged_bridge_dir(parent))
}

fn packaged_bridge_dir(parent: &Path) -> PathBuf {
    let bundled = parent.join("data/compat");
    if bundled.join("OtdCompat.runtimeconfig.json").is_file() {
        bundled
    } else {
        // Source builds and older portable installations keep this layout.
        parent.join("compat")
    }
}

#[cfg(test)]
mod package_tests {
    use super::*;

    #[test]
    fn bundled_bridge_takes_priority_over_legacy_and_source_layouts() {
        let root = std::env::temp_dir().join(format!(
            "otd-bridge-layout-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let legacy = root.join("compat");
        let bundled = root.join("data/compat");
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(legacy.join("OtdCompat.runtimeconfig.json"), b"{}").unwrap();
        assert_eq!(packaged_bridge_dir(&root), legacy);
        std::fs::create_dir_all(&bundled).unwrap();
        assert_eq!(packaged_bridge_dir(&root), legacy, "empty bundle is not selected");
        std::fs::write(bundled.join("OtdCompat.runtimeconfig.json"), b"{}").unwrap();
        assert_eq!(packaged_bridge_dir(&root), bundled, "old installed files cannot shadow the bundle");
        std::fs::remove_dir_all(root).unwrap();
    }
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
        endpoints: endpoints::Api::load(&entry).ok(),
        registry: registry::Api::load(&entry).ok(),
        parser: parser::Api::load(&entry).ok(),
        create_graph: unsafe {
            std::mem::transmute::<*mut c_void, graph::CreateGraph>(entry("CreateGraph").map_err(|error| format!("The installed .NET bridge lacks synchronous graph support. Replace the data/compat directory with this release's files: {error}"))?)
        },
        dispatch_graph: unsafe {
            std::mem::transmute::<*mut c_void, graph::DispatchGraph>(entry("DispatchGraph")?)
        },
        graph_failure: unsafe {
            std::mem::transmute::<*mut c_void, graph::GraphFailure>(entry("GraphFailure")?)
        },
        destroy_graph: unsafe {
            std::mem::transmute::<*mut c_void, graph::DestroyGraph>(entry("DestroyGraph")?)
        },
        // Bridges older than 0.14 lack timers; synchronous filters still run.
        graph_next_tick: entry("GraphNextTick").ok().map(|entry| unsafe {
            std::mem::transmute::<*mut c_void, graph::GraphNextTick>(entry)
        }),
        tick_graph: entry("TickGraph")
            .ok()
            .map(|entry| unsafe { std::mem::transmute::<*mut c_void, graph::TickGraph>(entry) }),
        // Bridges older than 0.15.3 take the three-call path.
        dispatch_graph2: entry("DispatchGraph2").ok().map(|entry| unsafe {
            std::mem::transmute::<*mut c_void, graph::DispatchGraph>(entry)
        }),
        tick_graph2: entry("TickGraph2")
            .ok()
            .map(|entry| unsafe { std::mem::transmute::<*mut c_void, graph::TickGraph>(entry) }),
        get_api: unsafe { std::mem::transmute::<*mut c_void, GetApi>(entry("GetApi")?) },
        get_position: unsafe {
            std::mem::transmute::<*mut c_void, GetPosition>(entry("GetPosition")?)
        },
        inspect: unsafe { std::mem::transmute::<*mut c_void, Inspect>(entry("Inspect")?) },
        // Bridges older than 0.13 lack tools; filters keep working with them.
        create_tool: entry("CreateTool")
            .ok()
            .map(|entry| unsafe { std::mem::transmute::<*mut c_void, CreateTool>(entry) }),
        destroy_tool: entry("DestroyTool")
            .ok()
            .map(|entry| unsafe { std::mem::transmute::<*mut c_void, DestroyTool>(entry) }),
        get_error: unsafe { std::mem::transmute::<*mut c_void, GetError>(entry("GetError")?) },
        process_report: unsafe {
            std::mem::transmute::<*mut c_void, ProcessReport>(entry("ProcessReport").map_err(|error|
                format!("The installed .NET bridge lacks owned raw-report support. Replace the data/compat directory with this release's files: {error}"))?)
        },
        reset_report: unsafe {
            std::mem::transmute::<*mut c_void, ResetReport>(entry("ResetReport").map_err(|error|
                format!("The installed .NET bridge lacks owned reset-report support. Replace the data/compat directory with this release's files: {error}"))?)
        },
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

/// Copies into fresh managed ownership during this synchronous call. The native
/// pointer is never retained by the bridge. The PTH adapter checks ID and length.
pub fn process_report(
    context: *mut c_void,
    sample: &mut Sample,
    pen: crate::protocol::PenReport,
    raw: &[u8],
) -> i32 {
    let (flags_at, button_count, minimum) = match pen.id {
        0x10 => (1, 2, 17),
        0x1e => (2, 3, 13),
        _ => return -1,
    };
    if raw.first() != Some(&pen.id) || raw.len() < minimum || raw.len() > 192 {
        return -1;
    }
    let report = NativePenReport {
        version: 1,
        size: std::mem::size_of::<NativePenReport>() as u32,
        raw: raw.as_ptr(),
        raw_length: raw.len() as u32,
        pen_buttons: u32::from(raw[flags_at] >> 1) & ((1 << button_count) - 1),
        pen_button_count: button_count,
        tilt_x: f32::from(pen.tilt[0]),
        tilt_y: f32::from(pen.tilt[1]),
        near_proximity: u32::from(pen.in_range),
        hover_distance: u32::from(pen.hover_distance.unwrap_or(0)),
        // IProximityReport requires both fields. Offset distance is unknown in
        // the checked native decoder; do not advertise an invented zero value.
        capabilities: u32::from(pen.hover_distance.is_some()),
    };
    let Ok(bridge) = bridge() else {
        return -1;
    };
    unsafe { (bridge.process_report)(context, sample, &report) }
}

pub fn reset_report(context: *mut c_void, raw: &[u8]) -> i32 {
    if raw.len() > 192 {
        return -1;
    }
    let Ok(bridge) = bridge() else {
        return -1;
    };
    unsafe { (bridge.reset_report)(context, raw.as_ptr(), raw.len() as u32) }
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

#[derive(Clone, Debug, Default, PartialEq, serde::Deserialize)]
pub struct PropertyMetadata {
    pub name: String,
    pub display_name: Option<String>,
    pub unit: Option<String>,
    pub tooltip: Option<String>,
    #[serde(default)]
    pub property_type: String,
    #[serde(default = "property_writable")]
    pub writable: bool,
    #[serde(default)]
    pub enum_flags: bool,
    #[serde(default)]
    pub enum_underlying_type: Option<String>,
    #[serde(default)]
    pub enum_choices: Vec<EnumChoice>,
    /// A `[PropertyValidated]` string's allowed values.
    #[serde(default)]
    pub valid_values: Option<Vec<String>>,
    #[serde(default)]
    pub slider: Option<Slider>,
    /// A `[BooleanProperty]`'s description.
    #[serde(default)]
    pub description: Option<String>,
    /// Known attribute/editor default; inspection never constructs the plugin.
    #[serde(default)]
    pub default_value: Option<serde_json::Value>,
    /// Explicit null selects only an attribute default, not a slider placeholder.
    #[serde(default)]
    pub default_is_attribute: bool,
}

/// `[SliderProperty]`: upstream shows the range as a tool tip.
#[derive(Clone, Debug, PartialEq, serde::Deserialize)]
pub struct Slider {
    pub min: f32,
    pub max: f32,
    pub default_value: f32,
}

fn property_writable() -> bool {
    true
}

#[derive(Clone, Debug, PartialEq, serde::Deserialize)]
pub struct EnumChoice {
    pub name: String,
    pub value: serde_json::Value,
}

#[derive(Clone, Debug, serde::Deserialize)]
pub struct FilterMetadata {
    #[serde(default = "filter_category")]
    pub category: String,
    #[serde(default = "property_writable")]
    pub supported: bool,
    #[serde(default)]
    pub absolute_output: bool,
    #[serde(default)]
    pub relative_output: bool,
    pub type_name: String,
    pub display_name: Option<String>,
    pub properties: Vec<PropertyMetadata>,
    /// Attribute defaults from inspection; omitted properties retain constructor defaults.
    pub default_settings_json: String,
}

fn filter_category() -> String { "filter".into() }
impl Default for FilterMetadata {
    fn default() -> Self { Self { category: filter_category(), supported: true, absolute_output: false, relative_output: false,
        type_name: String::new(), display_name: None, properties: Vec::new(), default_settings_json: "{}".into() } }
}

/// Starts an OpenTabletDriver tool: constructs it, applies its settings and
/// calls `Initialize`. Returns the handle `destroy_tool` disposes.
pub fn create_tool(config: &crate::plugins::PluginConfig) -> Result<*mut c_void, String> {
    let bridge = bridge()?;
    let create = bridge.create_tool.ok_or(
        "The installed .NET bridge lacks tool support. Replace the data/compat directory with this release's files.",
    )?;
    let settings = serde_json::json!({
        "assembly_path": config.path.canonicalize().map_err(|e| e.to_string())?,
        "type_name": config.type_name,
        "settings": serde_json::from_str::<serde_json::Value>(&config.settings_json).map_err(|e| e.to_string())?,
    })
    .to_string();
    let handle = unsafe { create(settings.as_ptr(), settings.len()) };
    if handle.is_null() {
        Err(last_error())
    } else {
        Ok(handle)
    }
}

pub fn destroy_tool(handle: *mut c_void) {
    if let Ok(Bridge {
        destroy_tool: Some(destroy),
        ..
    }) = bridge()
    {
        unsafe { destroy(handle) };
    }
}

#[derive(Clone, Debug)]
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
    inspect_metadata_bytes(&output[..size], &path)
}

fn inspect_metadata_bytes(bytes: &[u8], path: &Path) -> Result<Vec<InspectedFilter>, String> {
    #[derive(serde::Deserialize)]
    struct Entry {
        #[serde(default)]
        kind: Option<String>,
        #[serde(default = "property_writable")] supported: bool,
        #[serde(default)] absolute_output: bool,
        #[serde(default)] relative_output: bool,
        type_name: String,
        display_name: Option<String>,
        settings: serde_json::Value,
        properties: Vec<PropertyMetadata>,
    }
    let entries: Vec<Entry> = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    Ok(entries
        .into_iter()
        .map(|entry| InspectedFilter {
            config: crate::plugins::PluginConfig {
                path: path.to_owned(),
                kind: if entry.kind.as_deref() == Some("tool") {
                    crate::plugins::PluginKind::DotnetTool
                } else {
                    crate::plugins::PluginKind::Dotnet
                },
                enabled: false,
                type_name: entry.type_name.clone(),
                settings_json: entry.settings.to_string(),
            },
            metadata: FilterMetadata {
                category: entry.kind.unwrap_or_else(filter_category), supported: entry.supported,
                absolute_output: entry.absolute_output, relative_output: entry.relative_output,
                type_name: entry.type_name,
                display_name: entry.display_name,
                properties: entry.properties.into_iter().map(|mut property| {
                    property.default_value = entry.settings.get(&property.name).cloned();
                    property
                }).collect(),
                default_settings_json: entry.settings.to_string(),
            },
        })
        .collect())
}

pub fn inspect(path: &Path) -> Result<Vec<crate::plugins::PluginConfig>, String> {
    inspect_details(path).map(|entries| entries.into_iter().filter(|entry| entry.metadata.supported && matches!(entry.metadata.category.as_str(), "filter" | "tool")).map(|entry| entry.config).collect())
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

    // Existing synthetic pipeline fixtures now supply the raw context required
    // by managed dispatch, just as the transport session does.
    fn synthetic_pen_raw(pen: PenReport) -> [u8; 192] {
        let mut raw = [0u8; 192];
        raw[0] = pen.id;
        raw[1] = u8::from(pen.tip_switch)
            | (u8::from(pen.eraser) << 4)
            | (u8::from(pen.in_range) << 5)
            | (u8::from(pen.sense) << 6);
        raw[2..5].copy_from_slice(&pen.x.to_le_bytes()[..3]);
        raw[5..8].copy_from_slice(&pen.y.to_le_bytes()[..3]);
        raw[8..10].copy_from_slice(&pen.pressure.to_le_bytes());
        raw[10] = pen.tilt[0] as u8;
        raw[11] = pen.tilt[1] as u8;
        raw[12..14].copy_from_slice(&pen.rotation.unwrap_or(0).to_le_bytes());
        raw[16] = pen.hover_distance.unwrap_or(0);
        raw
    }

    #[test]
    #[ignore = "requires bridge and SettingsFixture DLL; set OTD_COMPAT_DIR and OTD_TEST_SETTINGS_PLUGIN"]
    fn dotnet_tools_start_with_settings_and_stop_on_drop() {
        let path: PathBuf = std::env::var_os("OTD_TEST_SETTINGS_PLUGIN")
            .expect("set OTD_TEST_SETTINGS_PLUGIN")
            .into();
        let entries = inspect_details(&path).unwrap();
        let entry = entries
            .iter()
            .find(|entry| entry.config.type_name == "SettingsFixture.MarkerTool")
            .unwrap();
        assert_eq!(entry.config.kind, PluginKind::DotnetTool);
        // Filters in the same assembly stay filters.
        assert!(
            entries
                .iter()
                .any(|entry| entry.config.kind == PluginKind::Dotnet)
        );

        let marker = std::env::temp_dir().join(format!("otd-tool-{}.txt", std::process::id()));
        let _ = std::fs::remove_file(&marker);
        let config = PluginConfig {
            path: path.clone(),
            kind: PluginKind::DotnetTool,
            enabled: true,
            type_name: "SettingsFixture.MarkerTool".into(),
            settings_json: serde_json::json!({ "MarkerPath": marker }).to_string(),
        };
        let lines = std::cell::RefCell::new(Vec::new());
        let tools = crate::plugins::Tools::start(std::slice::from_ref(&config), |line| {
            lines.borrow_mut().push(line.to_owned())
        });
        assert_eq!(std::fs::read_to_string(&marker).unwrap(), "started");
        drop(tools);
        assert_eq!(std::fs::read_to_string(&marker).unwrap(), "started stopped");
        std::fs::remove_file(&marker).unwrap();
        // A tool whose Initialize fails is reported and skipped, as upstream does.
        let failing = PluginConfig {
            settings_json: "{}".into(),
            ..config
        };
        let _none = crate::plugins::Tools::start(&[failing], |line| {
            lines.borrow_mut().push(line.to_owned())
        });
        let lines = lines.into_inner();
        assert_eq!(lines[0], "Started tool SettingsFixture.MarkerTool.");
        assert!(
            lines[1].starts_with("Failed to start tool SettingsFixture.MarkerTool:"),
            "{}",
            lines[1]
        );
    }

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
    fn dotnet_filters_run_on_tablets_without_the_intuos_layout() {
        let path: PathBuf = std::env::var_os("OTD_TEST_SETTINGS_PLUGIN")
            .expect("set OTD_TEST_SETTINGS_PLUGIN")
            .into();
        let tablet = otd_core::tablets::Database::builtin()
            .entries()
            .iter()
            .filter_map(|entry| entry.usable())
            .find(|tablet| tablet.name == "XP-Pen Deco 01 V2")
            .unwrap();
        let config = PluginConfig {
            path,
            kind: PluginKind::Dotnet,
            enabled: true,
            type_name: "SettingsFixture.DefaultsFilter".into(),
            settings_json: r#"{"InheritedOffset":5000.0}"#.into(),
        };
        let desktop = Rect {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1080,
        };
        let mapper = Mapper::new(Crop::default(), 0, desktop, desktop).unwrap();
        // An XP-Pen packet: report ID 0x07, not an IntuosV2 0x10/0x1e layout.
        let pen = PenReport {
            id: 0x07,
            x: 20_000,
            y: 10_000,
            pressure: 100,
            in_range: true,
            sense: true,
            tip_switch: true,
            eraser: false,
            tilt: [0; 2],
            rotation: None,
            hover_distance: None,
        };
        let raw = [0x07, 0xa1, 0x20, 0x4e, 0x10, 0x27, 0x64, 0x00, 0x00, 0x00];
        let buttons = otd_core::reports::Buttons::from_bits(0, 2).ok();
        let run = |chain: &mut PluginChain| {
            let mut pipeline = ReportPipeline::new(&Profile::default()).unwrap();
            let mut sent = Vec::new();
            pipeline
                .process_pen(
                    pen,
                    &raw,
                    buttons,
                    Instant::now(),
                    Some(mapper),
                    chain,
                    |packet| {
                        sent.push(packet);
                        Ok(())
                    },
                )
                .unwrap();
            assert_eq!(chain.take_failure(), None);
            sent
        };
        let filtered = run(&mut PluginChain::load_with_tablet(&[config], tablet).unwrap());
        let unfiltered = run(&mut PluginChain::load(&[]).unwrap());
        assert_eq!(filtered.len(), 1);
        // The fixture moves X by 5011.5 tablet units before mapping.
        // Unfiltered output keeps exact integer mapping; filtered output maps
        // floats, so Y may differ by rounding only.
        assert!(
            filtered[0].dx > unfiltered[0].dx + 100,
            "{filtered:?} {unfiltered:?}"
        );
        assert!(
            (filtered[0].dy - unfiltered[0].dy).abs() < 64,
            "{filtered:?} {unfiltered:?}"
        );
    }

    #[test]
    #[ignore = "requires bridge and SettingsFixture DLL; set OTD_COMPAT_DIR and OTD_TEST_SETTINGS_PLUGIN"]
    fn dotnet_inspection_reports_generated_control_attributes() {
        let path: PathBuf = std::env::var_os("OTD_TEST_SETTINGS_PLUGIN")
            .expect("set OTD_TEST_SETTINGS_PLUGIN")
            .into();
        let entries = inspect_details(&path).unwrap();
        let entry = entries
            .iter()
            .find(|entry| entry.config.type_name == "SettingsFixture.ControlsFilter")
            .unwrap();
        let property = |name: &str| {
            entry
                .metadata
                .properties
                .iter()
                .find(|property| property.name == name)
                .unwrap()
        };
        assert_eq!(
            property("Mode").valid_values.as_deref(),
            Some(&["Linear".to_owned(), "Smooth".to_owned()][..])
        );
        assert_eq!(
            property("Strength").slider,
            Some(Slider {
                min: 0.0,
                max: 2.0,
                default_value: 0.5
            })
        );
        assert_eq!(
            property("Snap").description.as_deref(),
            Some("Snap to the grid")
        );
        // A slider without DefaultPropertyValue saves its DefaultValue.
        let defaults: serde_json::Value =
            serde_json::from_str(&entry.config.settings_json).unwrap();
        assert_eq!(defaults, serde_json::json!({"Strength": 0.5}));
    }

    #[test]
    #[ignore = "requires bridge and SettingsFixture DLL; set OTD_COMPAT_DIR and OTD_TEST_SETTINGS_PLUGIN"]
    fn dotnet_timer_filters_emit_between_reports() {
        use otd_core::plugins::Filters;
        for (type_name, late) in [
            ("SettingsFixture.AsyncFixtureFilter", false),
            ("SettingsFixture.LateTimerFilter", true),
        ] {
            let path: PathBuf = std::env::var_os("OTD_TEST_SETTINGS_PLUGIN")
                .expect("set OTD_TEST_SETTINGS_PLUGIN")
                .into();
            let config = PluginConfig {
                path,
                kind: PluginKind::Dotnet,
                enabled: true,
                type_name: type_name.into(),
                settings_json: if late {
                    r#"{"Offset":1000.0}"#
                } else {
                    r#"{"Frequency":1000.0,"Offset":1000.0}"#
                }
                .into(),
            };
            let mut chain = PluginChain::load(&[config]).unwrap();
            // Frequency 1000 Hz: the Scheduler was injected before settings.
            if late {
                assert_eq!(chain.next_tick(), None, "timer starts on the first report");
            } else {
                let first = chain.next_tick().expect("the async filter has a timer");
                assert!(first <= Duration::from_millis(2), "{first:?}");
            }
            let desktop = Rect {
                left: 0,
                top: 0,
                right: 1920,
                bottom: 1080,
            };
            let mapper = Mapper::new(Crop::default(), 0, desktop, desktop).unwrap();
            let raw = [
                0x10, 0x60, 0x14, 0x56, 0x00, 0xa3, 0x16, 0x00, 0x00, 0x00, 0x07, 0x04, 0, 0, 0, 0,
                0x28,
            ];
            let pen = crate::protocol::parse(&raw).unwrap().unwrap();
            let mut pipeline = ReportPipeline::new(&Profile::default()).unwrap();
            let mut direct = Vec::new();
            pipeline
                .process_with_raw(
                    pen,
                    &raw,
                    Instant::now(),
                    Some(mapper),
                    &mut chain,
                    |packet| {
                        direct.push(packet);
                        Ok(())
                    },
                )
                .unwrap();
            assert!(
                direct.is_empty(),
                "the async filter holds reports for its timer"
            );
            let (mut reports, mut packets) = (0, Vec::new());
            let start = Instant::now();
            while start.elapsed() < Duration::from_millis(30) {
                if chain.next_tick() == Some(Duration::ZERO) {
                    let stats = pipeline
                        .process_tick(Instant::now(), Some(mapper), &mut chain, |packet| {
                            packets.push(packet);
                            Ok(())
                        })
                        .unwrap();
                    reports += stats.reports;
                } else {
                    std::thread::sleep(Duration::from_micros(200));
                }
            }
            assert_eq!(chain.take_failure(), None);
            assert!(reports >= 5, "only {reports} timer emissions in 30 ms");
            // The first tick moves the cursor; repeats at the same spot send nothing.
            assert_eq!(packets.len(), 1, "{packets:?}");
            let mut plain = ReportPipeline::new(&Profile::default()).unwrap();
            let mut unfiltered = Vec::new();
            plain
                .process_with_raw(
                    pen,
                    &raw,
                    Instant::now(),
                    Some(mapper),
                    &mut PluginChain::load(&[]).unwrap(),
                    |packet| {
                        unfiltered.push(packet);
                        Ok(())
                    },
                )
                .unwrap();
            assert!(
                packets[0].dx > unfiltered[0].dx,
                "{packets:?} {unfiltered:?}"
            );
        }
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
        assert!(chain.validate_output_mode(true).is_ok());
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
        let raw = synthetic_pen_raw(pen);
        assert!(
            pipeline
                .process_with_raw(
                    pen,
                    &raw,
                    Instant::now(),
                    Some(mapper),
                    &mut chain,
                    |packet| {
                        first = Some(packet);
                        Ok(())
                    }
                )
                .unwrap()
                .packets
                > 0
        );
        assert!(first.is_some());
        pen.x += 20; // Under one screen pixel, inside the configured 5 px dead zone.
        let mut second = None;
        let raw = synthetic_pen_raw(pen);
        assert!(
            pipeline
                .process_with_raw(
                    pen,
                    &raw,
                    Instant::now(),
                    Some(mapper),
                    &mut chain,
                    |packet| {
                        second = Some(packet);
                        Ok(())
                    }
                )
                .unwrap()
                .packets
                == 0
        );
        assert!(
            second.is_none(),
            "screen-space smoothing suppresses the move"
        );

        // Upstream ignores saved keys a plugin no longer declares.
        config.type_name = "RadialFollow.RadialFollowSmoothingTabletSpace".into();
        config.settings_json = r#"{"Typo":0.5}"#.into();
        assert!(Plugin::load(&config).is_ok());
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
                    let raw = synthetic_pen_raw(pen);
                    pipeline
                        .process_with_raw(
                            pen,
                            &raw,
                            Instant::now(),
                            Some(mapper),
                            &mut chain,
                            |packet| {
                                black_box(packet);
                                Ok(())
                            },
                        )
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
