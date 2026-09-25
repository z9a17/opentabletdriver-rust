use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::mapping::{Crop, OtdArea, OtdMapping};
use crate::plugins::PluginConfig;
use crate::protocol::MAX_PRESSURE;
use crate::radial_follow::{FILTER_NAME, FILTER_PATH, RadialFollowSettings};
use crate::relative::RelativeSettings;

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
    pub radial_follow: Vec<RadialFollowSettings>,
    pub plugins: Vec<PluginConfig>,
    pub auto_enabled_radial_follow: usize,
    pub ignored_filters: usize,
    pub source: String,
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
            radial_follow: Vec::new(),
            plugins: Vec::new(),
            auto_enabled_radial_follow: 0,
            ignored_filters: 0,
            source: "built-in full-area defaults".into(),
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
    #[serde(default)]
    radial_follow: Vec<RadialFollowSettings>,
    #[serde(default)]
    plugins: Vec<PluginConfig>,
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

fn binding_enabled(store: Option<&OtdStore>, expected: &str) -> Result<bool, String> {
    let Some(store) = store else {
        return Ok(false);
    };
    if !store.enable {
        return Ok(false);
    }
    if store.path != "OpenTabletDriver.Desktop.Binding.AdaptiveBinding" {
        return Err(format!(
            "unsupported enabled {expected} binding: {}",
            store.path
        ));
    }
    let selected = store
        .settings
        .iter()
        .rev()
        .find(|setting| setting.property == "Binding")
        .and_then(|setting| setting.value.as_str());
    if selected != Some(expected) {
        return Err(format!(
            "unsupported enabled {expected} binding action: {selected:?}"
        ));
    }
    Ok(true)
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

/// First raw pressure at which OpenTabletDriver's tip or eraser binding
/// presses. Its `ThresholdBindingState` compares
/// `pressure / MaxPressure * 100 > threshold` in single precision and treats
/// a 100 % threshold as met at full pressure.
pub fn activation_raw(percent: f64) -> Result<u16, String> {
    if !percent.is_finite() || !(0.0..=100.0).contains(&percent) {
        return Err("tip/eraser activation threshold must be 0..100 percent".into());
    }
    let threshold = percent as f32;
    let presses = |raw: u16| {
        let value = f32::from(raw) / f32::from(MAX_PRESSURE) * 100.0;
        value > threshold || (threshold == 100.0 && value == 100.0)
    };
    // `presses` is monotonic and true at full pressure.
    let (mut low, mut high) = (0, MAX_PRESSURE);
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

impl Profile {
    pub fn load(path: Option<&Path>) -> Result<Self, String> {
        if let Some(path) = path {
            return Self::load_toml(path);
        }
        let Some(local_app_data) = env::var_os("LOCALAPPDATA") else {
            return Ok(Self::default());
        };
        let otd_path = PathBuf::from(local_app_data)
            .join("OpenTabletDriver")
            .join("settings.json");
        if otd_path.exists() {
            Self::load_otd(&otd_path)
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
        let settings: OtdSettings = serde_json::from_str(text)
            .map_err(|e| format!("invalid OpenTabletDriver settings {}: {e}", path.display()))?;
        let selected_index = settings
            .profiles
            .iter()
            .position(|profile| {
                profile.get("Tablet").and_then(serde_json::Value::as_str) == Some("Wacom PTH-660")
            })
            .ok_or("OpenTabletDriver settings have no Wacom PTH-660 profile")?;
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
        let mut diagnostics = schema::import_diagnostics(text, selected_index)?;
        if !selected.output_mode.enable {
            return Err(format!(
                "the {tablet_name} output mode is disabled; its settings remain available in the import preview"
            ));
        }
        let (otd_mapping, relative) = match selected.output_mode.path.as_str() {
            "OpenTabletDriver.Desktop.Output.AbsoluteMode" => {
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
                        .validate()?,
                    ),
                )
            }
            _ => {
                return Err(format!(
                    "unsupported {tablet_name} output mode: {}; choose Absolute Mode or Relative Mode",
                    selected.output_mode.path
                ));
            }
        };
        let mut import_binding = |store: Option<&OtdStore>, name| {
            binding_enabled(store, name).unwrap_or_else(|message| {
                diagnostics.push(ProfileDiagnostic::unsupported(
                    format!("Bindings.{name}"),
                    message,
                ));
                false
            })
        };
        let tip_enabled = import_binding(selected.bindings.tip_button.as_ref(), "Tip");
        let eraser_enabled = import_binding(selected.bindings.eraser_button.as_ref(), "Eraser");
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
            contact: ContactPolicy {
                tip_enabled,
                eraser_enabled,
                tip_threshold_raw: Some(activation_raw(
                    selected.bindings.tip_activation_threshold,
                )?),
                eraser_threshold_raw: Some(activation_raw(
                    selected.bindings.eraser_activation_threshold,
                )?),
            },
            radial_follow,
            auto_enabled_radial_follow,
            ignored_filters,
            source: format!("OpenTabletDriver settings: {}", path.display()),
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
        if raw.absolute.is_some()
            && (raw.monitor.is_some() || raw.crop.is_some() || raw.rotation.is_some())
        {
            return Err(
                "absolute areas cannot be mixed with monitor/crop/top-level rotation".into(),
            );
        }
        for threshold in [
            raw.bindings.tip_threshold_raw,
            raw.bindings.eraser_threshold_raw,
        ]
        .into_iter()
        .flatten()
        {
            if threshold > MAX_PRESSURE {
                return Err("binding threshold exceeds tablet pressure range".into());
            }
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
        let profile = Self {
            settings_revision: raw.settings_revision,
            imported_otd: raw.imported_otd,
            preserved_fields: archived,
            diagnostics,
            otd_mapping: raw.absolute,
            contact: raw.bindings,
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
            ..Self::default()
        };
        if !profile.crop.valid() {
            return Err("crop must be nonzero and within 0..44800 X, 0..29600 Y".into());
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

    pub fn validate_runtime_tablet(&self, supported_tablet: &str) -> Result<(), String> {
        if let Some(name) = self.tablet_name()?
            && name != supported_tablet
        {
            return Err(format!(
                "Profile targets {name}; this runtime currently supports {supported_tablet}. The profile can be stored or exported, but cannot be started on this device backend."
            ));
        }
        Ok(())
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
            radial_follow: self.radial_follow.clone(),
            plugins: self.plugins.clone(),
        };
        toml::to_string_pretty(&raw).map_err(|e| e.to_string())
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
