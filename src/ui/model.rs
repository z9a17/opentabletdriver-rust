//! Editing model behind the control panel. It converts between the saved
//! profile and the values shown in the OpenTabletDriver-style editors and
//! holds the area arithmetic used by the area editors. Nothing here calls
//! Win32, so the behavior is covered by unit tests.

use std::path::Path;
use std::time::Duration;

#[cfg(test)]
use crate::config::activation_raw;
use crate::config::{OutputKind, Profile, activation_raw_for};
use crate::display::DisplaySnapshot;
use crate::dotnet::{FilterMetadata, PropertyMetadata};
#[cfg(test)]
use crate::mapping::Rect;
use crate::mapping::{Crop, OtdArea, OtdMapping};
use crate::plugins::{PluginConfig, PluginKind};
#[cfg(test)]
use crate::protocol::{MAX_PRESSURE, WIDTH_MM};
use crate::radial_follow::{FILTER_NAME, RadialFollowSettings};
use crate::relative::RelativeSettings;
#[cfg(test)]
use otd_core::spec::TabletSpec;

/// PTH-660 active area, the default tablet's.
#[cfg(test)]
pub const TABLET_WIDTH_MM: f64 = WIDTH_MM;

pub use otd_core::areas::{
    Align, AspectSource, Bounds, align, constrain, fit_aspect, flip_handedness, flip_horizontal,
    flip_vertical, lock_aspect,
};

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
    /// The DLL metadata does not identify the expected type of a null value,
    /// so keep it editable as an explicitly typed JSON scalar.
    JsonScalar,
    /// Preserve absence and explicit null independently; never guess constructor values.
    Typed {
        saved: Option<serde_json::Value>,
        metadata: Box<PropertyMetadata>,
    },
    Json(serde_json::Value),
}

impl PropertyValue {
    pub fn choices(&self) -> Vec<(String, serde_json::Value)> {
        let Self::Typed { metadata, .. } = self else {
            return Vec::new();
        };
        let mut choices = if metadata.property_type == "System.Boolean" {
            vec![("True".into(), true.into()), ("False".into(), false.into())]
        } else if let Some(values) = metadata
            .valid_values
            .as_ref()
            .filter(|_| metadata.property_type == "System.String")
        {
            values
                .iter()
                .map(|value| (value.clone(), serde_json::Value::String(value.clone())))
                .collect()
        } else if !metadata.enum_flags && !metadata.enum_choices.is_empty() {
            metadata
                .enum_choices
                .iter()
                .map(|choice| {
                    (
                        choice.name.clone(),
                        serde_json::Value::String(choice.name.clone()),
                    )
                })
                .collect()
        } else {
            return Vec::new();
        };
        choices.insert(0, (self.default_label(), serde_json::Value::Null));
        choices
    }

    pub fn choice_selected(&self, value: &serde_json::Value) -> bool {
        let Self::Typed { saved, metadata } = self else {
            return false;
        };
        let Some(saved) = saved.as_ref().filter(|value| !value.is_null()) else {
            return false;
        };
        saved == value
            || metadata.enum_choices.iter().any(|choice| {
                saved == &choice.value && value.as_str() == Some(choice.name.as_str())
            })
    }

    pub fn display_text(&self) -> String {
        if let Self::Typed { saved, metadata } = self {
            if saved.is_none() {
                return if self.choices().is_empty() { String::new() } else { "Constructor value".into() };
            }
            if saved.as_ref().is_some_and(serde_json::Value::is_null) {
                return if metadata.default_is_attribute {
                    metadata.default_value.as_ref().map_or_else(
                        || "Declared default".into(),
                        |value| match value {
                            serde_json::Value::String(text) => text.clone(),
                            _ => value.to_string(),
                        },
                    )
                } else if self.choices().is_empty() {
                    String::new()
                } else {
                    "Constructor value".into()
                };
            }
        }
        if let Some((label, _)) = self
            .choices()
            .into_iter()
            .find(|(_, value)| self.choice_selected(value))
        {
            return label;
        }
        match self {
            Self::Number(number) => number.to_string(),
            Self::Text(text) => text.clone(),
            Self::Bool(_) => String::new(),
            Self::JsonScalar => "null".into(),
            Self::Json(value) => value.to_string(),
            Self::Typed { saved, metadata } => match saved {
                None | Some(serde_json::Value::Null) => String::new(),
                Some(serde_json::Value::String(text))
                    if metadata.enum_underlying_type.is_some()
                        || matches!(
                            metadata.property_type.as_str(),
                            "System.String"
                                | "System.TimeSpan"
                                | "System.DateTime"
                                | "System.Boolean"
                                | "System.SByte"
                                | "System.Byte"
                                | "System.Int16"
                                | "System.UInt16"
                                | "System.Int32"
                                | "System.UInt32"
                                | "System.Int64"
                                | "System.UInt64"
                                | "System.Single"
                                | "System.Double"
                        ) =>
                {
                    text.clone()
                }
                Some(value) => value.to_string(),
            },
        }
    }

    pub fn writable(&self) -> bool {
        !matches!(self, Self::Typed { metadata, .. } if !metadata.writable)
    }

    /// Only properties with a supported field/choice control are editable.
    /// Keep complex values in the profile without exposing a raw JSON editor.
    pub fn field_writable(&self) -> bool {
        match self {
            Self::JsonScalar | Self::Json(_) => false,
            Self::Typed { metadata, .. } => metadata.writable
                && (metadata.enum_underlying_type.is_some() || !metadata.enum_choices.is_empty()
                    || matches!(metadata.property_type.as_str(),
                        "System.String" | "System.Boolean" | "System.TimeSpan" | "System.DateTime"
                        | "System.SByte" | "System.Byte" | "System.Int16" | "System.UInt16"
                        | "System.Int32" | "System.UInt32" | "System.Int64" | "System.UInt64"
                        | "System.Single" | "System.Double")),
            _ => true,
        }
    }

    pub fn uses_default(&self) -> bool {
        matches!(
            self,
            Self::Typed {
                saved: None | Some(serde_json::Value::Null),
                ..
            }
        )
    }

    pub fn reset_value(&self) -> serde_json::Value {
        match self {
            Self::Typed { metadata, .. } => metadata.default_value.clone().unwrap_or(serde_json::Value::Null),
            _ => serde_json::Value::Null,
        }
    }

    pub fn default_label(&self) -> String {
        match self {
            Self::Typed { metadata, .. } => metadata.default_value.as_ref().map_or_else(
                || "Constructor value".into(),
                |value| format!("Default: {}", scalar_text(value)),
            ),
            _ => "Use default".into(),
        }
    }

    pub fn default_cue(&self) -> String {
        match self {
            Self::Typed { saved: None, .. } => "Constructor value".into(),
            Self::Typed { saved: Some(value), metadata } if value.is_null() => {
                if metadata.default_is_attribute { self.default_label() } else { "Constructor value".into() }
            }
            _ => String::new(),
        }
    }
}

fn scalar_text(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(text) => format!("{text:?}"),
        _ => value.to_string(),
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PluginField {
    pub key: String,
    pub label: String,
    pub unit: String,
    pub tooltip: Option<String>,
    pub value: PropertyValue,
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
    let (mm_x, mm_y) = profile.tablet.mm_per_unit();
    // The default crop is the whole digitizer of the profile's tablet.
    let crop = if profile.crop == Crop::default() {
        Crop::full(profile.tablet)
    } else {
        profile.crop
    };
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
                .unwrap_or_else(|| simple_mapping(&self.blank(), displays))
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
                    .unwrap_or_else(|| simple_mapping(&self.blank(), displays));
                self.set_absolute(mapping);
            }
            OutputMode::Relative if self.profile.relative.is_none() => {
                // Pen output is absolute.
                self.profile.output = OutputKind::Mouse;
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

    /// Absolute output as a pen (Windows Ink) instead of the mouse.
    pub fn pen(&self) -> bool {
        self.profile.output == OutputKind::Pen
    }

    /// Pen output needs absolute mode; choosing it leaves relative mode.
    pub fn set_pen(&mut self, pen: bool, displays: &DisplaySnapshot) {
        if pen {
            self.set_mode(OutputMode::Absolute, displays);
        }
        self.profile.output = if pen {
            OutputKind::Pen
        } else {
            OutputKind::Mouse
        };
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
        raw.map(|raw| threshold_percent_for(raw, self.profile.tablet.max_pressure))
    }

    /// A full-area profile for the same tablet.
    fn blank(&self) -> Profile {
        Profile {
            tablet: self.profile.tablet,
            target_tablet: self.profile.target_tablet.clone(),
            ..Profile::default()
        }
    }

    /// The saved target, or the sole detected model. Never invent a model.
    pub fn tablet_label(&self, connected: &[String]) -> String {
        match self.profile.tablet_name() {
            Ok(Some(name)) => name,
            Ok(None) => match connected {
                [name] => name.clone(),
                [] => "No tablet detected".into(),
                _ => "Multiple tablets".into(),
            },
            Err(_) => "Unknown tablet".into(),
        }
    }

    /// Resolve the editor's transient geometry without naming/saving a target.
    pub fn update_detected_tablet(&mut self, connected: &[String]) -> Result<bool, String> {
        if self.profile.tablet_name()?.is_some() {
            return Ok(false);
        }
        let [name] = connected else { return Ok(false); };
        let spec = otd_core::config::spec_for_tablet(name)?;
        if self.profile.tablet == spec {
            return Ok(false);
        }
        self.profile = self.profile.for_tablet(spec)?;
        Ok(true)
    }

    /// Makes the profile target a tablet, or any tablet with `None`. Areas
    /// that no longer fit the tablet become its full area.
    pub fn set_tablet(&mut self, name: Option<String>) -> Result<(), String> {
        let spec = name
            .as_deref()
            .map(otd_core::config::spec_for_tablet).transpose()?.unwrap_or(self.profile.tablet);
        self.profile.target_tablet = Some(name.unwrap_or_else(|| "*".into()));
        if spec == self.profile.tablet {
            return Ok(());
        }
        self.profile.tablet = spec;
        if let Some(mapping) = &mut self.profile.otd_mapping {
            constrain(&mut mapping.tablet, Bounds::tablet_for(spec));
        } else if self.profile.relative.is_none() {
            // A simple full-area profile keeps covering the whole tablet.
            self.profile.crop = Crop::default();
        }
        for raw in [
            &mut self.profile.contact.tip_threshold_raw,
            &mut self.profile.contact.eraser_threshold_raw,
        ]
        .into_iter()
        .flatten()
        {
            *raw = (*raw).min(spec.max_pressure);
        }
        Ok(())
    }

    pub fn set_threshold_percent(
        &mut self,
        eraser: bool,
        percent: Option<f64>,
    ) -> Result<(), String> {
        let max = self.profile.tablet.max_pressure;
        let raw = percent
            .map(|percent| activation_raw_for(percent, max))
            .transpose()?;
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

    /// Reset only settings. Identity, order and enabled state are preserved.
    pub fn reset_filter(
        &mut self,
        target: FilterRef,
        metadata: Option<&FilterMetadata>,
    ) -> Result<(), String> {
        match target {
            FilterRef::Radial(index) => {
                if index >= self.profile.radial_follow.len()
                    && !(index == 0 && self.profile.radial_follow.is_empty())
                {
                    return Err("The selected filter no longer exists.".into());
                }
                self.set_radial(index, RadialFollowSettings::default());
            }
            FilterRef::Plugin(index) => {
                let plugin = self
                    .profile
                    .plugins
                    .get_mut(index)
                    .ok_or("The selected filter no longer exists.")?;
                let metadata = metadata
                    .filter(|metadata| {
                        plugin.kind.is_managed() && metadata.type_name == plugin.type_name
                    })
                    .ok_or(
                        "Defaults are unavailable for this plugin. Its settings have been kept.",
                    )?;
                let defaults: serde_json::Value =
                    serde_json::from_str(&metadata.default_settings_json)
                        .map_err(|error| error.to_string())?;
                if !defaults.is_object() {
                    return Err("Plugin defaults must be a JSON object.".into());
                }
                let candidate = PluginConfig {
                    settings_json: defaults.to_string(),
                    ..plugin.clone()
                };
                candidate.validate()?;
                *plugin = candidate;
            }
        }
        Ok(())
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
                    && plugin.kind.is_managed()
                    && plugin.type_name == crate::radial_follow::FILTER_PATH
            })
    }
}

/// The shortest percentage that `activation_raw` maps back to `raw`.
#[cfg(test)]
pub fn threshold_percent(raw: u16) -> f64 {
    threshold_percent_for(raw, MAX_PRESSURE)
}

/// `threshold_percent` for a tablet with another pressure range.
pub fn threshold_percent_for(raw: u16, max_pressure: u16) -> f64 {
    let max = f64::from(max_pressure);
    let low = f64::from(raw.saturating_sub(1)) / max * 100.0;
    let middle = (low + f64::from(raw) / max * 100.0) / 2.0;
    (0..=6)
        .map(|decimals| {
            let scale = 10f64.powi(decimals);
            (middle * scale).round() / scale
        })
        .find(|&percent| activation_raw_for(percent, max_pressure) == Ok(raw))
        .unwrap_or(low)
}

/// The tool tip upstream's generated control shows: the property's
/// `[ToolTip]`, a boolean's description, and a slider's range.
fn property_tooltip(descriptor: &PropertyMetadata) -> Option<String> {
    let parts: Vec<String> = [
        descriptor.tooltip.clone(),
        descriptor.description.clone(),
        descriptor.default_value.as_ref().map(|value| format!("Default: {}", scalar_text(value))),
        descriptor
            .slider
            .as_ref()
            .map(|slider| format!("Minimum: {}, Maximum: {}", slider.min, slider.max)),
    ]
    .into_iter()
    .flatten()
    .filter(|part| !part.trim().is_empty())
    .collect();
    (!parts.is_empty()).then(|| parts.join("\n\n"))
}

pub fn plugin_name(plugin: &PluginConfig) -> String {
    match plugin.kind {
        PluginKind::Dotnet | PluginKind::DotnetTool => plugin
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
        PluginKind::DotnetTool => format!(".NET tool · {file}"),
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
#[cfg(test)]
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
                serde_json::Value::Null => PropertyValue::JsonScalar,
                _ => return None,
            };
            Some((key.clone(), value))
        })
        .collect()
}

/// Show declared properties without writing missing values into the profile.
/// Unknown settings, including structured JSON, remain editable and preserved.
pub fn plugin_editor_fields(
    json: &str,
    metadata: Option<&FilterMetadata>,
) -> Option<Vec<PluginField>> {
    let mut properties = serde_json::from_str::<serde_json::Value>(json)
        .ok()?
        .as_object()?
        .clone();
    let mut fields = Vec::with_capacity(properties.len());
    if let Some(metadata) = metadata {
        for descriptor in &metadata.properties {
            let saved = properties.remove(&descriptor.name);
            if saved.is_some() || !descriptor.property_type.is_empty() {
                let key = descriptor.name.clone();
                let value = if descriptor.property_type.is_empty() {
                    property_value(saved.unwrap_or(serde_json::Value::Null))
                } else {
                    PropertyValue::Typed {
                        saved,
                        metadata: Box::new(descriptor.clone()),
                    }
                };
                fields.push(PluginField {
                    label: descriptor
                        .display_name
                        .as_deref()
                        .filter(|name| !name.trim().is_empty())
                        .map_or_else(|| display_name(&key), str::to_owned),
                    unit: descriptor.unit.clone().unwrap_or_default(),
                    tooltip: property_tooltip(descriptor),
                    key,
                    value,
                });
            }
        }
    }
    fields.extend(properties.into_iter().map(|(key, value)| PluginField {
        label: display_name(&key),
        unit: String::new(),
        tooltip: None,
        key,
        value: property_value(value),
    }));
    Some(fields)
}

fn property_value(value: serde_json::Value) -> PropertyValue {
    match value {
        serde_json::Value::Number(number) => PropertyValue::Number(number),
        serde_json::Value::Bool(value) => PropertyValue::Bool(value),
        serde_json::Value::String(text) => PropertyValue::Text(text),
        serde_json::Value::Null => PropertyValue::JsonScalar,
        value => PropertyValue::Json(value),
    }
}

/// Parses text typed for a setting, keeping integers integral.
pub fn parse_property(text: &str, previous: &PropertyValue) -> Result<serde_json::Value, String> {
    match previous {
        PropertyValue::Typed { metadata, .. } => super::property_validation::parse(text, metadata),
        PropertyValue::Json(_) => {
            serde_json::from_str(text).map_err(|error| format!("Enter valid JSON: {error}"))
        }
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
        PropertyValue::JsonScalar => {
            let value: serde_json::Value = serde_json::from_str(text.trim()).map_err(|_| {
                "Enter a JSON scalar: null, a boolean, a number, or a quoted string.".to_owned()
            })?;
            if value.is_null() || value.is_boolean() || value.is_number() || value.is_string() {
                Ok(value)
            } else {
                Err("Enter a JSON scalar: null, a boolean, a number, or a quoted string.".into())
            }
        }
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
    Profile::from_toml_text(&profile.to_toml()?, path)?.for_tablet(profile.tablet)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_control_attributes_become_choices_and_tool_tips() {
        let mode = crate::dotnet::PropertyMetadata {
            name: "Mode".into(),
            property_type: "System.String".into(),
            valid_values: Some(vec!["Linear".into(), "Smooth".into()]),
            ..Default::default()
        };
        let value = PropertyValue::Typed {
            saved: Some("Smooth".into()),
            metadata: Box::new(mode),
        };
        let labels: Vec<String> = value
            .choices()
            .into_iter()
            .map(|(label, _)| label)
            .collect();
        assert_eq!(labels, ["Constructor value", "Linear", "Smooth"]);
        assert_eq!(value.display_text(), "Smooth");
        let strength = crate::dotnet::PropertyMetadata {
            name: "Strength".into(),
            tooltip: Some("How strong".into()),
            slider: Some(crate::dotnet::Slider {
                min: 0.0,
                max: 2.0,
                default_value: 0.5,
            }),
            ..Default::default()
        };
        assert_eq!(
            property_tooltip(&strength).as_deref(),
            Some(
                "How strong

Minimum: 0, Maximum: 2"
            )
        );
        let snap = crate::dotnet::PropertyMetadata {
            name: "Snap".into(),
            description: Some("Snap to the grid".into()),
            ..Default::default()
        };
        assert_eq!(property_tooltip(&snap).as_deref(), Some("Snap to the grid"));
    }

    #[test]
    fn switching_tablets_resizes_areas_and_thresholds() {
        let mut editor = Editor::new(Profile::default());
        let mut mapping = simple_mapping(&editor.profile, &displays());
        mapping.tablet.width = 200.0;
        editor.set_absolute(mapping);
        editor.set_threshold_percent(false, Some(50.0)).unwrap();
        editor.set_tablet(Some("Wacom CTL-4100".into())).unwrap();
        let spec = editor.profile.tablet;
        assert_eq!(spec, otd_core::config::spec_for_tablet("Wacom CTL-4100").unwrap());
        assert!(spec.width_mm < 200.0);
        let area = editor.profile.otd_mapping.unwrap().tablet;
        assert!(area.width <= spec.width_mm, "{area:?}");
        assert!(editor.profile.contact.tip_threshold_raw.unwrap() <= spec.max_pressure);
        assert_eq!(
            editor.profile.tablet_name().unwrap().as_deref(),
            Some("Wacom CTL-4100")
        );
        editor.set_tablet(None).unwrap();
        assert_eq!(editor.profile.tablet, spec);
        assert_eq!(editor.profile.tablet_name().unwrap(), None);
    }

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
    fn pen_output_is_an_absolute_mode_that_relative_mode_turns_off() {
        let displays = displays();
        let mut editor = Editor::new(Profile::default());
        editor.set_mode(OutputMode::Relative, &displays);
        editor.set_pen(true, &displays);
        assert!(editor.pen());
        assert_eq!(editor.mode(), OutputMode::Absolute);
        validated(&editor.profile, Path::new("driver.toml")).unwrap();
        editor.set_mode(OutputMode::Relative, &displays);
        assert!(!editor.pen());
        validated(&editor.profile, Path::new("driver.toml")).unwrap();
        editor.set_pen(true, &displays);
        editor.set_pen(false, &displays);
        assert!(!editor.pen() && editor.mode() == OutputMode::Absolute);
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
        assert_eq!(editor.profile.contact.tip_threshold_raw, Some(82));
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
        // Profiles saved before the float comparison fix show their exact value.
        assert_eq!(threshold_percent(83), 1.01);
        for raw in 1..=MAX_PRESSURE {
            assert_eq!(activation_raw(threshold_percent(raw)), Ok(raw), "{raw}");
        }
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
    fn null_plugin_property_stays_in_editor_and_survives_profile_round_trip() {
        let mut profile = Profile::default();
        profile.plugins.push(PluginConfig {
            path: "C:/plugins/Example.dll".into(),
            kind: PluginKind::Dotnet,
            enabled: true,
            type_name: "Example.Filter".into(),
            settings_json: r#"{"OptionalValue":null}"#.into(),
        });

        let fields = plugin_editor_fields(&profile.plugins[0].settings_json, None).unwrap();
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].key, "OptionalValue");
        assert_eq!(fields[0].value, PropertyValue::JsonScalar);

        let saved = profile.to_toml().unwrap();
        let loaded = Profile::from_toml_text(&saved, Path::new("profile.toml")).unwrap();
        let settings = &loaded.plugins[0].settings_json;
        assert_eq!(
            plugin_editor_fields(settings, None).unwrap()[0].value,
            PropertyValue::JsonScalar
        );
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(settings).unwrap()["OptionalValue"],
            serde_json::Value::Null
        );
    }

    #[test]
    fn null_plugin_property_accepts_only_json_scalars() {
        let previous = PropertyValue::JsonScalar;
        for (text, expected) in [
            ("null", serde_json::Value::Null),
            ("true", serde_json::json!(true)),
            ("-2.5", serde_json::json!(-2.5)),
            (r#""text""#, serde_json::json!("text")),
        ] {
            assert_eq!(parse_property(text, &previous).unwrap(), expected);
        }
        for text in ["[1]", "{}", "not json"] {
            assert!(parse_property(text, &previous).is_err(), "accepted {text}");
        }
    }

    #[test]
    fn filter_controls_defaults_preserve_disabled_state_and_reject_unavailable_defaults() {
        let mut editor = Editor::new(Profile::default());
        editor.set_radial(
            0,
            RadialFollowSettings {
                outer_radius: 9.0,
                ..Default::default()
            },
        );
        editor.reset_filter(FilterRef::Radial(0), None).unwrap();
        assert!(editor.profile.radial_follow.is_empty());
        editor.set_filter_enabled(FilterRef::Radial(0), true);
        assert_eq!(
            editor.radial(0).outer_radius,
            RadialFollowSettings::default().outer_radius
        );
        editor.profile.plugins.push(PluginConfig {
            path: "filter.dll".into(),
            kind: PluginKind::Dotnet,
            enabled: false,
            type_name: "Example.Filter".into(),
            settings_json: r#"{"Radius":99,"Custom":true}"#.into(),
        });
        let target = FilterRef::Plugin(0);
        let original = editor.profile.plugins[0].settings_json.clone();
        assert!(editor.reset_filter(target, None).is_err());
        let mut metadata = FilterMetadata {
            type_name: "Wrong.Filter".into(),
            display_name: None,
            properties: Vec::new(),
            default_settings_json: r#"{"Radius":2,"Nullable":null}"#.into(),
        };
        assert!(editor.reset_filter(target, Some(&metadata)).is_err());
        assert_eq!(editor.profile.plugins[0].settings_json, original);
        metadata.type_name = "Example.Filter".into();
        editor.reset_filter(target, Some(&metadata)).unwrap();
        let plugin = &editor.profile.plugins[0];
        assert!(!plugin.enabled);
        assert_eq!(plugin.type_name, "Example.Filter");
        assert_eq!(plugin.path, std::path::PathBuf::from("filter.dll"));
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&plugin.settings_json).unwrap(),
            serde_json::json!({"Radius":2,"Nullable":null})
        );
        let reset = plugin.settings_json.clone();
        metadata.default_settings_json = "null".into();
        assert!(editor.reset_filter(target, Some(&metadata)).is_err());
        assert_eq!(editor.profile.plugins[0].settings_json, reset);
        editor.profile.plugins[0].kind = PluginKind::Native;
        assert!(editor.reset_filter(target, Some(&metadata)).is_err());
    }

    #[test]
    fn plugin_editor_uses_dll_labels_units_and_order_without_losing_unknown_settings() {
        let metadata = FilterMetadata {
            type_name: "RadialFollow.Screen".into(),
            display_name: Some("Screen smoothing".into()),
            default_settings_json: "{}".into(),
            properties: vec![
                crate::dotnet::PropertyMetadata {
                    name: "OuterRadius".into(),
                    display_name: Some("Outer Radius".into()),
                    unit: Some("px".into()),
                    tooltip: Some("Maximum lag".into()),
                    ..Default::default()
                },
                crate::dotnet::PropertyMetadata {
                    name: "InnerRadius".into(),
                    display_name: Some("Inner Radius".into()),
                    unit: Some("px".into()),
                    tooltip: None,
                    ..Default::default()
                },
            ],
        };
        let settings = r#"{"InnerRadius":5,"OuterRadius":10,"CustomFlag":true}"#;
        let fields = plugin_editor_fields(settings, Some(&metadata)).unwrap();
        assert_eq!(
            fields
                .iter()
                .map(|field| field.key.as_str())
                .collect::<Vec<_>>(),
            ["OuterRadius", "InnerRadius", "CustomFlag"]
        );
        assert_eq!(
            (fields[0].label.as_str(), fields[0].unit.as_str()),
            ("Outer Radius", "px")
        );
        assert_eq!(fields[0].tooltip.as_deref(), Some("Maximum lag"));
        assert_eq!(fields[2].label, "Custom Flag");
        assert_eq!(fields[2].value, PropertyValue::Bool(true));
        assert_eq!(
            plugin_editor_fields(settings, None).unwrap().len(),
            fields.len()
        );
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
