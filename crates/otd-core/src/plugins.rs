//! Plugin entries in a profile, and the interface the report pipeline uses to
//! run filters loaded from DLLs. Loading and calling DLLs is platform code.

use std::path::PathBuf;
use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::protocol::PenReport;

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PluginKind {
    #[default]
    Native,
    Dotnet,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PipelineStage {
    PreTransform,
    Pixels,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PluginConfig {
    pub path: PathBuf,
    #[serde(default)]
    pub kind: PluginKind,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub type_name: String,
    #[serde(default = "empty_settings")]
    pub settings_json: String,
}

fn empty_settings() -> String {
    "{}".into()
}

impl PluginConfig {
    pub fn validate(&self) -> Result<(), String> {
        if self.path.as_os_str().is_empty() {
            return Err("plugin DLL path is empty".into());
        }
        if self.settings_json.len() > 65_536 {
            return Err("plugin settings exceed 64 KiB".into());
        }
        let value: serde_json::Value = serde_json::from_str(&self.settings_json)
            .map_err(|e| format!("invalid plugin settings JSON: {e}"))?;
        if !value.is_object() {
            return Err("plugin settings must be a JSON object".into());
        }
        if self.kind == PluginKind::Dotnet && self.type_name.trim().is_empty() {
            return Err(".NET plugin requires a type_name".into());
        }
        Ok(())
    }
}

/// The enabled DLL filters of a profile, in profile order, as the report
/// pipeline calls them. PreTransform filters receive report units; Pixels
/// filters receive display pixels after absolute mapping.
pub trait Filters {
    /// Supplies the exact transport packet before its decoded pen is dispatched.
    /// Implementations retaining it must copy into storage allocated at setup.
    /// Synthetic position-only consumers may leave this hook as a no-op.
    fn prepare_report(&mut self, _pen: PenReport, _raw: &[u8]) {}
    fn has_pre(&self) -> bool;
    fn process_pre(&mut self, position: (f32, f32), pen: PenReport, now: Instant) -> (f32, f32);
    fn has_pixels(&self) -> bool;
    fn process_pixels(&mut self, position: (f32, f32), pen: PenReport, now: Instant) -> (f32, f32);
    /// Clears filter state; called when the pen is no longer detected.
    fn reset(&mut self);
    /// The name of a filter disabled by a failure since the last call.
    fn take_failure(&mut self) -> Option<&str>;
}

/// A profile without DLL filters.
pub struct NoFilters;

impl Filters for NoFilters {
    fn has_pre(&self) -> bool {
        false
    }

    fn process_pre(&mut self, position: (f32, f32), _: PenReport, _: Instant) -> (f32, f32) {
        position
    }

    fn has_pixels(&self) -> bool {
        false
    }

    fn process_pixels(&mut self, position: (f32, f32), _: PenReport, _: Instant) -> (f32, f32) {
        position
    }

    fn reset(&mut self) {}

    fn take_failure(&mut self) -> Option<&str> {
        None
    }
}
