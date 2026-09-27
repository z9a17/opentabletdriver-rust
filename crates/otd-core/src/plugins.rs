//! Plugin entries in a profile, and the interface the report pipeline uses to
//! run filters loaded from DLLs. Loading and calling DLLs is platform code.

use std::path::PathBuf;
use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::protocol::PenReport;
use crate::reports::{ReportKind, ReportValues};
use std::io;

/// One synchronous graph input. Raw and properties are independent; never
/// reparse plugin-mutated raw bytes to replace the exposed property values.
pub struct DispatchInput<'a> {
    pub kind: ReportKind,
    pub values: ReportValues,
    pub raw: &'a [u8],
    pub pen: Option<PenReport>,
    pub now: Instant,
}

/// Host-owned stages. A suppressed transform returns false and never reaches
/// bindings/output. Each final emission invokes output separately, in order.
pub trait PipelineRuntime {
    fn builtins(&mut self, values: &mut ReportValues) -> io::Result<()>;
    fn transform(&mut self, kind: ReportKind, values: &mut ReportValues) -> io::Result<bool>;
    fn output(&mut self, kind: ReportKind, values: &ReportValues, raw: &[u8]) -> io::Result<()>;
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PluginKind {
    #[default]
    Native,
    Dotnet,
    /// An OpenTabletDriver `ITool`: started with the driver, stopped with it.
    #[serde(rename = "dotnet_tool")]
    DotnetTool,
}

impl PluginKind {
    /// Runs through the .NET bridge.
    pub fn is_managed(self) -> bool {
        matches!(self, Self::Dotnet | Self::DotnetTool)
    }
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
        if self.kind.is_managed() && self.type_name.trim().is_empty() {
            return Err(".NET plugin requires a type_name".into());
        }
        Ok(())
    }
}

/// The enabled DLL filters of a profile, in profile order, as the report
/// pipeline calls them. PreTransform filters receive report units; Pixels
/// filters receive pixels after absolute mapping or deltas in relative mode.
pub trait Filters {
    /// Managed graph callbacks expose upstream f32 positions. Native-only
    /// chains can retain the host mapper's f64 precision until final output.
    fn uses_managed_graph(&self) -> bool {
        false
    }
    /// Synchronous continuation graph. Implementations may emit zero or many
    /// reports; a failure never authorizes replaying the original input.
    fn dispatch(
        &mut self,
        input: DispatchInput<'_>,
        runtime: &mut dyn PipelineRuntime,
    ) -> io::Result<()> {
        let mut values = input.values;
        runtime.builtins(&mut values)?;
        // Compatibility for native/synthetic implementations of the older
        // position-only trait. The real DLL chain overrides this whole method.
        if self.has_pre()
            && let (Some(position), Some(pen)) = (values.position, input.pen)
        {
            let point = self.process_pre((position[0], position[1]), pen, input.now);
            values.position = Some([point.0, point.1]);
        } else if input.kind == ReportKind::OutOfRange {
            self.reset();
        }
        if !runtime.transform(input.kind, &mut values)? {
            return Ok(());
        }
        if self.has_pixels()
            && let (Some(position), Some(pen)) = (values.position, input.pen)
        {
            let point = self.process_pixels((position[0], position[1]), pen, input.now);
            values.position = Some([point.0, point.1]);
        }
        runtime.output(input.kind, &values, input.raw)
    }
    /// Time until the next timer tick of a timer-driven (async) filter, or
    /// `None` without one. The session wakes for it on the report thread.
    fn next_tick(&mut self) -> Option<std::time::Duration> {
        None
    }
    /// Fires due filter timers; their emissions continue downstream of the
    /// emitting filter through `runtime`, like a synchronous emission.
    fn tick(&mut self, _now: Instant, _runtime: &mut dyn PipelineRuntime) -> io::Result<()> {
        Ok(())
    }
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
