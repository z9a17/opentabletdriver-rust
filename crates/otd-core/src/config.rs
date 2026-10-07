use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::keys;
use crate::mapping::{Crop, OtdArea, OtdMapping};
use crate::output::buttons::{ButtonAction, WheelBinding, default_pen_buttons};
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
    /// Preserve upstream single-precision percentages for the drag pressure
    /// remap. Raw-only native profiles derive a representable percentage.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tip_threshold_percent: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub eraser_threshold_percent: Option<f32>,
    /// Gate initial pen-button activation on pressure > 0, as BindingState does.
    pub drag_only: bool,
    pub disable_pressure: bool,
    pub disable_tilt: bool,
}

impl Default for ContactPolicy {
    fn default() -> Self {
        Self {
            tip_enabled: true,
            eraser_enabled: true,
            tip_threshold_raw: None,
            eraser_threshold_raw: None,
            tip_threshold_percent: None,
            eraser_threshold_percent: None,
            drag_only: false,
            disable_pressure: false,
            disable_tilt: false,
        }
    }
}

impl ContactPolicy {
    pub fn validate_percentages(self) -> Result<(), String> {
        for percent in [self.tip_threshold_percent, self.eraser_threshold_percent].into_iter().flatten() {
            if !percent.is_finite() || !(0.0..=100.0).contains(&percent) {
                return Err("contact threshold percentages must be finite values from 0 to 100".into());
            }
        }
        Ok(())
    }

    /// BindingHandler invokes ThresholdBindingState before side buttons. Its
    /// remapped uint pressure, not raw pressure or contact enabled, gates drag.
    /// Native raw-only thresholds use the same midpoint representation as OTD
    /// export; a hardware-tip-switch profile retains its raw pressure gate.
    pub fn drag_pressure(self, pressure: Option<u32>, max_pressure: u32, eraser: bool) -> Option<u32> {
        let pressure = pressure?;
        let (percent, raw) = if eraser {
            (self.eraser_threshold_percent, self.eraser_threshold_raw)
        } else { (self.tip_threshold_percent, self.tip_threshold_raw) };
        let threshold = match percent {
            Some(percent) => percent,
            None => match raw {
                Some(0) | None => return Some(pressure),
                Some(raw) if u32::from(raw) == max_pressure => 100.0,
                Some(raw) => (f32::from(raw) - 0.5) * 100.0 / max_pressure as f32,
            },
        };
        if max_pressure == 0 || !threshold.is_finite() { return Some(0); }
        let value = pressure as f32 / max_pressure as f32 * 100.0;
        let maxed = threshold == 100.0 && value == 100.0;
        Some(if maxed { max_pressure } else if value > threshold {
            (max_pressure as f32 * ((value - threshold) / (100.0 - threshold))) as u32
        } else { 0 })
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
const MOUSE_SCROLL_BINDING: &str = "OpenTabletDriver.Desktop.Binding.MouseScrollBinding";
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
    /// What each express key does, by button index. Upstream's default
    /// binds none.
    pub aux_buttons: Vec<ButtonAction>,
    /// Tablet mouse/puck buttons, independent of pen and auxiliary input.
    pub mouse_buttons: Vec<ButtonAction>,
    pub mouse_scroll_up: ButtonAction,
    pub mouse_scroll_down: ButtonAction,
    /// What each wheel, ring or dial does, by wheel index.
    pub wheels: Vec<WheelBinding>,
    /// Absolute output only; relative output always moves the mouse.
    pub output: OutputKind,
    pub radial_follow: Vec<RadialFollowSettings>,
    /// Native filter values retained while disabled; never executed.
    pub disabled_radial_follow: Option<RadialFollowSettings>,
    pub plugins: Vec<PluginConfig>,
    pub auto_enabled_radial_follow: usize,
    pub ignored_filters: usize,
    pub source: String,
    /// Configuration name, or `"*"` for explicit automatic selection.
    /// `None` preserves legacy behavior: imported profiles inherit their
    /// document's tablet name; other profiles select any connected tablet.
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
            aux_buttons: Vec::new(),
            mouse_buttons: Vec::new(),
            mouse_scroll_up: ButtonAction::None,
            mouse_scroll_down: ButtonAction::None,
            wheels: Vec::new(),
            output: OutputKind::Mouse,
            radial_follow: Vec::new(),
            disabled_radial_follow: None,
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
    /// Express key actions as text, absent when none is bound.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    aux_buttons: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    mouse_buttons: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    mouse_scroll_up: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    mouse_scroll_down: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    wheels: Vec<RawWheel>,
    #[serde(default, skip_serializing_if = "OutputKind::is_mouse")]
    output: OutputKind,
    #[serde(default)]
    radial_follow: Vec<RadialFollowSettings>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    disabled_radial_follow: Option<RadialFollowSettings>,
    #[serde(default)]
    plugins: Vec<PluginConfig>,
    /// The tablet a native profile is for, by configuration name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tablet: Option<String>,
}

/// One `[[wheels]]` entry; actions are `ButtonAction` text.
#[derive(Deserialize, Serialize, Default)]
#[serde(deny_unknown_fields)]
struct RawWheel {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    clockwise: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    counter_clockwise: Option<String>,
    /// Degrees of rotation per activation; absent for one wheel step.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    clockwise_threshold: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    counter_clockwise_threshold: Option<f32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    buttons: Vec<String>,
}

impl RawWheel {
    fn parse(&self, wheel: usize) -> Result<WheelBinding, String> {
        let action = |text: &Option<String>, field: &str| {
            text.as_deref().map_or(Ok(ButtonAction::None), |text| {
                text.parse::<ButtonAction>()
                    .map_err(|error| format!("wheels[{wheel}].{field}: {error}"))
            })
        };
        for (value, field) in [
            (self.clockwise_threshold, "clockwise_threshold"),
            (self.counter_clockwise_threshold, "counter_clockwise_threshold"),
        ] {
            if value.is_some_and(|value| !value.is_finite() || value <= 0.0) {
                return Err(format!(
                    "wheels[{wheel}].{field} must be a positive number of degrees"
                ));
            }
        }
        Ok(WheelBinding {
            clockwise: action(&self.clockwise, "clockwise")?,
            counter_clockwise: action(&self.counter_clockwise, "counter_clockwise")?,
            clockwise_threshold: self.clockwise_threshold,
            counter_clockwise_threshold: self.counter_clockwise_threshold,
            buttons: parse_actions(&self.buttons, &format!("wheels[{wheel}].buttons"))?,
        })
    }

    fn from_binding(binding: &WheelBinding) -> Self {
        let text = |action: &ButtonAction| {
            (*action != ButtonAction::None).then(|| action.to_string())
        };
        Self {
            clockwise: text(&binding.clockwise),
            counter_clockwise: text(&binding.counter_clockwise),
            clockwise_threshold: binding.clockwise_threshold,
            counter_clockwise_threshold: binding.counter_clockwise_threshold,
            buttons: binding.buttons.iter().map(ToString::to_string).collect(),
        }
    }
}

/// Button actions from their TOML text, at most `MAX_PEN_BUTTONS`.
fn parse_actions(texts: &[String], field: &str) -> Result<Vec<ButtonAction>, String> {
    if texts.len() > MAX_PEN_BUTTONS {
        return Err(format!("at most {MAX_PEN_BUTTONS} {field} are supported"));
    }
    texts
        .iter()
        .enumerate()
        .map(|(index, text)| {
            text.parse::<ButtonAction>()
                .map_err(|error| format!("{field}[{index}]: {error}"))
        })
        .collect()
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
    #[serde(default)]
    aux_buttons: serde_json::Value,
    #[serde(default)]
    wheel_bindings: serde_json::Value,
    #[serde(default)]
    mouse_buttons: serde_json::Value,
    #[serde(default)]
    mouse_scroll_up: serde_json::Value,
    #[serde(default)]
    mouse_scroll_down: serde_json::Value,
    #[serde(default)]
    enable_drag_bindings: bool,
    #[serde(default)]
    disable_pressure: bool,
    #[serde(default)]
    disable_tilt: bool,
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
            aux_buttons: serde_json::Value::Null,
            wheel_bindings: serde_json::Value::Null,
            mouse_buttons: serde_json::Value::Null,
            mouse_scroll_up: serde_json::Value::Null,
            mouse_scroll_down: serde_json::Value::Null,
            enable_drag_bindings: false,
            disable_pressure: false,
            disable_tilt: false,
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
        MOUSE_SCROLL_BINDING => {
            use crate::output::buttons::{ScrollAction, ScrollAxis};
            let integer = |name: &str, constructor: i32, default: i32| -> Result<i32, String> {
                match store.settings.iter().rev().find(|setting| setting.property == name).map(|setting| &setting.value) {
                    None => Ok(constructor),
                    Some(serde_json::Value::Null) => Ok(default),
                    Some(value) => value.as_i64().and_then(|value| i32::try_from(value).ok())
                        .or_else(|| value.as_str().and_then(|value| value.parse().ok()))
                        .ok_or_else(|| format!("scroll {name} must be a signed 32-bit integer")),
                }
            };
            // Direction's upstream string setter uses Enum.TryParse, which
            // also accepts numeric strings. Scroll() treats any nonzero enum
            // value as horizontal; an invalid name keeps the initial vertical.
            let direction = store.settings.iter().rev().find(|setting| setting.property == "Direction").map(|setting| &setting.value);
            let horizontal = match direction {
                Some(serde_json::Value::String(value)) => value.trim() == "Horizontal"
                    || value.trim().parse::<i32>().is_ok_and(|value| value != 0),
                Some(value) => value.as_i64().and_then(|value| i32::try_from(value).ok()).is_some_and(|value| value != 0),
                None => false,
            };
            let axis = if horizontal { ScrollAxis::Horizontal } else { ScrollAxis::Vertical };
            let amount = integer("Amount", 120, 120)?;
            let amount = if amount == 0 { 1 } else { amount };
            // ApplySettings visits saved entries only: an absent Interval
            // keeps _interval=1; an explicit null selects the attribute's 300.
            let interval_ms = integer("Interval", 1, 300)?.max(1) as u32;
            Ok(ButtonAction::Scroll(ScrollAction { axis, amount, interval_ms }))
        }
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
    import_buttons(value, pen, "Bindings.PenButtons", "pen button", diagnostics)
}

/// One binding store (or null) as an action, or a diagnostic at `location`.
fn import_store(
    entry: &serde_json::Value,
    pen: bool,
    location: String,
    noun: &str,
    diagnostics: &mut Vec<ProfileDiagnostic>,
) -> ButtonAction {
    let action = if entry.is_null() {
        Ok(ButtonAction::None)
    } else {
        serde_json::from_value::<OtdStore>(entry.clone())
            .map_err(|error| format!("unreadable {noun} binding: {error}"))
            .and_then(|store| pen_button_action(Some(&store), pen))
    };
    action.unwrap_or_else(|message| {
        diagnostics.push(ProfileDiagnostic::unsupported(
            location,
            format!("{message}; this {noun} does nothing."),
        ));
        ButtonAction::None
    })
}

/// A `PluginSettingStoreCollection` of button bindings at `location`.
fn import_buttons(
    value: &serde_json::Value,
    pen: bool,
    location: &str,
    noun: &str,
    diagnostics: &mut Vec<ProfileDiagnostic>,
) -> Vec<ButtonAction> {
    let Some(entries) = value.as_array() else {
        return Vec::new();
    };
    let mut actions = Vec::with_capacity(entries.len().min(MAX_PEN_BUTTONS));
    for (index, entry) in entries.iter().take(MAX_PEN_BUTTONS).enumerate() {
        actions.push(import_store(
            entry,
            pen,
            format!("{location}[{index}]"),
            noun,
            diagnostics,
        ));
    }
    if entries.len() > MAX_PEN_BUTTONS {
        diagnostics.push(ProfileDiagnostic::unsupported(
            location,
            format!("Only the first {MAX_PEN_BUTTONS} {noun}s are applied."),
        ));
    }
    actions
}

/// Upstream's `Bindings.WheelBindings`: one `WheelBindingSettings` per wheel.
fn import_wheels(
    value: &serde_json::Value,
    pen: bool,
    diagnostics: &mut Vec<ProfileDiagnostic>,
) -> Vec<WheelBinding> {
    let Some(entries) = value.as_array() else {
        return Vec::new();
    };
    let mut wheels = Vec::new();
    for (index, entry) in entries
        .iter()
        .take(crate::reports::MAX_WHEELS)
        .enumerate()
    {
        let at = format!("Bindings.WheelBindings[{index}]");
        let threshold = |name: &str| {
            entry[name]
                .as_f64()
                .filter(|value| value.is_finite())
                .map(|value| value as f32)
        };
        wheels.push(WheelBinding {
            clockwise: import_store(
                &entry["ClockwiseRotation"],
                pen,
                format!("{at}.ClockwiseRotation"),
                "wheel rotation",
                diagnostics,
            ),
            counter_clockwise: import_store(
                &entry["CounterClockwiseRotation"],
                pen,
                format!("{at}.CounterClockwiseRotation"),
                "wheel rotation",
                diagnostics,
            ),
            clockwise_threshold: threshold("ClockwiseActivationThreshold"),
            counter_clockwise_threshold: threshold("CounterClockwiseActivationThreshold"),
            buttons: import_buttons(
                &entry["WheelButtons"],
                pen,
                &format!("{at}.WheelButtons"),
                "wheel button",
                diagnostics,
            ),
        });
    }
    if entries.len() > crate::reports::MAX_WHEELS {
        diagnostics.push(ProfileDiagnostic::unsupported(
            "Bindings.WheelBindings",
            format!(
                "Only the first {} wheels are applied.",
                crate::reports::MAX_WHEELS
            ),
        ));
    }
    wheels
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

/// Resolves ranges for profile editing without requiring runtime parser support.
/// A named tablet never silently acquires another tablet's dimensions.
pub fn spec_for_tablet(name: &str) -> Result<TabletSpec, String> {
    spec_for_tablet_in(name, configured_tablets()?.as_ref())
}

fn spec_for_tablet_in(name: &str, database: &crate::tablets::Database) -> Result<TabletSpec, String> {
    let configuration = database.entries().iter()
        .filter_map(crate::tablets::Entry::usable)
        .find(|configuration| configuration.name == name)
        .ok_or_else(|| format!("no usable tablet configuration is named {name}"))?;
    TabletSpec::from_configuration(configuration)
}

/// The specification of a named tablet whose parser the runtime supports.
pub fn runtime_tablet(name: &str) -> Result<TabletSpec, String> {
    runtime_tablet_in(name, configured_tablets()?.as_ref())
}

/// The effective configuration set, shared by import and runtime selection.
/// This performs setup-time file I/O; report processing uses the selected spec.
pub fn configured_tablets() -> Result<std::borrow::Cow<'static, crate::tablets::Database>, String> {
    let directory = configurations_directory();
    let directory = match directory {
        Some(directory) if directory.try_exists().map_err(|error| format!("cannot inspect tablet configurations: {error}"))? => Some(directory),
        _ => None,
    };
    tablets_from_directory(directory.as_deref()).map(|(database, _)| database)
}

pub fn configurations_directory() -> Option<PathBuf> {
    otd_settings_path().and_then(|path| path.parent().map(|p| p.join("Configurations")))
}

/// One configuration read per operation; an explicit directory must be readable.
pub fn tablets_from_directory(directory: Option<&Path>) -> Result<(std::borrow::Cow<'static, crate::tablets::Database>, usize), String> {
    use crate::tablets::{Database, read_directory};
    let files = match directory {
        Some(directory) => read_directory(directory).map_err(|error| error.to_string())?,
        None => Vec::new(),
    };
    let database = if files.is_empty() {
        std::borrow::Cow::Borrowed(Database::builtin())
    } else {
        std::borrow::Cow::Owned(Database::with_overrides(&files))
    };
    Ok((database, files.len()))
}

pub fn runtime_tablet_in(name: &str, database: &crate::tablets::Database) -> Result<TabletSpec, String> {
    use crate::tablets::{Entry, ParserSupport, parser_support};
    let configuration = database
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

    /// Only this tablet's saved profile. Absence permits defaults; a rejected
    /// profile is an error and must never silently activate different output.
    pub fn load_otd_tablet(tablet: &str) -> Result<Option<Self>, String> {
        let Some(path) = otd_settings_path() else { return Ok(None); };
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(format!("cannot read {}: {error}", path.display())),
        };
        let settings: OtdSettings = serde_json::from_str(&text)
            .map_err(|error| format!("invalid OpenTabletDriver settings: {error}"))?;
        let Some(index) = settings.profiles.iter().position(|profile| {
            profile.get("Tablet").and_then(serde_json::Value::as_str) == Some(tablet)
        }) else { return Ok(None); };
        let database = configured_tablets()?;
        Self::from_otd_settings(&settings, &text, &path, index, ImportOptions::default(), &database).map(Some)
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
        let database = configured_tablets()?;
        let selected_index = connected
            .iter()
            .find_map(|name| tablets.iter().position(|tablet| tablet == name))
            .or_else(|| tablets.iter().position(|tablet| *tablet == "Wacom PTH-660"))
            .or_else(|| {
                tablets
                    .iter()
                    .position(|tablet| runtime_tablet_in(tablet, &database).is_ok())
            })
            .ok_or("OpenTabletDriver settings have no profile for a tablet this driver supports")?;
        Self::from_otd_settings(&settings, text, path, selected_index, options, &database)
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
        let database = configured_tablets()?;
        Self::from_otd_settings(&settings, text, path, selected_index, options, &database)
    }

    fn from_otd_settings(
        settings: &OtdSettings,
        text: &str,
        path: &Path,
        selected_index: usize,
        options: ImportOptions,
        database: &crate::tablets::Database,
    ) -> Result<Self, String> {
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
        let tablet = spec_for_tablet_in(tablet_name, database)?;
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
        let aux_buttons = import_buttons(
            &selected.bindings.aux_buttons,
            pen,
            "Bindings.AuxButtons",
            "express key",
            &mut diagnostics,
        );
        let mouse_buttons = import_buttons(&selected.bindings.mouse_buttons, false, "Bindings.MouseButtons", "mouse button", &mut diagnostics);
        let mouse_scroll_up = import_store(&selected.bindings.mouse_scroll_up, false, "Bindings.MouseScrollUp".into(), "mouse scroll up", &mut diagnostics);
        let mouse_scroll_down = import_store(&selected.bindings.mouse_scroll_down, false, "Bindings.MouseScrollDown".into(), "mouse scroll down", &mut diagnostics);
        let wheels = import_wheels(&selected.bindings.wheel_bindings, pen, &mut diagnostics);
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
                    drag_only: selected.bindings.enable_drag_bindings,
                    disable_pressure: selected.bindings.disable_pressure,
                    disable_tilt: selected.bindings.disable_tilt,
                    tip_threshold_percent: Some(0.0),
                    eraser_threshold_percent: Some(0.0),
                    tip_threshold_raw: Some(1),
                    eraser_threshold_raw: Some(1),
                }
            } else {
                ContactPolicy {
                    tip_enabled,
                    eraser_enabled,
                    drag_only: selected.bindings.enable_drag_bindings,
                    disable_pressure: selected.bindings.disable_pressure,
                    disable_tilt: selected.bindings.disable_tilt,
                    tip_threshold_percent: Some(selected.bindings.tip_activation_threshold as f32),
                    eraser_threshold_percent: Some(selected.bindings.eraser_activation_threshold as f32),
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
            aux_buttons,
            mouse_buttons,
            mouse_scroll_up,
            mouse_scroll_down,
            wheels,
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
        for filter in raw
            .radial_follow
            .iter()
            .chain(raw.disabled_radial_follow.iter())
        {
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
                .validate_values()
            })
            .transpose()?;
        let target_tablet = raw.tablet;
        let pen_buttons = match &raw.pen_buttons {
            None => default_pen_buttons(),
            Some(texts) => parse_actions(texts, "pen_buttons")?,
        };
        let aux_buttons = parse_actions(&raw.aux_buttons, "aux_buttons")?;
        let mouse_buttons = parse_actions(&raw.mouse_buttons, "mouse_buttons")?;
        let mouse_scroll_up = raw.mouse_scroll_up.as_deref().unwrap_or("none").parse::<ButtonAction>()?;
        let mouse_scroll_down = raw.mouse_scroll_down.as_deref().unwrap_or("none").parse::<ButtonAction>()?;
        if raw.wheels.len() > crate::reports::MAX_WHEELS {
            return Err(format!(
                "at most {} wheels are supported",
                crate::reports::MAX_WHEELS
            ));
        }
        let wheels = raw
            .wheels
            .iter()
            .enumerate()
            .map(|(index, wheel)| wheel.parse(index))
            .collect::<Result<_, _>>()?;
        let mut profile = Self {
            settings_revision: raw.settings_revision,
            imported_otd: raw.imported_otd,
            preserved_fields: archived,
            diagnostics,
            otd_mapping: raw.absolute,
            contact: raw.bindings,
            pen_buttons,
            aux_buttons,
            mouse_buttons,
            mouse_scroll_up,
            mouse_scroll_down,
            wheels,
            output: raw.output,
            radial_follow: raw.radial_follow,
            disabled_radial_follow: raw.disabled_radial_follow,
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
        profile.contact.validate_percentages()?;
        if profile.crop.width == 0 || profile.crop.height == 0
            || profile.crop.x.checked_add(profile.crop.width).is_none()
            || profile.crop.y.checked_add(profile.crop.height).is_none()
        {
            return Err("crop must be nonzero without coordinate overflow".into());
        }
        if let Some(name) = profile.tablet_name()? {
            profile = profile.for_tablet(spec_for_tablet(&name)?)?;
        }
        if !matches!(profile.rotation, 0 | 90 | 180 | 270) {
            return Err("rotation must be 0, 90, 180, or 270".into());
        }
        profile.validate_filter_execution()?;
        Ok(profile)
    }

    pub fn validate_actions(&self) -> Result<(), String> {
        let actions = self.pen_buttons.iter().chain(&self.aux_buttons).chain(&self.mouse_buttons)
            .chain([&self.mouse_scroll_up, &self.mouse_scroll_down])
            .chain(self.wheels.iter().flat_map(|wheel| [&wheel.clockwise, &wheel.counter_clockwise].into_iter().chain(&wheel.buttons)));
        for action in actions {
            if let ButtonAction::Scroll(scroll) = action { scroll.validate()?; }
        }
        Ok(())
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

    /// Where the built-in Radial Follow runs among the enabled DLL filters
    /// that `PluginChain` loads (all but tools), or `None` to run it before
    /// them all. OpenTabletDriver runs filters in profile order, so the port
    /// takes the place of a tablet-space Radial Follow DLL entry, enabled or
    /// not. Switching between the two then keeps the filter order, which
    /// changes how it combines with other pre-transform filters such as
    /// resamplers.
    pub fn builtin_filter_slot(&self) -> Option<usize> {
        if self.radial_follow.is_empty() {
            return None;
        }
        let anchor = self.plugins.iter().position(|plugin| {
            plugin.kind == crate::plugins::PluginKind::Dotnet && plugin.type_name == FILTER_PATH
        })?;
        let slot = self.plugins[..anchor]
            .iter()
            .filter(|plugin| {
                plugin.enabled && plugin.kind != crate::plugins::PluginKind::DotnetTool
            })
            .count();
        (slot > 0).then_some(slot)
    }

    pub fn tablet_name(&self) -> Result<Option<String>, String> {
        if let Some(name) = &self.target_tablet {
            // Explicit automatic selection overrides an imported document's name.
            // None retains the legacy imported-identity fallback on older profiles.
            return Ok((name != "*").then(|| name.clone()));
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
        self.validate_runtime_tablet_in(configured_tablets()?.as_ref())
    }

    pub fn validate_runtime_tablet_in(&self, database: &crate::tablets::Database) -> Result<(), String> {
        if let Some(name) = self.tablet_name()? {
            runtime_tablet_in(&name, database)?;
        }
        Ok(())
    }

    /// This profile for the tablet the runtime selected. Raw pressure
    /// thresholds must fit the tablet's pressure range.
    pub fn for_tablet(&self, spec: TabletSpec) -> Result<Self, String> {
        self.contact.validate_percentages()?;
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
        if self.crop != Crop::default() && !self.crop.valid_for(spec) {
            return Err(format!("crop must be nonzero and within 0..{} X, 0..{} Y", spec.max_x, spec.max_y));
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
        self.contact.validate_percentages()?;
        self.validate_actions()?;
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
            aux_buttons: self.aux_buttons.iter().map(ToString::to_string).collect(),
            mouse_buttons: self.mouse_buttons.iter().map(ToString::to_string).collect(),
            mouse_scroll_up: (self.mouse_scroll_up != ButtonAction::None).then(|| self.mouse_scroll_up.to_string()),
            mouse_scroll_down: (self.mouse_scroll_down != ButtonAction::None).then(|| self.mouse_scroll_down.to_string()),
            wheels: {
                // Trailing wheels that do nothing need no entry.
                let used = self
                    .wheels
                    .iter()
                    .rposition(|wheel| *wheel != WheelBinding::default())
                    .map_or(0, |last| last + 1);
                self.wheels[..used].iter().map(RawWheel::from_binding).collect()
            },
            output: self.output,
            radial_follow: self.radial_follow.clone(),
            disabled_radial_follow: self.disabled_radial_follow,
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

    #[test]
    fn built_in_radial_follow_takes_the_place_of_its_dll_entry() {
        use crate::plugins::{PluginConfig, PluginKind};
        let plugin = |type_name: &str, kind, enabled| PluginConfig {
            path: "plugin.dll".into(),
            kind,
            enabled,
            type_name: type_name.into(),
            settings_json: "{}".into(),
        };
        let mut profile = Profile {
            radial_follow: vec![RadialFollowSettings::default()],
            plugins: vec![
                plugin("TemporalResampler", PluginKind::Dotnet, true),
                plugin("Disabled", PluginKind::Dotnet, false),
                plugin("Tool", PluginKind::DotnetTool, true),
                plugin(FILTER_PATH, PluginKind::Dotnet, false),
                plugin("After", PluginKind::Dotnet, true),
            ],
            ..Profile::default()
        };
        // Only the enabled filter before the DLL entry runs before it.
        assert_eq!(profile.builtin_filter_slot(), Some(1));
        profile.plugins.swap(0, 3);
        assert_eq!(profile.builtin_filter_slot(), None, "first in the list");
        profile.plugins.remove(0);
        assert_eq!(
            profile.builtin_filter_slot(),
            None,
            "no DLL entry to replace"
        );
        profile
            .plugins
            .push(plugin(FILTER_PATH, PluginKind::Dotnet, false));
        assert_eq!(profile.builtin_filter_slot(), Some(2));
        profile.radial_follow.clear();
        assert_eq!(profile.builtin_filter_slot(), None, "built-in disabled");
    }

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
        assert_eq!(loaded.tablet, spec_for_tablet("Wacom CTL-4100").unwrap());
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
        assert!(Profile::from_toml_text("[disabled_radial_follow]\nouter_radius=nan\n", &path).is_err());
        // Thresholds are checked against the tablet that runs the profile,
        // since other tablets report more pressure levels than the PTH-660.
        let above = Profile::from_toml_text("[bindings]\ntip_threshold_raw=8192\n", &path).unwrap();
        assert!(above.for_tablet(crate::spec::TabletSpec::PTH_660).is_err());
        let deeper = crate::spec::TabletSpec {
            max_pressure: 16_383,
            ..crate::spec::TabletSpec::PTH_660
        };
        assert_eq!(
            above.for_tablet(deeper).unwrap().contact.tip_threshold_raw,
            Some(8192)
        );
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
    fn edited_pen_buttons_export_and_reimport_all_supported_action_types() {
        let mut profile = import_pen_buttons_from(serde_json::json!([]));
        assert!(profile.to_otd_json().is_ok());
        profile.pen_buttons = ["barrel:3", "mouse:forward", "keys:Escape", "keys:Control+Shift+Z", "none"]
            .into_iter().map(|action| action.parse().unwrap()).collect();
        let exported = profile.to_otd_json().unwrap();
        let json: serde_json::Value = serde_json::from_str(&exported).unwrap();
        let buttons = &json["Profiles"][0]["Bindings"]["PenButtons"];
        assert_eq!(buttons[0]["Path"], ADAPTIVE_BINDING);
        assert_eq!(buttons[0]["Settings"][0]["Value"], "Button 3");
        assert_eq!(buttons[1]["Path"], MOUSE_BINDING);
        assert_eq!(buttons[1]["Settings"][0]["Value"], "Forward");
        assert_eq!(buttons[2]["Path"], KEY_BINDING);
        assert_eq!(buttons[2]["Settings"][0]["Value"], "Escape");
        assert_eq!(buttons[3]["Path"], MULTI_KEY_BINDING);
        assert_eq!(buttons[3]["Settings"][0]["Value"], "LeftControl+LeftShift+Z");
        assert!(buttons[4].is_null());
        let reimported = Profile::from_otd_text(&exported, Path::new("exported.json")).unwrap();
        assert_eq!(reimported.pen_buttons, profile.pen_buttons);
    }

    #[test]
    fn edited_pen_buttons_preserve_unknown_stores_and_same_type_properties() {
        let unknown = serde_json::json!({"Path": "Plugin.UnknownBinding", "Enable": true,
            "Settings": [{"Property": "Mystery", "Value": [null, {"x": 17}]}]});
        let mut original = store(MOUSE_BINDING, "Button", "Right".into());
        original["Metadata"] = serde_json::json!({"keep": true});
        original["Settings"].as_array_mut().unwrap().push(serde_json::json!(
            {"Property": "FutureProperty", "Value": "preserved"}
        ));
        let mut profile = import_pen_buttons_from(serde_json::json!([original.clone(), unknown.clone()]));
        profile.pen_buttons[0] = "mouse:left".parse().unwrap();
        let exported: serde_json::Value = serde_json::from_str(&profile.to_otd_json().unwrap()).unwrap();
        original["Settings"][0]["Value"] = "Left".into();
        assert_eq!(exported["Profiles"][0]["Bindings"]["PenButtons"], serde_json::json!([original, unknown]));
        profile.pen_buttons[1] = "mouse:middle".parse().unwrap();
        let error = profile.to_otd_json().unwrap_err();
        assert!(error.contains("pen button 2") && error.contains("unsupported"), "{error}");
    }

    #[test]
    fn edited_pen_buttons_change_known_types_and_disable_without_losing_properties() {
        let source = store(ADAPTIVE_BINDING, "Binding", "Button 1".into());
        let mut profile = import_pen_buttons_from(serde_json::json!([source.clone()]));
        profile.pen_buttons[0] = "keys:Escape".parse().unwrap();
        let exported = profile.to_otd_json().unwrap();
        let reimported = Profile::from_otd_text(&exported, Path::new("edited.json")).unwrap();
        assert_eq!(reimported.pen_buttons, profile.pen_buttons);
        profile.pen_buttons[0] = ButtonAction::None;
        let exported: serde_json::Value = serde_json::from_str(&profile.to_otd_json().unwrap()).unwrap();
        let mut disabled = source;
        disabled["Enable"] = false.into();
        assert_eq!(exported["Profiles"][0]["Bindings"]["PenButtons"][0], disabled);
    }

    #[test]
    fn edited_pen_buttons_refuse_type_changes_that_would_drop_unknown_data() {
        for setting in [
            serde_json::json!({"Property": "Unknown", "Value": 42}),
            serde_json::json!({"Property": "Binding", "Value": "Button 1", "Metadata": "keep"}),
        ] {
            let mut source = store(ADAPTIVE_BINDING, "Binding", "Button 1".into());
            source["Settings"].as_array_mut().unwrap().push(setting);
            let mut profile = import_pen_buttons_from(serde_json::json!([source]));
            profile.pen_buttons[0] = "keys:Escape".parse().unwrap();
            let error = profile.to_otd_json().unwrap_err();
            assert!(error.contains("unknown source properties"), "{error}");
        }
    }

    #[test]
    fn edited_pen_buttons_reject_invalid_native_actions_and_preserve_over_limit_source() {
        let unknown = serde_json::json!({"Path": "Plugin.UnknownBinding", "Enable": true,
            "Settings": [{"Property": "Keep", "Value": 42}]});
        let mut source = vec![serde_json::Value::Null; MAX_PEN_BUTTONS];
        source.push(unknown.clone());
        let mut profile = import_pen_buttons_from(serde_json::json!(source));
        profile.pen_buttons[0] = "mouse:right".parse().unwrap();
        let exported: serde_json::Value = serde_json::from_str(&profile.to_otd_json().unwrap()).unwrap();
        assert_eq!(exported["Profiles"][0]["Bindings"]["PenButtons"][64], unknown);
        profile.pen_buttons.pop();
        assert!(profile.to_otd_json().unwrap_err().contains("cannot shrink"));
        for action in [ButtonAction::Barrel(4), ButtonAction::Keys(vec![]),
            ButtonAction::Keys(vec![crate::actions::KeyboardUsage::new(0xffff).unwrap()])] {
            let mut profile = import_pen_buttons_from(serde_json::json!([]));
            profile.pen_buttons.push(action);
            assert!(profile.to_otd_json().is_err());
        }
    }

    #[test]
    fn pen_buttons_export_preserves_mouse_action_when_output_changes_to_pen() {
        let mut json = relative_profile();
        json["Profiles"][0]["OutputMode"]["Path"] = "OpenTabletDriver.Desktop.Output.AbsoluteMode".into();
        json["Profiles"][0]["AbsoluteModeSettings"] = serde_json::json!({
            "Display": {"Width": 2560, "Height": 1440, "X": 1280, "Y": 720, "Rotation": 0},
            "Tablet": {"Width": 85, "Height": 47.8125, "X": 110, "Y": 23.90625, "Rotation": 0},
            "EnableClipping": true, "EnableAreaLimiting": false
        });
        json["Profiles"][0]["Bindings"]["PenButtons"] = serde_json::json!([
            store(ADAPTIVE_BINDING, "Binding", "Tip".into())
        ]);
        let mut profile = Profile::from_otd_text(&json.to_string(), Path::new("mouse.json")).unwrap();
        profile.output = OutputKind::Pen;
        let exported = profile.to_otd_json().unwrap();
        let reimported = Profile::from_otd_text(&exported, Path::new("pen.json")).unwrap();
        assert_eq!(reimported.pen_buttons[0].to_string(), "mouse:left");
        assert_eq!(reimported.output, OutputKind::Pen);
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

    fn import_bindings(bindings: serde_json::Value) -> Profile {
        let mut json = relative_profile();
        for (key, value) in bindings.as_object().unwrap() {
            json["Profiles"][0]["Bindings"][key] = value.clone();
        }
        Profile::from_otd_text(&json.to_string(), Path::new("bindings.json")).unwrap()
    }

    #[test]
    fn imports_express_keys_and_wheels_with_their_thresholds() {
        let profile = import_bindings(serde_json::json!({
            "AuxButtons": [
                store(KEY_BINDING, "Key", "E".into()),
                null,
                store(MULTI_KEY_BINDING, "Keys", "Control+Z".into()),
                store("OpenTabletDriver.Desktop.Binding.PresetBinding", "Preset", "Art".into()),
            ],
            "WheelBindings": [{
                "WheelButtons": [store(MOUSE_BINDING, "Button", "Middle".into())],
                "ClockwiseRotation": store(KEY_BINDING, "Key", "PageDown".into()),
                "ClockwiseActivationThreshold": 15.0,
                "CounterClockwiseRotation": null,
                "CounterClockwiseActivationThreshold": 5.0,
                "StepSize": 5.0
            }]
        }));
        let texts: Vec<String> = profile.aux_buttons.iter().map(ToString::to_string).collect();
        assert_eq!(texts, ["keys:E", "none", "keys:LeftControl+Z", "none"]);
        let wheel = &profile.wheels[0];
        assert_eq!(wheel.clockwise.to_string(), "keys:PageDown");
        assert_eq!(wheel.counter_clockwise, ButtonAction::None);
        assert_eq!(
            (wheel.clockwise_threshold, wheel.counter_clockwise_threshold),
            (Some(15.0), Some(5.0))
        );
        assert_eq!(wheel.buttons[0].to_string(), "mouse:middle");
        let unsupported: Vec<&str> = profile
            .diagnostics
            .iter()
            .filter(|d| d.location.contains("AuxButtons") || d.location.contains("WheelBindings"))
            .map(|d| d.location.as_str())
            .collect();
        assert_eq!(unsupported, ["Bindings.AuxButtons[3]"], "only the preset binding");
    }

    #[test]
    fn express_keys_and_wheels_survive_a_toml_save_and_reload() {
        let profile = Profile::from_toml_text(
            concat!(
                "aux_buttons = [\"keys:Control+Z\", \"none\", \"mouse:right\"]\n",
                "[[wheels]]\n",
                "clockwise = \"keys:PageDown\"\n",
                "counter_clockwise = \"keys:PageUp\"\n",
                "clockwise_threshold = 10.0\n",
                "buttons = [\"keys:Escape\"]\n",
            ),
            Path::new("aux.toml"),
        )
        .unwrap();
        assert_eq!(profile.aux_buttons.len(), 3);
        assert_eq!(profile.wheels[0].clockwise_threshold, Some(10.0));
        assert_eq!(profile.wheels[0].counter_clockwise_threshold, None);
        let text = profile.to_toml().unwrap();
        let reloaded = Profile::from_toml_text(&text, Path::new("aux.toml")).unwrap();
        assert_eq!(reloaded.aux_buttons, profile.aux_buttons);
        assert_eq!(reloaded.wheels, profile.wheels);
        // Nothing bound writes nothing.
        let empty = Profile {
            wheels: vec![crate::output::buttons::WheelBinding::default(); 2],
            ..Profile::default()
        };
        let text = empty.to_toml().unwrap();
        assert!(!text.contains("wheels") && !text.contains("aux_buttons"), "{text}");
        for bad in [
            "aux_buttons = [\"keys:Mute\"]\n",
            "[[wheels]]\nclockwise = \"wheel:1\"\n",
            "[[wheels]]\nclockwise_threshold = 0.0\n",
            "[[wheels]]\ncounter_clockwise_threshold = -5.0\n",
            "[[wheels]]\nspeed = 2\n",
        ] {
            assert!(Profile::from_toml_text(bad, Path::new("bad.toml")).is_err(), "{bad}");
        }
    }

    #[test]
    fn edited_express_keys_and_wheels_export_and_reimport() {
        let mut profile = import_bindings(serde_json::json!({
            "AuxButtons": [store(KEY_BINDING, "Key", "E".into()), null],
        }));
        profile.aux_buttons = ["keys:E", "keys:Control+Z", "mouse:middle"]
            .into_iter()
            .map(|action| action.parse().unwrap())
            .collect();
        profile.wheels = vec![crate::output::buttons::WheelBinding {
            clockwise: "keys:PageDown".parse().unwrap(),
            counter_clockwise_threshold: Some(20.0),
            buttons: vec!["keys:Escape".parse().unwrap()],
            ..Default::default()
        }];
        let exported = profile.to_otd_json().unwrap();
        let json: serde_json::Value = serde_json::from_str(&exported).unwrap();
        let bindings = &json["Profiles"][0]["Bindings"];
        assert_eq!(bindings["AuxButtons"][1]["Path"], MULTI_KEY_BINDING);
        assert_eq!(bindings["AuxButtons"][2]["Settings"][0]["Value"], "Middle");
        let wheel = &bindings["WheelBindings"][0];
        assert_eq!(wheel["ClockwiseRotation"]["Settings"][0]["Value"], "PageDown");
        assert!(wheel["CounterClockwiseRotation"].is_null());
        // A new wheel gets upstream's one-step thresholds unless set.
        assert_eq!(wheel["ClockwiseActivationThreshold"], 5.0);
        assert_eq!(wheel["CounterClockwiseActivationThreshold"], 20.0);
        let reimported = Profile::from_otd_text(&exported, Path::new("exported.json")).unwrap();
        assert_eq!(reimported.aux_buttons, profile.aux_buttons);
        assert_eq!(reimported.wheels[0].clockwise, profile.wheels[0].clockwise);
        assert_eq!(reimported.wheels[0].buttons, profile.wheels[0].buttons);
        assert_eq!(reimported.wheels[0].counter_clockwise_threshold, Some(20.0));
        // Shortening the archived list would drop source entries.
        profile.aux_buttons.truncate(1);
        assert!(profile.to_otd_json().is_err());
    }
    #[test]
    fn mouse_and_binding_policies_round_trip_native_and_otd_without_losing_unknown_stores() {
        let mut profile = import_bindings(serde_json::json!({
            "EnableDragBindings": true, "DisablePressure": true, "DisableTilt": true,
            "MouseButtons": [store(MOUSE_BINDING, "Button", "Right".into()), null],
        }));
        assert!(profile.contact.drag_only && profile.contact.disable_pressure && profile.contact.disable_tilt);
        assert_eq!(profile.mouse_buttons[0].to_string(), "mouse:right");
        let text = profile.to_toml().unwrap();
        let reloaded = Profile::from_toml_text(&text, Path::new("copy.toml")).unwrap();
        assert!(reloaded.contact.drag_only && reloaded.contact.disable_pressure && reloaded.contact.disable_tilt);
        assert_eq!(reloaded.mouse_buttons, profile.mouse_buttons);
        profile.contact.drag_only = false;
        profile.mouse_buttons[1] = "keys:Control+Z".parse().unwrap();
        let exported = profile.to_otd_json().unwrap();
        let imported = Profile::from_otd_text(&exported, Path::new("export.json")).unwrap();
        assert!(!imported.contact.drag_only);
        assert!(imported.contact.disable_pressure && imported.contact.disable_tilt);
        assert_eq!(imported.mouse_buttons, profile.mouse_buttons);
    }

    #[test]
    fn drag_pressure_uses_upstream_threshold_remap_at_equality_zero_and_maximum() {
        let mut policy = ContactPolicy { tip_threshold_percent: Some(50.0), ..Default::default() };
        assert_eq!(policy.drag_pressure(Some(50), 100, false), Some(0));
        assert_eq!(policy.drag_pressure(Some(51), 100, false), Some(2));
        assert_eq!(policy.drag_pressure(Some(100), 100, false), Some(100));
        policy.tip_threshold_percent = Some(100.0);
        assert_eq!(policy.drag_pressure(Some(99), 100, false), Some(0));
        assert_eq!(policy.drag_pressure(Some(100), 100, false), Some(100));
        policy.tip_threshold_percent = Some(0.0);
        assert_eq!(policy.drag_pressure(Some(0), 100, false), Some(0));
        assert_eq!(policy.drag_pressure(Some(1), 100, false), Some(1));
        assert_eq!(policy.drag_pressure(None, 100, false), None);
        policy.tip_threshold_percent = None;
        policy.tip_threshold_raw = Some(51);
        assert_eq!(policy.drag_pressure(Some(50), 100, false), Some(0));
        assert!(policy.drag_pressure(Some(51), 100, false).unwrap() > 0);
    }

    #[test]
    fn ordinary_native_profiles_reject_invalid_threshold_percentages() {
        for field in ["tip_threshold_percent", "eraser_threshold_percent"] {
            for value in ["nan", "inf", "-inf", "-1", "101"] {
                let text = format!("[bindings]\n{field} = {value}\n");
                assert!(Profile::from_toml_text(&text, Path::new("unnamed.toml")).is_err(), "{text}");
            }
        }
        let mut profile = Profile::default();
        profile.contact.tip_threshold_percent = Some(f32::NAN);
        assert!(profile.to_toml().is_err());
        assert!(crate::pipeline::ReportPipeline::new(&profile).is_err());
    }

    #[test]
    fn mouse_scroll_actions_round_trip_with_upstream_amount_and_interval() {
        let binding = serde_json::json!({ "Path": MOUSE_SCROLL_BINDING, "Enable": true, "Settings": [
            {"Property":"Direction", "Value":"Horizontal"}, {"Property":"Amount", "Value":-240}, {"Property":"Interval", "Value":25},
            {"Property":"Retain", "Value":"custom"}
        ] });
        let mut profile = import_bindings(serde_json::json!({"MouseScrollUp": binding}));
        assert_eq!(profile.mouse_scroll_up.to_string(), "scroll:horizontal:-240:25");
        assert_eq!(profile.mouse_scroll_down, ButtonAction::None);
        assert!(!profile.diagnostics.iter().any(|diagnostic| diagnostic.location.contains("MouseScroll")));
        let text = profile.to_toml().unwrap();
        let copy = Profile::from_toml_text(&text, Path::new("scroll.toml")).unwrap();
        assert_eq!(copy.mouse_scroll_up, profile.mouse_scroll_up);
        profile.mouse_scroll_up = "scroll:vertical:-120:300".parse().unwrap();
        profile.mouse_scroll_down = "scroll:down".parse().unwrap();
        let exported: serde_json::Value = serde_json::from_str(&profile.to_otd_json().unwrap()).unwrap();
        let settings = exported["Profiles"][0]["Bindings"]["MouseScrollUp"]["Settings"].as_array().unwrap();
        assert!(settings.iter().any(|property| property["Property"] == "Retain" && property["Value"] == "custom"));
        let imported = Profile::from_otd_text(&exported.to_string(), Path::new("export.json")).unwrap();
        assert_eq!(imported.mouse_scroll_up, profile.mouse_scroll_up);
        assert_eq!(imported.mouse_scroll_down, profile.mouse_scroll_down);
        profile.mouse_scroll_up = "keys:Escape".parse().unwrap();
        assert!(profile.to_otd_json().unwrap_err().contains("unknown source properties"));
    }

    #[test]
    fn invalid_native_scroll_settings_are_rejected_without_a_named_tablet() {
        for text in ["mouse_scroll_up = \"scroll:vertical:0\"", "mouse_scroll_down = \"scroll:vertical:120:0\"", "pen_buttons = [\"scroll:horizontal:1:2147483648\"]"] {
            assert!(Profile::from_toml_text(text, Path::new("unnamed.toml")).is_err());
        }
        let mut profile = Profile::default();
        profile.mouse_scroll_up = ButtonAction::Scroll(crate::output::buttons::ScrollAction { axis: crate::output::buttons::ScrollAxis::Vertical, amount: 0, interval_ms: 1 });
        assert!(profile.to_toml().is_err());
        assert!(crate::pipeline::ReportPipeline::new(&profile).is_err());
    }

    #[test]
    fn imported_scroll_defaults_and_upstream_setter_normalization_are_retained() {
        let sparse = import_bindings(serde_json::json!({"MouseScrollDown": {
            "Path": MOUSE_SCROLL_BINDING, "Enable": true, "Settings": []
        }}));
        assert_eq!(sparse.mouse_scroll_down.to_string(), "scroll:vertical:120:1");
        for (direction, amount, interval, expected) in [
            (serde_json::Value::Null, serde_json::Value::Null, serde_json::Value::Null, "scroll:vertical:120:300"),
            (serde_json::json!("invalid"), serde_json::json!(0), serde_json::json!(-20), "scroll:vertical:1:1"),
            (serde_json::json!("1"), serde_json::json!("-120"), serde_json::json!("25"), "scroll:horizontal:-120:25"),
            (serde_json::json!(2), serde_json::json!(120), serde_json::json!(1), "scroll:horizontal:120:1"),
        ] {
            let profile = import_bindings(serde_json::json!({"MouseScrollDown": {
                "Path": MOUSE_SCROLL_BINDING, "Enable": true, "Settings": [
                    {"Property":"Direction", "Value":direction}, {"Property":"Amount", "Value":amount}, {"Property":"Interval", "Value":interval},
                ]
            }}));
            assert_eq!(profile.mouse_scroll_down.to_string(), expected);
        }
    }

}
