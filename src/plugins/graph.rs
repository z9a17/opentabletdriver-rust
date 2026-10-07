use super::{PipelineStage, Plugin, PluginChain};
use crate::dotnet::{Graph, GraphNode, GraphReport};
use otd_core::plugins::{DispatchInput, PipelineRuntime};
use otd_core::reports::{ReportKind, ReportValues};
use otd_plugin_api::Sample;
use std::ffi::c_void;
use std::io;

/// `builtin_slot` places the host's built-in filters as a native node before
/// that plugin, so they also see emissions from managed filters before them.
pub(super) fn create(
    plugins: &[Plugin],
    builtin_slot: Option<usize>,
) -> Result<Option<Graph>, String> {
    create_if(plugins, builtin_slot, false)
}
pub(super) fn create_if(plugins: &[Plugin], builtin_slot: Option<usize>, force: bool) -> Result<Option<Graph>, String> {
    if !force && !plugins.iter().any(|plugin| plugin.managed) {
        return Ok(None);
    }
    let mut nodes: Vec<_> = plugins
        .iter()
        .enumerate()
        .map(|(index, plugin)| GraphNode {
            context: if plugin.managed {
                plugin.context
            } else {
                std::ptr::null_mut()
            },
            index: index as u32,
            stage: match plugin.stage {
                PipelineStage::PreTransform => 1,
                PipelineStage::Pixels => 2,
            },
        })
        .collect();
    if let Some(slot) = builtin_slot {
        nodes.insert(
            slot.min(nodes.len()),
            GraphNode {
                context: std::ptr::null_mut(),
                index: builtin_node(plugins),
                stage: 1,
            },
        );
    }
    Graph::new(&nodes).map(Some)
}

/// The native node index that runs the built-in filters: one past the plugins.
fn builtin_node(plugins: &[Plugin]) -> u32 {
    plugins.len() as u32
}

struct Scope<'a> {
    plugins: &'a mut [Plugin],
    runtime: &'a mut dyn PipelineRuntime,
    failure: &'a mut Option<usize>,
    time_ns: u64,
    /// The graph has a node for the built-in filters; nothing else runs them.
    builtins_in_graph: bool,
    error: Option<io::Error>,
}

impl Scope<'_> {
    fn native(
        &mut self,
        index: usize,
        kind: ReportKind,
        values: &mut ReportValues,
    ) -> io::Result<()> {
        if self.builtins_in_graph && index == self.plugins.len() {
            return self.runtime.builtins(values);
        }
        let Some(plugin) = self.plugins.get_mut(index) else {
            return Err(io::Error::other("invalid native graph node"));
        };
        if plugin.managed {
            return Err(io::Error::other(
                "managed graph attempted native dispatch of a managed instance",
            ));
        }
        if plugin.disabled {
            return Ok(());
        }
        if kind == ReportKind::OutOfRange {
            plugin.reset();
            return Ok(());
        }
        let Some([x, y]) = values.position else {
            return Ok(());
        };
        let mut sample = Sample {
            x,
            y,
            time_ns: self.time_ns,
            pressure: values.pressure.unwrap_or(0),
            // V1's proximity flag means a detected positional sample, including
            // the PTH Sense-only band. The separate near-proximity interface
            // remains available to managed consumers without redefining v1.
            flags: otd_plugin_api::PROXIMITY
                | if values.eraser.unwrap_or(false) {
                    otd_plugin_api::ERASER
                } else {
                    0
                },
        };
        if !plugin.process(&mut sample) {
            *self.failure = Some(index);
            return Err(io::Error::other(format!(
                "Native filter {} failed or returned a nonfinite position",
                plugin.name
            )));
        }
        values.position = Some([sample.x, sample.y]);
        Ok(())
    }

    fn step(&mut self, operation: u32, index: u32, frame: &mut GraphReport) -> io::Result<bool> {
        if operation == 6 { self.runtime.managed_command(frame.managed_command()?)?; return Ok(true); }
        let (kind, mut values) = frame.decode()?;
        let keep = match operation {
            0 => {
                if !self.builtins_in_graph {
                    self.runtime.builtins(&mut values)?;
                }
                true
            }
            1 => {
                self.native(index as usize, kind, &mut values)?;
                true
            }
            2 => self.runtime.transform(kind, &mut values)?,
            4 => {
                // Fused transform and output, for a graph without managed
                // post-transform filters: one native call instead of two.
                let keep = self.runtime.transform(kind, &mut values)?;
                if keep {
                    // .NET pins this report's Raw array until the callback returns.
                    self.runtime
                        .output(kind, &values, unsafe { frame.raw()? })?;
                }
                keep
            }
            5 => { self.runtime.bindings(kind, &mut values, unsafe { frame.raw()? })?; true }
            3 => {
                // .NET pins this report's current Raw array until the callback
                // returns. The output stage cannot retain this borrowed view.
                self.runtime
                    .output(kind, &values, unsafe { frame.raw()? })?;
                true
            }
            _ => return Err(io::Error::other("unknown synchronous graph operation")),
        };
        if operation == 5 { frame.pressure = values.pressure.unwrap_or(0); }
        if operation != 3 && operation != 5 {
            frame.set_position(&values);
        }
        Ok(keep)
    }
}

unsafe extern "C" fn callback(
    scope: *mut c_void,
    operation: u32,
    index: u32,
    frame: *mut GraphReport,
) -> i32 {
    // The FFI entry cannot unwind into CoreCLR. No successful native operation
    // allocates, and no mutable Plugin reference survives a managed continuation.
    let scope = unsafe { &mut *scope.cast::<Scope<'_>>() };
    if scope.error.is_some() {
        return -1;
    }
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let frame =
            unsafe { frame.as_mut() }.ok_or_else(|| io::Error::other("missing graph frame"))?;
        scope.step(operation, index, frame)
    }));
    match result {
        Ok(Ok(true)) => 0,
        Ok(Ok(false)) => 1,
        Ok(Err(error)) => {
            scope.error = Some(error);
            -1
        }
        Err(_) => {
            scope.error = Some(io::Error::other("native pipeline continuation panicked"));
            -1
        }
    }
}

impl PluginChain {
    pub(super) fn next_tick_graph(&self) -> Option<std::time::Duration> {
        self.graph.as_ref()?.next_tick()
    }

    pub(super) fn tick_graph(
        &mut self,
        now: std::time::Instant,
        runtime: &mut dyn PipelineRuntime,
    ) -> io::Result<()> {
        let Some(graph) = &self.graph else {
            return Ok(());
        };
        let time_ns = now
            .saturating_duration_since(self.epoch)
            .as_nanos()
            .min(u128::from(u64::MAX)) as u64;
        let mut scope = Scope {
            plugins: &mut self.plugins,
            runtime,
            failure: &mut self.failure,
            time_ns,
            builtins_in_graph: self.builtin_slot.is_some(),
            error: None,
        };
        let result = unsafe { graph.tick(callback, (&mut scope as *mut Scope<'_>).cast()) };
        let result = match scope.error.take() {
            Some(error) => Err(error),
            None => result,
        };
        if result.is_err()
            && let Some(index) = graph.failed_index()
            && let Some(plugin) = self.plugins.get_mut(index)
        {
            plugin.disabled = true;
            self.failure = Some(index);
        }
        result
    }

    pub(super) fn dispatch_graph(
        &mut self,
        input: DispatchInput<'_>,
        runtime: &mut dyn PipelineRuntime,
    ) -> io::Result<()> {
        let mut values = input.values;
        let raw = input.raw;
        let time_ns = input
            .now
            .saturating_duration_since(self.epoch)
            .as_nanos()
            .min(u128::from(u64::MAX)) as u64;
        if let Some(graph) = &self.graph {
            // Runtime decoders supply their exact interfaces, including
            // IntuosV2 pen buttons. Positional puck reports remain mouse reports.
            if input.pen.is_some() {
                if raw.is_empty() || raw.len() > usize::from(u16::MAX) {
                    return Err(io::Error::other(
                        "managed dispatch requires the complete raw pen packet",
                    ));
                }
            }
            // The built-in filters run first unless the graph has a node for
            // them. The fused bridge leaves them to the host, which saves a
            // native continuation per report.
            let builtins_in_graph = self.builtin_slot.is_some();
            if graph.runs_builtins_in_host() && !builtins_in_graph {
                runtime.builtins(&mut values)?;
            }
            let frame = GraphReport::new(input.kind, &values, raw)?;
            // Disjoint field borrows: graph owns only managed references; this
            // callback scope owns the native nodes and host stages for this call.
            let mut scope = Scope {
                plugins: &mut self.plugins,
                runtime,
                failure: &mut self.failure,
                time_ns,
                builtins_in_graph,
                error: None,
            };
            let result =
                unsafe { graph.dispatch(&frame, callback, (&mut scope as *mut Scope<'_>).cast()) };
            if let Some(error) = scope.error {
                return Err(error);
            }
            if result.is_err()
                && let Some(index) = graph.failed_index()
                && let Some(plugin) = self.plugins.get_mut(index)
            {
                plugin.disabled = true;
                self.failure = Some(index);
            }
            result
        } else {
            let mut scope = Scope {
                plugins: &mut self.plugins,
                runtime,
                failure: &mut self.failure,
                time_ns,
                builtins_in_graph: false,
                error: None,
            };
            // The built-in filters run before the plugin at their slot, or first.
            let slot = self.builtin_slot.unwrap_or(0);
            for stage in [PipelineStage::PreTransform, PipelineStage::Pixels] {
                if stage == PipelineStage::Pixels
                    && !scope.runtime.transform(input.kind, &mut values)?
                {
                    return Ok(());
                }
                for index in 0..scope.plugins.len() {
                    if stage == PipelineStage::PreTransform && index == slot {
                        scope.runtime.builtins(&mut values)?;
                    }
                    if scope.plugins[index].stage == stage {
                        scope.native(index, input.kind, &mut values)?;
                    }
                }
                if stage == PipelineStage::PreTransform && slot >= scope.plugins.len() {
                    scope.runtime.builtins(&mut values)?;
                }
            }
            scope.runtime.output(input.kind, &values, raw)
        }
    }
}
