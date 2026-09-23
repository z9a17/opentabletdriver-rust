//! The stages after a report is decoded: contact state, the built-in Radial
//! Follow filters, DLL filters, absolute or relative mapping, and output. The
//! device session and the golden-trace tests both run reports through this
//! type, so the traces cover exactly what the driver does.
//!
//! Order within one report: contact state from the raw report, built-in
//! filters, PreTransform DLL filters, mapping, Pixels DLL filters, then at
//! most one output packet carrying both the move and any button change. See
//! `docs/parity/BEHAVIOR_CONTRACTS.md` for how this compares with upstream.

use std::io;
use std::time::Instant;

use crate::config::{ContactPolicy, Profile};
use crate::mapping::Mapper;
use crate::output::{MouseOutput, MousePacket};
use crate::plugins::Filters;
use crate::protocol::PenReport;
use crate::radial_follow::RadialFollowSmoothingTabletSpace;
use crate::relative::RelativeMapper;
use crate::state;

pub struct ReportPipeline {
    contact: ContactPolicy,
    filters: Vec<RadialFollowSmoothingTabletSpace>,
    relative: Option<RelativeMapper>,
    output: MouseOutput,
}

impl ReportPipeline {
    pub fn new(profile: &Profile) -> Result<Self, String> {
        Ok(Self {
            contact: profile.contact,
            filters: profile
                .radial_follow
                .iter()
                .copied()
                .map(RadialFollowSmoothingTabletSpace::new)
                .collect(),
            relative: profile.relative.map(RelativeMapper::new).transpose()?,
            output: MouseOutput::new(),
        })
    }

    pub fn is_relative(&self) -> bool {
        self.relative.is_some()
    }

    /// Runs one decoded report. `now` is its processing time; `mapper` is the
    /// current absolute mapping, unused in relative mode. Returns whether a
    /// packet was sent.
    pub fn process(
        &mut self,
        pen: PenReport,
        now: Instant,
        mapper: Option<Mapper>,
        plugins: &mut impl Filters,
        send: impl FnOnce(MousePacket) -> io::Result<()>,
    ) -> io::Result<bool> {
        let frame = state::frame(pen, self.contact);
        let mut filtered = frame.position.and_then(|(x, y)| {
            let (first, rest) = self.filters.split_first_mut()?;
            let mut position = first.filter_raw_at(x as f32, y as f32, now);
            for filter in rest {
                position = filter.filter_raw_at(position.0, position.1, now);
            }
            Some(position)
        });
        if let Some((x, y)) = frame.position {
            if plugins.has_pre() {
                filtered =
                    Some(plugins.process_pre(filtered.unwrap_or((x as f32, y as f32)), pen, now));
            }
        } else {
            plugins.reset();
        }
        if let Some(relative) = &mut self.relative {
            let delta = relative.map_at(frame.position, filtered, now);
            // Consume failed movement rather than accumulating a cursor jump.
            // MouseOutput retries button transitions.
            self.output.emit_relative(delta, frame.contact, send)
        } else if let Some(mapper) = mapper {
            if plugins.has_pixels() {
                let mapped = frame.position.and_then(|(x, y)| {
                    let position = filtered.unwrap_or((x as f32, y as f32));
                    mapper.map_filtered_pixels(position.0, position.1)
                });
                if frame.position.is_some() && mapped.is_none() {
                    // Area limiting stops the report before PostTransform.
                    return Ok(false);
                }
                let position = mapped.and_then(|(x, y)| {
                    let (x, y) = plugins.process_pixels((x as f32, y as f32), pen, now);
                    mapper.normalize_pixels(f64::from(x), f64::from(y))
                });
                self.output.emit_mapped(position, frame.contact, send)
            } else {
                self.output.emit_filtered(frame, mapper, filtered, send)
            }
        } else {
            Ok(false)
        }
    }

    /// Releases a held button, as when a session ends or the mapping stops.
    pub fn release_all(
        &mut self,
        send: impl FnOnce(MousePacket) -> io::Result<()>,
    ) -> io::Result<bool> {
        self.output.release_all(send)
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
