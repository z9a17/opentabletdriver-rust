use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::keys;
use crate::mapping::{Crop, OtdArea, OtdMapping};
use crate::output::buttons::{ButtonAction, default_pen_buttons};
use crate::plugins::PluginConfig;
use crate::protocol::MAX_PRESSURE;
use crate::radial_follow::{FILTER_NAME, FILTER_PATH, RadialFollowSettings};
use crate::relative::RelativeSettings;
use crate::spec::TabletSpec;

mod schema;
pub use schema::{
    ImportOptions, ImportedOtdSettings, NamedProfile, NativeProfileCollection, OtdImportPreview,
    OtdProfileSummary, OtdSettingsDocument, PROFILE_SCHEMA_VERSION, ProfileDiagnostic,
};

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct ContactPolicy {
    pub tip_enabled: bool,
    pub eraser_enabled: bool,
    pub tip_threshold_raw: Option<u16>,
    pub eraser_threshold_raw: Option<u16>,
}

impl Default for ContactPolicy {
    fn default() -> Self {
        Self {
            tip_enabled: true,
            eraser_enabled: true,
            tip_threshold_raw: None,
            eraser_threshold_raw: None,
        }
    }
}

/// What the absolute output drives.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputKind {
    /// The mouse cursor, with the tip/eraser as the left button.
    #[default]
    Mouse,
    /// A pen device with pressure, tilt, eraser and hover: synthetic pointer
    /// injection (Windows Ink) on Windows, a virtual tablet on Linux.
    Pen,
}

impl OutputKind {
    fn is_mouse(&self) -> bool {
        *self == Self::Mouse
    }
}

/// The Windows Ink plugin's output modes and bindings. Importing them selects
/// the native pen output; the plugin DLL and its VMulti driver are not used.
/// <https://github.com/X9VoiD/VoiDPlugins/tree/a69fe346b27512a34bda5e4a9481795b1ce2264b/src/OutputMode/WindowsInk>
pub const WINDOWS_INK_ABSOLUTE_MODE: &str = "VoiDPlugins.OutputMode.WinInkAbsoluteMode";
pub const WINDOWS_INK_RELATIVE_MODE: &str = "VoiDPlugins.OutputMode.WinInkRelativeMode";
pub const WINDOWS_INK_BINDING: &str = "VoiDPlugins.OutputMode.WindowsInkButtonHandler";
/// The Windows Pen Pointer plugin's output mode, which injects a synthetic
/// pen as the native pen output does. It touches whenever pressure is above
/// zero, without tip or eraser bindings.
/// <https://github.com/Kuuuube/VoiDPlugins/tree/02c3ed3a54937e39157f984c42400a755b82eb9e/src/OutputMode/WindowsPenPointer>
pub const WINDOWS_PEN_POINTER_MODE: &str = "VoiDPlugins.OutputMode.WindowsPenPointerOutputMode";
/// OpenTabletDriver's Linux Artist Mode, a virtual pressure-sensitive tablet
/// that touches whenever pressure is above zero.
/// <https://github.com/OpenTabletDriver/OpenTabletDriver/blob/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.Desktop/Interop/Input/Absolute/EvdevVirtualTablet.cs>
pub const LINUX_ARTIST_MODE: &str = "OpenTabletDriver.Desktop.Output.LinuxArtistMode";
pub(crate) const ADAPTIVE_BINDING: &str = "OpenTabletDriver.Desktop.Binding.AdaptiveBinding";
const MOUSE_BINDING: &str = "OpenTabletDriver.Desktop.Binding.MouseBinding";
const KEY_BINDING: &str = "OpenTabletDriver.Desktop.Binding.KeyBinding";
const MULTI_KEY_BINDING: &str = "OpenTabletDriver.Desktop.Binding.MultiKeyBinding";
/// Most pen buttons a profile can bind; a report's button set holds 64.
pub const MAX_PEN_BUTTONS: usize = 64;

#[derive(Clone, Debug)]
pub struct Profile {
    pub schema_version: u32,
    pub settings_revision: u64,
    /// Original settings collection, retained without rewriting JSON values.
    pub imported_otd: Option<ImportedOtdSettings>,
    /// Unrecognized native fields are archived by their original JSON-pointer path.
    pub preserved_fields: std::collections::BTreeMap<String, toml::Value>,
    pub diagnostics: Vec<ProfileDiagnostic>,
    pub monitor: Option<usize>,
    pub crop: Crop,
    pub rotation: u16,
    pub device_path: Option<String>,
    pub otd_mapping: Option<OtdMapping>,
    pub relative: Option<RelativeSettings>,
    pub contact: ContactPolicy,
    /// What each pen side button does, by button index. OpenTabletDriver's
    /// defaults are barrel buttons 1, 2 and 3. Buttons past the end do nothing.
    pub pen_buttons: Vec<ButtonAction>,
    /// Absolute output only; relative output always moves the mouse.
    pub output: OutputKind,
    pub radial_follow: Vec<RadialFollowSettings>,
    pub plugins: Vec<PluginConfig>,
    pub auto_enabled_radial_follow: usize,
    pub ignored_filters: usize,
    pub source: String,
    /// The tablet a native profile is for, by configuration name, or `None`
    /// for whichever tablet is connected. Imported profiles name their
    /// tablet in the preserved OpenTabletDriver document instead.
    pub target_tablet: Option<String>,
    /// The tablet this profile drives. Not saved: an imported profile takes
    /// it from its tablet's configuration, and the runtime sets it for the
    /// device it selected (`for_tablet`).
    pub tablet: TabletSpec,
}

impl Default for Profile {
    fn default() -> Self {
        Self {
            schema_version: PROFILE_SCHEMA_VERSION,
            settings_revision: 0,
            imported_otd: None,
            preserved_fields: Default::default(),
            diagnostics: Vec::new(),
            monitor: None,
            crop: Crop::default(),
            rotation: 0,
            device_path: None,
            otd_mapping: None,
            relative: None,
            contact: ContactPolicy::default(),
            pen_buttons: default_pen_buttons(),
            output: OutputKind::Mouse,
            radial_follow: Vec::new(),
            plugins: Vec::new(),
            auto_enabled_radial_follow: 0,
            ignored_filters: 0,
            source: "built-in full-area defaults".into(),
            tablet: TabletSpec::PTH_660,
            target_tablet: None,
        }
    }
}

#[derive(Deserialize, Serialize, Default)]
#[serde(deny_unknown_fields)]
struct RawProfile {
    #[serde(default)]
    schema_version: u32,
    #[serde(default)]
    settings_revision: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    imported_otd: Option<ImportedOtdSettings>,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    preserved_fields: std::collections::BTreeMap<String, toml::Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    diagnostics: Vec<ProfileDiagnostic>,
    #[serde(skip_serializing_if = "Option::is_none")]
    monitor: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    rotation: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    crop: Option<RawCrop>,
    #[serde(skip_serializing_if = "Option::is_none")]
    device_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    relative: Option<RawRelative>,
    #[serde(skip_serializing_if = "Option::is_none")]
    absolute: Option<OtdMapping>,
    #[serde(default)]
    bindings: ContactPolicy,
    /// Pen button actions as text (see `ButtonAction`), absent for the defaults.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pen_buttons: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "OutputKind::is_mouse")]
    output: OutputKind,
    #[serde(default)]
    radial_follow: Vec<RadialFollowSettings>,
    #[serde(default)]
    plugins: Vec<PluginConfig>,
    /// The tablet a native profile is for, by configuration name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tablet: Option<String>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RawRelative {
    x_sensitivity: f64,
    y_sensitivity: f64,
    #[serde(default)]
    rotation: f64,
    reset_delay_ms: f64,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RawCrop {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct OtdSettings {
    profiles: Vec<serde_json::Value>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct OtdProfile {
    output_mode: OtdStore,
    absolute_mode_settings: Option<OtdAbsolute>,
    relative_mode_settings: Option<OtdRelative>,
    #[serde(default, deserialize_with = "default_on_null")]
    bindings: OtdBindings,
    #[serde(default, deserialize_with = "default_on_null")]
    filters: Vec<OtdStore>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct OtdAbsolute {
    display: OtdArea,
    tablet: OtdArea,
    enable_clipping: bool,
    enable_area_limiting: bool,
}

#[derive(Deserialize)]
struct OtdRelative {
    #[serde(rename = "XSensitivity")]
    x_sensitivity: f64,
    #[serde(rename = "YSensitivity")]
    y_sensitivity: f64,
    #[serde(rename = "RelativeRotation")]
    rotation: f64,
    #[serde(rename = "RelativeResetDelay")]
    reset_delay: String,
}

/// Newtonsoft serializes TimeSpan as [days.]hh:mm:ss[.fffffff]. Parse integer
/// ticks, so fractional milliseconds and the strict reset boundary survive.
fn parse_reset_delay(value: &str) -> Result<Duration, String> {
    fn parse(value: &str) -> Option<Duration> {
        fn number(value: &str) -> Option<u64> {
            if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            value.parse().ok()
        }
        let mut parts = value.split(':');
        let day_hours = parts.next()?;
        let minutes = number(parts.next()?)?;
        let seconds_fraction = parts.next()?;
        if parts.next().is_some() {
            return None;
        }
        let (days, hours) = match day_hours.split_once('.') {
            Some((days, hours)) => (number(days)?, number(hours)?),
            None => (0, number(day_hours)?),
        };
        let (seconds, nanos) = match seconds_fraction.split_once('.') {
            Some((seconds, fraction)) => {
                if fraction.len() > 7 {
                    return None;
                }
                let nanos = number(fraction)? * 10u64.pow(9 - fraction.len() as u32);
                (number(seconds)?, nanos as u32)
            }
            None => (number(seconds_fraction)?, 0),
        };
        if hours >= 24 || minutes >= 60 || seconds >= 60 {
            return None;
        }
        let total = days
            .checked_mul(86_400)?
            .checked_add(hours * 3_600 + minutes * 60 + seconds)?;
        let ticks = total
            .checked_mul(10_000_000)?
            .checked_add(u64::from(nanos / 100))?;
        if ticks > i64::MAX as u64 {
            return None;
        }
        Some(Duration::new(total, nanos))
    }
    parse(value).ok_or_else(|| {
        "RelativeResetDelay must be a nonnegative TimeSpan: [days.]hh:mm:ss[.fffffff]".into()
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct OtdBindings {
    #[serde(default = "default_activation_percent")]
    tip_activation_threshold: f64,
    tip_button: Option<OtdStore>,
    #[serde(default = "default_activation_percent")]
    eraser_activation_threshold: f64,
    eraser_button: Option<OtdStore>,
    /// Kept as JSON so one odd entry cannot fail the whole import.
    #[serde(default)]
    pen_buttons: serde_json::Value,
}

fn default_activation_percent() -> f64 {
    1.0
}

impl Default for OtdBindings {
    fn default() -> Self {
        Self {
            tip_activation_threshold: 1.0,
            tip_button: None,
            eraser_activation_threshold: 1.0,
            eraser_button: None,
            pen_buttons: serde_json::Value::Null,
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct OtdStore {
    #[serde(default, deserialize_with = "default_on_null")]
    path: String,
    #[serde(default)]
    enable: bool,
    #[serde(default, deserialize_with = "default_on_null")]
    settings: Vec<OtdProperty>,
}

fn default_on_null<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct OtdProperty {
    property: String,
    #[serde(default)]
    value: serde_json::Value,
}

/// Whether an OTD tip/eraser binding store means contact. `expected` is the
/// mouse binding's action, `Tip` or `Eraser`. With a Windows Ink output mode,
/// that plugin's `Pen Tip` binding also means contact; the plugin presses its
/// eraser bit instead when the eraser is in range.
fn binding_enabled(store: Option<&OtdStore>, expected: &str, pen: bool) -> Result<bool, String> {
    let Some(store) = store else {
        return Ok(false);
    };
    if !store.enable {
        return Ok(false);
    }
    let property = |name: &str| {
        store
            .settings
            .iter()
            .rev()
            .find(|setting| setting.property == name)
            .and_then(|setting| setting.value.as_str())
    };
    match store.path.as_str() {
        ADAPTIVE_BINDING => {
            let selected = property("Binding");
            if selected != Some(expected) {
                return Err(format!(
                    "unsupported enabled {expected} binding action: {selected:?}"
                ));
            }
            Ok(true)
        }
        WINDOWS_INK_BINDING if !pen => Err(format!(
            "the {expected} binding is a Windows Ink binding, which needs a Windows Ink output mode"
        )),
        WINDOWS_INK_BINDING => match property("Button") {
            Some("Pen Tip") => Ok(true),
            button => Err(format!(
                "unsupported Windows Ink {expected} binding: {button:?}; only Pen Tip is applied"
            )),
        },
        path => Err(format!("unsupported enabled {expected} binding: {path}")),
    }
}

/// One enabled OTD pen-button store as a button action. A disabled or missing
/// store does nothing, as upstream's `BindingHandler` skips null bindings.
/// `pen` says the output is a pen device, where `Button n` is a barrel button.
fn pen_button_action(store: Option<&OtdStore>, pen: bool) -> Result<ButtonAction, String> {
    let Some(store) = store.filter(|store| store.enable) else {
        return Ok(ButtonAction::None);
    };
    let property = |name: &str| {
        store
            .settings
            .iter()
            .rev()
            .find(|setting| setting.property == name)
            .and_then(|setting| setting.value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
    };
    match store.path.as_str() {
        ADAPTIVE_BINDING => match property("Binding") {
            Some("Button 1") => Ok(ButtonAction::Barrel(1)),
            Some("Button 2") => Ok(ButtonAction::Barrel(2)),
            Some("Button 3") => Ok(ButtonAction::Barrel(3)),
            // On a mouse both are the left button; a pen device touches by
            // pressure, which a side button cannot express.
            Some("Tip" | "Eraser") if !pen => {
                Ok(ButtonAction::Mouse(crate::actions::MouseButton::Left))
            }
            other => Err(format!("unsupported Adaptive Binding action: {other:?}")),
        },
        MOUSE_BINDING => match property("Button") {
            // Upstream parses the name ignoring case; an unknown one does nothing.
            None => Ok(ButtonAction::None),
            Some(name) if name.eq_ignore_ascii_case("none") => Ok(ButtonAction::None),
            Some(name) => format!("mouse:{name}").parse(),
        },
        KEY_BINDING => match property("Key") {
            None | Some("None") => Ok(ButtonAction::None),
            Some(name) => keys::usage_from_name(name)
                .map(|key| ButtonAction::Keys(vec![key]))
                .ok_or_else(|| format!("unsupported key {name:?}")),
        },
        MULTI_KEY_BINDING => match property("Keys") {
            None => Ok(ButtonAction::None),
            Some(names) => keys::parse_chord(names).map(ButtonAction::Keys),
        },
        path => Err(format!("unsupported pen button binding: {path}")),
    }
}

/// The pen button actions of an OTD profile's `Bindings.PenButtons`. Entries
/// this driver cannot carry out become diagnostics and do nothing.
fn import_pen_buttons(
    value: &serde_json::Value,
    pen: bool,
    diagnostics: &mut Vec<ProfileDiagnostic>,
) -> Vec<ButtonAction> {
    let Some(entries) = value.as_array() else {
        return Vec::new();
    };
    let mut actions = Vec::with_capacity(entries.len().min(MAX_PEN_BUTTONS));
    for (index, entry) in entries.iter().take(MAX_PEN_BUTTONS).enumerate() {
        let action = if entry.is_null() {
            Ok(ButtonAction::None)
        } else {
            serde_json::from_value::<OtdStore>(entry.clone())
                .map_err(|error| format!("unreadable pen button binding: {error}"))
                .and_then(|store| pen_button_action(Some(&store), pen))
        };
        actions.push(action.unwrap_or_else(|message| {
            diagnostics.push(ProfileDiagnostic::unsupported(
                format!("Bindings.PenButtons[{index}]"),
                format!("{message}; this pen button does nothing."),
            ));
            ButtonAction::None
        }));
    }
    if entries.len() > MAX_PEN_BUTTONS {
        diagnostics.push(ProfileDiagnostic::unsupported(
            "Bindings.PenButtons",
            format!("Only the first {MAX_PEN_BUTTONS} pen buttons are applied."),
        ));
    }
    actions
}

fn radial_property(store: &OtdStore, name: &str, default: f64) -> Result<f64, String> {
    let Some(value) = store
        .settings
        .iter()
        .rev()
        .find(|setting| setting.property == name)
        .map(|setting| &setting.value)
    else {
        return Ok(default);
    };
    if value.is_null() {
        return Ok(default);
    }
    value
        .as_f64()
        .filter(|value| value.is_finite())
        .ok_or_else(|| format!("{FILTER_NAME} setting {name} must be a finite number"))
}

fn radial_settings(store: &OtdStore) -> Result<RadialFollowSettings, String> {
    let defaults = RadialFollowSettings::default();
    Ok(RadialFollowSettings {
        outer_radius: radial_property(store, "OuterRadius", defaults.outer_radius)?,
        inner_radius: radial_property(store, "InnerRadius", defaults.inner_radius)?,
        smoothing_coefficient: radial_property(
            store,
            "SmoothingCoefficient",
            defaults.smoothing_coefficient,
        )?,
        soft_knee_scale: radial_property(store, "SoftKneeScale", defaults.soft_knee_scale)?,
        smoothing_leak_coefficient: radial_property(
            store,
            "SmoothingLeakCoefficient",
            defaults.smoothing_leak_coefficient,
        )?,
    }
    .clamped())
}

/// The specification of a named tablet in the built-in database, or the
/// PTH-660's when the name is unknown.
pub fn spec_for_tablet(name: &str) -> TabletSpec {
    runtime_tablet(name).unwrap_or(TabletSpec::PTH_660)
}

/// The specification of a named tablet whose parser the runtime supports.
pub fn runtime_tablet(name: &str) -> Result<TabletSpec, String> {
    use crate::tablets::{Database, Entry, ParserSupport, parser_support};
    let configuration = Database::builtin()
        .entries()
        .iter()
        .filter_map(Entry::usable)
        .find(|configuration| configuration.name == name)
        .ok_or_else(|| format!("no tablet configuration is named {name}"))?;
    if let Some(identifier) = configuration
        .digitizer_identifiers
        .iter()
        .find(|identifier| parser_support(identifier.parser()) == ParserSupport::Missing)
    {
        return Err(format!(
            "{name} uses {}, which this driver cannot decode",
            identifier.parser()
        ));
    }
    TabletSpec::from_configuration(configuration)
}

/// First raw pressure at which OpenTabletDriver's tip or eraser binding
/// presses. Its `ThresholdBindingState` compares
/// `pressure / MaxPressure * 100 > threshold` in single precision and treats
/// a 100 % threshold as met at full pressure.
pub fn activation_raw(percent: f64) -> Result<u16, String> {
    activation_raw_for(percent, MAX_PRESSURE)
}

/// `activation_raw` for a tablet with another pressure range.
pub fn activation_raw_for(percent: f64, max_pressure: u16) -> Result<u16, String> {
    if !percent.is_finite() || !(0.0..=100.0).contains(&percent) {
        return Err("tip/eraser activation threshold must be 0..100 percent".into());
    }
    let threshold = percent as f32;
    let presses = |raw: u16| {
        let value = f32::from(raw) / f32::from(max_pressure) * 100.0;
        value > threshold || (threshold == 100.0 && value == 100.0)
    };
    // `presses` is monotonic and true at full pressure.
    let (mut low, mut high) = (0, max_pressure);
    while low < high {
        let middle = low + (high - low) / 2;
        if presses(middle) {
            high = middle;
        } else {
            low = middle + 1;
        }
    }
    Ok(low)
}

/// OpenTabletDriver's settings file in its default app data directory, as
/// upstream `AppInfo` places it on each platform.
fn otd_settings_path() -> Option<PathBuf> {
    let directory = if cfg!(target_os = "windows") {
        PathBuf::from(env::var_os("LOCALAPPDATA")?)
    } else if cfg!(target_os = "macos") {
        PathBuf::from(env::var_os("HOME")?).join("Library/Application Support")
    } else {
        env::var_os("XDG_CONFIG_HOME")
            .filter(|path| !path.is_empty())
            .map(PathBuf::from)
            .or_else(|| Some(PathBuf::from(env::var_os("HOME")?).join(".config")))?
    };
    Some(directory.join("OpenTabletDriver").join("settings.json"))
}

impl Profile {
    pub fn load(path: Option<&Path>) -> Result<Self, String> {
        Self::load_connected(path, &[])
    }

    /// `load`, importing OpenTabletDriver's profile for one of the connected
    /// tablets (configuration names, in preference order) when no path is given.
    pub fn load_connected(path: Option<&Path>, connected: &[String]) -> Result<Self, String> {
        if let Some(path) = path {
            return Self::load_toml(path);
        }
        let Some(otd_path) = otd_settings_path() else {
            return Ok(Self::default());
        };
        if otd_path.exists() {
            let text = fs::read_to_string(&otd_path).map_err(|e| {
                format!(
                    "cannot read OpenTabletDriver settings {}: {e}",
                    otd_path.display()
                )
            })?;
            Self::from_otd_text_connected(&text, &otd_path, connected, ImportOptions::default())
        } else {
            Ok(Self::default())
        }
    }

    pub fn load_otd(path: &Path) -> Result<Self, String> {
        Self::load_otd_with_options(path, ImportOptions::default())
    }

    pub fn load_otd_with_options(path: &Path, options: ImportOptions) -> Result<Self, String> {
        let text = fs::read_to_string(path).map_err(|e| {
            format!(
                "cannot read OpenTabletDriver settings {}: {e}",
                path.display()
            )
        })?;
        Self::from_otd_text_with_options(&text, path, options)
    }

    pub fn from_otd_text(text: &str, path: &Path) -> Result<Self, String> {
        Self::from_otd_text_with_options(text, path, ImportOptions::default())
    }

    pub fn from_otd_text_with_options(
        text: &str,
        path: &Path,
        options: ImportOptions,
    ) -> Result<Self, String> {
        Self::from_otd_text_connected(text, path, &[], options)
    }

    /// Imports the profile of the first connected tablet that has one, else
    /// the PTH-660's, else the first profile whose tablet this driver runs.
    pub fn from_otd_text_connected(
        text: &str,
        path: &Path,
        connected: &[String],
        options: ImportOptions,
    ) -> Result<Self, String> {
        let settings: OtdSettings = serde_json::from_str(text)
            .map_err(|e| format!("invalid OpenTabletDriver settings {}: {e}", path.display()))?;
        let tablets: Vec<&str> = settings
            .profiles
            .iter()
            .map(|profile| {
                profile
                    .get("Tablet")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("")
            })
            .collect();
        let selected_index = connected
            .iter()
            .find_map(|name| tablets.iter().position(|tablet| tablet == name))
            .or_else(|| tablets.iter().position(|tablet| *tablet == "Wacom PTH-660"))
            .or_else(|| {
                tablets
                    .iter()
                    .position(|tablet| runtime_tablet(tablet).is_ok())
            })
            .ok_or("OpenTabletDriver settings have no profile for a tablet this driver supports")?;
        Self::from_otd_profile_text(text, path, selected_index, options)
    }

    /// Import a chosen profile without claiming that its tablet is supported by
    /// the active device backend. Call validate_runtime_tablet before execution.
    pub fn from_otd_profile_text(
        text: &str,
        path: &Path,
        selected_index: usize,
        options: ImportOptions,
    ) -> Result<Self, String> {
        let settings: OtdSettings = serde_json::from_str(text)
            .map_err(|e| format!("invalid OpenTabletDriver settings {}: {e}", path.display()))?;
        let selected_value = settings
            .profiles
            .get(selected_index)
            .ok_or_else(|| format!("OTD profile index {selected_index} does not exist"))?;
        let tablet_name = selected_value
            .get("Tablet")
            .and_then(serde_json::Value::as_str)
            .filter(|name| !name.is_empty())
            .ok_or("selected OTD profile has no tablet name")?;
        let selected: OtdProfile = serde_json::from_value(selected_value.clone())
            .map_err(|e| format!("invalid {tablet_name} profile: {e}"))?;
        // Thresholds and relative scaling depend on the tablet's ranges.
        let tablet = spec_for_tablet(tablet_name);
        let mut diagnostics = schema::import_diagnostics(text, selected_index)?;
        if !selected.output_mode.enable {
            return Err(format!(
                "the {tablet_name} output mode is disabled; its settings remain available in the import preview"
            ));
        }
        let artist = selected.output_mode.path == LINUX_ARTIST_MODE;
        let pen_pointer = artist || selected.output_mode.path == WINDOWS_PEN_POINTER_MODE;
        let pen = pen_pointer || selected.output_mode.path == WINDOWS_INK_ABSOLUTE_MODE;
        if artist {
            diagnostics.push(ProfileDiagnostic::warning(
                "linux_artist_mode",
                "Artist Mode was imported as pen output (a virtual tablet on Linux, Windows Ink on Windows). As upstream, the pen touches whenever pressure is above zero; the profile's tip/eraser bindings and thresholds are preserved but not applied.".into(),
            ));
        } else if pen_pointer {
            diagnostics.push(ProfileDiagnostic::warning(
                "windows_pen_pointer_native",
                "Windows Pen Pointer was imported as this driver's native pen output, which injects a synthetic pen the same way; the plugin DLL is not used. As in the plugin, the pen touches whenever pressure is above zero; the profile's tip/eraser bindings and thresholds are preserved but not applied.".into(),
            ));
        } else if pen {
            diagnostics.push(ProfileDiagnostic::warning(
                "windows_ink_native",
                "Windows Ink Absolute Mode was imported as this driver's native pen output. The Windows Ink plugin and its VMulti driver are not used; its Sync settings are preserved but not applied.".into(),
            ));
        }
        let (otd_mapping, relative) = match selected.output_mode.path.as_str() {
            "OpenTabletDriver.Desktop.Output.AbsoluteMode"
            | WINDOWS_INK_ABSOLUTE_MODE
            | WINDOWS_PEN_POINTER_MODE
            | LINUX_ARTIST_MODE => {
                let absolute = selected
                    .absolute_mode_settings
                    .ok_or("Absolute Mode requires AbsoluteModeSettings")?;
                (
                    Some(OtdMapping {
                        display: absolute.display,
                        tablet: absolute.tablet,
                        clipping: absolute.enable_clipping,
                        limiting: absolute.enable_area_limiting,
                    }),
                    None,
                )
            }
            "OpenTabletDriver.Desktop.Output.RelativeMode" => {
                let relative = selected
                    .relative_mode_settings
                    .ok_or("Relative Mode requires RelativeModeSettings")?;
                (
                    None,
                    Some(
                        RelativeSettings {
                            sensitivity: (relative.x_sensitivity, relative.y_sensitivity),
                            rotation: relative.rotation,
                            reset_delay: parse_reset_delay(&relative.reset_delay)?,
                        }
                        .validate_for(tablet)?,
                    ),
                )
            }
            WINDOWS_INK_RELATIVE_MODE => {
                return Err(format!(
                    "unsupported {tablet_name} output mode: Windows Ink Relative Mode; the native pen output is absolute, so choose Windows Ink Absolute Mode, Absolute Mode or Relative Mode"
                ));
            }
            _ => {
                return Err(format!(
                    "unsupported {tablet_name} output mode: {}; choose Absolute Mode, Relative Mode or Windows Ink Absolute Mode",
                    selected.output_mode.path
                ));
            }
        };
        let mut import_binding = |store: Option<&OtdStore>, name| {
            binding_enabled(store, name, pen).unwrap_or_else(|message| {
                diagnostics.push(ProfileDiagnostic::unsupported(
                    format!("Bindings.{name}"),
                    message,
                ));
                false
            })
        };
        let (tip_enabled, eraser_enabled) = if pen_pointer {
            (true, true)
        } else {
            (
                import_binding(selected.bindings.tip_button.as_ref(), "Tip"),
                import_binding(selected.bindings.eraser_button.as_ref(), "Eraser"),
            )
        };
        let pen_buttons = import_pen_buttons(&selected.bindings.pen_buttons, pen, &mut diagnostics);
        let mut radial_follow = Vec::new();
        let mut auto_enabled_radial_follow = 0;
        let mut ignored_filters = 0;
        for filter in &selected.filters {
            if filter.path == FILTER_PATH && (filter.enable || options.legacy_force_radial_follow) {
                radial_follow.push(radial_settings(filter)?);
                for setting in &filter.settings {
                    if !matches!(
                        setting.property.as_str(),
                        "OuterRadius"
                            | "InnerRadius"
                            | "SmoothingCoefficient"
                            | "SoftKneeScale"
                            | "SmoothingLeakCoefficient"
                    ) {
                        diagnostics.push(ProfileDiagnostic::unsupported(
                            format!("Profiles[{selected_index}].Filters.{}", setting.property),
                            "This native Radial Follow property is preserved but not applied."
                                .into(),
                        ));
                    }
                }
                if !filter.enable {
                    auto_enabled_radial_follow += 1;
                }
            } else if filter.enable {
                ignored_filters += 1;
                diagnostics.push(ProfileDiagnostic::unsupported(
                    format!("Profiles[{selected_index}].Filters"),
                    format!("Enabled filter {} is preserved in imported_otd.settings_json but is not executed.", filter.path),
                ));
            }
        }
        if auto_enabled_radial_follow > 0 {
            diagnostics.push(ProfileDiagnostic::warning(
                "legacy_radial_follow",
                format!("Explicit legacy import option activated {auto_enabled_radial_follow} disabled Radial Follow entries. Future imports honor Enable unless this option is requested again."),
            ));
        }
        Ok(Self {
            imported_otd: Some(ImportedOtdSettings {
                source_path: path.to_string_lossy().into_owned(),
                settings_json: text.to_owned(),
                selected_profile: selected_index,
                legacy_force_radial_follow: options.legacy_force_radial_follow,
            }),
            diagnostics,
            otd_mapping,
            relative,
            contact: if pen_pointer {
                // Any pressure above zero.
                ContactPolicy {
                    tip_enabled,
                    eraser_enabled,
                    tip_threshold_raw: Some(1),
                    eraser_threshold_raw: Some(1),
                }
            } else {
                ContactPolicy {
                    tip_enabled,
                    eraser_enabled,
                    tip_threshold_raw: Some(activation_raw_for(
                        selected.bindings.tip_activation_threshold,
                        tablet.max_pressure,
                    )?),
                    eraser_threshold_raw: Some(activation_raw_for(
                        selected.bindings.eraser_activation_threshold,
                        tablet.max_pressure,
                    )?),
                }
            },
            pen_buttons,
            output: if pen {
                OutputKind::Pen
            } else {
                OutputKind::Mouse
            },
            radial_follow,
            auto_enabled_radial_follow,
            ignored_filters,
            source: format!("OpenTabletDriver settings: {}", path.display()),
            tablet,
            ..Self::default()
        })
    }

    fn load_toml(path: &Path) -> Result<Self, String> {
        let text = fs::read_to_string(path)
            .map_err(|e| format!("cannot read profile {}: {e}", path.display()))?;
        Self::from_toml_text(&text, path)
    }

    pub fn from_toml_text(text: &str, path: &Path) -> Result<Self, String> {
        let mut document: toml::Value =
            toml::from_str(text).map_err(|e| format!("invalid profile {}: {e}", path.display()))?;
        if document.get("format").and_then(toml::Value::as_str) == Some("profile_collection") {
            return Err("This file is a profile collection. Load it with NativeProfileCollection and select a profile before editing or running it.".into());
        }
        let preserved_fields = schema::extract_unknown_fields(&mut document);
        let raw: RawProfile = document
            .try_into()
            .map_err(|e| format!("invalid profile {}: {e}", path.display()))?;
        if raw.schema_version > PROFILE_SCHEMA_VERSION {
            return Err(format!(
                "Profile schema {} is newer than supported schema {PROFILE_SCHEMA_VERSION}; update the driver before loading it.",
                raw.schema_version
            ));
        }
        if let Some(imported) = &raw.imported_otd {
            imported.validate()?;
        }
        let mut diagnostics = raw.diagnostics;
        if raw.schema_version == 0 {
            diagnostics.push(ProfileDiagnostic::warning(
                "schema_migration",
                "Loaded an unversioned Rust profile. Existing native Radial Follow entries remain active; saving writes schema 1. New OTD imports honor Enable.".into(),
            ));
        }
        let mut archived = raw.preserved_fields;
        if !preserved_fields.is_empty() {
            diagnostics.push(ProfileDiagnostic::warning(
                "unknown_native_fields",
                format!("{} unsupported native field(s) were archived under preserved_fields; they are not executed.", preserved_fields.len()),
            ));
            archived.extend(preserved_fields);
        }
        if raw.relative.is_some()
            && (raw.monitor.is_some()
                || raw.crop.is_some()
                || raw.rotation.is_some()
                || raw.absolute.is_some())
        {
            return Err("relative profiles use [relative].rotation; omit absolute monitor, crop, and top-level rotation".into());
        }
        if raw.relative.is_some() && raw.output == OutputKind::Pen {
            return Err(
                "pen output is absolute; remove [relative] or set output = \"mouse\"".into(),
            );
        }
        if raw.absolute.is_some()
            && (raw.monitor.is_some() || raw.crop.is_some() || raw.rotation.is_some())
        {
            return Err(
                "absolute areas cannot be mixed with monitor/crop/top-level rotation".into(),
            );
        }
        let base = path.parent().unwrap_or_else(|| Path::new("."));
        for filter in &raw.radial_follow {
            if ![
                filter.outer_radius,
                filter.inner_radius,
                filter.smoothing_coefficient,
                filter.soft_knee_scale,
                filter.smoothing_leak_coefficient,
            ]
            .into_iter()
            .all(f64::is_finite)
            {
                return Err("Radial Follow settings must be finite".into());
            }
        }
        let mut plugins = raw.plugins;
        if plugins.len() > 32 {
            return Err("at most 32 plugins are supported per profile".into());
        }
        for plugin in &mut plugins {
            plugin.validate()?;
            if plugin.path.is_relative() {
                plugin.path =
                    std::path::absolute(base.join(&plugin.path)).map_err(|e| e.to_string())?;
            }
        }
        let relative = raw
            .relative
            .map(|settings| {
                RelativeSettings {
                    sensitivity: (settings.x_sensitivity, settings.y_sensitivity),
                    rotation: settings.rotation,
                    reset_delay: Duration::try_from_secs_f64(settings.reset_delay_ms / 1000.0)
                        .map_err(|_| "relative reset delay must be finite and nonnegative")?,
                }
                .validate()
            })
            .transpose()?;
        let target_tablet = raw.tablet;
        let pen_buttons = match &raw.pen_buttons {
            None => default_pen_buttons(),
            Some(texts) => {
                if texts.len() > MAX_PEN_BUTTONS {
                    return Err(format!(
                        "at most {MAX_PEN_BUTTONS} pen_buttons are supported"
                    ));
                }
                texts
                    .iter()
                    .enumerate()
                    .map(|(index, text)| {
                        text.parse::<ButtonAction>()
                            .map_err(|error| format!("pen_buttons[{index}]: {error}"))
                    })
                    .collect::<Result<_, _>>()?
            }
        };
        let mut profile = Self {
            settings_revision: raw.settings_revision,
            imported_otd: raw.imported_otd,
            preserved_fields: archived,
            diagnostics,
            otd_mapping: raw.absolute,
            contact: raw.bindings,
            pen_buttons,
            output: raw.output,
            radial_follow: raw.radial_follow,
            plugins,
            relative,
            monitor: raw.monitor,
            crop: raw.crop.map_or_else(Crop::default, |c| Crop {
                x: c.x,
                y: c.y,
                width: c.width,
                height: c.height,
            }),
            rotation: raw.rotation.unwrap_or(0),
            device_path: raw.device_path,
            source: format!("Rust profile: {}", path.display()),
            target_tablet,
            ..Self::default()
        };
        profile.tablet = profile
            .tablet_name()?
            .map_or(TabletSpec::PTH_660, |name| spec_for_tablet(&name));
        for threshold in [
            profile.contact.tip_threshold_raw,
            profile.contact.eraser_threshold_raw,
        ]
        .into_iter()
        .flatten()
        {
            if threshold > profile.tablet.max_pressure {
                return Err("binding threshold exceeds tablet pressure range".into());
            }
        }
        // The default crop means the whole digitizer of any tablet.
        if profile.crop != Crop::default() && !profile.crop.valid_for(profile.tablet) {
            return Err(format!(
                "crop must be nonzero and within 0..{} X, 0..{} Y",
                profile.tablet.max_x, profile.tablet.max_y
            ));
        }
        if let Some(relative) = profile.relative {
            relative.validate_for(profile.tablet)?;
        }
        if !matches!(profile.rotation, 0 | 90 | 180 | 270) {
            return Err("rotation must be 0, 90, 180, or 270".into());
        }
        profile.validate_filter_execution()?;
        Ok(profile)
    }

    /// Reject double execution after migrating a native filter to a managed DLL.
    pub fn validate_filter_execution(&self) -> Result<(), String> {
        if !self.radial_follow.is_empty()
            && self.plugins.iter().any(|plugin| {
                plugin.enabled
                    && plugin.kind == crate::plugins::PluginKind::Dotnet
                    && plugin.type_name == FILTER_PATH
            })
        {
            return Err("Radial Follow tablet-space is enabled both natively and as a .NET filter. Remove the native entry before enabling its managed replacement.".into());
        }
        Ok(())
    }

    pub fn tablet_name(&self) -> Result<Option<String>, String> {
        if let Some(name) = &self.target_tablet {
            return Ok(Some(name.clone()));
        }
        let Some(imported) = &self.imported_otd else {
            return Ok(None);
        };
        let document: serde_json::Value = serde_json::from_str(&imported.settings_json)
            .map_err(|error| format!("invalid preserved OTD settings: {error}"))?;
        document
            .get("Profiles")
            .and_then(serde_json::Value::as_array)
            .and_then(|profiles| profiles.get(imported.selected_profile))
            .and_then(|profile| profile.get("Tablet"))
            .and_then(serde_json::Value::as_str)
            .map(|name| Some(name.to_owned()))
            .ok_or_else(|| "preserved OTD profile has no tablet identity".into())
    }

    /// Checks that a profile targeting a named tablet can run: its
    /// configuration exists and its parser and specifications are supported.
    /// Profiles without a tablet name run on whichever tablet is selected.
    pub fn validate_runtime_tablet(&self) -> Result<(), String> {
        if let Some(name) = self.tablet_name()? {
            runtime_tablet(&name)?;
        }
        Ok(())
    }

    /// This profile for the tablet the runtime selected. Raw pressure
    /// thresholds must fit the tablet's pressure range.
    pub fn for_tablet(&self, spec: TabletSpec) -> Result<Self, String> {
        for threshold in [
            self.contact.tip_threshold_raw,
            self.contact.eraser_threshold_raw,
        ]
        .into_iter()
        .flatten()
        {
            if threshold > spec.max_pressure {
                return Err(format!(
                    "pressure threshold {threshold} exceeds this tablet's range 0..{}",
                    spec.max_pressure
                ));
            }
        }
        if let Some(relative) = self.relative {
            relative.validate_for(spec)?;
        }
        let mut profile = self.clone();
        profile.tablet = spec;
        Ok(profile)
    }

    /// Reconcile representable edits into a copy of the complete OTD document.
    /// Never writes the original settings file.
    pub fn to_otd_json(&self) -> Result<String, String> {
        schema::export_otd(self)
    }

    /// Call once when an edit is committed; serialization itself is stable.
    pub fn advance_revision(&mut self) -> Result<(), String> {
        self.settings_revision = self
            .settings_revision
            .checked_add(1)
            .ok_or("profile settings revision exhausted")?;
        Ok(())
    }

    pub fn to_toml(&self) -> Result<String, String> {
        self.validate_filter_execution()?;
        if self.relative.is_some() && self.output == OutputKind::Pen {
            return Err("pen output is absolute; choose mouse output for relative mode".into());
        }
        let simple = self.otd_mapping.is_none() && self.relative.is_none();
        let raw = RawProfile {
            schema_version: PROFILE_SCHEMA_VERSION,
            settings_revision: self.settings_revision,
            imported_otd: self.imported_otd.clone(),
            preserved_fields: self.preserved_fields.clone(),
            diagnostics: self.diagnostics.clone(),
            monitor: if simple { self.monitor } else { None },
            rotation: simple.then_some(self.rotation),
            crop: simple.then_some(RawCrop {
                x: self.crop.x,
                y: self.crop.y,
                width: self.crop.width,
                height: self.crop.height,
            }),
            device_path: self.device_path.clone(),
            relative: self.relative.map(|r| RawRelative {
                x_sensitivity: r.sensitivity.0,
                y_sensitivity: r.sensitivity.1,
                rotation: r.rotation,
                reset_delay_ms: r.reset_delay.as_secs_f64() * 1000.0,
            }),
            absolute: self.otd_mapping,
            bindings: self.contact,
            pen_buttons: (self.pen_buttons != default_pen_buttons())
                .then(|| self.pen_buttons.iter().map(ToString::to_string).collect()),
            output: self.output,
            radial_follow: self.radial_follow.clone(),
            plugins: self.plugins.clone(),
            tablet: self.target_tablet.clone(),
        };
        toml::to_string_pretty(&raw).map_err(|e| e.to_string())
    }

    /// Preserve plugin locations within the destination directory, or within an
    /// explicitly configured portable tree. External plugins stay absolute.
    pub fn to_toml_at(&self, destination: &Path) -> Result<String, String> {
        let destination = std::path::absolute(destination).map_err(|error| error.to_string())?;
        let directory = destination
            .parent()
            .ok_or("profile destination has no parent")?;
        let root = match std::env::var_os("OTD_RUST_PORTABLE_DIR") {
            Some(root) => {
                let root = PathBuf::from(root);
                if !root.is_absolute() {
                    return Err("OTD_RUST_PORTABLE_DIR must be an absolute directory".into());
                }
                if directory.starts_with(&root) {
                    root
                } else {
                    directory.to_owned()
                }
            }
            None => directory.to_owned(),
        };
        let mut saved = self.clone();
        for plugin in &mut saved.plugins {
            if !plugin.path.is_absolute() {
                continue;
            }
            let absolute = std::path::absolute(&plugin.path).map_err(|error| error.to_string())?;
            let (base, target, root) = match (
                directory.canonicalize(),
                absolute.canonicalize(),
                root.canonicalize(),
            ) {
                (Ok(base), Ok(target), Ok(root)) => (base, target, root),
                _ => (directory.to_owned(), absolute, root.clone()),
            };
            let (Ok(base), Ok(target)) = (base.strip_prefix(&root), target.strip_prefix(&root))
            else {
                continue;
            };
            let base: Vec<_> = base.components().collect();
            let target: Vec<_> = target.components().collect();
            if target.is_empty()
                || !base
                    .iter()
                    .chain(&target)
                    .all(|component| matches!(component, std::path::Component::Normal(_)))
            {
                continue;
            }
            let common = base.iter().zip(&target).take_while(|(a, b)| a == b).count();
            let mut relative = PathBuf::new();
            for _ in common..base.len() {
                relative.push("..");
            }
            for component in &target[common..] {
                relative.push(component.as_os_str());
            }
            if !relative.as_os_str().is_empty() {
                plugin.path = relative;
            }
        }
        saved.to_toml()
    }

    pub fn print_summary(&self) {
        println!("Profile: {}", self.source);
        for diagnostic in &self.diagnostics {
            println!(
                "Profile {} [{}]: {}",
                diagnostic.kind, diagnostic.location, diagnostic.message
            );
        }
        for plugin in &self.plugins {
            println!(
                "Plugin {:?}: {} [{}] enabled={}",
                plugin.kind,
                plugin.path.display(),
                plugin.type_name,
                plugin.enabled
            );
        }
        if self.output == OutputKind::Pen {
            println!(
                "Pen output: pressure, tilt, eraser and hover go to a pen device (Windows Ink on Windows)."
            );
        }
        if let Some(relative) = self.relative {
            println!(
                "Relative Mode: X={:.4}, Y={:.4} counts/mm; rotation={:.2}°; reset delay={:.4} ms",
                relative.sensitivity.0,
                relative.sensitivity.1,
                relative.rotation,
                relative.reset_delay.as_secs_f64() * 1_000.0
            );
            println!("Windows pointer speed and acceleration apply to relative movement.");
            println!(
                "Tip threshold: {:?} raw / {}; eraser threshold: {:?} raw / {}",
                self.contact.tip_threshold_raw,
                self.contact.tip_enabled,
                self.contact.eraser_threshold_raw,
                self.contact.eraser_enabled
            );
        } else if let Some(mapping) = self.otd_mapping {
            let t = mapping.tablet;
            let d = mapping.display;
            println!(
                "Tablet area: {:.4} x {:.4} mm, center ({:.4}, {:.4}), rotation {:.2}°",
                t.width, t.height, t.x, t.y, t.rotation
            );
            println!(
                "Display area: {:.0} x {:.0} px, center ({:.0}, {:.0})",
                d.width, d.height, d.x, d.y
            );
            println!(
                "Clipping: {}; area limiting: {}; tip threshold: {} raw / {}; eraser threshold: {} raw / {}",
                mapping.clipping,
                mapping.limiting,
                self.contact.tip_threshold_raw.unwrap_or(0),
                self.contact.tip_enabled,
                self.contact.eraser_threshold_raw.unwrap_or(0),
                self.contact.eraser_enabled
            );
        } else {
            println!(
                "Tablet crop: {:?}; rotation: {}°; monitor: {:?}",
                self.crop, self.rotation, self.monitor
            );
        }
        if self.ignored_filters > 0 {
            println!("Skipped {} enabled plugin filter(s).", self.ignored_filters);
        }
        for (index, filter) in self.radial_follow.iter().enumerate() {
            println!(
                "Enabled {FILTER_NAME} #{}: OuterRadius={:.4} mm, InnerRadius={:.4} mm, SmoothingCoefficient={:.4}, SoftKneeScale={:.4}, SmoothingLeakCoefficient={:.4}",
                index + 1,
                filter.outer_radius,
                filter.inner_radius,
                filter.smoothing_coefficient,
                filter.soft_knee_scale,
                filter.smoothing_leak_coefficient
            );
        }
        if self.auto_enabled_radial_follow > 0 {
            println!(
                "Rust auto-enabled {} Radial Follow filter(s) despite OpenTabletDriver Enable=false.",
                self.auto_enabled_radial_follow
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn windows_ink_settings(tip: &str, eraser: &str) -> String {
        format!(
            r#"{{"Profiles":[{{"Tablet":"Wacom PTH-660","OutputMode":{{"Path":"VoiDPlugins.OutputMode.WinInkAbsoluteMode","Enable":true,"Settings":[{{"Property":"Sync","Value":true}}]}},"AbsoluteModeSettings":{{"Display":{{"Width":2560,"Height":1440,"X":1280,"Y":720,"Rotation":0}},"Tablet":{{"Width":85,"Height":47.8125,"X":110,"Y":23.90625,"Rotation":0}},"EnableClipping":true,"EnableAreaLimiting":false}},"Bindings":{{"TipActivationThreshold":2,"TipButton":{{"Path":"VoiDPlugins.OutputMode.WindowsInkButtonHandler","Enable":true,"Settings":[{{"Property":"Button","Value":"{tip}"}}]}},"EraserActivationThreshold":1,"EraserButton":{{"Path":"VoiDPlugins.OutputMode.WindowsInkButtonHandler","Enable":true,"Settings":[{{"Property":"Button","Value":"{eraser}"}}]}}}}}}]}}"#
        )
    }

    #[test]
    fn windows_ink_absolute_mode_imports_as_native_pen_output() {
        let path = Path::new("settings.json");
        let profile =
            Profile::from_otd_text(&windows_ink_settings("Pen Tip", "Pen Tip"), path).unwrap();
        assert_eq!(profile.output, OutputKind::Pen);
        assert!(profile.otd_mapping.is_some() && profile.relative.is_none());
        assert!(profile.contact.tip_enabled && profile.contact.eraser_enabled);
        assert!(
            profile
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.location == "windows_ink_native"),
            "{:?}",
            profile.diagnostics
        );
        // Pen output survives the native profile format.
        let text = profile.to_toml().unwrap();
        assert!(text.contains("output = \"pen\""), "{text}");
        let reloaded = Profile::from_toml_text(&text, Path::new("driver.toml")).unwrap();
        assert_eq!(reloaded.output, OutputKind::Pen);
        // Mouse output is the default and is not written.
        assert!(!Profile::default().to_toml().unwrap().contains("output"));

        // Buttons other than Pen Tip are not applied and say so.
        let toggle =
            Profile::from_otd_text(&windows_ink_settings("Pen Tip", "Eraser (Toggle)"), path)
                .unwrap();
        assert!(toggle.contact.tip_enabled && !toggle.contact.eraser_enabled);
        assert!(toggle.diagnostics.iter().any(|diagnostic| {
            diagnostic.location == "Bindings.Eraser" && diagnostic.message.contains("Toggle")
        }));
        let relative = windows_ink_settings("Pen Tip", "Pen Tip")
            .replace("WinInkAbsoluteMode", "WinInkRelativeMode");
        assert!(
            Profile::from_otd_text(&relative, path)
                .unwrap_err()
                .contains("Windows Ink Relative Mode")
        );
    }

    #[test]
    fn windows_pen_pointer_imports_as_pen_output_touching_above_zero_pressure() {
        let text = windows_ink_settings("Pen Button", "Eraser (Toggle)")
            .replace(WINDOWS_INK_ABSOLUTE_MODE, WINDOWS_PEN_POINTER_MODE);
        let profile = Profile::from_otd_text(&text, Path::new("settings.json")).unwrap();
        assert_eq!(profile.output, OutputKind::Pen);
        assert!(profile.contact.tip_enabled && profile.contact.eraser_enabled);
        assert_eq!(profile.contact.tip_threshold_raw, Some(1));
        assert_eq!(profile.contact.eraser_threshold_raw, Some(1));
        assert!(
            profile
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.location == "windows_pen_pointer_native")
        );
        let artist = text.replace(WINDOWS_PEN_POINTER_MODE, LINUX_ARTIST_MODE);
        let artist = Profile::from_otd_text(&artist, Path::new("settings.json")).unwrap();
        assert_eq!(artist.output, OutputKind::Pen);
        assert_eq!(artist.contact.tip_threshold_raw, Some(1));
        let exported: serde_json::Value =
            serde_json::from_str(&artist.to_otd_json().unwrap()).unwrap();
        assert_eq!(
            exported["Profiles"][0]["OutputMode"]["Path"],
            LINUX_ARTIST_MODE
        );
        // Exporting keeps the plugin the profile came from.
        let exported: serde_json::Value =
            serde_json::from_str(&profile.to_otd_json().unwrap()).unwrap();
        assert_eq!(
            exported["Profiles"][0]["OutputMode"]["Path"],
            WINDOWS_PEN_POINTER_MODE
        );
    }

    #[test]
    fn windows_ink_bindings_need_a_windows_ink_output_mode() {
        let mouse = windows_ink_settings("Pen Tip", "Pen Tip").replace(
            "VoiDPlugins.OutputMode.WinInkAbsoluteMode",
            "OpenTabletDriver.Desktop.Output.AbsoluteMode",
        );
        let profile = Profile::from_otd_text(&mouse, Path::new("settings.json")).unwrap();
        assert_eq!(profile.output, OutputKind::Mouse);
        assert!(!profile.contact.tip_enabled && !profile.contact.eraser_enabled);
    }

    #[test]
    fn pen_output_is_absolute_only() {
        let error = Profile::from_toml_text(
            "output = \"pen\"\n[relative]\nx_sensitivity = 10.0\ny_sensitivity = 10.0\nreset_delay_ms = 100.0\n",
            Path::new("driver.toml"),
        )
        .unwrap_err();
        assert!(error.contains("pen output is absolute"), "{error}");
        let profile = Profile {
            output: OutputKind::Pen,
            relative: Some(RelativeSettings {
                sensitivity: (10.0, 10.0),
                rotation: 0.0,
                reset_delay: Duration::from_millis(100),
            }),
            ..Profile::default()
        };
        assert!(profile.to_toml().is_err());
        assert!(
            Profile::from_toml_text("output = \"tablet\"\n", Path::new("driver.toml")).is_err()
        );
    }

    #[test]
    fn pen_output_exports_as_the_windows_ink_mode_and_back() {
        let path = Path::new("settings.json");
        let mouse_text = windows_ink_settings("Pen Tip", "Pen Tip")
            .replace(
                "VoiDPlugins.OutputMode.WinInkAbsoluteMode\",\"Enable\":true,\"Settings\":[{\"Property\":\"Sync\",\"Value\":true}]",
                "OpenTabletDriver.Desktop.Output.AbsoluteMode\",\"Enable\":true",
            )
            .replace(
                r#""Path":"VoiDPlugins.OutputMode.WindowsInkButtonHandler","Enable":true,"Settings":[{"Property":"Button","Value":"Pen Tip"}]"#,
                r#""Path":"OpenTabletDriver.Desktop.Binding.AdaptiveBinding","Enable":true,"Settings":[{"Property":"Binding","Value":"Tip"}]"#,
            );
        let mut profile = Profile::from_otd_text(&mouse_text, path).unwrap();
        assert_eq!(profile.output, OutputKind::Mouse);
        assert!(profile.contact.tip_enabled);
        profile.output = OutputKind::Pen;
        let exported: serde_json::Value =
            serde_json::from_str(&profile.to_otd_json().unwrap()).unwrap();
        let selected = &exported["Profiles"][0];
        assert_eq!(selected["OutputMode"]["Path"], WINDOWS_INK_ABSOLUTE_MODE);
        assert_eq!(
            selected["Bindings"]["TipButton"]["Path"],
            WINDOWS_INK_BINDING
        );
        assert_eq!(
            selected["Bindings"]["TipButton"]["Settings"],
            serde_json::json!([{"Property": "Button", "Value": "Pen Tip"}])
        );
        // The exported document imports as pen output with the same contact.
        let reimported =
            Profile::from_otd_text(&serde_json::to_string(&exported).unwrap(), path).unwrap();
        assert_eq!(reimported.output, OutputKind::Pen);
        assert_eq!(reimported.contact.tip_enabled, profile.contact.tip_enabled);
        assert_eq!(
            reimported.contact.tip_threshold_raw,
            profile.contact.tip_threshold_raw
        );
    }

    #[test]
    fn a_native_profile_keeps_its_tablet() {
        let profile = Profile {
            target_tablet: Some("Wacom CTL-4100".into()),
            ..Profile::default()
        };
        let text = profile.to_toml().unwrap();
        assert!(text.contains("tablet = \"Wacom CTL-4100\""), "{text}");
        let loaded = Profile::from_toml_text(&text, Path::new("driver.toml")).unwrap();
        assert_eq!(loaded.target_tablet.as_deref(), Some("Wacom CTL-4100"));
        assert_eq!(loaded.tablet, spec_for_tablet("Wacom CTL-4100"));
        assert_ne!(loaded.tablet, TabletSpec::PTH_660);
        // Earlier profiles have no tablet and keep the PTH-660's ranges.
        let old = Profile::from_toml_text("", Path::new("driver.toml")).unwrap();
        assert_eq!((old.target_tablet, old.tablet), (None, TabletSpec::PTH_660));
    }

    #[test]
    fn default_profile_is_valid() {
        let p = Profile::default();
        assert!(p.crop.valid());
        assert_eq!(p.rotation, 0);
    }

    #[test]
    fn gui_profile_serialization_preserves_mapping_bindings_and_plugins() {
        let mut profile =
            Profile::from_otd_text(&relative_profile().to_string(), Path::new("otd.json")).unwrap();
        profile.plugins.push(crate::plugins::PluginConfig {
            path: PathBuf::from("C:/filters/example.dll"),
            kind: crate::plugins::PluginKind::Dotnet,
            enabled: false,
            type_name: "Example.Filter".into(),
            settings_json: r#"{"Radius":0.5}"#.into(),
        });
        let serialized = profile.to_toml().unwrap();
        let loaded = Profile::from_toml_text(&serialized, Path::new("driver.toml")).unwrap();
        assert_eq!(
            loaded.relative.unwrap().sensitivity,
            profile.relative.unwrap().sensitivity
        );
        assert_eq!(loaded.contact.tip_threshold_raw, Some(82));
        assert_eq!(loaded.radial_follow.len(), 1);
        assert_eq!(loaded.plugins[0].type_name, "Example.Filter");
        assert_eq!(loaded.plugins[0].settings_json, r#"{"Radius":0.5}"#);
        assert!(!loaded.plugins[0].enabled);
        let defaults = Profile::default();
        assert!(
            Profile::from_toml_text(&defaults.to_toml().unwrap(), Path::new("default.toml"))
                .unwrap()
                .crop
                .valid()
        );
    }

    #[test]
    fn plugin_paths_are_relative_to_profile_and_invalid_filter_settings_are_rejected() {
        let text = "[[plugins]]\npath='filters/test.dll'\nsettings_json='{}'\n";
        let path = std::env::temp_dir().join("otd-profile-tests/driver.toml");
        let profile = Profile::from_toml_text(text, &path).unwrap();
        assert_eq!(
            profile.plugins[0].path,
            path.parent().unwrap().join("filters/test.dll")
        );
        assert!(Profile::from_toml_text("[[radial_follow]]\nouter_radius=nan\n", &path).is_err());
        assert!(Profile::from_toml_text("[bindings]\ntip_threshold_raw=8192\n", &path).is_err());
    }

    #[test]
    fn activation_matches_open_tablet_driver_float_threshold() {
        // First raw value with pressure / 8191 * 100 > threshold.
        for (percent, first) in [
            (0.0, 1),
            (0.5, 41),
            (1.0, 82),
            (10.0, 820),
            (25.0, 2048),
            (50.0, 4096),
            (99.99, MAX_PRESSURE),
            (100.0, MAX_PRESSURE),
        ] {
            assert_eq!(activation_raw(percent), Ok(first), "{percent}%");
        }
        assert!(activation_raw(-0.1).is_err());
        assert!(activation_raw(100.1).is_err());
        assert!(activation_raw(f64::NAN).is_err());
    }

    #[test]
    fn reads_absolute_area_and_tip_threshold_from_otd_profile() {
        let json = r#"{"Profiles":[{"Tablet":"Wacom PTH-660","OutputMode":{"Path":"OpenTabletDriver.Desktop.Output.AbsoluteMode","Enable":true},"Filters":[{"Path":"Example.Filter","Enable":true}],"AbsoluteModeSettings":{"Display":{"Width":2560,"Height":1440,"X":1280,"Y":720,"Rotation":0},"Tablet":{"Width":85,"Height":47.8125,"X":110,"Y":23.90625,"Rotation":0},"EnableClipping":true,"EnableAreaLimiting":false},"Bindings":{"TipActivationThreshold":1,"TipButton":{"Path":"OpenTabletDriver.Desktop.Binding.AdaptiveBinding","Enable":true,"Settings":[{"Property":"Binding","Value":"Tip"}]},"EraserActivationThreshold":1,"EraserButton":{"Path":"OpenTabletDriver.Desktop.Binding.AdaptiveBinding","Enable":true,"Settings":[{"Property":"Binding","Value":"Eraser"}]}}}]}"#;
        let profile = Profile::from_otd_text(json, Path::new("settings.json")).unwrap();
        assert_eq!(profile.otd_mapping.unwrap().tablet.width, 85.0);
        assert_eq!(profile.contact.tip_threshold_raw, Some(82));
        assert!(profile.contact.tip_enabled);
        assert_eq!(profile.ignored_filters, 1);
    }

    #[test]
    fn enables_radial_follow_with_original_setting_names() {
        let json = r#"{"Profiles":[{"Tablet":"Wacom PTH-660","OutputMode":{"Path":"OpenTabletDriver.Desktop.Output.AbsoluteMode","Enable":true},"Filters":[{"Path":"RadialFollow.RadialFollowSmoothingTabletSpace","Enable":true,"Settings":[{"Property":"OuterRadius","Value":0.7039},{"Property":"InnerRadius","Value":0.302},{"Property":"SmoothingCoefficient","Value":0.302},{"Property":"SoftKneeScale","Value":0.603},{"Property":"SmoothingLeakCoefficient","Value":0.201}]}],"AbsoluteModeSettings":{"Display":{"Width":2560,"Height":1440,"X":1280,"Y":720,"Rotation":0},"Tablet":{"Width":85,"Height":47.8125,"X":110,"Y":23.90625,"Rotation":0},"EnableClipping":true,"EnableAreaLimiting":false},"Bindings":{}}]}"#;
        let profile = Profile::from_otd_text(json, Path::new("settings.json")).unwrap();
        assert_eq!(profile.ignored_filters, 0);
        assert_eq!(profile.radial_follow.len(), 1);
        assert_eq!(profile.auto_enabled_radial_follow, 0);
        let filter = profile.radial_follow[0];
        assert_eq!(filter.outer_radius, 0.7039);
        assert_eq!(filter.inner_radius, 0.302);
        assert_eq!(filter.smoothing_coefficient, 0.302);
        assert_eq!(filter.soft_knee_scale, 0.603);
        assert_eq!(filter.smoothing_leak_coefficient, 0.201);
    }

    #[test]
    fn honors_disabled_saved_radial_follow_when_otd_flag_is_off() {
        let json = r#"{"Profiles":[{"Tablet":"Wacom PTH-660","OutputMode":{"Path":"OpenTabletDriver.Desktop.Output.AbsoluteMode","Enable":true},"Filters":[{"Path":"RadialFollow.RadialFollowSmoothingTabletSpace","Enable":false,"Settings":[{"Property":"OuterRadius","Value":0.7039}]}],"AbsoluteModeSettings":{"Display":{"Width":2560,"Height":1440,"X":1280,"Y":720,"Rotation":0},"Tablet":{"Width":85,"Height":47.8125,"X":110,"Y":23.90625,"Rotation":0},"EnableClipping":true,"EnableAreaLimiting":false},"Bindings":{}}]}"#;
        let profile = Profile::from_otd_text(json, Path::new("settings.json")).unwrap();
        assert!(profile.radial_follow.is_empty());
        assert_eq!(profile.auto_enabled_radial_follow, 0);
    }

    fn relative_profile() -> serde_json::Value {
        serde_json::json!({"Profiles": [{
            "Tablet": "Wacom PTH-660",
            "OutputMode": {"Path": "OpenTabletDriver.Desktop.Output.RelativeMode", "Enable": true},
            "RelativeModeSettings": {
                "XSensitivity": 12.5, "YSensitivity": 8.0,
                "RelativeRotation": 30.0, "RelativeResetDelay": "00:00:00.1000000"
            },
            "Bindings": {
                "TipActivationThreshold": 1,
                "TipButton": {"Path": "OpenTabletDriver.Desktop.Binding.AdaptiveBinding", "Enable": true,
                    "Settings": [{"Property": "Binding", "Value": "Tip"}]}
            },
            "Filters": [{"Path": "RadialFollow.RadialFollowSmoothingTabletSpace", "Enable": true}]
        }]})
    }

    #[test]
    fn imports_relative_mode_without_absolute_areas() {
        let profile =
            Profile::from_otd_text(&relative_profile().to_string(), Path::new("relative.json"))
                .unwrap();
        let relative = profile.relative.unwrap();
        assert_eq!(relative.sensitivity, (12.5, 8.0));
        assert_eq!(relative.rotation, 30.0);
        assert_eq!(relative.reset_delay, Duration::from_millis(100));
        assert!(profile.otd_mapping.is_none());
        assert!(profile.contact.tip_enabled);
        assert_eq!(profile.contact.tip_threshold_raw, Some(82));
        assert_eq!(profile.radial_follow.len(), 1);
    }

    fn store(path: &str, property: &str, value: serde_json::Value) -> serde_json::Value {
        serde_json::json!({"Path": path, "Enable": true,
            "Settings": [{"Property": property, "Value": value}]})
    }

    fn import_pen_buttons_from(buttons: serde_json::Value) -> Profile {
        let mut json = relative_profile();
        json["Profiles"][0]["Bindings"]["PenButtons"] = buttons;
        Profile::from_otd_text(&json.to_string(), Path::new("buttons.json")).unwrap()
    }

    #[test]
    fn a_native_profile_starts_with_upstreams_barrel_buttons() {
        let profile = Profile::default();
        assert_eq!(
            profile.pen_buttons,
            [
                ButtonAction::Barrel(1),
                ButtonAction::Barrel(2),
                ButtonAction::Barrel(3)
            ]
        );
        assert!(!profile.to_toml().unwrap().contains("pen_buttons"));
    }

    #[test]
    fn imports_upstreams_default_pen_button_bindings_without_a_warning() {
        let adaptive = |number: u8| {
            store(
                ADAPTIVE_BINDING,
                "Binding",
                format!("Button {number}").into(),
            )
        };
        let profile = import_pen_buttons_from(serde_json::json!([adaptive(1), adaptive(2)]));
        assert_eq!(
            profile.pen_buttons,
            [ButtonAction::Barrel(1), ButtonAction::Barrel(2)]
        );
        assert!(
            profile
                .diagnostics
                .iter()
                .all(|diagnostic| !diagnostic.location.contains("PenButtons")),
            "{:?}",
            profile.diagnostics
        );
    }

    #[test]
    fn imports_mouse_key_and_chord_bindings() {
        use crate::actions::MouseButton;
        let profile = import_pen_buttons_from(serde_json::json!([
            store(
                "OpenTabletDriver.Desktop.Binding.MouseBinding",
                "Button",
                "Forward".into()
            ),
            store(
                "OpenTabletDriver.Desktop.Binding.KeyBinding",
                "Key",
                "Escape".into()
            ),
            store(
                "OpenTabletDriver.Desktop.Binding.MultiKeyBinding",
                "Keys",
                "Control+Shift+Z".into()
            ),
        ]));
        assert_eq!(
            profile.pen_buttons[0],
            ButtonAction::Mouse(MouseButton::Forward)
        );
        assert_eq!(profile.pen_buttons[1].to_string(), "keys:Escape");
        assert_eq!(
            profile.pen_buttons[2].to_string(),
            "keys:LeftControl+LeftShift+Z"
        );
        assert!(
            profile
                .diagnostics
                .iter()
                .all(|d| !d.location.contains("PenButtons"))
        );
    }

    #[test]
    fn disabled_null_and_empty_pen_buttons_do_nothing_without_a_warning() {
        let mut disabled = store(ADAPTIVE_BINDING, "Binding", "Button 1".into());
        disabled["Enable"] = false.into();
        let profile = import_pen_buttons_from(serde_json::json!([
            disabled,
            null,
            store(
                "OpenTabletDriver.Desktop.Binding.MouseBinding",
                "Button",
                serde_json::Value::Null
            ),
        ]));
        assert_eq!(profile.pen_buttons, [const { ButtonAction::None }; 3]);
        assert!(
            profile
                .diagnostics
                .iter()
                .all(|d| !d.location.contains("PenButtons"))
        );
        // A profile with no PenButtons at all binds nothing, as upstream.
        assert!(
            import_pen_buttons_from(serde_json::json!([]))
                .pen_buttons
                .is_empty()
        );
    }

    #[test]
    fn unsupported_pen_button_bindings_are_reported_and_disabled() {
        let profile = import_pen_buttons_from(serde_json::json!([
            store(
                "OpenTabletDriver.Desktop.Binding.PresetBinding",
                "Preset",
                "Gaming".into()
            ),
            store(
                "OpenTabletDriver.Desktop.Binding.KeyBinding",
                "Key",
                "Mute".into()
            ),
            store(
                "OpenTabletDriver.Desktop.Binding.MouseBinding",
                "Button",
                "Sideways".into()
            ),
            store(ADAPTIVE_BINDING, "Binding", "Tip".into()),
        ]));
        // On a mouse, an adaptive Tip is the left button; the rest are refused.
        assert_eq!(profile.pen_buttons[0..3], [const { ButtonAction::None }; 3]);
        assert_eq!(
            profile.pen_buttons[3],
            ButtonAction::Mouse(crate::actions::MouseButton::Left)
        );
        let reported: Vec<_> = profile
            .diagnostics
            .iter()
            .filter(|d| d.location.starts_with("Bindings.PenButtons"))
            .map(|d| d.location.as_str())
            .collect();
        assert_eq!(
            reported,
            [
                "Bindings.PenButtons[0]",
                "Bindings.PenButtons[1]",
                "Bindings.PenButtons[2]"
            ]
        );
    }

    #[test]
    fn pen_buttons_survive_a_save_and_reload() {
        let profile = import_pen_buttons_from(serde_json::json!([
            store(ADAPTIVE_BINDING, "Binding", "Button 2".into()),
            store(
                "OpenTabletDriver.Desktop.Binding.MultiKeyBinding",
                "Keys",
                "Control+Z".into()
            ),
        ]));
        let text = profile.to_toml().unwrap();
        assert!(text.contains("pen_buttons"), "{text}");
        let reloaded = Profile::from_toml_text(&text, Path::new("saved.toml")).unwrap();
        assert_eq!(reloaded.pen_buttons, profile.pen_buttons);

        let none = import_pen_buttons_from(serde_json::json!([]));
        let reloaded =
            Profile::from_toml_text(&none.to_toml().unwrap(), Path::new("saved.toml")).unwrap();
        assert!(reloaded.pen_buttons.is_empty(), "an empty list stays empty");
    }

    #[test]
    fn native_toml_reads_pen_buttons_and_rejects_bad_ones() {
        let profile = Profile::from_toml_text(
            "pen_buttons = [\"mouse:right\", \"none\", \"keys:Control+Z\"]\n",
            Path::new("native.toml"),
        )
        .unwrap();
        assert_eq!(profile.pen_buttons.len(), 3);
        assert_eq!(profile.pen_buttons[1], ButtonAction::None);
        for bad in [
            "pen_buttons = [\"mouse:sideways\"]\n",
            "pen_buttons = [\"keys:Mute\"]\n",
            "pen_buttons = [1]\n",
        ] {
            assert!(
                Profile::from_toml_text(bad, Path::new("bad.toml")).is_err(),
                "{bad}"
            );
        }
        let many = format!("pen_buttons = [{}]\n", vec!["\"none\""; 65].join(","));
        assert!(Profile::from_toml_text(&many, Path::new("many.toml")).is_err());
    }

    #[test]
    fn editing_pen_buttons_cannot_be_exported_to_otd_settings() {
        let mut profile = import_pen_buttons_from(serde_json::json!([]));
        assert!(profile.to_otd_json().is_ok());
        profile.pen_buttons = vec![ButtonAction::Barrel(1)];
        let error = profile.to_otd_json().unwrap_err();
        assert!(error.contains("pen button"), "{error}");
    }

    #[test]
    fn parses_timespan_days_and_submillisecond_precision() {
        assert_eq!(parse_reset_delay("00:00:00").unwrap(), Duration::ZERO);
        assert_eq!(
            parse_reset_delay("00:00:00.1").unwrap(),
            Duration::from_millis(100)
        );
        assert_eq!(
            parse_reset_delay("1.02:03:04.1234567").unwrap(),
            Duration::new(93_784, 123_456_700)
        );
        assert_eq!(
            parse_reset_delay("10675199.02:48:05.4775807")
                .unwrap()
                .as_nanos(),
            (i64::MAX as u128) * 100
        );
        for invalid in [
            "",
            "100",
            "-00:00:00.1",
            "00:00:00.",
            "00:00:00.12345678",
            "24:00:00",
            "00:60:00",
            "00:00:60",
            "00:00:00:00",
            "NaN",
            "1e3:00:00",
            "18446744073709551615.00:00:00",
            "10675199.02:48:05.4775808",
        ] {
            assert!(parse_reset_delay(invalid).is_err(), "accepted {invalid}");
        }
    }

    #[test]
    fn rejects_missing_relative_settings_disabled_and_unknown_modes() {
        for (field, value) in [
            ("RelativeModeSettings", serde_json::Value::Null),
            (
                "OutputMode",
                serde_json::json!({"Path": "OpenTabletDriver.Desktop.Output.RelativeMode", "Enable": false}),
            ),
            (
                "OutputMode",
                serde_json::json!({"Path": "Unknown.Mode", "Enable": true}),
            ),
        ] {
            let mut json = relative_profile();
            json["Profiles"][0][field] = value;
            assert!(Profile::from_otd_text(&json.to_string(), Path::new("invalid.json")).is_err());
        }
    }

    #[test]
    fn reads_relative_toml_and_rejects_ambiguous_absolute_options() {
        let text = "[relative]\nx_sensitivity = 10.0\ny_sensitivity = 20.0\nrotation = -45.0\nreset_delay_ms = 100\n";
        let profile = Profile::from_toml_text(text, Path::new("relative.toml")).unwrap();
        assert_eq!(profile.relative.unwrap().rotation, -45.0);
        assert_eq!(profile.relative.unwrap().sensitivity, (10.0, 20.0));
        for prefix in [
            "rotation = 0\n",
            "monitor = 0\n",
            "[crop]\nx=0\ny=0\nwidth=10\nheight=10\n",
        ] {
            assert!(
                Profile::from_toml_text(&format!("{prefix}{text}"), Path::new("invalid.toml"))
                    .is_err()
            );
        }
        for invalid in [
            text.replace("10.0", "nan"),
            text.replace("20.0", "inf"),
            text.replace("100\n", "-1\n"),
        ] {
            assert!(Profile::from_toml_text(&invalid, Path::new("invalid.toml")).is_err());
        }
    }
}
