//! Schema-1 preservation is an archive, not a claim that every imported store runs.
//! Contract: OTD 736003ed72c8bbb28033b039d5a0bb76c344145c,
//! Desktop/Settings.cs, Profiles/Profile.cs and Reflection/PluginSettingStore.cs.
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

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
        "The full original OTD settings collection is preserved as an archive. Only the selected PTH-660 mapping, supported tip/eraser actions and native Radial Follow entries are imported for execution; edits to the Rust profile do not rewrite the archived JSON.".into(),
    )];
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
            if matches!(name.as_str(), "TipButton" | "EraserButton") {
                continue;
            }
            if matches!(
                name.as_str(),
                "DisablePressure" | "DisableTilt" | "EnableDragBindings"
            ) && value.as_bool() == Some(true)
            {
                diagnostics.push(ProfileDiagnostic::unsupported(
                    format!("Profiles[{selected}].Bindings.{name}"),
                    "This enabled binding option is preserved but is not implemented.".into(),
                ));
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
            "radial_follow",
            "plugins",
        ],
        &mut preserved,
    );
    for (name, keys) in [
        ("crop", &["x", "y", "width", "height"][..]),
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
