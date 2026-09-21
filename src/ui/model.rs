//! Editing model behind the control panel. It converts between the saved
//! profile and the values shown in the OpenTabletDriver-style editors and
//! holds the area arithmetic used by the area editors. Nothing here calls
//! Win32, so the behavior is covered by unit tests.

use std::path::Path;
use std::time::Duration;

use crate::config::{Profile, activation_raw};
use crate::display::DisplaySnapshot;
use crate::mapping::{Crop, OtdArea, OtdMapping, Rect};
use crate::plugins::{PluginConfig, PluginKind};
use crate::protocol::{MAX_PRESSURE, MAX_X, MAX_Y};
use crate::radial_follow::{FILTER_NAME, RadialFollowSettings};
use crate::relative::RelativeSettings;

/// PTH-660 active area. The absolute and relative mappers use the same size.
pub const TABLET_WIDTH_MM: f64 = 224.0;
pub const TABLET_HEIGHT_MM: f64 = 148.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bounds {
    pub left: f64,
    pub top: f64,
    pub right: f64,
    pub bottom: f64,
}

impl Bounds {
    pub fn tablet() -> Self {
        Self {
            left: 0.0,
            top: 0.0,
            right: TABLET_WIDTH_MM,
            bottom: TABLET_HEIGHT_MM,
        }
    }

    pub fn from_rect(rect: Rect) -> Self {
        Self {
            left: f64::from(rect.left),
            top: f64::from(rect.top),
            right: f64::from(rect.right),
            bottom: f64::from(rect.bottom),
        }
    }

    pub fn width(self) -> f64 {
        self.right - self.left
    }

    pub fn height(self) -> f64 {
        self.bottom - self.top
    }

    pub fn center(self) -> (f64, f64) {
        (
            (self.left + self.right) / 2.0,
            (self.top + self.bottom) / 2.0,
        )
    }

    pub fn valid(self) -> bool {
        self.width() > 0.0 && self.height() > 0.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputMode {
    Absolute,
    Relative,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FilterRef {
    Radial(usize),
    Plugin(usize),
}

#[derive(Clone, Debug, PartialEq)]
pub struct FilterItem {
    pub target: FilterRef,
    pub name: String,
    pub detail: String,
    pub enabled: bool,
}

/// A scalar plugin setting that can be edited with a single control.
#[derive(Clone, Debug, PartialEq)]
pub enum PropertyValue {
    Number(serde_json::Number),
    Bool(bool),
    Text(String),
}

/// The profile being edited plus settings the user switched away from, so
/// toggling an output mode or filter off and on again restores them.
pub struct Editor {
    pub profile: Profile,
    absolute_stash: Option<OtdMapping>,
    relative_stash: Option<RelativeSettings>,
    radial_stash: RadialFollowSettings,
}

pub fn default_relative() -> RelativeSettings {
    // OpenTabletDriver's RelativeModeSettings defaults.
    RelativeSettings {
        sensitivity: (10.0, 10.0),
        rotation: 0.0,
        reset_delay: Duration::from_millis(100),
    }
}

/// Converts a crop/monitor profile into the center/size form used by
/// OpenTabletDriver, so it can be shown in the same editors. The mapping is
/// equivalent within one output pixel.
pub fn simple_mapping(profile: &Profile, displays: &DisplaySnapshot) -> OtdMapping {
    let mm_x = TABLET_WIDTH_MM / f64::from(MAX_X);
    let mm_y = TABLET_HEIGHT_MM / f64::from(MAX_Y);
    let crop = profile.crop;
    let (width, height) = (f64::from(crop.width) * mm_x, f64::from(crop.height) * mm_y);
    let (width, height) = if matches!(profile.rotation, 90 | 270) {
        (height, width)
    } else {
        (width, height)
    };
    let tablet = OtdArea {
        width,
        height,
        x: (f64::from(crop.x) + f64::from(crop.width) / 2.0) * mm_x,
        y: (f64::from(crop.y) + f64::from(crop.height) / 2.0) * mm_y,
        rotation: f64::from(profile.rotation),
    };
    let destination = profile
        .monitor
        .and_then(|index| displays.monitors.get(index).copied())
        .unwrap_or(displays.virtual_screen);
    let display = OtdArea {
        width: f64::from(destination.width()),
        height: f64::from(destination.height()),
        x: f64::from(destination.left) + f64::from(destination.width()) / 2.0,
        y: f64::from(destination.top) + f64::from(destination.height()) / 2.0,
        rotation: 0.0,
    };
    OtdMapping {
        display,
        tablet,
        clipping: true,
        limiting: false,
    }
}

impl Editor {
    pub fn new(profile: Profile) -> Self {
        let radial_stash = profile.radial_follow.first().copied().unwrap_or_default();
        Self {
            profile,
            absolute_stash: None,
            relative_stash: None,
            radial_stash,
        }
    }

    pub fn mode(&self) -> OutputMode {
        if self.profile.relative.is_some() {
            OutputMode::Relative
        } else {
            OutputMode::Absolute
        }
    }

    /// Absolute areas as shown in the editor. Simple crop/monitor profiles
    /// are converted, but stay unchanged in the file until the user edits them.
    pub fn absolute(&self, displays: &DisplaySnapshot) -> OtdMapping {
        if let Some(mapping) = self.profile.otd_mapping {
            mapping
        } else if self.profile.relative.is_some() {
            self.absolute_stash
                .unwrap_or_else(|| simple_mapping(&Profile::default(), displays))
        } else {
            simple_mapping(&self.profile, displays)
        }
    }

    pub fn set_absolute(&mut self, mapping: OtdMapping) {
        self.profile.otd_mapping = Some(mapping);
        self.profile.relative = None;
        self.profile.monitor = None;
        self.profile.crop = Crop::default();
        self.profile.rotation = 0;
    }

    pub fn relative(&self) -> RelativeSettings {
        self.profile
            .relative
            .or(self.relative_stash)
            .unwrap_or_else(default_relative)
    }

    pub fn set_relative(&mut self, settings: RelativeSettings) {
        self.profile.relative = Some(settings);
    }

    pub fn set_mode(&mut self, mode: OutputMode, displays: &DisplaySnapshot) {
        match mode {
            OutputMode::Absolute if self.profile.relative.is_some() => {
                self.relative_stash = self.profile.relative.take();
                let mapping = self
                    .absolute_stash
                    .take()
                    .unwrap_or_else(|| simple_mapping(&Profile::default(), displays));
                self.set_absolute(mapping);
            }
            OutputMode::Relative if self.profile.relative.is_none() => {
                self.absolute_stash = Some(self.absolute(displays));
                self.profile.otd_mapping = None;
                self.profile.monitor = None;
                self.profile.crop = Crop::default();
                self.profile.rotation = 0;
                self.profile.relative =
                    Some(self.relative_stash.take().unwrap_or_else(default_relative));
            }
            _ => {}
        }
    }

    pub fn binding_enabled(&self, eraser: bool) -> bool {
        if eraser {
            self.profile.contact.eraser_enabled
        } else {
            self.profile.contact.tip_enabled
        }
    }

    pub fn set_binding_enabled(&mut self, eraser: bool, enabled: bool) {
        if eraser {
            self.profile.contact.eraser_enabled = enabled;
        } else {
            self.profile.contact.tip_enabled = enabled;
        }
    }

    /// Activation threshold in OpenTabletDriver's percent units. `None` means
    /// the profile uses the pen's own tip switch.
    pub fn threshold_percent(&self, eraser: bool) -> Option<f64> {
        let raw = if eraser {
            self.profile.contact.eraser_threshold_raw
        } else {
            self.profile.contact.tip_threshold_raw
        };
        raw.map(threshold_percent)
    }

    pub fn set_threshold_percent(
        &mut self,
        eraser: bool,
        percent: Option<f64>,
    ) -> Result<(), String> {
        let raw = percent.map(activation_raw).transpose()?;
        if eraser {
            self.profile.contact.eraser_threshold_raw = raw;
        } else {
            self.profile.contact.tip_threshold_raw = raw;
        }
        Ok(())
    }

    pub fn filters(&self) -> Vec<FilterItem> {
        let mut items = Vec::new();
        let radial = &self.profile.radial_follow;
        let detail = "Built-in Rust port".to_owned();
        if radial.is_empty() {
            items.push(FilterItem {
                target: FilterRef::Radial(0),
                name: FILTER_NAME.into(),
                detail: detail.clone(),
                enabled: false,
            });
        }
        for index in 0..radial.len() {
            items.push(FilterItem {
                target: FilterRef::Radial(index),
                name: if radial.len() > 1 {
                    format!("{FILTER_NAME} #{}", index + 1)
                } else {
                    FILTER_NAME.into()
                },
                detail: detail.clone(),
                enabled: true,
            });
        }
        for (index, plugin) in self.profile.plugins.iter().enumerate() {
            items.push(FilterItem {
                target: FilterRef::Plugin(index),
                name: plugin_name(plugin),
                detail: plugin_detail(plugin),
                enabled: plugin.enabled,
            });
        }
        items
    }

    pub fn filter_enabled(&self, target: FilterRef) -> bool {
        match target {
            FilterRef::Radial(index) => index < self.profile.radial_follow.len(),
            FilterRef::Plugin(index) => self.profile.plugins.get(index).is_some_and(|p| p.enabled),
        }
    }

    pub fn radial(&self, index: usize) -> RadialFollowSettings {
        self.profile
            .radial_follow
            .get(index)
            .copied()
            .unwrap_or(self.radial_stash)
    }

    /// Edits of a disabled filter are kept for when it is enabled again.
    pub fn set_radial(&mut self, index: usize, settings: RadialFollowSettings) {
        match self.profile.radial_follow.get_mut(index) {
            Some(slot) => *slot = settings,
            None => self.radial_stash = settings,
        }
    }

    pub fn set_filter_enabled(&mut self, target: FilterRef, enabled: bool) {
        match target {
            FilterRef::Radial(index) => {
                if enabled && self.profile.radial_follow.is_empty() {
                    self.profile.radial_follow.push(self.radial_stash);
                } else if !enabled && index < self.profile.radial_follow.len() {
                    self.radial_stash = self.profile.radial_follow.remove(index);
                }
            }
            FilterRef::Plugin(index) => {
                if let Some(plugin) = self.profile.plugins.get_mut(index) {
                    plugin.enabled = enabled;
                }
            }
        }
    }

    /// A tablet-space Radial Follow DLL next to the built-in port would
    /// smooth every report twice.
    pub fn duplicate_radial_follow(&self) -> bool {
        !self.profile.radial_follow.is_empty()
            && self.profile.plugins.iter().any(|plugin| {
                plugin.enabled
                    && plugin.kind == PluginKind::Dotnet
                    && plugin.type_name == crate::radial_follow::FILTER_PATH
            })
    }
}

pub fn threshold_percent(raw: u16) -> f64 {
    // Inverse of activation_raw: raw = ceil(1 + fraction * (MAX - 1)).
    f64::from(raw.saturating_sub(1)) / f64::from(MAX_PRESSURE - 1) * 100.0
}

pub fn plugin_name(plugin: &PluginConfig) -> String {
    match plugin.kind {
        PluginKind::Dotnet => plugin
            .type_name
            .rsplit('.')
            .next()
            .filter(|name| !name.is_empty())
            .unwrap_or("Unnamed .NET filter")
            .to_owned(),
        PluginKind::Native => plugin
            .path
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Native filter".into()),
    }
}

pub fn plugin_detail(plugin: &PluginConfig) -> String {
    let file = plugin
        .path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    match plugin.kind {
        PluginKind::Dotnet => format!(".NET plugin · {file}"),
        PluginKind::Native => format!("Native plugin · {file}"),
    }
}

/// Splits a .NET property name into words, e.g. `OuterRadius` becomes
/// "Outer Radius". Upstream shows each property's display name; the bridge
/// only reports property names.
pub fn display_name(key: &str) -> String {
    let characters: Vec<char> = key.chars().collect();
    let mut name = String::with_capacity(key.len() + 4);
    for (index, &character) in characters.iter().enumerate() {
        if index > 0 && character.is_uppercase() {
            let previous = characters[index - 1];
            let next_lower = characters.get(index + 1).is_some_and(|c| c.is_lowercase());
            if previous.is_lowercase()
                || previous.is_ascii_digit()
                || (previous.is_uppercase() && next_lower)
            {
                name.push(' ');
            }
        }
        name.push(if character == '_' { ' ' } else { character });
    }
    name
}

/// Scalar settings, in file order, or `None` when the object holds values
/// that need the JSON editor.
pub fn plugin_properties(json: &str) -> Option<Vec<(String, PropertyValue)>> {
    let value: serde_json::Value = serde_json::from_str(json).ok()?;
    value
        .as_object()?
        .iter()
        .map(|(key, value)| {
            let value = match value {
                serde_json::Value::Number(number) => PropertyValue::Number(number.clone()),
                serde_json::Value::Bool(value) => PropertyValue::Bool(*value),
                serde_json::Value::String(text) => PropertyValue::Text(text.clone()),
                _ => return None,
            };
            Some((key.clone(), value))
        })
        .collect()
}

/// Parses text typed for a setting, keeping integers integral.
pub fn parse_property(text: &str, previous: &PropertyValue) -> Result<serde_json::Value, String> {
    match previous {
        PropertyValue::Number(_) => {
            let trimmed = text.trim();
            if let Ok(integer) = trimmed.parse::<i64>() {
                return Ok(integer.into());
            }
            parse_number(trimmed)
                .and_then(serde_json::Number::from_f64)
                .map(serde_json::Value::Number)
                .ok_or_else(|| "Enter a number.".into())
        }
        PropertyValue::Bool(value) => Ok((*value).into()),
        PropertyValue::Text(_) => Ok(text.into()),
    }
}

pub fn set_plugin_property(
    json: &str,
    key: &str,
    value: serde_json::Value,
) -> Result<String, String> {
    let mut settings: serde_json::Value =
        serde_json::from_str(json).map_err(|e| format!("invalid plugin settings JSON: {e}"))?;
    let object = settings
        .as_object_mut()
        .ok_or("plugin settings must be a JSON object")?;
    object.insert(key.into(), value);
    Ok(settings.to_string())
}

/// Serializes and reloads a profile exactly as Save and Start do, so the
/// panel reports the same validation errors the file loader would.
pub fn validated(profile: &Profile, path: &Path) -> Result<Profile, String> {
    Profile::from_toml_text(&profile.to_toml()?, path)
}

pub fn format_number(value: f64, decimals: usize) -> String {
    if !value.is_finite() {
        return value.to_string();
    }
    let mut text = format!("{value:.decimals$}");
    if text.contains('.') {
        while text.ends_with('0') {
            text.pop();
        }
        if text.ends_with('.') {
            text.pop();
        }
    }
    if text == "-0" {
        text = "0".into();
    }
    text
}

pub fn parse_number(text: &str) -> Option<f64> {
    let text = text.trim();
    let normalized = if text.contains(',') && !text.contains('.') {
        text.replace(',', ".")
    } else {
        text.to_owned()
    };
    normalized
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite())
}

/// Half extents of an area's bounding box after rotation about its center.
pub fn rotated_extent(area: &OtdArea) -> (f64, f64) {
    let (sin, cos) = area.rotation.to_radians().sin_cos();
    let (half_width, half_height) = (area.width / 2.0, area.height / 2.0);
    (
        half_width * cos.abs() + half_height * sin.abs(),
        half_width * sin.abs() + half_height * cos.abs(),
    )
}

/// OpenTabletDriver's "Lock to usable area": shrink unrotated areas to the
/// bounds, then move the rotated area back inside them.
pub fn constrain(area: &mut OtdArea, bounds: Bounds) {
    if !(area.width > 0.0 && area.height > 0.0) || !bounds.valid() {
        return;
    }
    if area.rotation.rem_euclid(180.0) == 0.0 {
        area.width = area.width.min(bounds.width());
        area.height = area.height.min(bounds.height());
    }
    let (extent_x, extent_y) = rotated_extent(area);
    area.x = clamp_center(area.x, extent_x, bounds.left, bounds.right);
    area.y = clamp_center(area.y, extent_y, bounds.top, bounds.bottom);
}

fn clamp_center(center: f64, extent: f64, low: f64, high: f64) -> f64 {
    if 2.0 * extent >= high - low {
        (low + high) / 2.0
    } else {
        center.clamp(low + extent, high - extent)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Left,
    Right,
    Top,
    Bottom,
    Center,
}

pub fn align(area: &mut OtdArea, bounds: Bounds, align: Align) {
    let (extent_x, extent_y) = rotated_extent(area);
    match align {
        Align::Left => area.x = bounds.left + extent_x,
        Align::Right => area.x = bounds.right - extent_x,
        Align::Top => area.y = bounds.top + extent_y,
        Align::Bottom => area.y = bounds.bottom - extent_y,
        Align::Center => (area.x, area.y) = bounds.center(),
    }
}

pub fn flip_horizontal(area: &mut OtdArea, bounds: Bounds) {
    area.x = bounds.left + bounds.right - area.x;
}

pub fn flip_vertical(area: &mut OtdArea, bounds: Bounds) {
    area.y = bounds.top + bounds.bottom - area.y;
}

/// Rotates the area by 180 degrees for the other hand, as upstream does.
pub fn flip_handedness(area: &mut OtdArea, bounds: Bounds) {
    area.rotation = (area.rotation + 180.0).rem_euclid(360.0);
    flip_horizontal(area, bounds);
    flip_vertical(area, bounds);
}

/// Largest size with the given width/height ratio that fits the bounds.
pub fn fit_aspect(bounds: Bounds, ratio: f64) -> (f64, f64) {
    if ratio <= 0.0 || !ratio.is_finite() {
        return (bounds.width(), bounds.height());
    }
    if bounds.width() / bounds.height() > ratio {
        (bounds.height() * ratio, bounds.height())
    } else {
        (bounds.width(), bounds.width() / ratio)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AspectSource {
    TabletWidth,
    TabletHeight,
    DisplayWidth { previous: f64 },
    DisplayHeight { previous: f64 },
}

/// OpenTabletDriver's "Lock aspect ratio": the tablet area follows the
/// display area's shape.
pub fn lock_aspect(mapping: &mut OtdMapping, source: AspectSource) {
    let display = mapping.display;
    if !(display.width > 0.0 && display.height > 0.0) {
        return;
    }
    let tablet = &mut mapping.tablet;
    match source {
        AspectSource::TabletWidth => tablet.height = display.height / display.width * tablet.width,
        AspectSource::TabletHeight => tablet.width = display.width / display.height * tablet.height,
        AspectSource::DisplayWidth { previous } if previous > 0.0 => {
            tablet.width *= display.width / previous;
        }
        AspectSource::DisplayHeight { previous } if previous > 0.0 => {
            tablet.height *= display.height / previous;
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn displays() -> DisplaySnapshot {
        DisplaySnapshot {
            virtual_screen: Rect {
                left: 0,
                top: 0,
                right: 4160,
                bottom: 1440,
            },
            monitors: vec![
                Rect {
                    left: 0,
                    top: 0,
                    right: 2560,
                    bottom: 1440,
                },
                Rect {
                    left: 2560,
                    top: 0,
                    right: 4160,
                    bottom: 900,
                },
            ],
        }
    }

    fn pixels(normalized: (i32, i32), screen: Rect) -> (f64, f64) {
        (
            f64::from(normalized.0) * f64::from(screen.width() - 1) / 65_535.0,
            f64::from(normalized.1) * f64::from(screen.height() - 1) / 65_535.0,
        )
    }

    #[test]
    fn simple_profiles_convert_to_equivalent_center_areas() {
        let displays = displays();
        for rotation in [0, 90, 180, 270] {
            for monitor in [None, Some(1)] {
                let profile = Profile {
                    rotation,
                    monitor,
                    crop: Crop {
                        x: 4_000,
                        y: 2_000,
                        width: 30_000,
                        height: 20_000,
                    },
                    ..Profile::default()
                };
                let simple = displays.mapper(&profile).unwrap();
                let converted = Profile {
                    otd_mapping: Some(simple_mapping(&profile, &displays)),
                    ..Profile::default()
                };
                let otd = displays.mapper(&converted).unwrap();
                for (x, y) in [
                    (4_000, 2_000),
                    (34_000, 22_000),
                    (19_000, 12_000),
                    (9_000, 20_000),
                ] {
                    let a = pixels(simple.map(x, y).unwrap(), displays.virtual_screen);
                    let b = pixels(otd.map(x, y).unwrap(), displays.virtual_screen);
                    assert!(
                        (a.0 - b.0).abs() <= 1.5 && (a.1 - b.1).abs() <= 1.5,
                        "rotation {rotation}, monitor {monitor:?}, ({x}, {y}): {a:?} != {b:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn switching_output_modes_restores_previous_settings() {
        let displays = displays();
        let mut editor = Editor::new(Profile::default());
        let mut mapping = editor.absolute(&displays);
        mapping.tablet.width = 100.0;
        editor.set_absolute(mapping);
        editor.set_mode(OutputMode::Relative, &displays);
        assert_eq!(editor.mode(), OutputMode::Relative);
        assert!(editor.profile.otd_mapping.is_none());
        let mut relative = editor.relative();
        relative.sensitivity = (12.0, 8.0);
        editor.set_relative(relative);
        editor.set_mode(OutputMode::Absolute, &displays);
        assert_eq!(editor.absolute(&displays).tablet.width, 100.0);
        editor.set_mode(OutputMode::Relative, &displays);
        assert_eq!(editor.relative().sensitivity, (12.0, 8.0));
        validated(&editor.profile, Path::new("driver.toml")).unwrap();
    }

    #[test]
    fn simple_profile_stays_simple_until_the_area_is_edited() {
        let displays = displays();
        let editor = Editor::new(Profile::default());
        let shown = editor.absolute(&displays);
        assert_eq!(shown.display.width, 4160.0);
        assert_eq!(shown.tablet.width, TABLET_WIDTH_MM);
        assert!(editor.profile.otd_mapping.is_none());
        assert!(editor.profile.to_toml().unwrap().contains("[crop]"));
    }

    #[test]
    fn threshold_percent_round_trips_through_raw_values() {
        let mut editor = Editor::new(Profile::default());
        assert_eq!(editor.threshold_percent(false), None);
        editor.set_threshold_percent(false, Some(1.0)).unwrap();
        assert_eq!(editor.profile.contact.tip_threshold_raw, Some(83));
        assert_eq!(
            format_number(editor.threshold_percent(false).unwrap(), 2),
            "1"
        );
        editor.set_threshold_percent(true, Some(50.0)).unwrap();
        assert_eq!(editor.threshold_percent(true), Some(50.0));
        editor.set_threshold_percent(true, Some(100.0)).unwrap();
        assert_eq!(
            editor.profile.contact.eraser_threshold_raw,
            Some(MAX_PRESSURE)
        );
        assert_eq!(threshold_percent(1), 0.0);
        assert!(editor.set_threshold_percent(false, Some(101.0)).is_err());
        editor.set_threshold_percent(false, None).unwrap();
        assert_eq!(editor.profile.contact.tip_threshold_raw, None);
    }

    #[test]
    fn radial_follow_toggle_keeps_edited_settings() {
        let mut editor = Editor::new(Profile::default());
        let items = editor.filters();
        assert_eq!(items.len(), 1);
        assert!(!items[0].enabled);
        let mut settings = editor.radial(0);
        settings.outer_radius = 0.7;
        editor.set_radial(0, settings);
        assert!(editor.profile.radial_follow.is_empty());
        editor.set_filter_enabled(FilterRef::Radial(0), true);
        assert_eq!(editor.profile.radial_follow[0].outer_radius, 0.7);
        editor.set_filter_enabled(FilterRef::Radial(0), false);
        assert!(editor.profile.radial_follow.is_empty());
        editor.set_filter_enabled(FilterRef::Radial(0), true);
        assert_eq!(editor.radial(0).outer_radius, 0.7);
    }

    #[test]
    fn plugin_settings_edit_scalars_and_fall_back_for_nested_values() {
        let properties =
            plugin_properties(r#"{"Radius":1.5,"Count":3,"On":true,"Name":"a"}"#).unwrap();
        assert_eq!(properties.len(), 4);
        let count = &properties.iter().find(|(key, _)| key == "Count").unwrap().1;
        assert_eq!(parse_property("4", count).unwrap(), serde_json::json!(4));
        assert_eq!(
            parse_property("4.5", count).unwrap(),
            serde_json::json!(4.5)
        );
        assert!(parse_property("x", count).is_err());
        assert!(plugin_properties(r#"{"Curve":[1,2]}"#).is_none());
        assert!(plugin_properties("[]").is_none());
        let updated =
            set_plugin_property(r#"{"Radius":1.5}"#, "Radius", serde_json::json!(2)).unwrap();
        assert_eq!(updated, r#"{"Radius":2}"#);
    }

    #[test]
    fn property_names_become_words() {
        assert_eq!(
            display_name("SmoothingLeakCoefficient"),
            "Smoothing Leak Coefficient"
        );
        assert_eq!(display_name("DPIScale"), "DPI Scale");
        assert_eq!(display_name("reset_ms"), "reset ms");
        assert_eq!(display_name("Radius2X"), "Radius2 X");
    }

    #[test]
    fn plugin_labels_use_type_or_file_names() {
        let plugin = PluginConfig {
            path: "C:/plugins/RadialFollow.dll".into(),
            kind: PluginKind::Dotnet,
            enabled: true,
            type_name: "RadialFollow.RadialFollowSmoothingTabletSpace".into(),
            settings_json: "{}".into(),
        };
        assert_eq!(plugin_name(&plugin), "RadialFollowSmoothingTabletSpace");
        assert_eq!(plugin_detail(&plugin), ".NET plugin · RadialFollow.dll");
        let mut editor = Editor::new(Profile::default());
        editor.profile.plugins.push(plugin);
        editor.set_filter_enabled(FilterRef::Radial(0), true);
        assert!(editor.duplicate_radial_follow());
    }

    #[test]
    fn usable_area_lock_keeps_areas_inside_bounds() {
        let bounds = Bounds::tablet();
        let mut area = OtdArea {
            width: 300.0,
            height: 50.0,
            x: 200.0,
            y: 140.0,
            rotation: 0.0,
        };
        constrain(&mut area, bounds);
        assert_eq!(area.width, TABLET_WIDTH_MM);
        assert_eq!((area.x, area.y), (112.0, 123.0));
        let mut rotated = OtdArea {
            width: 40.0,
            height: 20.0,
            x: 0.0,
            y: 0.0,
            rotation: 90.0,
        };
        constrain(&mut rotated, bounds);
        assert!((rotated.x - 10.0).abs() < 1e-9 && (rotated.y - 20.0).abs() < 1e-9);
    }

    #[test]
    fn align_flip_and_aspect_helpers_match_upstream_actions() {
        let bounds = Bounds::tablet();
        let mut area = OtdArea {
            width: 80.0,
            height: 40.0,
            x: 100.0,
            y: 50.0,
            rotation: 0.0,
        };
        align(&mut area, bounds, Align::Right);
        assert_eq!(area.x, 184.0);
        align(&mut area, bounds, Align::Top);
        assert_eq!(area.y, 20.0);
        flip_horizontal(&mut area, bounds);
        assert_eq!(area.x, 40.0);
        flip_handedness(&mut area, bounds);
        assert_eq!((area.x, area.y, area.rotation), (184.0, 128.0, 180.0));
        assert_eq!(fit_aspect(bounds, 16.0 / 9.0), (224.0, 126.0));
        let mut mapping = simple_mapping(&Profile::default(), &displays());
        mapping.display.width = 2560.0;
        mapping.display.height = 1440.0;
        mapping.tablet.width = 160.0;
        lock_aspect(&mut mapping, AspectSource::TabletWidth);
        assert_eq!(mapping.tablet.height, 90.0);
        mapping.display.width = 1280.0;
        lock_aspect(
            &mut mapping,
            AspectSource::DisplayWidth { previous: 2560.0 },
        );
        assert_eq!(mapping.tablet.width, 80.0);
    }

    #[test]
    fn numbers_format_like_the_upstream_fields() {
        assert_eq!(format_number(47.8125, 4), "47.8125");
        assert_eq!(format_number(2560.0, 4), "2560");
        assert_eq!(format_number(-0.00001, 3), "0");
        assert_eq!(parse_number(" 1,5 "), Some(1.5));
        assert_eq!(parse_number("inf"), None);
        assert_eq!(parse_number(""), None);
    }
}
