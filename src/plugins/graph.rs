use super::{PipelineStage, Plugin, PluginChain};
use crate::dotnet::{Graph, GraphNode, GraphReport};
use otd_core::plugins::{DispatchInput, PipelineRuntime};
use otd_core::reports::{Buttons, ReportKind, ReportValues};
use otd_plugin_api::Sample;
use std::ffi::c_void;
use std::io;

pub(super) fn create(plugins: &[Plugin]) -> Result<Option<Graph>, String> {
    if !plugins.iter().any(|plugin| plugin.managed) {
        return Ok(None);
    }
    let nodes: Vec<_> = plugins
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
    Graph::new(&nodes).map(Some)
}

struct Scope<'a> {
    plugins: &'a mut [Plugin],
    runtime: &'a mut dyn PipelineRuntime,
    failure: &'a mut Option<usize>,
    time_ns: u64,
    error: Option<io::Error>,
}

impl Scope<'_> {
    fn native(
        &mut self,
        index: usize,
        kind: ReportKind,
        values: &mut ReportValues,
    ) -> io::Result<()> {
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
        let (kind, mut values) = frame.decode()?;
        let keep = match operation {
            0 => {
                self.runtime.builtins(&mut values)?;
                true
            }
            1 => {
                self.native(index as usize, kind, &mut values)?;
                true
            }
            2 => self.runtime.transform(kind, &mut values)?,
            3 => {
                // .NET pins this report's current Raw array until the callback
                // returns. The output stage cannot retain this borrowed view.
                self.runtime
                    .output(kind, &values, unsafe { frame.raw()? })?;
                true
            }
            _ => return Err(io::Error::other("unknown synchronous graph operation")),
        };
        if operation != 3 {
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
    pub(super) fn dispatch_graph(
        &mut self,
        input: DispatchInput<'_>,
        runtime: &mut dyn PipelineRuntime,
    ) -> io::Result<()> {
        let mut values = input.values;
        let mut raw = input.raw;
        let time_ns = input
            .now
            .saturating_duration_since(self.epoch)
            .as_nanos()
            .min(u128::from(u64::MAX)) as u64;
        if let Some(graph) = &self.graph {
            // Legacy synthetic callers prepare their actual raw bytes before
            // process(). Production dispatch passes the transport borrow directly.
            if raw.is_empty() && input.pen.is_some() && self.prepared_pen == input.pen {
                raw = &self.raw[..self.raw_length];
            }
            if let Some(pen) = input.pen {
                let (minimum, flags_at, count) = match pen.id {
                    0x10 => (17, 1, 2),
                    0x1e => (13, 2, 3),
                    _ => {
                        return Err(io::Error::other(
                            "managed dispatch has an unsupported native pen ID",
                        ));
                    }
                };
                if raw.first() != Some(&pen.id) || raw.len() < minimum || raw.len() > 192 {
                    return Err(io::Error::other(
                        "managed dispatch requires the matching complete raw pen packet",
                    ));
                }
                if input.kind == ReportKind::Data && values.pen_buttons.is_none() {
                    values.pen_buttons =
                        Buttons::from_bits(u64::from(raw[flags_at] >> 1), count).ok();
                }
            }
            let frame = GraphReport::new(input.kind, &values, raw)?;
            // Disjoint field borrows: graph owns only managed references; this
            // callback scope owns the native nodes and host stages for this call.
            let mut scope = Scope {
                plugins: &mut self.plugins,
                runtime,
                failure: &mut self.failure,
                time_ns,
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
                error: None,
            };
            scope.runtime.builtins(&mut values)?;
            for stage in [PipelineStage::PreTransform, PipelineStage::Pixels] {
                if stage == PipelineStage::Pixels
                    && !scope.runtime.transform(input.kind, &mut values)?
                {
                    return Ok(());
                }
                for index in 0..scope.plugins.len() {
                    if scope.plugins[index].stage == stage {
                        scope.native(index, input.kind, &mut values)?;
                    }
                }
            }
            scope.runtime.output(input.kind, &values, raw)
        }
    }
}
