//! Synchronous report graph: built-ins, pre filters, transform, post filters,
//! bindings, then output. Every emission completes before the emitting filter
//! resumes. Native-only processing uses inline report state and no queue.

use std::io;
use std::time::Instant;

use crate::config::{ContactPolicy, Profile};
use crate::mapping::Mapper;
use crate::output::{MouseOutput, MousePacket};
use crate::plugins::{DispatchInput, Filters, PipelineRuntime};
use crate::protocol::PenReport;
use crate::radial_follow::RadialFollowSmoothingTabletSpace;
use crate::relative::RelativeMapper;
use crate::reports::{Buttons, ReportKind, ReportValues};

#[derive(Clone, Copy, Debug, Default)]
pub struct DispatchStats {
    pub reports: u64,
    pub packets: u64,
}

pub struct ReportPipeline {
    contact: ContactPolicy,
    filters: Vec<RadialFollowSmoothingTabletSpace>,
    relative: Option<RelativeMapper>,
    output: MouseOutput,
    is_eraser: bool,
    desired_contact: bool,
    faulted: bool,
}

impl ReportPipeline {
    pub fn new(profile: &Profile) -> Result<Self, String> {
        profile.validate_filter_execution()?;
        Ok(Self {
            contact: profile.contact,
            filters: profile
                .radial_follow
                .iter()
                .copied()
                .map(|settings| RadialFollowSmoothingTabletSpace::new_for(settings, profile.tablet))
                .collect(),
            relative: profile
                .relative
                .map(|settings| RelativeMapper::new_for(settings, profile.tablet))
                .transpose()?,
            output: MouseOutput::new(),
            is_eraser: false,
            desired_contact: false,
            faulted: false,
        })
    }

    pub fn is_relative(&self) -> bool {
        self.relative.is_some()
    }

    /// Compatibility wrapper for callers interested only in whether any packet
    /// was sent. The sink can now be invoked several times for one input.
    pub fn process(
        &mut self,
        pen: PenReport,
        now: Instant,
        mapper: Option<Mapper>,
        plugins: &mut impl Filters,
        send: impl FnMut(MousePacket) -> io::Result<()>,
    ) -> io::Result<bool> {
        self.process_with_raw(pen, &[], now, mapper, plugins, send)
            .map(|stats| stats.packets != 0)
    }

    pub fn process_with_raw(
        &mut self,
        pen: PenReport,
        raw: &[u8],
        now: Instant,
        mapper: Option<Mapper>,
        plugins: &mut impl Filters,
        send: impl FnMut(MousePacket) -> io::Result<()>,
    ) -> io::Result<DispatchStats> {
        // A synthetic caller without the complete transport packet has no
        // button reading.
        let buttons = crate::decoders::intuos_v2_pen_buttons(raw).filter(|_| raw[0] == pen.id);
        self.process_pen(pen, raw, buttons, now, mapper, plugins, send)
    }

    /// `process_with_raw` with the decoder's pen buttons, so managed filters
    /// see `ITabletReport` on every tablet, not only IntuosV2 layouts.
    #[allow(clippy::too_many_arguments)] // Mirrors process_with_raw's seams.
    pub fn process_pen(
        &mut self,
        pen: PenReport,
        raw: &[u8],
        buttons: Option<Buttons>,
        now: Instant,
        mapper: Option<Mapper>,
        plugins: &mut impl Filters,
        send: impl FnMut(MousePacket) -> io::Result<()>,
    ) -> io::Result<DispatchStats> {
        let kind = if pen.in_range || pen.sense {
            ReportKind::Data
        } else {
            ReportKind::OutOfRange
        };
        let values = if kind == ReportKind::Data {
            ReportValues {
                position: Some([pen.x as f32, pen.y as f32]),
                pressure: Some(u32::from(pen.pressure)),
                eraser: Some(pen.eraser),
                pen_buttons: buttons,
                tilt: Some([f32::from(pen.tilt[0]), f32::from(pen.tilt[1])]),
                near_proximity: Some(pen.in_range),
                hover_distance: pen.hover_distance.map(u32::from),
                tip_switch: Some(pen.tip_switch),
                sense: Some(pen.sense),
                rotation: pen.rotation,
                ..ReportValues::default()
            }
        } else {
            ReportValues::default()
        };
        self.process_report(
            DispatchInput {
                kind,
                values,
                raw,
                pen: Some(pen),
                now,
            },
            mapper,
            plugins,
            send,
        )
    }

    /// Non-positional reports retain their category; absent pressure/buttons do
    /// not clear unrelated held state. This also accepts reports from aux/touch
    /// decoders without pretending they are pen samples.
    pub fn process_report(
        &mut self,
        input: DispatchInput<'_>,
        mapper: Option<Mapper>,
        plugins: &mut impl Filters,
        mut send: impl FnMut(MousePacket) -> io::Result<()>,
    ) -> io::Result<DispatchStats> {
        let mut initial_stats = DispatchStats::default();
        let mapping_paused = self.relative.is_none() && mapper.is_none();
        if self.faulted || mapping_paused {
            // Pausing the graph must also revoke held output, including when
            // loss arrives while no absolute mapping exists. Failed cleanup
            // remains pending before any subsequent graph input may run.
            initial_stats.packets += u64::from(self.release_all(&mut send)?);
        }
        if let Some(relative) = &mut self.relative {
            if input.kind == ReportKind::OutOfRange {
                relative.note_range_loss();
            } else if let Some([x, y]) = input.values.position
                && !relative.begin_input((x, y), input.now)
            {
                return Ok(initial_stats);
            }
        } else if mapping_paused {
            return Ok(initial_stats);
        }
        // An incoming loss is a transport notification even when it came from
        // a general report source without the legacy PenReport adapter. Loss
        // emitted by a plugin reaches Runtime::output instead of this entry.
        let physical_loss = input.kind == ReportKind::OutOfRange;
        let preserve_precision = !plugins.uses_managed_graph() && !plugins.has_pixels();
        let unfiltered_raw = if preserve_precision && !plugins.has_pre() && self.filters.is_empty()
        {
            input
                .pen
                .filter(|pen| input.values.position == Some([pen.x as f32, pen.y as f32]))
                .map(|pen| (pen.x, pen.y))
        } else {
            None
        };
        let mut runtime = Runtime {
            pipeline: self,
            mapper,
            now: input.now,
            send: &mut send,
            stats: initial_stats,
            exact_position: None,
            shown_position: None,
            preserve_precision,
            unfiltered_raw,
        };
        let result = plugins.dispatch(input, &mut runtime);
        let mut stats = runtime.stats;
        if let Err(error) = result {
            // A successful output prefix is already acknowledged. Never replay
            // the original report. Cleanup must succeed before dispatch resumes.
            self.faulted = true;
            if self.release_all(&mut send).is_ok() {
                self.faulted = false;
            }
            return Err(error);
        }
        if physical_loss {
            // Physical endpoint loss revokes native action ownership even when
            // a plugin suppresses the explicit OutOfRangeReport notification.
            match self.release_all(&mut send) {
                Ok(sent) => stats.packets += u64::from(sent),
                Err(error) => {
                    self.faulted = true;
                    return Err(error);
                }
            }
        }
        Ok(stats)
    }

    /// Fires due timers of timer-driven filters (upstream's
    /// `AsyncPositionedPipelineElement`). Their emissions take the same
    /// transform, contact and output path as any plugin emission.
    pub fn process_tick(
        &mut self,
        now: Instant,
        mapper: Option<Mapper>,
        plugins: &mut impl Filters,
        mut send: impl FnMut(MousePacket) -> io::Result<()>,
    ) -> io::Result<DispatchStats> {
        if self.faulted || (self.relative.is_none() && mapper.is_none()) {
            return Ok(DispatchStats::default());
        }
        let mut runtime = Runtime {
            pipeline: self,
            mapper,
            now,
            send: &mut send,
            stats: DispatchStats::default(),
            exact_position: None,
            shown_position: None,
            preserve_precision: false,
            unfiltered_raw: None,
        };
        let result = plugins.tick(now, &mut runtime);
        let stats = runtime.stats;
        if let Err(error) = result {
            self.faulted = true;
            if self.release_all(&mut send).is_ok() {
                self.faulted = false;
            }
            return Err(error);
        }
        Ok(stats)
    }

    pub fn release_all(
        &mut self,
        send: impl FnOnce(MousePacket) -> io::Result<()>,
    ) -> io::Result<bool> {
        self.desired_contact = false;
        let result = self.output.release_all(send);
        // Display-change/session cleanup also calls this outside process_report.
        // A failed release there must be retried before the graph can resume.
        self.faulted = result.is_err();
        result
    }
}

struct Runtime<'a, F> {
    pipeline: &'a mut ReportPipeline,
    mapper: Option<Mapper>,
    now: Instant,
    send: &'a mut F,
    stats: DispatchStats,
    exact_position: Option<(f64, f64)>,
    shown_position: Option<[f32; 2]>,
    preserve_precision: bool,
    unfiltered_raw: Option<(u32, u32)>,
}

impl<F: FnMut(MousePacket) -> io::Result<()>> PipelineRuntime for Runtime<'_, F> {
    fn builtins(&mut self, values: &mut ReportValues) -> io::Result<()> {
        if let Some([mut x, mut y]) = values.position {
            for filter in &mut self.pipeline.filters {
                (x, y) = filter.filter_raw_at(x, y, self.now);
            }
            values.position = Some([x, y]);
        }
        Ok(())
    }

    fn transform(&mut self, _kind: ReportKind, values: &mut ReportValues) -> io::Result<bool> {
        self.exact_position = None;
        self.shown_position = None;
        if let Some([x, y]) = values.position {
            if !x.is_finite() || !y.is_finite() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "nonfinite filter position",
                ));
            }
            let point = if let Some(relative) = &mut self.pipeline.relative {
                relative.transform_emission((x, y))
            } else {
                let Some(point) = self
                    .mapper
                    .and_then(|mapper| mapper.map_filtered_pixels(x, y))
                else {
                    return Ok(false);
                };
                point
            };
            let position = [point.0 as f32, point.1 as f32];
            if position.iter().any(|value| !value.is_finite()) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "transformed position exceeds finite range",
                ));
            }
            self.exact_position = Some(point);
            self.shown_position = Some(position);
            values.position = Some(position);
        }
        Ok(true)
    }

    fn output(&mut self, kind: ReportKind, values: &ReportValues, _raw: &[u8]) -> io::Result<()> {
        self.stats.reports += 1;
        if let Some(eraser) = values.eraser {
            self.pipeline.is_eraser = eraser;
        }
        if kind == ReportKind::OutOfRange {
            self.pipeline.desired_contact = false;
        } else if let Some(pressure) = values.pressure {
            let policy = self.pipeline.contact;
            let (enabled, threshold) = if self.pipeline.is_eraser {
                (policy.eraser_enabled, policy.eraser_threshold_raw)
            } else {
                (policy.tip_enabled, policy.tip_threshold_raw)
            };
            self.pipeline.desired_contact = enabled
                && threshold.map_or_else(
                    || values.tip_switch.unwrap_or(pressure != 0),
                    |threshold| pressure >= u32::from(threshold),
                );
        }
        let position = values.position.map(|[x, y]| {
            if self.preserve_precision && self.shown_position == values.position {
                self.exact_position.unwrap_or((f64::from(x), f64::from(y)))
            } else {
                (f64::from(x), f64::from(y))
            }
        });
        if position.is_some_and(|(x, y)| !x.is_finite() || !y.is_finite()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "nonfinite post-transform output",
            ));
        }
        let contact = self.pipeline.desired_contact;
        let emitted = if position.is_none() && kind == ReportKind::Data {
            self.pipeline
                .output
                .emit_contact(contact, &mut *self.send)?
        } else if let Some(relative) = &mut self.pipeline.relative {
            let delta = relative.quantize(position.unwrap_or((0.0, 0.0)))?;
            self.pipeline
                .output
                .emit_relative(delta, contact, &mut *self.send)?
        } else {
            let mapped = if let Some((x, y)) = self.unfiltered_raw {
                self.mapper.and_then(|mapper| mapper.map(x, y))
            } else {
                position
                    .and_then(|(x, y)| self.mapper.and_then(|mapper| mapper.normalize_pixels(x, y)))
            };
            self.pipeline
                .output
                .emit_mapped(mapped, contact, &mut *self.send)?
        };
        self.stats.packets += u64::from(emitted);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::display::DisplaySnapshot;
    use crate::mapping::Rect;
    use crate::protocol;
    use crate::radial_follow::RadialFollowSettings;
    use crate::relative::RelativeSettings;
    use std::hint::black_box;
    use std::time::Duration;

    // Real USB report prefixes: hover, contact, and hover above In Range.
    const CAPTURE: [[u8; 17]; 3] = [
        [
            0x10, 0x60, 0x14, 0x56, 0x00, 0xa3, 0x16, 0x00, 0x00, 0x00, 0x07, 0x04, 0, 0, 0, 0,
            0x28,
        ],
        [
            0x10, 0x61, 0x11, 0x55, 0x00, 0x8c, 0x12, 0x00, 0x0e, 0x11, 0x00, 0x07, 0, 0, 0, 0,
            0x19,
        ],
        [
            0x10, 0x40, 0xb8, 0x51, 0x00, 0x7e, 0x12, 0x00, 0x00, 0x00, 0x00, 0x04, 0, 0, 0, 0,
            0x3f,
        ],
    ];

    fn profile(relative: bool, filtered: bool) -> Profile {
        Profile {
            relative: relative.then_some(RelativeSettings {
                sensitivity: (10.0, 10.0),
                rotation: 0.0,
                reset_delay: Duration::from_millis(100),
            }),
            radial_follow: if filtered {
                vec![RadialFollowSettings::default()]
            } else {
                Vec::new()
            },
            ..Profile::default()
        }
    }

    /// Replays the captured reports through the whole pipeline with a
    /// simulated output sink, checking that nothing allocates.
    fn replay(iterations: u32, relative: bool, filtered: bool) -> u64 {
        let profile = profile(relative, filtered);
        let mut pipeline = ReportPipeline::new(&profile).unwrap();
        let screen = Rect {
            left: 0,
            top: 0,
            right: 2560,
            bottom: 1440,
        };
        let mapper = DisplaySnapshot {
            virtual_screen: screen,
            monitors: vec![screen],
        }
        .mapper(&profile)
        .unwrap();
        let mut plugins = crate::plugins::NoFilters;
        let mut emitted = 0;
        crate::test_alloc::assert_no_allocations(|| {
            for index in 0..iterations {
                let bytes = black_box(&CAPTURE[index as usize % CAPTURE.len()]);
                let pen = protocol::parse(bytes).unwrap().unwrap();
                pipeline
                    .process(pen, Instant::now(), Some(mapper), &mut plugins, |packet| {
                        black_box(packet);
                        emitted += 1;
                        Ok(())
                    })
                    .unwrap();
            }
        });
        emitted
    }

    #[test]
    fn report_pipeline_allocates_nothing() {
        for relative in [false, true] {
            for filtered in [false, true] {
                assert!(replay(10_000, relative, filtered) > 0);
            }
        }
    }

    #[test]
    #[ignore = "manual release-mode timing; does not call HID or SendInput"]
    fn benchmark_relative_pipeline() {
        const REPORTS: u32 = 1_000_000;
        for relative in [false, true] {
            for filtered in [false, true] {
                let start = Instant::now();
                let emitted = replay(REPORTS, relative, filtered);
                let elapsed = start.elapsed();
                println!(
                    "{} replay: filtered={filtered}, reports={REPORTS}, packets={emitted}, {:.1} ns/report; simulated output, no HID or SendInput",
                    if relative { "relative" } else { "absolute" },
                    elapsed.as_nanos() as f64 / f64::from(REPORTS)
                );
            }
        }
    }
}
