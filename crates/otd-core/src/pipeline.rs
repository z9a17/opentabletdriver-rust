//! Synchronous report graph: built-ins, pre filters, transform, post filters,
//! bindings, then output. Every emission completes before the emitting filter
//! resumes. Native-only processing uses inline report state and no queue.

use std::io;
use std::time::Instant;

use crate::actions::Action;
use crate::config::{ContactPolicy, OutputKind, Profile};
use crate::decoders::DecodedInput;
use crate::mapping::Mapper;
use crate::output::buttons::{ActionSink, ButtonOutput};
use crate::output::pen::{PenOutput, PenSample, PenSink};
use crate::output::{MouseOutput, MousePacket};
use crate::plugins::{DispatchInput, Filters, PipelineRuntime};
use crate::protocol::PenReport;
use crate::radial_follow::RadialFollowSmoothingTabletSpace;
use crate::relative::RelativeMapper;
use crate::reports::{Buttons, ReportKind, ReportValues, ToolType};

#[derive(Clone, Copy, Debug, Default)]
pub struct DispatchStats {
    pub reports: u64,
    /// Acknowledged sink packets; a shared sink may coalesce before OS output.
    pub packets: u64,
}

pub struct ReportPipeline {
    contact: ContactPolicy,
    filters: Vec<RadialFollowSmoothingTabletSpace>,
    relative: Option<RelativeMapper>,
    output: MouseOutput,
    /// Pen side buttons, express keys and wheels. Until the platform
    /// supplies an action sink they only drive a pen device's barrel buttons.
    buttons: ButtonOutput,
    profile_buttons: Vec<crate::output::buttons::ButtonAction>,
    aux_buttons: Vec<crate::output::buttons::ButtonAction>,
    mouse_buttons: Vec<crate::output::buttons::ButtonAction>,
    mouse_scroll_up: crate::output::buttons::ButtonAction,
    mouse_scroll_down: crate::output::buttons::ButtonAction,
    wheels: Vec<crate::output::buttons::WheelBinding>,
    controls: crate::spec::Controls,
    /// The profile asks for pen output; the platform supplies the device.
    pen_requested: bool,
    pen: Option<PenOutput>,
    max_pressure: u32,
    is_eraser: bool,
    desired_contact: bool,
    faulted: bool,
    physical_present: bool,
}

impl ReportPipeline {
    pub fn new(profile: &Profile) -> Result<Self, String> {
        profile.validate_filter_execution()?;
        profile.contact.validate_percentages()?;
        profile.validate_actions()?;
        let pen_requested = profile.output == OutputKind::Pen;
        if pen_requested && profile.relative.is_some() {
            return Err("pen output is absolute; choose mouse output for relative mode".into());
        }
        let mut contact = profile.contact;
        // Native raw-threshold edits remain authoritative. Retain an imported
        // percentage only while it represents that raw threshold on this tablet.
        for (percent, raw) in [
            (&mut contact.tip_threshold_percent, contact.tip_threshold_raw),
            (&mut contact.eraser_threshold_percent, contact.eraser_threshold_raw),
        ] {
            if let (Some(value), Some(raw)) = (*percent, raw) {
                if crate::config::activation_raw_for(f64::from(value), profile.tablet.max_pressure).ok() != Some(raw) {
                    *percent = None;
                }
            }
        }
        Ok(Self {
            contact,
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
            buttons: ButtonOutput::new(&profile.pen_buttons, pen_requested, Box::new(NoActions)).0,
            profile_buttons: profile.pen_buttons.clone(),
            aux_buttons: profile.aux_buttons.clone(),
            mouse_buttons: profile.mouse_buttons.clone(),
            mouse_scroll_up: profile.mouse_scroll_up.clone(),
            mouse_scroll_down: profile.mouse_scroll_down.clone(),
            wheels: profile.wheels.clone(),
            controls: profile.tablet.controls,
            pen_requested,
            pen: None,
            max_pressure: u32::from(profile.tablet.max_pressure),
            is_eraser: false,
            desired_contact: false,
            faulted: false,
            physical_present: false,
        })
    }

    pub fn is_relative(&self) -> bool {
        self.relative.is_some()
    }

    pub fn needs_cleanup(&self) -> bool {
        self.faulted
    }

    pub fn share_output(&mut self) {
        self.output.share_position();
    }

    /// Whether the profile asks for pen output.
    pub fn wants_pen(&self) -> bool {
        self.pen_requested
    }

    /// Sends output to a pen device instead of the mouse. Pen packets go to
    /// this sink; the mouse sink then receives nothing.
    pub fn set_pen_sink(&mut self, sink: Box<dyn PenSink>) {
        self.pen = Some(PenOutput::new(sink, self.max_pressure));
    }

    /// Sends keys and mouse buttons for the pen side buttons, express keys
    /// and wheels through `sink`. Returns a message for each binding the
    /// platform cannot carry out; those buttons do nothing.
    pub fn set_action_sink(&mut self, sink: Box<dyn ActionSink>) -> Vec<String> {
        let (mut buttons, mut rejected) =
            ButtonOutput::new(&self.profile_buttons, self.pen_requested, sink);
        rejected.extend(buttons.set_auxiliary(
            &self.aux_buttons,
            &self.wheels,
            self.controls.wheels(),
        ));
        rejected.extend(buttons.set_mouse(&self.mouse_buttons));
        rejected.extend(buttons.set_mouse_scroll(&self.mouse_scroll_up, &self.mouse_scroll_down));
        self.buttons = buttons;
        rejected
    }

    /// The endpoint that reports express keys and wheels was lost: release
    /// what it held. Pen output is unaffected.
    pub fn release_auxiliary(&mut self) -> io::Result<bool> {
        let result = self.buttons.release_auxiliary();
        if result.is_err() {
            self.faulted = true;
        }
        result
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

    /// Dispatch runtime decoder input without reducing generic reports to the
    /// compact IntuosV2 pen layout. Missing interfaces remain missing.
    #[inline]
    pub fn process_input(
        &mut self,
        input: DecodedInput<'_>,
        now: Instant,
        mapper: Option<Mapper>,
        plugins: &mut impl Filters,
        send: impl FnMut(MousePacket) -> io::Result<()>,
    ) -> io::Result<DispatchStats> {
        match input {
            DecodedInput::Pen(decoded) => self.process_pen(
                decoded.pen,
                decoded.raw,
                decoded.buttons.or(Some(Buttons::default())),
                now,
                mapper,
                plugins,
                send,
            ),
            DecodedInput::Report { kind, report, pen } => self.process_report(
                DispatchInput {
                    kind,
                    values: report.values,
                    raw: report.raw,
                    pen,
                    now,
                },
                mapper,
                plugins,
                send,
            ),
        }
    }

    /// `process_with_raw` with an IntuosV2 decoder's pen buttons. Generic
    /// runtime reports use `process_input` to retain their original interfaces.
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
        // Upstream IntuosV2Report always implements ITabletReport, including
        // when proximity flags clear. Filters must still consume its position.
        let kind = ReportKind::Data;
        let values = ReportValues {
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
        // Transport state must advance even while cleanup or mapping pauses
        // dispatch. A retained timer report cannot resurrect a lost pen.
        let physical_loss = input.kind == ReportKind::OutOfRange
            || input.pen.is_some_and(|pen| !pen.in_range && !pen.sense);
        if physical_loss {
            self.physical_present = false;
        } else if input.values.position.is_some() {
            self.physical_present = true;
        }
        let mapping_paused = self.relative.is_none() && mapper.is_none();
        if self.faulted || mapping_paused {
            // Pausing the graph must also revoke held output, including when
            // loss arrives while no absolute mapping exists. Failed cleanup
            // remains pending before any subsequent graph input may run.
            initial_stats.packets += u64::from(self.release_all(&mut send)?);
        }
        if let Some(relative) = &mut self.relative {
            if physical_loss {
                relative.note_range_loss();
            } else if let Some([x, y]) = input.values.position
                && !relative.begin_input((x, y), input.now)
            {
                return Ok(initial_stats);
            }
        } else if mapping_paused {
            return Ok(initial_stats);
        }
        // Explicit loss from a general report source also revokes ownership.
        // Loss emitted by a plugin reaches Runtime::output instead of this entry.
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
            timer: false,
            physical_loss,
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
            // Physical loss revokes native ownership even if a plugin suppresses
            // the report. IntuosV2 loss retains its positional interfaces.
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

    pub fn next_binding_tick(&self, now: Instant) -> Option<std::time::Duration> {
        self.buttons.next_tick(now)
    }

    pub fn process_binding_tick(&mut self, now: Instant) -> io::Result<()> {
        let result = self.buttons.tick(now);
        if result.is_err() { self.faulted = true; }
        result
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
        let mut stats = DispatchStats::default();
        if self.faulted {
            stats.packets += u64::from(self.release_all(&mut send)?);
        }
        if self.relative.is_none() && mapper.is_none() {
            return Ok(stats);
        }
        let mut runtime = Runtime {
            pipeline: self,
            mapper,
            now,
            send: &mut send,
            stats,
            exact_position: None,
            shown_position: None,
            preserve_precision: false,
            unfiltered_raw: None,
            timer: true,
            physical_loss: false,
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
        let pen = self.pen.as_mut().map_or(Ok(false), PenOutput::release);
        let mouse = self.output.release_all(send);
        let buttons = self.buttons.release_all();
        let result = match (pen, mouse, buttons) {
            (Ok(pen), Ok(mouse), Ok(buttons)) => Ok(pen || mouse || buttons),
            (Err(error), _, _) | (_, Err(error), _) | (_, _, Err(error)) => Err(error),
        };
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
    timer: bool,
    physical_loss: bool,
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
                // Lost-pen positions still advance filters, but cannot become
                // the origin for the next live relative report. Retained timer
                // emissions after loss must not establish that origin either.
                if self.physical_loss || (self.timer && !self.pipeline.physical_present) {
                    (0.0, 0.0)
                } else {
                    relative.transform_emission((x, y))
                }
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
        // A plugin may retain a contact report after transport range loss.
        // Timers still advance, but cannot revive that output without new input.
        if self.timer && !self.pipeline.physical_present && kind != ReportKind::OutOfRange {
            return Ok(());
        }
        self.pipeline.buttons.set_time(self.now);
        self.stats.reports += 1;
        // Keep transport cleanup independent of the interfaces filters see.
        // A positional loss packet must not reacquire held actions or pen contact.
        if self.physical_loss {
            self.stats.packets += u64::from(self.pipeline.release_all(&mut *self.send)?);
            return Ok(());
        }
        if let Some(eraser) = values
            .eraser
            .or_else(|| values.tool.map(|tool| tool.tool == ToolType::Eraser))
        {
            self.pipeline.is_eraser = eraser;
        }
        if kind == ReportKind::Data {
            // Express keys and wheels follow their own readings; a pen
            // leaving range does not release them (upstream's
            // `HandleOutOfRangeReport` releases pen buttons only).
            self.pipeline.buttons.apply_auxiliary(values)?;
        }
        if kind == ReportKind::OutOfRange {
            self.pipeline.desired_contact = false;
        } else if values.mouse_buttons.is_some() && values.pressure.is_none() {
            // Switching from a pen to a puck must release the pen's tip.
            // Preserve the absent pressure interface seen by plugins.
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
        if kind == ReportKind::Data && position.is_none() && values.pressure.is_none() {
            // Tool/aux/wheel/touch packets remain visible to filters, but must
            // not replay an old pointer position or emit unrelated contact.
            return Ok(());
        }
        let contact = self.pipeline.desired_contact;
        // Side buttons follow the report after the pointer has moved, so a
        // click lands where the pen is. A pen out of range holds none.
        let drag_pressure = if self.pipeline.contact.drag_only {
            self.pipeline.contact.drag_pressure(values.pressure, self.pipeline.max_pressure, self.pipeline.is_eraser)
        } else { values.pressure };
        let wanted = self
            .pipeline
            .buttons
            .wanted_with_pressure(values.pen_buttons, kind != ReportKind::OutOfRange, drag_pressure, self.pipeline.contact.drag_only);
        let barrel = self.pipeline.buttons.barrel(wanted);
        if let Some(pen) = &mut self.pipeline.pen {
            let emitted = if kind == ReportKind::OutOfRange {
                pen.release()?
            } else if let Some((x, y)) = position {
                match self.mapper.and_then(|mapper| mapper.clamp_pixels(x, y)) {
                    Some((x, y)) => pen.sample(PenSample {
                        x,
                        y,
                        pressure: if self.pipeline.contact.disable_pressure { None } else { values.pressure },
                        tilt: if self.pipeline.contact.disable_tilt { None } else { values.tilt },
                        eraser: self.pipeline.is_eraser,
                        contact,
                        barrel,
                    })?,
                    None => false,
                }
            } else {
                pen.contact(contact, if self.pipeline.contact.disable_pressure { None } else { values.pressure }, barrel)?
            };
            self.stats.packets += u64::from(emitted);
            return self.pipeline.buttons.apply(wanted);
        }
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
        self.pipeline.buttons.apply(wanted)
    }
}

/// The action sink of a pipeline the platform has not given one: pen buttons
/// still reach a pen device, and nothing else is sent.
struct NoActions;

impl ActionSink for NoActions {
    fn supports(&self, _: Action) -> bool {
        false
    }

    fn hold(&mut self, _: u32, _: Action, _: bool) -> io::Result<()> {
        Ok(())
    }

    fn flush(&mut self) -> io::Result<usize> {
        Ok(0)
    }

    fn release_all(&mut self) -> io::Result<usize> {
        Ok(0)
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

    /// Expected state comes from the unchanged AbstractQbit 0.3.0 DLL on
    /// OTD 0.6.7: one initial report, eight positions with cleared proximity,
    /// then the first returning report, at 2ms intervals.
    #[test]
    fn radial_follow_consumes_positions_while_proximity_is_clear() {
        struct Observe {
            seen: Option<ReportValues>,
        }
        impl Filters for Observe {
            fn dispatch(
                &mut self,
                input: DispatchInput<'_>,
                runtime: &mut dyn PipelineRuntime,
            ) -> io::Result<()> {
                assert_eq!(input.kind, ReportKind::Data);
                let mut values = input.values;
                runtime.builtins(&mut values)?;
                self.seen = Some(values);
                if runtime.transform(input.kind, &mut values)? {
                    runtime.output(input.kind, &values, input.raw)?;
                }
                Ok(())
            }
            fn has_pre(&self) -> bool {
                false
            }
            fn has_pixels(&self) -> bool {
                false
            }
            fn process_pre(&mut self, p: (f32, f32), _: PenReport, _: Instant) -> (f32, f32) {
                p
            }
            fn process_pixels(&mut self, p: (f32, f32), _: PenReport, _: Instant) -> (f32, f32) {
                p
            }
            fn reset(&mut self) {}
            fn take_failure(&mut self) -> Option<&str> {
                None
            }
        }
        let profile = Profile {
            radial_follow: vec![RadialFollowSettings {
                outer_radius: 0.7039,
                inner_radius: 0.302,
                smoothing_coefficient: 0.302,
                soft_knee_scale: 0.603,
                smoothing_leak_coefficient: 0.201,
            }],
            ..Profile::default()
        };
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
        let mut filters = Observe { seen: None };
        let start = Instant::now() + Duration::from_millis(100);
        for index in 0..=9 {
            let mut pen = protocol::parse(&CAPTURE[0]).unwrap().unwrap();
            pen.x = if index == 0 { 20000 } else { 20200 };
            pen.y = 5000;
            pen.in_range = index == 0 || index == 9;
            pen.sense = pen.in_range;
            pipeline
                .process_pen(
                    pen,
                    &[],
                    None,
                    start + Duration::from_millis(index * 2),
                    Some(mapper),
                    &mut filters,
                    |_| {
                        assert!(
                            pen.in_range,
                            "loss advances filters without moving the pointer"
                        );
                        Ok(())
                    },
                )
                .unwrap();
            let seen = filters.seen.unwrap();
            assert!(seen.position.is_some());
            assert_eq!(seen.near_proximity, Some(pen.in_range));
            assert_eq!(seen.sense, Some(pen.sense));
            assert_eq!(pipeline.physical_present, pen.in_range);
        }
        assert_eq!(filters.seen.unwrap().position, Some([20139.598, 5000.0]));
        assert!(pipeline.physical_present);
    }

    #[test]
    fn retained_timer_positions_cannot_rebase_relative_motion_after_loss() {
        struct Retained;
        impl Filters for Retained {
            fn has_pre(&self) -> bool { false }
            fn has_pixels(&self) -> bool { false }
            fn process_pre(&mut self, p: (f32, f32), _: PenReport, _: Instant) -> (f32, f32) { p }
            fn process_pixels(&mut self, p: (f32, f32), _: PenReport, _: Instant) -> (f32, f32) { p }
            fn reset(&mut self) {}
            fn take_failure(&mut self) -> Option<&str> { None }
            fn tick(&mut self, _: Instant, runtime: &mut dyn PipelineRuntime) -> io::Result<()> {
                let mut values = ReportValues { position: Some([20200.0, 5000.0]), ..ReportValues::default() };
                if runtime.transform(ReportKind::Data, &mut values)? {
                    runtime.output(ReportKind::Data, &values, &[])?;
                }
                Ok(())
            }
        }
        let mut pipeline = ReportPipeline::new(&profile(true, false)).unwrap();
        let mut filters = Retained;
        let start = Instant::now();
        let mut pen = protocol::parse(&CAPTURE[0]).unwrap().unwrap();
        pen.x = 20000;
        pen.y = 5000;
        let mut packets = Vec::new();
        pipeline.process(pen, start, None, &mut filters, |packet| { packets.push(packet); Ok(()) }).unwrap();
        pen.in_range = false;
        pen.sense = false;
        pen.x = 20200;
        pipeline.process(pen, start + Duration::from_millis(2), None, &mut filters, |packet| { packets.push(packet); Ok(()) }).unwrap();
        for step in 3..6 {
            pipeline.process_tick(start + Duration::from_millis(step), None, &mut filters, |packet| { packets.push(packet); Ok(()) }).unwrap();
        }
        pen.in_range = true;
        pen.sense = true;
        pen.x = 23000;
        pipeline.process(pen, start + Duration::from_millis(6), None, &mut filters, |packet| { packets.push(packet); Ok(()) }).unwrap();
        assert!(packets.is_empty(), "loss, timers and reentry must not move the pointer: {packets:?}");
        pen.x = 23200;
        pipeline.process(pen, start + Duration::from_millis(8), None, &mut filters, |packet| { packets.push(packet); Ok(()) }).unwrap();
        assert_eq!(packets.len(), 1);
        assert_eq!((packets[0].dx, packets[0].dy, packets[0].flags), (10, 0, crate::output::flags::MOVE));
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
    fn pen_output_allocates_nothing_and_sends_no_mouse_packets() {
        for filtered in [false, true] {
            let profile = Profile {
                output: crate::config::OutputKind::Pen,
                ..profile(false, filtered)
            };
            let mut pipeline = ReportPipeline::new(&profile).unwrap();
            let screen = Rect {
                left: -1920,
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
            let sent = std::rc::Rc::new(std::cell::Cell::new(0u32));
            let counter = std::rc::Rc::clone(&sent);
            pipeline.set_pen_sink(Box::new(move |packet: crate::output::pen::PenPacket| {
                assert!((-1920.0..2560.0).contains(&packet.x), "{packet:?}");
                counter.set(counter.get() + 1);
                Ok(())
            }));
            let mut plugins = crate::plugins::NoFilters;
            crate::test_alloc::assert_no_allocations(|| {
                for index in 0..10_000usize {
                    let bytes = black_box(&CAPTURE[index % CAPTURE.len()]);
                    let pen = protocol::parse(bytes).unwrap().unwrap();
                    pipeline
                        .process(pen, Instant::now(), Some(mapper), &mut plugins, |packet| {
                            panic!("mouse packet in pen mode: {packet:?}")
                        })
                        .unwrap();
                }
                pipeline
                    .release_all(|packet| panic!("mouse packet in pen mode: {packet:?}"))
                    .unwrap();
            });
            assert!(sent.get() > 0);
        }
        let relative = Profile {
            output: crate::config::OutputKind::Pen,
            ..profile(true, false)
        };
        assert!(ReportPipeline::new(&relative).is_err());
    }

    /// A captured hover report with the pen side buttons in bits 1 and 2 of
    /// the flags byte (`0x02` barrel button 1, `0x04` button 2), or with
    /// neither in-range bit, which is the pen leaving.
    fn with_buttons(bits: u8, in_range: bool) -> [u8; 17] {
        let mut report = CAPTURE[0];
        report[1] = if in_range { 0x60 } else { 0x00 } | bits;
        report
    }

    #[derive(Clone, Debug, PartialEq)]
    enum Sent {
        Mouse(MousePacket),
        Action(crate::actions::ActionTransition),
    }

    fn feed(
        pipeline: &mut ReportPipeline,
        mapper: Mapper,
        report: &[u8; 17],
        log: &std::rc::Rc<std::cell::RefCell<Vec<Sent>>>,
    ) {
        let pen = protocol::parse(report).unwrap().unwrap();
        let mouse = std::rc::Rc::clone(log);
        pipeline
            .process_with_raw(
                pen,
                report,
                Instant::now(),
                Some(mapper),
                &mut crate::plugins::NoFilters,
                move |packet| {
                    mouse.borrow_mut().push(Sent::Mouse(packet));
                    Ok(())
                },
            )
            .unwrap();
    }

    fn buttons_pipeline(
        profile: &Profile,
    ) -> (
        ReportPipeline,
        Mapper,
        std::rc::Rc<std::cell::RefCell<Vec<Sent>>>,
    ) {
        let mut pipeline = ReportPipeline::new(profile).unwrap();
        let log: std::rc::Rc<std::cell::RefCell<Vec<Sent>>> = Default::default();
        let actions = std::rc::Rc::clone(&log);
        let rejected =
            pipeline.set_action_sink(Box::new(crate::output::buttons::LocalActions::new(
                move |transition| {
                    actions.borrow_mut().push(Sent::Action(transition));
                    Ok(())
                },
                |_| true,
            )));
        assert!(rejected.is_empty(), "{rejected:?}");
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
        .mapper(profile)
        .unwrap();
        (pipeline, mapper, log)
    }

    fn actions(
        log: &std::rc::Rc<std::cell::RefCell<Vec<Sent>>>,
    ) -> Vec<(crate::actions::Action, bool)> {
        log.borrow()
            .iter()
            .filter_map(|sent| match sent {
                Sent::Action(transition) => Some((transition.action, transition.pressed)),
                Sent::Mouse(_) => None,
            })
            .collect()
    }

    #[test]
    fn side_buttons_click_where_the_pen_is_and_release_when_it_leaves() {
        use crate::actions::{Action, MouseButton};
        let (mut pipeline, mapper, log) = buttons_pipeline(&profile(false, false));
        feed(&mut pipeline, mapper, &with_buttons(0, true), &log);
        assert!(actions(&log).is_empty());
        log.borrow_mut().clear();

        feed(&mut pipeline, mapper, &with_buttons(0x02, true), &log);
        feed(&mut pipeline, mapper, &with_buttons(0x02, true), &log);
        assert_eq!(actions(&log), [(Action::Mouse(MouseButton::Right), true)]);
        feed(&mut pipeline, mapper, &with_buttons(0x06, true), &log);
        assert_eq!(
            actions(&log).last(),
            Some(&(Action::Mouse(MouseButton::Middle), true))
        );
        feed(&mut pipeline, mapper, &with_buttons(0x04, true), &log);
        assert_eq!(
            actions(&log).last(),
            Some(&(Action::Mouse(MouseButton::Right), false))
        );
        // The pen leaves with a button still down.
        feed(&mut pipeline, mapper, &with_buttons(0x04, false), &log);
        assert_eq!(
            actions(&log).last(),
            Some(&(Action::Mouse(MouseButton::Middle), false))
        );
        let before = log.borrow().len();
        feed(&mut pipeline, mapper, &with_buttons(0x04, false), &log);
        assert_eq!(log.borrow().len(), before, "nothing left to release");
    }

    #[test]
    fn the_pointer_moves_before_its_side_button_presses() {
        let (mut pipeline, mapper, log) = buttons_pipeline(&profile(false, false));
        feed(&mut pipeline, mapper, &with_buttons(0, true), &log);
        log.borrow_mut().clear();
        let mut moved = with_buttons(0x02, true);
        moved[2] ^= 0x40; // a different X position
        feed(&mut pipeline, mapper, &moved, &log);
        let log = log.borrow();
        assert!(matches!(log[0], Sent::Mouse(_)), "{log:?}");
        assert!(matches!(log[1], Sent::Action(_)), "{log:?}");
    }

    #[test]
    fn a_configured_key_chord_and_disabled_buttons_follow_the_profile() {
        use crate::actions::{Action, KeyboardUsage};
        use crate::output::buttons::ButtonAction;
        let profile = Profile {
            pen_buttons: vec![
                ButtonAction::Keys(crate::keys::parse_chord("Control+Z").unwrap()),
                ButtonAction::None,
            ],
            ..profile(false, false)
        };
        let (mut pipeline, mapper, log) = buttons_pipeline(&profile);
        feed(&mut pipeline, mapper, &with_buttons(0x06, true), &log);
        let key = |usage| Action::Key(KeyboardUsage::new(usage).unwrap());
        assert_eq!(actions(&log), [(key(0xe0), true), (key(0x1d), true)]);
        pipeline.release_all(|_| Ok(())).unwrap();
        assert_eq!(
            actions(&log)[2..],
            [(key(0x1d), false), (key(0xe0), false)],
            "cleanup releases the chord"
        );
    }

    #[test]
    fn relative_mode_has_side_buttons_too() {
        use crate::actions::{Action, MouseButton};
        let (mut pipeline, mapper, log) = buttons_pipeline(&profile(true, false));
        feed(&mut pipeline, mapper, &with_buttons(0x02, true), &log);
        assert_eq!(actions(&log), [(Action::Mouse(MouseButton::Right), true)]);
    }

    #[test]
    fn pen_output_reports_barrel_buttons_instead_of_clicks() {
        use crate::output::pen::PenPacket;
        let profile = Profile {
            output: crate::config::OutputKind::Pen,
            ..profile(false, false)
        };
        let (mut pipeline, mapper, log) = buttons_pipeline(&profile);
        let packets: std::rc::Rc<std::cell::RefCell<Vec<PenPacket>>> = Default::default();
        let sink = std::rc::Rc::clone(&packets);
        pipeline.set_pen_sink(Box::new(move |packet: PenPacket| {
            sink.borrow_mut().push(packet);
            Ok(())
        }));
        for report in [
            with_buttons(0, true),
            with_buttons(0x02, true),
            with_buttons(0x06, true),
            with_buttons(0x04, true),
            with_buttons(0, false),
        ] {
            feed(&mut pipeline, mapper, &report, &log);
        }
        let barrel: Vec<u8> = packets
            .borrow()
            .iter()
            .map(|packet| packet.barrel)
            .collect();
        assert_eq!(
            barrel,
            [0, 0b001, 0b011, 0b010, 0],
            "leaving range releases the barrel"
        );
        assert!(actions(&log).is_empty(), "no mouse buttons in pen mode");
    }

    /// An IntuosV2 auxiliary report (upstream `IntuosV2AuxReport`): express
    /// keys in byte 1, the ring button in byte 3 bit 0 and the ring in byte 4,
    /// whose bit 7 means touched.
    fn aux_report(keys: u8, ring: Option<u8>) -> [u8; 10] {
        let mut report = [0u8; 10];
        report[0] = 0x11;
        report[1] = keys;
        report[4] = ring.map_or(0, |position| 0x80 | position);
        report
    }

    #[test]
    fn express_keys_and_the_ring_reach_their_bindings_through_the_decoder() {
        use crate::actions::{Action, KeyboardUsage, MouseButton};
        use crate::decoders::{PenDecoder, TabletDecoder};
        use crate::output::buttons::WheelBinding;
        for output in [OutputKind::Mouse, OutputKind::Pen] {
            let profile = Profile {
                output,
                aux_buttons: vec!["none".parse().unwrap(), "mouse:forward".parse().unwrap()],
                wheels: vec![WheelBinding {
                    clockwise: "keys:PageDown".parse().unwrap(),
                    ..WheelBinding::default()
                }],
                ..profile(false, false)
            };
            let (mut pipeline, mapper, log) = buttons_pipeline(&profile);
            if output == OutputKind::Pen {
                pipeline.set_pen_sink(Box::new(|_: crate::output::pen::PenPacket| Ok(())));
            }
            let mut decoder = TabletDecoder::for_parser(
                "OpenTabletDriver.Configurations.Parsers.Wacom.IntuosV2.IntuosV2ReportParser",
                profile.tablet,
            )
            .unwrap();
            for report in [
                aux_report(0b10, None),
                aux_report(0b10, Some(3)),
                aux_report(0b00, Some(4)),
                aux_report(0b00, Some(5)),
                aux_report(0b00, None),
            ] {
                let input = decoder.decode_input(&report).unwrap().unwrap();
                pipeline
                    .process_input(
                        input,
                        Instant::now(),
                        Some(mapper),
                        &mut crate::plugins::NoFilters,
                        |packet| panic!("an aux report moved the pointer: {packet:?}"),
                    )
                    .unwrap();
            }
            let page_down = Action::Key(KeyboardUsage::new(0x4e).unwrap());
            assert_eq!(
                actions(&log),
                [
                    (Action::Mouse(MouseButton::Forward), true),
                    (Action::Mouse(MouseButton::Forward), false),
                    (page_down, true),
                    (page_down, false),
                    (page_down, true),
                    (page_down, false),
                ],
                "{output:?}"
            );
        }
    }

    #[test]
    fn side_buttons_allocate_nothing() {
        let profile = profile(false, false);
        let mut pipeline = ReportPipeline::new(&profile).unwrap();
        let injected = std::rc::Rc::new(std::cell::Cell::new(0u32));
        let counter = std::rc::Rc::clone(&injected);
        pipeline.set_action_sink(Box::new(crate::output::buttons::LocalActions::new(
            move |_| {
                counter.set(counter.get() + 1);
                Ok(())
            },
            |_| true,
        )));
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
        let reports = [
            with_buttons(0, true),
            with_buttons(0x02, true),
            with_buttons(0x06, true),
            with_buttons(0x04, true),
            with_buttons(0, false),
        ];
        let mut plugins = crate::plugins::NoFilters;
        crate::test_alloc::assert_no_allocations(|| {
            for index in 0..10_000usize {
                let bytes = black_box(&reports[index % reports.len()]);
                let pen = protocol::parse(bytes).unwrap().unwrap();
                pipeline
                    .process_with_raw(
                        pen,
                        bytes,
                        Instant::now(),
                        Some(mapper),
                        &mut plugins,
                        |_| Ok(()),
                    )
                    .unwrap();
            }
        });
        // Per 5-report cycle: right down, middle down, right up, middle up.
        assert_eq!(injected.get(), 8_000);
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
