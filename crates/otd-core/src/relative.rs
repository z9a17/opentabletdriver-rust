// SPDX-License-Identifier: LGPL-3.0-only
// Based on OpenTabletDriver's RelativeOutputMode and WindowsRelativePointer.
// See docs/RELATIVE_MODE.md for the pinned sources and intentional differences.

use std::time::{Duration, Instant};

use crate::spec::TabletSpec;

#[derive(Clone, Copy, Debug)]
pub struct RelativeSettings {
    /// Mouse counts per millimetre, before Windows pointer speed/acceleration.
    pub sensitivity: (f64, f64),
    pub rotation: f64,
    pub reset_delay: Duration,
}

impl RelativeSettings {
    /// Validates against the PTH-660.
    pub fn validate(self) -> Result<Self, String> {
        self.validate_for(TabletSpec::PTH_660)
    }

    pub fn validate_values(self) -> Result<Self, String> {
        if ![self.sensitivity.0, self.sensitivity.1, self.rotation]
            .into_iter()
            .all(f64::is_finite)
        {
            return Err("relative sensitivity and rotation must be finite".into());
        }
        Ok(self)
    }

    pub fn validate_for(self, spec: TabletSpec) -> Result<Self, String> {
        self.validate_values()?;
        let [a, b, c, d] = self.transform(spec);
        // A full-diagonal movement plus fractional carry must fit a LONG.
        let (width, height) = (f64::from(spec.max_x), f64::from(spec.max_y));
        let max_x = a.abs() * width + b.abs() * height;
        let max_y = c.abs() * width + d.abs() * height;
        if !max_x.is_finite()
            || !max_y.is_finite()
            || max_x > f64::from(i32::MAX - 1)
            || max_y > f64::from(i32::MAX - 1)
        {
            return Err("relative sensitivity is too large for Windows mouse output".into());
        }
        Ok(self)
    }

    fn transform(self, spec: TabletSpec) -> [f64; 4] {
        let (sin, cos) = (-(self.rotation % 360.0)).to_radians().sin_cos();
        let (mm_x, mm_y) = spec.mm_per_unit();
        let sx = self.sensitivity.0 * mm_x;
        let sy = self.sensitivity.1 * mm_y;
        [cos * sx, -sin * sx, sin * sy, cos * sy]
    }
}

/// Fixed-size relative state; no allocations or trigonometry per report.
pub struct RelativeMapper {
    transform: [f64; 4],
    reset_delay: Duration,
    last_sample: Option<Instant>,
    last_raw: Option<(f32, f32)>,
    last_position: Option<(f64, f64)>,
    remainder: (f64, f64),
    awaiting_movement: bool,
}

impl RelativeMapper {
    pub fn new(settings: RelativeSettings) -> Result<Self, String> {
        Self::new_for(settings, TabletSpec::PTH_660)
    }

    pub fn new_for(settings: RelativeSettings, spec: TabletSpec) -> Result<Self, String> {
        let settings = settings.validate_for(spec)?;
        Ok(Self {
            transform: settings.transform(spec),
            reset_delay: settings.reset_delay,
            last_sample: None,
            last_raw: None,
            last_position: None,
            remainder: (0.0, 0.0),
            awaiting_movement: false,
        })
    }

    pub fn reset(&mut self) {
        self.last_sample = None;
        self.last_raw = None;
        self.last_position = None;
        self.remainder = (0.0, 0.0);
        self.awaiting_movement = false;
    }

    /// Reset before passing a physical range-loss input through the filters.
    /// Preserve the existing Rust lift/reentry contract (BC-18): the next
    /// position establishes a fresh origin, even inside reset_delay. Upstream
    /// RelativeOutputMode.Read only sets outOfRange here and keeps its origin
    /// until timeout; retaining that origin would reintroduce quick-reentry
    /// jumps into the native path. Plugin-emitted loss is not a new input.
    pub fn note_range_loss(&mut self) {
        self.reset();
    }

    pub fn map_at(
        &mut self,
        raw: Option<(u32, u32)>,
        filtered: Option<(f32, f32)>,
        now: Instant,
    ) -> (i32, i32) {
        let Some(raw) = raw else {
            self.reset();
            return (0, 0);
        };
        if !self.begin_input((raw.0 as f32, raw.1 as f32), now) {
            return (0, 0);
        }
        let position = filtered.unwrap_or((raw.0 as f32, raw.1 as f32));
        let delta = self.transform_emission(position);
        self.quantize(delta).unwrap_or((0, 0))
    }

    /// Upstream RelativeOutputMode.Read gates each transport input before any
    /// filters. Multiple emissions from it must not repeat this timeout gate.
    pub fn begin_input(&mut self, raw: (f32, f32), now: Instant) -> bool {
        if self
            .last_sample
            .is_some_and(|last| now.saturating_duration_since(last) > self.reset_delay)
        {
            self.last_position = None;
            self.remainder = (0.0, 0.0);
            self.awaiting_movement = true;
        }
        self.last_sample = Some(now);
        // Some tablets repeat their old position on redetection. Wait for a
        // changed raw position before establishing the new origin. Filtering
        // must not turn this stale report into apparent movement.
        if self.awaiting_movement && self.last_raw == Some(raw) {
            return false;
        }
        self.awaiting_movement = false;
        self.last_raw = Some(raw);
        true
    }

    /// One transform per pre-stage emission, even if a post-stage filter later
    /// suppresses it. Fractional output carry belongs after post-stage filters.
    pub fn transform_emission(&mut self, position: (f32, f32)) -> (f64, f64) {
        let position = (f64::from(position.0), f64::from(position.1));
        if !position.0.is_finite() || !position.1.is_finite() {
            self.reset();
            return (0.0, 0.0);
        }
        let previous = self.last_position.replace(position);
        let Some(previous) = previous else {
            return (0.0, 0.0);
        };
        // Transform the difference, avoiding cancellation between large
        // transformed absolute positions at low sensitivities.
        let x = position.0 - previous.0;
        let y = position.1 - previous.1;
        let [a, b, c, d] = self.transform;
        (a * x + b * y, c * x + d * y)
    }

    pub fn quantize(&mut self, delta: (f64, f64)) -> std::io::Result<(i32, i32)> {
        let dx = delta.0 + self.remainder.0;
        let dy = delta.1 + self.remainder.1;
        if !dx.is_finite()
            || !dy.is_finite()
            || dx < f64::from(i32::MIN)
            || dx > f64::from(i32::MAX)
            || dy < f64::from(i32::MIN)
            || dy > f64::from(i32::MAX)
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "relative plugin output exceeds finite mouse range",
            ));
        }
        self.remainder = (dx % 1.0, dy % 1.0);
        Ok((dx as i32, dy as i32))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub fn settings() -> RelativeSettings {
        RelativeSettings {
            sensitivity: (10.0, 20.0),
            rotation: 0.0,
            reset_delay: Duration::from_millis(100),
        }
    }

    #[test]
    fn origin_then_independent_axis_sensitivity() {
        let mut mapper = RelativeMapper::new(settings()).unwrap();
        let now = Instant::now();
        assert_eq!(mapper.map_at(Some((10_000, 8_000)), None, now), (0, 0));
        // 200 raw units = 1 mm on the PTH-660.
        assert_eq!(mapper.map_at(Some((10_200, 8_200)), None, now), (10, 20));
        assert_eq!(mapper.map_at(Some((10_000, 8_000)), None, now), (-10, -20));
        assert_eq!(mapper.map_at(Some((10_000, 8_000)), None, now), (0, 0));
    }

    #[test]
    fn rotation_precedes_axis_sensitivity() {
        let mut mapper = RelativeMapper::new(RelativeSettings {
            rotation: 90.0,
            ..settings()
        })
        .unwrap();
        let now = Instant::now();
        mapper.map_at(Some((1_000, 1_000)), None, now);
        assert_eq!(mapper.map_at(Some((1_200, 1_200)), None, now), (10, -20));
    }

    #[test]
    fn fractional_movement_accumulates_in_both_directions() {
        let mut mapper = RelativeMapper::new(RelativeSettings {
            sensitivity: (100.0, 100.0),
            ..settings()
        })
        .unwrap();
        let now = Instant::now();
        mapper.map_at(Some((100, 100)), None, now);
        assert_eq!(mapper.map_at(Some((101, 99)), None, now), (0, 0));
        assert_eq!(mapper.map_at(Some((102, 98)), None, now), (1, -1));
        assert_eq!(mapper.map_at(Some((101, 99)), None, now), (0, 0));
        assert_eq!(mapper.map_at(Some((100, 100)), None, now), (-1, 1));
    }

    #[test]
    fn filtering_keeps_fractional_raw_coordinates() {
        let mut mapper = RelativeMapper::new(RelativeSettings {
            sensitivity: (400.0, 400.0),
            ..settings()
        })
        .unwrap();
        let now = Instant::now();
        mapper.map_at(Some((100, 100)), Some((100.0, 100.0)), now);
        assert_eq!(
            mapper.map_at(Some((101, 101)), Some((100.5, 100.5)), now),
            (1, 1)
        );
    }

    #[test]
    fn timeout_skips_repeated_stale_reports_then_rebases() {
        let mut mapper = RelativeMapper::new(settings()).unwrap();
        let now = Instant::now();
        mapper.map_at(Some((100, 100)), None, now);
        let later = now + Duration::from_millis(101);
        assert_eq!(mapper.map_at(Some((100, 100)), None, later), (0, 0));
        assert_eq!(
            mapper.map_at(Some((100, 100)), Some((99.5, 99.5)), later),
            (0, 0)
        );
        assert_eq!(mapper.map_at(Some((10_000, 10_000)), None, later), (0, 0));
        assert_eq!(mapper.map_at(Some((10_200, 10_200)), None, later), (10, 20));
    }

    #[test]
    fn exact_reset_delay_still_moves_and_longer_gap_resets() {
        let mut mapper = RelativeMapper::new(settings()).unwrap();
        let now = Instant::now();
        mapper.map_at(Some((100, 100)), None, now);
        assert_eq!(
            mapper.map_at(Some((300, 300)), None, now + Duration::from_millis(100)),
            (10, 20)
        );
        assert_eq!(
            mapper.map_at(Some((500, 500)), None, now + Duration::from_millis(201)),
            (0, 0)
        );
    }

    #[test]
    fn proximity_loss_clears_origin_and_fractional_carry() {
        let mut mapper = RelativeMapper::new(RelativeSettings {
            sensitivity: (100.0, 100.0),
            ..settings()
        })
        .unwrap();
        let now = Instant::now();
        mapper.map_at(Some((100, 100)), None, now);
        mapper.map_at(Some((101, 101)), None, now);
        assert_eq!(mapper.map_at(None, None, now), (0, 0));
        assert_eq!(mapper.map_at(Some((10_000, 10_000)), None, now), (0, 0));
        assert_eq!(mapper.map_at(Some((10_001, 10_001)), None, now), (0, 0));
        assert_eq!(mapper.map_at(Some((10_002, 10_002)), None, now), (1, 1));
    }

    #[test]
    fn rejects_nonfinite_or_overflowing_settings() {
        for value in [f64::NAN, f64::INFINITY, f64::MAX] {
            assert!(
                RelativeMapper::new(RelativeSettings {
                    sensitivity: (value, 10.0),
                    ..settings()
                })
                .is_err()
            );
        }
        assert!(
            RelativeMapper::new(RelativeSettings {
                rotation: f64::NAN,
                ..settings()
            })
            .is_err()
        );
    }

    #[test]
    fn zero_and_negative_sensitivity_disable_and_invert_axes() {
        let mut mapper = RelativeMapper::new(RelativeSettings {
            sensitivity: (0.0, -10.0),
            ..settings()
        })
        .unwrap();
        let now = Instant::now();
        mapper.map_at(Some((100, 100)), None, now);
        assert_eq!(mapper.map_at(Some((300, 300)), None, now), (0, -10));
    }
}
