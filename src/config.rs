use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::mapping::{Crop, OtdArea, OtdMapping};
use crate::protocol::MAX_PRESSURE;
use crate::radial_follow::{FILTER_NAME, FILTER_PATH, RadialFollowSettings};

#[derive(Clone, Copy, Debug)]
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
    pub monitor: Option<usize>,
    pub crop: Crop,
    pub rotation: u16,
    pub device_path: Option<String>,
    pub otd_mapping: Option<OtdMapping>,
    pub contact: ContactPolicy,
    pub radial_follow: Vec<RadialFollowSettings>,
    pub auto_enabled_radial_follow: usize,
    pub ignored_filters: usize,
    pub source: String,
}

impl Default for Profile {
    fn default() -> Self {
        Self {
            monitor: None,
            crop: Crop::default(),
            rotation: 0,
            device_path: None,
            otd_mapping: None,
            contact: ContactPolicy::default(),
            radial_follow: Vec::new(),
            auto_enabled_radial_follow: 0,
            ignored_filters: 0,
            source: "built-in full-area defaults".into(),
        }
    }
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawProfile {
    monitor: Option<usize>,
    rotation: Option<u16>,
    crop: Option<RawCrop>,
    device_path: Option<String>,
}

#[derive(Deserialize)]
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
    profiles: Vec<OtdProfile>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct OtdProfile {
    tablet: String,
    output_mode: OtdStore,
    absolute_mode_settings: OtdAbsolute,
    bindings: OtdBindings,
    #[serde(default)]
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
#[serde(rename_all = "PascalCase")]
struct OtdBindings {
    #[serde(default)]
    tip_activation_threshold: f64,
    tip_button: Option<OtdStore>,
    #[serde(default)]
    eraser_activation_threshold: f64,
    eraser_button: Option<OtdStore>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct OtdStore {
    path: String,
    enable: bool,
    #[serde(default)]
    settings: Vec<OtdProperty>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct OtdProperty {
    property: String,
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
        .find(|setting| setting.property == name)
        .map(|setting| &setting.value)
    else {
        return Ok(default);
    };
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

/// OpenTabletDriver rewrites pressure before its tip binding. The resulting
/// integer pressure first becomes nonzero at this raw value.
fn activation_raw(percent: f64) -> Result<u16, String> {
    if !percent.is_finite() || !(0.0..=100.0).contains(&percent) {
        return Err("tip/eraser activation threshold must be 0..100 percent".into());
    }
    if percent == 100.0 {
        return Ok(MAX_PRESSURE);
    }
    let fraction = percent / 100.0;
    let first = (f64::from(MAX_PRESSURE) * fraction + 1.0 - fraction).ceil() as u16;
    Ok(first.min(MAX_PRESSURE))
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
        let text = fs::read_to_string(path).map_err(|e| {
            format!(
                "cannot read OpenTabletDriver settings {}: {e}",
                path.display()
            )
        })?;
        Self::from_otd_text(&text, path)
    }

    fn from_otd_text(text: &str, path: &Path) -> Result<Self, String> {
        let settings: OtdSettings = serde_json::from_str(text)
            .map_err(|e| format!("invalid OpenTabletDriver settings {}: {e}", path.display()))?;
        let selected = settings
            .profiles
            .into_iter()
            .find(|profile| profile.tablet == "Wacom PTH-660")
            .ok_or("OpenTabletDriver settings have no Wacom PTH-660 profile")?;
        if !selected.output_mode.enable
            || selected.output_mode.path != "OpenTabletDriver.Desktop.Output.AbsoluteMode"
        {
            return Err(format!(
                "the PTH-660 profile must use enabled Absolute Mode; found {}",
                selected.output_mode.path
            ));
        }
        let tip_enabled = binding_enabled(selected.bindings.tip_button.as_ref(), "Tip")?;
        let eraser_enabled = binding_enabled(selected.bindings.eraser_button.as_ref(), "Eraser")?;
        let mut radial_follow = Vec::new();
        let mut auto_enabled_radial_follow = 0;
        let mut ignored_filters = 0;
        for filter in &selected.filters {
            if filter.path == FILTER_PATH {
                radial_follow.push(radial_settings(filter)?);
                if !filter.enable {
                    auto_enabled_radial_follow += 1;
                }
            } else if filter.enable {
                ignored_filters += 1;
            }
        }
        Ok(Self {
            otd_mapping: Some(OtdMapping {
                display: selected.absolute_mode_settings.display,
                tablet: selected.absolute_mode_settings.tablet,
                clipping: selected.absolute_mode_settings.enable_clipping,
                limiting: selected.absolute_mode_settings.enable_area_limiting,
            }),
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
        let raw: RawProfile = toml::from_str(&text)
            .map_err(|e| format!("invalid profile {}: {e}", path.display()))?;
        let profile = Self {
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
        Ok(profile)
    }

    pub fn print_summary(&self) {
        println!("Profile: {}", self.source);
        if let Some(mapping) = self.otd_mapping {
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
        } else {
            println!(
                "Tablet crop: {:?}; rotation: {}°; monitor: {:?}",
                self.crop, self.rotation, self.monitor
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
    fn reads_absolute_area_and_tip_threshold_from_otd_profile() {
        let json = r#"{"Profiles":[{"Tablet":"Wacom PTH-660","OutputMode":{"Path":"OpenTabletDriver.Desktop.Output.AbsoluteMode","Enable":true},"Filters":[{"Path":"Example.Filter","Enable":true}],"AbsoluteModeSettings":{"Display":{"Width":2560,"Height":1440,"X":1280,"Y":720,"Rotation":0},"Tablet":{"Width":85,"Height":47.8125,"X":110,"Y":23.90625,"Rotation":0},"EnableClipping":true,"EnableAreaLimiting":false},"Bindings":{"TipActivationThreshold":1,"TipButton":{"Path":"OpenTabletDriver.Desktop.Binding.AdaptiveBinding","Enable":true,"Settings":[{"Property":"Binding","Value":"Tip"}]},"EraserActivationThreshold":1,"EraserButton":{"Path":"OpenTabletDriver.Desktop.Binding.AdaptiveBinding","Enable":true,"Settings":[{"Property":"Binding","Value":"Eraser"}]}}}]}"#;
        let profile = Profile::from_otd_text(json, Path::new("settings.json")).unwrap();
        assert_eq!(profile.otd_mapping.unwrap().tablet.width, 85.0);
        assert_eq!(profile.contact.tip_threshold_raw, Some(83));
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
    fn automatically_enables_saved_radial_follow_when_otd_flag_is_off() {
        let json = r#"{"Profiles":[{"Tablet":"Wacom PTH-660","OutputMode":{"Path":"OpenTabletDriver.Desktop.Output.AbsoluteMode","Enable":true},"Filters":[{"Path":"RadialFollow.RadialFollowSmoothingTabletSpace","Enable":false,"Settings":[{"Property":"OuterRadius","Value":0.7039}]}],"AbsoluteModeSettings":{"Display":{"Width":2560,"Height":1440,"X":1280,"Y":720,"Rotation":0},"Tablet":{"Width":85,"Height":47.8125,"X":110,"Y":23.90625,"Rotation":0},"EnableClipping":true,"EnableAreaLimiting":false},"Bindings":{}}]}"#;
        let profile = Profile::from_otd_text(json, Path::new("settings.json")).unwrap();
        assert_eq!(profile.radial_follow.len(), 1);
        assert_eq!(profile.auto_enabled_radial_follow, 1);
        assert_eq!(profile.radial_follow[0].outer_radius, 0.7039);
    }
}
