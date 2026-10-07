//! Schema-1 preservation is an archive, not a claim that every imported store runs.
//! Contract: OTD 736003ed72c8bbb28033b039d5a0bb76c344145c,
//! Desktop/Settings.cs, Profiles/Profile.cs and Reflection/PluginSettingStore.cs.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::Profile;

pub const PROFILE_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Default)]
pub struct ImportOptions {
    /// Compatibility with the early Rust importer, never enabled implicitly.
    pub legacy_force_radial_follow: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ImportedOtdSettings {
    pub source_path: String,
    /// Keep the original text: null/missing, large numeric values, disabled
    /// stores, unknown fields and the complete profile/tool collections survive.
    pub settings_json: String,
    pub selected_profile: usize,
    #[serde(default)]
    pub legacy_force_radial_follow: bool,
}

impl ImportedOtdSettings {
    pub fn validate(&self) -> Result<(), String> {
        let document: serde_json::Value = serde_json::from_str(&self.settings_json)
            .map_err(|error| format!("invalid preserved OTD settings: {error}"))?;
        if document
            .get("Profiles")
            .and_then(serde_json::Value::as_array)
            .and_then(|profiles| profiles.get(self.selected_profile))
            .is_none()
        {
            return Err("preserved OTD selected_profile does not exist in Profiles".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ProfileDiagnostic {
    pub kind: String,
    pub location: String,
    pub message: String,
}

impl ProfileDiagnostic {
    pub(super) fn warning(location: impl Into<String>, message: String) -> Self {
        Self {
            kind: "warning".into(),
            location: location.into(),
            message,
        }
    }

    pub(super) fn unsupported(location: impl Into<String>, message: String) -> Self {
        Self {
            kind: "unsupported_active".into(),
            location: location.into(),
            message,
        }
    }
}

pub(super) fn import_diagnostics(
    text: &str,
    selected: usize,
) -> Result<Vec<ProfileDiagnostic>, String> {
    let document: serde_json::Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let mut diagnostics = vec![ProfileDiagnostic::warning(
        "imported_otd",
        "The full original OTD settings collection is preserved as an archive. Only the selected mapping, supported tip/eraser actions and native Radial Follow entries are imported; tablet execution support is checked separately. Export reconciles supported edits into a copy and never rewrites the archive.".into(),
    )];
    if let Some(revision) = document.get("Revision").and_then(Value::as_str) {
        let version = revision.split(['+', '-']).next().unwrap_or(revision);
        if !matches!(version, "0.6.7" | "0.6.7.0") {
            diagnostics.push(ProfileDiagnostic::warning(
                "Revision",
                format!("Source revision {revision} differs from the pinned OTD 0.6.7 baseline; unsupported settings remain preserved."),
            ));
        }
    }
    if let Some(root) = document.as_object() {
        for (name, value) in root {
            if name != "Profiles" {
                find_active_stores(value, name, &mut diagnostics);
            }
        }
    }
    let profile = &document["Profiles"][selected];
    if let Some(bindings) = profile["Bindings"].as_object() {
        for (name, value) in bindings {
            // Buttons and wheels are imported one by one, with their own diagnostics.
            if matches!(
                name.as_str(),
                "TipButton" | "EraserButton" | "PenButtons" | "AuxButtons" | "MouseButtons" | "MouseScrollUp" | "MouseScrollDown" | "WheelBindings"
                    | "DisablePressure" | "DisableTilt" | "EnableDragBindings"
            ) {
                continue;
            }
            find_active_stores(
                value,
                &format!("Profiles[{selected}].Bindings.{name}"),
                &mut diagnostics,
            );
        }
    }
    if let Some(values) = profile["OutputMode"]["Settings"].as_array()
        && !values.is_empty()
    {
        diagnostics.push(ProfileDiagnostic::unsupported(
            format!("Profiles[{selected}].OutputMode.Settings"),
            "Output-mode property stores are preserved but are not applied by this importer."
                .into(),
        ));
    }
    if let Some(properties) = profile.as_object() {
        for (name, value) in properties {
            if !matches!(name.as_str(), "OutputMode" | "Filters" | "Bindings") {
                find_active_stores(
                    value,
                    &format!("Profiles[{selected}].{name}"),
                    &mut diagnostics,
                );
            }
        }
    }
    Ok(diagnostics)
}

fn find_active_stores(
    value: &serde_json::Value,
    location: &str,
    diagnostics: &mut Vec<ProfileDiagnostic>,
) {
    match value {
        serde_json::Value::Object(fields) => {
            if fields.get("Enable").and_then(serde_json::Value::as_bool) == Some(false) {
                return;
            }
            if fields.get("Enable").and_then(serde_json::Value::as_bool) == Some(true) {
                let name = fields
                    .get("Path")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("unknown feature");
                diagnostics.push(ProfileDiagnostic::unsupported(
                    location,
                    format!("Enabled store {name} is preserved in imported_otd.settings_json but is not executed."),
                ));
                return;
            }
            for (name, child) in fields {
                find_active_stores(child, &format!("{location}.{name}"), diagnostics);
            }
        }
        serde_json::Value::Array(values) => {
            for (index, child) in values.iter().enumerate() {
                find_active_stores(child, &format!("{location}[{index}]"), diagnostics);
            }
        }
        _ => {}
    }
}

/// Remove unsupported fields before strict typed deserialization and archive
/// their exact TOML values. Array paths identify original positions; archived
/// properties never get attached to a different filter after a reorder.
pub(super) fn extract_unknown_fields(document: &mut toml::Value) -> BTreeMap<String, toml::Value> {
    let mut preserved = BTreeMap::new();
    extract_table(
        document,
        "",
        &[
            "schema_version",
            "settings_revision",
            "imported_otd",
            "preserved_fields",
            "diagnostics",
            "monitor",
            "rotation",
            "crop",
            "device_path",
            "relative",
            "absolute",
            "bindings",
            "pen_buttons",
            "aux_buttons",
            "mouse_buttons",
            "mouse_scroll_up",
            "mouse_scroll_down",
            "wheels",
            "output",
            "radial_follow",
            "disabled_radial_follow",
            "plugins",
            "tablet",
        ],
        &mut preserved,
    );
    for (name, keys) in [
        ("crop", &["x", "y", "width", "height"][..]),
        (
            "disabled_radial_follow",
            &[
                "outer_radius",
                "inner_radius",
                "smoothing_coefficient",
                "soft_knee_scale",
                "smoothing_leak_coefficient",
            ][..],
        ),
        (
            "relative",
            &[
                "x_sensitivity",
                "y_sensitivity",
                "rotation",
                "reset_delay_ms",
            ][..],
        ),
        (
            "absolute",
            &["display", "tablet", "clipping", "limiting"][..],
        ),
        (
            "bindings",
            &[
                "tip_enabled",
                "eraser_enabled",
                "tip_threshold_raw",
                "eraser_threshold_raw",
                "tip_threshold_percent",
                "eraser_threshold_percent",
                "drag_only",
                "disable_pressure",
                "disable_tilt",
            ][..],
        ),
        (
            "imported_otd",
            &[
                "source_path",
                "settings_json",
                "selected_profile",
                "legacy_force_radial_follow",
            ][..],
        ),
    ] {
        if let Some(value) = document.get_mut(name) {
            extract_table(value, &format!("/{name}"), keys, &mut preserved);
        }
    }
    if let Some(absolute) = document.get_mut("absolute") {
        for name in ["display", "tablet"] {
            if let Some(value) = absolute.get_mut(name) {
                extract_table(
                    value,
                    &format!("/absolute/{name}"),
                    &["Width", "Height", "X", "Y", "Rotation"],
                    &mut preserved,
                );
            }
        }
    }
    for (name, keys) in [
        (
            "radial_follow",
            &[
                "outer_radius",
                "inner_radius",
                "smoothing_coefficient",
                "soft_knee_scale",
                "smoothing_leak_coefficient",
            ][..],
        ),
        (
            "plugins",
            &["path", "kind", "enabled", "type_name", "settings_json"][..],
        ),
        ("diagnostics", &["kind", "location", "message"][..]),
    ] {
        if let Some(values) = document.get_mut(name).and_then(toml::Value::as_array_mut) {
            for (index, value) in values.iter_mut().enumerate() {
                extract_table(value, &format!("/{name}/{index}"), keys, &mut preserved);
            }
        }
    }
    preserved
}

fn extract_table(
    value: &mut toml::Value,
    path: &str,
    known: &[&str],
    preserved: &mut BTreeMap<String, toml::Value>,
) {
    let Some(fields) = value.as_table_mut() else {
        return;
    };
    let unknown: Vec<_> = fields
        .keys()
        .filter(|name| !known.contains(&name.as_str()))
        .cloned()
        .collect();
    for name in unknown {
        if let Some(value) = fields.remove(&name) {
            let segment = name.replace('~', "~0").replace('/', "~1");
            preserved.insert(format!("{path}/{segment}"), value);
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct OtdProfileSummary {
    pub index: usize,
    pub tablet: String,
    pub output_mode: String,
    pub enabled: bool,
    pub runtime_tablet_supported: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct OtdImportPreview {
    pub profile: OtdProfileSummary,
    pub migrated_fields: Vec<String>,
    pub diagnostics: Vec<ProfileDiagnostic>,
    pub import_error: Option<String>,
    pub legacy_force_radial_follow: bool,
}

/// A complete source document. Previewing/importing does not write files or
/// resolve/load plugins. Unsupported profiles remain available for later work.
#[derive(Clone, Debug)]
pub struct OtdSettingsDocument {
    source_path: PathBuf,
    source_json: String,
    document: Value,
}

impl OtdSettingsDocument {
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|error| format!("cannot read OTD settings {}: {error}", path.display()))?;
        Self::from_json(&text, path)
    }

    pub fn from_json(text: &str, source_path: &Path) -> Result<Self, String> {
        let document: Value = serde_json::from_str(text).map_err(|error| error.to_string())?;
        if document.get("Profiles").and_then(Value::as_array).is_none() {
            return Err("OTD settings require a Profiles array".into());
        }
        Ok(Self {
            source_path: source_path.to_owned(),
            source_json: text.to_owned(),
            document,
        })
    }

    pub fn profiles(&self) -> Vec<OtdProfileSummary> {
        let database = super::configured_tablets();
        self.document["Profiles"]
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
            .map(|(index, profile)| {
                let tablet = profile["Tablet"].as_str().unwrap_or("").to_owned();
                OtdProfileSummary {
                    index,
                    runtime_tablet_supported: database.as_ref().is_ok_and(|database| {
                        super::runtime_tablet_in(&tablet, database).is_ok()
                    }),
                    tablet,
                    output_mode: profile["OutputMode"]["Path"]
                        .as_str()
                        .unwrap_or("")
                        .to_owned(),
                    enabled: profile["OutputMode"]["Enable"].as_bool().unwrap_or(false),
                }
            })
            .collect()
    }

    pub fn revision(&self) -> Option<&str> {
        self.document.get("Revision").and_then(Value::as_str)
    }

    pub fn original_json(&self) -> &str {
        &self.source_json
    }

    pub fn preview(
        &self,
        index: usize,
        options: ImportOptions,
    ) -> Result<OtdImportPreview, String> {
        let profile = self
            .profiles()
            .into_iter()
            .nth(index)
            .ok_or_else(|| format!("OTD profile index {index} does not exist"))?;
        let mut migrated_fields = Vec::new();
        let (mut diagnostics, import_error) = match self.import(index, options) {
            Ok(imported) => {
                migrated_fields.push(
                    if imported.relative.is_some() {
                        "RelativeModeSettings"
                    } else {
                        "AbsoluteModeSettings"
                    }
                    .into(),
                );
                migrated_fields.push("Bindings.TipActivationThreshold".into());
                migrated_fields.push("Bindings.EraserActivationThreshold".into());
                if imported.contact.tip_enabled {
                    migrated_fields.push("Bindings.TipButton".into());
                }
                if imported.contact.eraser_enabled {
                    migrated_fields.push("Bindings.EraserButton".into());
                }
                if imported
                    .pen_buttons
                    .iter()
                    .any(|action| *action != crate::output::buttons::ButtonAction::None)
                {
                    migrated_fields.push("Bindings.PenButtons".into());
                }
                if imported
                    .aux_buttons
                    .iter()
                    .any(|action| *action != crate::output::buttons::ButtonAction::None)
                {
                    migrated_fields.push("Bindings.AuxButtons".into());
                }
                if imported.wheels.iter().any(|wheel| !wheel.is_unbound()) {
                    migrated_fields.push("Bindings.WheelBindings".into());
                }
                if !imported.radial_follow.is_empty() {
                    migrated_fields.push(format!(
                        "{} native Radial Follow filter(s)",
                        imported.radial_follow.len()
                    ));
                }
                (imported.diagnostics, None)
            }
            Err(error) => (import_diagnostics(&self.source_json, index)?, Some(error)),
        };
        if !profile.runtime_tablet_supported {
            diagnostics.push(ProfileDiagnostic::unsupported(
                format!("Profiles[{index}].Tablet"),
                format!(
                    "{} can be stored or exported, but this driver cannot run it: {}",
                    profile.tablet,
                    super::runtime_tablet(&profile.tablet)
                        .err()
                        .unwrap_or_default()
                ),
            ));
        }
        if let Some(error) = &import_error {
            diagnostics.push(ProfileDiagnostic::unsupported(
                format!("Profiles[{index}]"),
                error.clone(),
            ));
        }
        Ok(OtdImportPreview {
            profile,
            migrated_fields,
            diagnostics,
            import_error,
            legacy_force_radial_follow: options.legacy_force_radial_follow,
        })
    }

    pub fn import(&self, index: usize, options: ImportOptions) -> Result<Profile, String> {
        Profile::from_otd_profile_text(&self.source_json, &self.source_path, index, options)
    }
}

#[derive(Clone, Debug)]
pub struct NamedProfile {
    pub name: String,
    pub profile: Profile,
}

#[derive(Clone, Debug, Default)]
pub struct NativeProfileCollection {
    pub settings_revision: u64,
    pub selected_profile: usize,
    pub profiles: Vec<NamedProfile>,
    pub preserved_fields: BTreeMap<String, toml::Value>,
}

#[derive(Deserialize, Serialize)]
struct RawCollection {
    format: String,
    schema_version: u32,
    #[serde(default)]
    settings_revision: u64,
    #[serde(default)]
    selected_profile: usize,
    profiles: Vec<RawNamedProfile>,
    #[serde(default, flatten)]
    preserved_fields: BTreeMap<String, toml::Value>,
}

#[derive(Deserialize, Serialize)]
struct RawNamedProfile {
    name: String,
    profile: toml::Value,
    #[serde(default, flatten)]
    preserved_fields: BTreeMap<String, toml::Value>,
}

impl NativeProfileCollection {
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|error| error.to_string())?;
        Self::from_toml_text(&text, path)
    }

    pub fn from_toml_text(text: &str, path: &Path) -> Result<Self, String> {
        let raw: RawCollection = toml::from_str(text).map_err(|error| error.to_string())?;
        if raw.format != "profile_collection" || raw.schema_version != PROFILE_SCHEMA_VERSION {
            return Err("unsupported native profile collection format or schema version".into());
        }
        let profiles = raw
            .profiles
            .into_iter()
            .map(|entry| {
                let text = toml::to_string(&entry.profile).map_err(|error| error.to_string())?;
                let mut profile = Profile::from_toml_text(&text, path)?;
                for (key, value) in entry.preserved_fields {
                    let escaped = key.replace('~', "~0").replace('/', "~1");
                    profile
                        .preserved_fields
                        .insert(format!("/collection_entry/{escaped}"), value);
                }
                Ok(NamedProfile {
                    name: entry.name,
                    profile,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        let collection = Self {
            settings_revision: raw.settings_revision,
            selected_profile: raw.selected_profile,
            profiles,
            preserved_fields: raw.preserved_fields,
        };
        collection.validate()?;
        Ok(collection)
    }

    fn validate(&self) -> Result<(), String> {
        if self.profiles.is_empty() || self.selected_profile >= self.profiles.len() {
            return Err("a profile collection needs at least one profile and a valid selected_profile index".into());
        }
        let mut names = std::collections::BTreeSet::new();
        for entry in &self.profiles {
            if entry.name.trim().is_empty() || !names.insert(entry.name.as_str()) {
                return Err("profile collection names must be nonempty and unique".into());
            }
        }
        Ok(())
    }

    pub fn selected(&self) -> Result<&Profile, String> {
        self.profiles
            .get(self.selected_profile)
            .map(|entry| &entry.profile)
            .ok_or_else(|| "selected native profile does not exist".into())
    }

    pub fn select(&mut self, index: usize) -> Result<(), String> {
        if index >= self.profiles.len() {
            return Err(format!("native profile index {index} does not exist"));
        }
        if self.selected_profile != index {
            self.settings_revision = self
                .settings_revision
                .checked_add(1)
                .ok_or("settings revision exhausted")?;
            self.selected_profile = index;
        }
        Ok(())
    }

    pub fn to_toml(&self) -> Result<String, String> {
        self.serialize(None)
    }

    pub fn to_toml_at(&self, destination: &Path) -> Result<String, String> {
        self.serialize(Some(destination))
    }

    fn serialize(&self, destination: Option<&Path>) -> Result<String, String> {
        self.validate()?;
        let profiles = self
            .profiles
            .iter()
            .map(|entry| {
                let text = match destination {
                    Some(path) => entry.profile.to_toml_at(path)?,
                    None => entry.profile.to_toml()?,
                };
                let profile: toml::Value =
                    toml::from_str(&text).map_err(|error| error.to_string())?;
                Ok(RawNamedProfile {
                    name: entry.name.clone(),
                    profile,
                    preserved_fields: BTreeMap::new(),
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        toml::to_string_pretty(&RawCollection {
            format: "profile_collection".into(),
            schema_version: PROFILE_SCHEMA_VERSION,
            settings_revision: self.settings_revision,
            selected_profile: self.selected_profile,
            profiles,
            preserved_fields: self.preserved_fields.clone(),
        })
        .map_err(|error| error.to_string())
    }
}

pub(super) fn export_otd(profile: &Profile) -> Result<String, String> {
    profile.validate_filter_execution()?;
    let imported = profile.imported_otd.as_ref()
        .ok_or("OTD export requires an imported source document; a standalone Rust profile has no original tablet/store identities")?;
    imported.validate()?;
    if !profile.plugins.is_empty() {
        return Err("OTD export cannot represent Rust DLL paths or reconcile their order with preserved OTD filters yet. Remove DLL entries from the export copy or keep the Rust TOML profile.".into());
    }
    if profile.device_path.is_some()
        || profile.monitor.is_some()
        || profile.rotation != 0
        || profile.crop != crate::mapping::Crop::default()
    {
        return Err("Rust device_path/monitor/crop/rotation overrides cannot be represented exactly in an OTD export; use explicit absolute areas or relative settings.".into());
    }
    let baseline = Profile::from_otd_profile_text(
        &imported.settings_json,
        Path::new(&imported.source_path),
        imported.selected_profile,
        ImportOptions {
            legacy_force_radial_follow: imported.legacy_force_radial_follow,
        },
    )?;
    let original: Value =
        serde_json::from_str(&imported.settings_json).map_err(|error| error.to_string())?;
    let mut document = original.clone();
    let selected = &mut document["Profiles"][imported.selected_profile];
    let target = profile.tablet_name()?.ok_or("OTD export requires a named tablet; choose a tablet before exporting an automatic profile")?;
    selected["Tablet"] = Value::String(target);
    let target_mode;
    let pen = profile.output == super::OutputKind::Pen;
    if let Some(relative) = profile.relative {
        if profile.otd_mapping.is_some() || pen {
            return Err("OTD export requires exactly one output mode".into());
        }
        relative.validate_for(profile.tablet)?;
        target_mode = "OpenTabletDriver.Desktop.Output.RelativeMode";
        let old = baseline.relative;
        let settings = &mut selected["RelativeModeSettings"];
        for (key, current, previous) in [
            (
                "XSensitivity",
                relative.sensitivity.0,
                old.map(|value| value.sensitivity.0),
            ),
            (
                "YSensitivity",
                relative.sensitivity.1,
                old.map(|value| value.sensitivity.1),
            ),
            (
                "RelativeRotation",
                relative.rotation,
                old.map(|value| value.rotation),
            ),
        ] {
            if previous != Some(current) {
                set_field(settings, key, json!(current))?;
            }
        }
        if old.map(|value| value.reset_delay) != Some(relative.reset_delay) {
            set_field(
                settings,
                "RelativeResetDelay",
                Value::String(timespan(relative.reset_delay)?),
            )?;
        }
    } else if let Some(mapping) = profile.otd_mapping {
        // OpenTabletDriver has pen output through plugins: keep the source's
        // pen plugin, or else use Windows Ink.
        let source_mode = selected["OutputMode"]["Path"].as_str();
        target_mode = match (pen, source_mode) {
            (true, Some(super::WINDOWS_PEN_POINTER_MODE)) => super::WINDOWS_PEN_POINTER_MODE,
            (true, Some(super::LINUX_ARTIST_MODE)) => super::LINUX_ARTIST_MODE,
            (true, _) => super::WINDOWS_INK_ABSOLUTE_MODE,
            (false, _) => "OpenTabletDriver.Desktop.Output.AbsoluteMode",
        };
        let settings = &mut selected["AbsoluteModeSettings"];
        for (name, area, previous) in [
            (
                "Display",
                mapping.display,
                baseline.otd_mapping.map(|value| value.display),
            ),
            (
                "Tablet",
                mapping.tablet,
                baseline.otd_mapping.map(|value| value.tablet),
            ),
        ] {
            if ![area.width, area.height, area.x, area.y, area.rotation]
                .into_iter()
                .all(f64::is_finite)
                || area.width <= 0.0
                || area.height <= 0.0
            {
                return Err(
                    "OTD export requires finite absolute areas with positive dimensions".into(),
                );
            }
            if previous != Some(area) {
                ensure_object(settings)?;
                for (key, current, old) in [
                    ("Width", area.width, previous.map(|value| value.width)),
                    ("Height", area.height, previous.map(|value| value.height)),
                    ("X", area.x, previous.map(|value| value.x)),
                    ("Y", area.y, previous.map(|value| value.y)),
                    (
                        "Rotation",
                        area.rotation,
                        previous.map(|value| value.rotation),
                    ),
                ] {
                    if old != Some(current) {
                        set_field(&mut settings[name], key, json!(current))?;
                    }
                }
            }
        }
        for (key, current, old) in [
            (
                "EnableClipping",
                mapping.clipping,
                baseline.otd_mapping.map(|value| value.clipping),
            ),
            (
                "EnableAreaLimiting",
                mapping.limiting,
                baseline.otd_mapping.map(|value| value.limiting),
            ),
        ] {
            if old != Some(current) {
                set_field(settings, key, json!(current))?;
            }
        }
    } else {
        return Err("OTD export needs explicit absolute areas or relative settings; simple Rust crops need display/device geometry conversion first".into());
    }
    if selected["OutputMode"]["Path"].as_str() != Some(target_mode) {
        if selected["OutputMode"]["Settings"]
            .as_array()
            .is_some_and(|values| !values.is_empty())
        {
            return Err(
                "cannot change output-mode type while preserving type-specific source settings"
                    .into(),
            );
        }
        set_field(&mut selected["OutputMode"], "Path", json!(target_mode))?;
    }
    for (
        button,
        threshold,
        current_enabled,
        old_enabled,
        current_threshold,
        old_threshold,
        current_percent,
        old_percent,
        action,
    ) in [
        (
            "TipButton",
            "TipActivationThreshold",
            profile.contact.tip_enabled,
            baseline.contact.tip_enabled,
            profile.contact.tip_threshold_raw,
            baseline.contact.tip_threshold_raw,
            profile.contact.tip_threshold_percent,
            baseline.contact.tip_threshold_percent,
            "Tip",
        ),
        (
            "EraserButton",
            "EraserActivationThreshold",
            profile.contact.eraser_enabled,
            baseline.contact.eraser_enabled,
            profile.contact.eraser_threshold_raw,
            baseline.contact.eraser_threshold_raw,
            profile.contact.eraser_threshold_percent,
            baseline.contact.eraser_threshold_percent,
            "Eraser",
        ),
    ] {
        // A changed output kind rewrites an enabled contact binding for it.
        let rebind = current_enabled && profile.output != baseline.output;
        if current_enabled != old_enabled || rebind {
            ensure_object(&mut selected["Bindings"])?;
            let store = &mut selected["Bindings"][button];
            if current_enabled {
                let (path, property, value) = if pen {
                    (super::WINDOWS_INK_BINDING, "Button", "Pen Tip")
                } else {
                    (super::ADAPTIVE_BINDING, "Binding", action)
                };
                if !store.is_null()
                    && !matches!(
                        store["Path"].as_str(),
                        Some(super::ADAPTIVE_BINDING | super::WINDOWS_INK_BINDING)
                    )
                {
                    return Err(format!(
                        "cannot replace preserved unsupported {button}; its original binding would be lost"
                    ));
                }
                if store["Path"].as_str() != Some(path) {
                    // The other binding type's properties do not carry over.
                    if store.is_object() {
                        set_field(store, "Settings", json!([]))?;
                    }
                    set_field(store, "Path", json!(path))?;
                }
                set_store_property(store, property, json!(value))?;
            }
            set_field(store, "Enable", json!(current_enabled))?;
        }
        if current_threshold != old_threshold || current_percent != old_percent {
            let raw = current_threshold.ok_or(
                "hardware tip-switch contact cannot be represented as an OTD pressure threshold",
            )?;
            let max_pressure = profile.tablet.max_pressure;
            let percent = current_percent.filter(|percent| super::activation_raw_for(f64::from(*percent), max_pressure).ok() == Some(raw)).map(f64::from).unwrap_or_else(|| (f64::from(raw) - 0.5) * 100.0 / f64::from(max_pressure));
            if super::activation_raw_for(percent, max_pressure).ok() != Some(raw) {
                return Err(format!(
                    "raw threshold {raw} has no supported OTD percent representation"
                ));
            }
            set_field(&mut selected["Bindings"], threshold, json!(percent))?;
        }
    }
    for (name, current, old) in [
        ("EnableDragBindings", profile.contact.drag_only, baseline.contact.drag_only),
        ("DisablePressure", profile.contact.disable_pressure, baseline.contact.disable_pressure),
        ("DisableTilt", profile.contact.disable_tilt, baseline.contact.disable_tilt),
    ] {
        if current != old { set_field(&mut selected["Bindings"], name, json!(current))?; }
    }
    if profile.mouse_buttons != baseline.mouse_buttons {
        ensure_object(&mut selected["Bindings"])?;
        export_button_list(&mut selected["Bindings"]["MouseButtons"], "mouse button", &profile.mouse_buttons, &baseline.mouse_buttons, false)?;
    }
    for (name, current, old) in [
        ("MouseScrollUp", &profile.mouse_scroll_up, &baseline.mouse_scroll_up),
        ("MouseScrollDown", &profile.mouse_scroll_down, &baseline.mouse_scroll_down),
    ] {
        if current != old {
            ensure_object(&mut selected["Bindings"])?;
            write_pen_button(&mut selected["Bindings"][name], current, false)?;
        }
    }
    export_pen_buttons(selected, profile, &baseline)?;
    let source_pen = baseline.output == super::OutputKind::Pen;
    if profile.aux_buttons != baseline.aux_buttons {
        ensure_object(&mut selected["Bindings"])?;
        export_button_list(
            &mut selected["Bindings"]["AuxButtons"],
            "express key",
            &profile.aux_buttons,
            &baseline.aux_buttons,
            source_pen,
        )?;
    }
    export_wheels(selected, profile, &baseline, source_pen)?;
    if profile.radial_follow.len() != baseline.radial_follow.len() {
        return Err("OTD export cannot infer filter identity/order after adding or removing native Radial Follow entries. Export edits to existing entries or keep the Rust TOML profile.".into());
    }
    if !profile.radial_follow.is_empty() {
        let filters = selected["Filters"]
            .as_array_mut()
            .ok_or("source Filters is not an array")?;
        let mut active = filters.iter_mut().filter(|store| {
            store["Path"].as_str() == Some(crate::radial_follow::FILTER_PATH)
                && (store["Enable"].as_bool() == Some(true) || imported.legacy_force_radial_follow)
        });
        for (current, old) in profile.radial_follow.iter().zip(&baseline.radial_follow) {
            let store = active
                .next()
                .ok_or("source Radial Follow identities no longer match")?;
            if imported.legacy_force_radial_follow {
                set_field(store, "Enable", json!(true))?;
            }
            for (key, value, previous) in [
                ("OuterRadius", current.outer_radius, old.outer_radius),
                ("InnerRadius", current.inner_radius, old.inner_radius),
                (
                    "SmoothingCoefficient",
                    current.smoothing_coefficient,
                    old.smoothing_coefficient,
                ),
                (
                    "SoftKneeScale",
                    current.soft_knee_scale,
                    old.soft_knee_scale,
                ),
                (
                    "SmoothingLeakCoefficient",
                    current.smoothing_leak_coefficient,
                    old.smoothing_leak_coefficient,
                ),
            ] {
                if !value.is_finite() {
                    return Err(format!("{key} must be finite for OTD export"));
                }
                if value != previous {
                    set_store_property(store, key, json!(value))?;
                }
            }
        }
    }
    if document == original {
        return Ok(imported.settings_json.clone());
    }
    reject_precision_loss(&imported.settings_json)?;
    serde_json::to_string_pretty(&document).map_err(|error| error.to_string())
}

/// Reconcile only changed bindings. Unknown stores and buttons beyond the
/// runtime limit remain in the archived document; replacing them is refused.
fn export_pen_buttons(
    selected: &mut Value,
    profile: &Profile,
    baseline: &Profile,
) -> Result<(), String> {
    use crate::output::buttons::ButtonAction;

    if profile.pen_buttons.len() > super::MAX_PEN_BUTTONS {
        return Err("OTD export supports at most 64 pen button actions".into());
    }
    for action in &profile.pen_buttons {
        if action.to_string().parse::<ButtonAction>().as_ref() != Ok(action) {
            return Err("OTD export requires supported, nonempty pen button actions".into());
        }
    }
    if profile.pen_buttons == baseline.pen_buttons && profile.output == baseline.output {
        return Ok(());
    }
    if profile.pen_buttons.len() < baseline.pen_buttons.len() {
        return Err("OTD export cannot shrink the preserved pen button list; set unwanted buttons to none instead".into());
    }
    ensure_object(&mut selected["Bindings"])?;
    let source = &mut selected["Bindings"]["PenButtons"];
    if source.is_null() {
        *source = json!([]);
    }
    let stores = source
        .as_array_mut()
        .ok_or("cannot edit preserved pen buttons: source PenButtons is not an array")?;
    stores.resize(stores.len().max(profile.pen_buttons.len()), Value::Null);
    for (index, action) in profile.pen_buttons.iter().enumerate() {
        let store = &mut stores[index];
        let unchanged = baseline.pen_buttons.get(index) == Some(action);
        if unchanged {
            if profile.output == baseline.output {
                continue;
            }
            // Adaptive Tip/Eraser on mouse output cannot mean the same action
            // on a pen output. Rebind such stores to the explicit mouse action.
            let adapted = serde_json::from_value::<super::OtdStore>(store.clone())
                .ok()
                .and_then(|store| {
                    super::pen_button_action(Some(&store), profile.output == super::OutputKind::Pen)
                        .ok()
                });
            if adapted.as_ref() == Some(action) || *action == ButtonAction::None {
                continue;
            }
        }
        write_pen_button(store, action, baseline.output == super::OutputKind::Pen)
            .map_err(|error| format!("cannot export pen button {}: {error}", index + 1))?;
    }
    Ok(())
}

fn check_actions(actions: &[crate::output::buttons::ButtonAction], noun: &str) -> Result<(), String> {
    if actions.len() > super::MAX_PEN_BUTTONS {
        return Err(format!(
            "OTD export supports at most {} {noun} actions",
            super::MAX_PEN_BUTTONS
        ));
    }
    for action in actions {
        if action.to_string().parse::<crate::output::buttons::ButtonAction>().as_ref() != Ok(action) {
            return Err(format!("OTD export requires supported, nonempty {noun} actions"));
        }
    }
    Ok(())
}

/// Reconciles one `PluginSettingStoreCollection` of button bindings, like
/// `export_pen_buttons` without the output-kind rebinding.
fn export_button_list(
    stores: &mut Value,
    noun: &str,
    current: &[crate::output::buttons::ButtonAction],
    baseline: &[crate::output::buttons::ButtonAction],
    source_pen: bool,
) -> Result<(), String> {
    check_actions(current, noun)?;
    if current == baseline {
        return Ok(());
    }
    if current.len() < baseline.len() {
        return Err(format!(
            "OTD export cannot shrink the preserved {noun} list; set unwanted entries to none instead"
        ));
    }
    if stores.is_null() {
        *stores = json!([]);
    }
    let stores = stores
        .as_array_mut()
        .ok_or_else(|| format!("cannot edit preserved {noun}s: the source list is not an array"))?;
    stores.resize(stores.len().max(current.len()), Value::Null);
    for (index, action) in current.iter().enumerate() {
        if baseline.get(index) == Some(action) {
            continue;
        }
        write_pen_button(&mut stores[index], action, source_pen)
            .map_err(|error| format!("cannot export {noun} {}: {error}", index + 1))?;
    }
    Ok(())
}

/// Reconciles changed wheels into `Bindings.WheelBindings`, adding
/// `WheelBindingSettings` entries for wheels the source did not list.
fn export_wheels(
    selected: &mut Value,
    profile: &Profile,
    baseline: &Profile,
    source_pen: bool,
) -> Result<(), String> {
    use crate::output::buttons::WheelBinding;

    let unbound = WheelBinding::default();
    let count = profile.wheels.len().max(baseline.wheels.len());
    if (0..count).all(|index| {
        profile.wheels.get(index).unwrap_or(&unbound) == baseline.wheels.get(index).unwrap_or(&unbound)
    }) {
        return Ok(());
    }
    ensure_object(&mut selected["Bindings"])?;
    let wheels = &mut selected["Bindings"]["WheelBindings"];
    if wheels.is_null() {
        *wheels = json!([]);
    }
    let wheels = wheels
        .as_array_mut()
        .ok_or("cannot edit preserved wheels: source WheelBindings is not an array")?;
    let step = |index: usize| {
        profile
            .tablet
            .controls
            .wheels()
            .get(index)
            .and_then(|wheel| wheel.degrees_per_step())
    };
    for index in 0..count {
        let current = profile.wheels.get(index).unwrap_or(&unbound);
        let old = baseline.wheels.get(index).unwrap_or(&unbound);
        if current == old {
            continue;
        }
        while wheels.len() <= index {
            // Upstream writes one step as both thresholds of a new wheel.
            let degrees = step(wheels.len()).unwrap_or(1.0);
            wheels.push(json!({
                "WheelButtons": [],
                "ClockwiseRotation": null,
                "ClockwiseActivationThreshold": degrees,
                "CounterClockwiseRotation": null,
                "CounterClockwiseActivationThreshold": degrees,
            }));
        }
        let entry = &mut wheels[index];
        ensure_object(entry)?;
        let wheel = index + 1;
        for (key, now, before, noun) in [
            ("ClockwiseRotation", &current.clockwise, &old.clockwise, "clockwise rotation"),
            (
                "CounterClockwiseRotation",
                &current.counter_clockwise,
                &old.counter_clockwise,
                "counter-clockwise rotation",
            ),
        ] {
            check_actions(std::slice::from_ref(now), noun)?;
            if now != before {
                write_pen_button(&mut entry[key], now, source_pen)
                    .map_err(|error| format!("cannot export wheel {wheel} {noun}: {error}"))?;
            }
        }
        for (key, now, before) in [
            (
                "ClockwiseActivationThreshold",
                current.clockwise_threshold,
                old.clockwise_threshold,
            ),
            (
                "CounterClockwiseActivationThreshold",
                current.counter_clockwise_threshold,
                old.counter_clockwise_threshold,
            ),
        ] {
            if now == before {
                continue;
            }
            let degrees = match now {
                Some(value) if value.is_finite() && value > 0.0 => f64::from(value),
                Some(_) => return Err(format!("wheel {wheel} thresholds must be positive")),
                None => step(index).ok_or_else(|| {
                    format!("wheel {wheel} has no step count to write as its default threshold")
                })?,
            };
            set_field(entry, key, json!(degrees))?;
        }
        if current.buttons != old.buttons {
            export_button_list(
                &mut entry["WheelButtons"],
                &format!("wheel {wheel} button"),
                &current.buttons,
                &old.buttons,
                source_pen,
            )?;
        }
    }
    Ok(())
}

fn pen_binding_property(path: &str) -> Option<&'static str> {
    match path {
        super::ADAPTIVE_BINDING => Some("Binding"),
        super::MOUSE_BINDING => Some("Button"),
        super::KEY_BINDING => Some("Key"),
        super::MULTI_KEY_BINDING => Some("Keys"),
        super::MOUSE_SCROLL_BINDING => Some("Direction"),
        _ => None,
    }
}

fn unknown_binding_properties(store: &Value, path: &str) -> bool {
    let allowed: &[&str] = match path {
        super::ADAPTIVE_BINDING => &["Binding"], super::MOUSE_BINDING => &["Button"],
        super::KEY_BINDING => &["Key"], super::MULTI_KEY_BINDING => &["Keys"],
        super::MOUSE_SCROLL_BINDING => &["Direction", "Amount", "Interval"],
        _ => return true,
    };
    store["Settings"].as_array().is_some_and(|settings| settings.iter().any(|setting| {
        setting["Property"].as_str().is_none_or(|property| !allowed.contains(&property))
            || setting.as_object().is_none_or(|fields| fields.keys().any(|key| key != "Property" && key != "Value"))
    }))
}

fn write_pen_button(
    store: &mut Value,
    action: &crate::output::buttons::ButtonAction,
    source_pen: bool,
) -> Result<(), String> {
    use crate::actions::MouseButton;
    use crate::output::buttons::ButtonAction;

    if !store.is_null() {
        let parsed: super::OtdStore = serde_json::from_value(store.clone())
            .map_err(|error| format!("unreadable preserved binding: {error}"))?;
        pen_binding_property(&parsed.path)
            .ok_or("replacing an unsupported source binding would lose its original settings")?;
        super::pen_button_action(Some(&parsed), source_pen)?;
    }
    if let ButtonAction::Scroll(scroll) = action {
        scroll.validate()?;
        ensure_object(store)?;
        if store["Path"].as_str() != Some(super::MOUSE_SCROLL_BINDING) {
            if let Some(path) = store["Path"].as_str() {
                if unknown_binding_properties(store, path) {
                    return Err("changing binding type would discard unknown source properties; keep the existing type or edit a new button".into());
                }
            }
            set_field(store, "Settings", json!([]))?;
            set_field(store, "Path", json!(super::MOUSE_SCROLL_BINDING))?;
        }
        set_field(store, "Enable", json!(true))?;
        set_store_property(store, "Direction", json!(match scroll.axis { crate::output::buttons::ScrollAxis::Vertical => "Vertical", crate::output::buttons::ScrollAxis::Horizontal => "Horizontal" }))?;
        set_store_property(store, "Amount", json!(scroll.amount))?;
        set_store_property(store, "Interval", json!(scroll.interval_ms))?;
        return Ok(());
    }
    let (path, property, value) = match action {
        ButtonAction::Scroll(_) => unreachable!("scroll handled above"),
        ButtonAction::None => {
            if !store.is_null() {
                set_field(store, "Enable", json!(false))?;
            }
            return Ok(());
        }
        ButtonAction::Barrel(number) => (
            super::ADAPTIVE_BINDING,
            "Binding",
            format!("Button {number}"),
        ),
        ButtonAction::Mouse(button) => (
            super::MOUSE_BINDING,
            "Button",
            match button {
                MouseButton::Left => "Left",
                MouseButton::Right => "Right",
                MouseButton::Middle => "Middle",
                MouseButton::Backward => "Backward",
                MouseButton::Forward => "Forward",
            }
            .to_owned(),
        ),
        ButtonAction::Keys(keys) if keys.len() == 1 => (
            super::KEY_BINDING,
            "Key",
            crate::keys::name_of(keys[0])
                .ok_or("unsupported key usage")?
                .to_owned(),
        ),
        ButtonAction::Keys(keys) => (
            super::MULTI_KEY_BINDING,
            "Keys",
            crate::keys::chord_text(keys),
        ),
    };
    if !store.is_null() && store["Path"].as_str() != Some(path) {
        let old_path = store["Path"].as_str().ok_or("unsupported source binding")?;
        if unknown_binding_properties(store, old_path) {
            return Err("changing binding type would discard unknown source properties; keep the existing type or edit a new button".into());
        }
        set_field(store, "Settings", json!([]))?;
    }
    set_field(store, "Path", json!(path))?;
    set_store_property(store, property, json!(value))?;
    set_field(store, "Enable", json!(true))
}

fn ensure_object(value: &mut Value) -> Result<(), String> {
    if value.is_null() {
        *value = json!({});
    }
    if !value.is_object() {
        return Err("cannot reconcile edits into a non-object source setting".into());
    }
    Ok(())
}

fn set_field(value: &mut Value, key: &str, replacement: Value) -> Result<(), String> {
    ensure_object(value)?;
    value[key] = replacement;
    Ok(())
}

fn set_store_property(store: &mut Value, property: &str, value: Value) -> Result<(), String> {
    ensure_object(store)?;
    if store["Settings"].is_null() {
        store["Settings"] = json!([]);
    }
    let settings = store["Settings"]
        .as_array_mut()
        .ok_or("source Settings is not an array")?;
    if let Some(setting) = settings
        .iter_mut()
        .rev()
        .find(|setting| setting["Property"].as_str() == Some(property))
    {
        set_field(setting, "Value", value)?;
    } else {
        settings.push(json!({"Property": property, "Value": value}));
    }
    Ok(())
}

fn timespan(duration: std::time::Duration) -> Result<String, String> {
    if !duration.as_nanos().is_multiple_of(100) || duration.as_nanos() / 100 > i64::MAX as u128 {
        return Err("OTD TimeSpan cannot exactly represent this reset delay; use nonnegative 100-nanosecond ticks within Int64 range".into());
    }
    let seconds = duration.as_secs();
    let days = seconds / 86_400;
    let prefix = if days == 0 {
        String::new()
    } else {
        format!("{days}.")
    };
    Ok(format!(
        "{prefix}{:02}:{:02}:{:02}.{:07}",
        (seconds / 3600) % 24,
        (seconds / 60) % 60,
        seconds % 60,
        duration.subsec_nanos() / 100
    ))
}

/// The archive is exact. Changed exports use serde_json's numeric storage, so
/// reject source numbers whose decimal value would change during serialization.
fn reject_precision_loss(text: &str) -> Result<(), String> {
    let mut chars = text.chars().peekable();
    while let Some(character) = chars.next() {
        if character == '"' {
            while let Some(inner) = chars.next() {
                if inner == '\\' {
                    chars.next();
                } else if inner == '"' {
                    break;
                }
            }
        } else if character == '-' || character.is_ascii_digit() {
            let mut token = String::from(character);
            while chars.peek().is_some_and(|value| {
                value.is_ascii_digit() || matches!(value, '.' | 'e' | 'E' | '+' | '-')
            }) {
                token.push(chars.next().unwrap());
            }
            let value: serde_json::Number =
                serde_json::from_str(&token).map_err(|error| error.to_string())?;
            if decimal_identity(&token) != decimal_identity(&value.to_string()) {
                return Err("OTD export would round a preserved JSON number. Keep the exact imported source or use the Rust TOML archive; arbitrary-precision JSON export is not implemented.".into());
            }
        }
    }
    Ok(())
}

fn decimal_identity(text: &str) -> Option<(bool, String, i64)> {
    let negative = text.starts_with('-');
    let unsigned = text.trim_start_matches('-');
    let (mantissa, exponent) = unsigned.split_once(['e', 'E']).unwrap_or((unsigned, "0"));
    let fractional = mantissa
        .split_once('.')
        .map_or(0, |(_, digits)| digits.len());
    let mut exponent = exponent
        .parse::<i64>()
        .ok()?
        .checked_sub(fractional as i64)?;
    let digits: String = mantissa
        .chars()
        .filter(|character| *character != '.')
        .collect();
    let mut digits = digits.trim_start_matches('0').to_owned();
    if digits.is_empty() {
        return Some((false, "0".into(), 0));
    }
    while digits.ends_with('0') {
        digits.pop();
        exponent = exponent.checked_add(1)?;
    }
    Some((negative, digits, exponent))
}
