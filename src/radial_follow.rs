// SPDX-License-Identifier: GPL-3.0-only
// Rust port of AbstractQbit's RadialFollow 0.3.0 tablet-space filter.
// Original source: https://github.com/AbstractQbit/AbstractOTDPlugins/tree/0.3.0/RadialFollow

use std::time::{Duration, Instant};

use crate::protocol::{MAX_X, MAX_Y};

pub const FILTER_PATH: &str = "RadialFollow.RadialFollowSmoothingTabletSpace";
pub const FILTER_NAME: &str = "AbstractQbit's Radial Follow Smoothing (Tablet coordinates)";

#[derive(Clone, Copy, Debug, serde::Deserialize, serde::Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct RadialFollowSettings {
    pub outer_radius: f64,
    pub inner_radius: f64,
    pub smoothing_coefficient: f64,
    pub soft_knee_scale: f64,
    pub smoothing_leak_coefficient: f64,
}

impl Default for RadialFollowSettings {
    fn default() -> Self {
        Self {
            outer_radius: 1.0,
            inner_radius: 0.0,
            smoothing_coefficient: 0.95,
            soft_knee_scale: 1.0,
            smoothing_leak_coefficient: 0.0,
        }
    }
}

impl RadialFollowSettings {
    pub fn clamped(mut self) -> Self {
        self.outer_radius = self.outer_radius.clamp(0.0, 1_000_000.0);
        self.inner_radius = self.inner_radius.clamp(0.0, 1_000_000.0);
        self.smoothing_coefficient = self.smoothing_coefficient.clamp(f64::from(0.0001_f32), 1.0);
        self.soft_knee_scale = self.soft_knee_scale.clamp(0.0, 100.0);
        self.smoothing_leak_coefficient = self.smoothing_leak_coefficient.clamp(0.0, 1.0);
        self
    }
}

/// Original radial curve and position state. Math is evaluated in f64 and
/// report-space vectors in f32, matching the C# double/Vector2 split.
pub struct RadialFollowCore {
    settings: RadialFollowSettings,
    x_offset: f64,
    scale_comp: f64,
    cursor: (f32, f32),
    last_sample: Instant,
}

impl RadialFollowCore {
    pub fn new(settings: RadialFollowSettings) -> Self {
        let settings = settings.clamped();
        let (x_offset, scale_comp) = if settings.soft_knee_scale > f64::from(0.0001_f32) {
            let knee = settings.soft_knee_scale;
            let exponential = ((0.0 - 1.0) / knee).exp();
            let inverse_tanh = ((1.0 + exponential) / (1.0 - exponential)).ln() / 2.0;
            let offset = knee * inverse_tanh.ln();
            let e = (offset / knee).exp();
            let tanh = e.tanh();
            (offset, (e - e * tanh * tanh) / tanh)
        } else {
            (-1.0, 1.0)
        };
        Self {
            settings,
            x_offset,
            scale_comp,
            cursor: (0.0, 0.0),
            last_sample: Instant::now(),
        }
    }

    fn knee_scaled(&self, x: f64) -> f64 {
        let knee = self.settings.soft_knee_scale;
        if knee > f64::from(0.0001_f32) {
            let argument = x / knee;
            let value = if argument < -3.0 {
                argument
            } else if argument < 3.0 {
                argument.exp().tanh().ln()
            } else {
                0.0
            };
            knee * value + 1.0
        } else if x > 0.0 {
            1.0
        } else {
            1.0 + x
        }
    }

    /// Distance moved toward the latest report, in millimetres.
    pub fn sample_radial_curve(&self, distance: f32) -> f32 {
        let outer = self
            .settings
            .outer_radius
            .max(self.settings.inner_radius + f64::from(0.0001_f32));
        let inner = self.settings.inner_radius;
        let x = f64::from(distance);
        if x <= inner {
            return 0.0;
        }
        let span = outer - inner;
        let scaled = (x - inner) / span * self.settings.smoothing_coefficient / self.scale_comp;
        let leaked = self.knee_scaled(scaled + self.x_offset)
            * (1.0 - self.settings.smoothing_leak_coefficient)
            + scaled * self.settings.smoothing_leak_coefficient * self.scale_comp;
        (x - span * leaked - inner) as f32
    }

    pub fn filter_at(&mut self, target: (f32, f32), now: Instant) -> (f32, f32) {
        let direction = (target.0 - self.cursor.0, target.1 - self.cursor.1);
        let distance = (direction.0 * direction.0 + direction.1 * direction.1).sqrt();
        let move_distance = self.sample_radial_curve(distance);
        self.cursor.0 += direction.0 / distance * move_distance;
        self.cursor.1 += direction.1 / distance * move_distance;
        if !self.cursor.0.is_finite()
            || !self.cursor.1.is_finite()
            || now.saturating_duration_since(self.last_sample) >= Duration::from_millis(50)
        {
            self.cursor = target;
        }
        self.last_sample = now;
        self.cursor
    }
}

/// The original filter runs at PipelinePosition.PreTransform and converts
/// PTH-660 report units to mm before invoking RadialFollowCore.
pub struct RadialFollowSmoothingTabletSpace {
    core: RadialFollowCore,
    mm_scale: (f32, f32),
}

impl RadialFollowSmoothingTabletSpace {
    pub fn new(settings: RadialFollowSettings) -> Self {
        Self {
            core: RadialFollowCore::new(settings),
            mm_scale: (224.0 / MAX_X as f32, 148.0 / MAX_Y as f32),
        }
    }

    pub fn filter_raw(&mut self, x: u32, y: u32) -> (f32, f32) {
        self.filter_raw_f32(x as f32, y as f32)
    }

    pub fn filter_raw_f32(&mut self, x: f32, y: f32) -> (f32, f32) {
        self.filter_raw_at(x, y, Instant::now())
    }

    fn filter_raw_at(&mut self, x: f32, y: f32, now: Instant) -> (f32, f32) {
        let mm = (x * self.mm_scale.0, y * self.mm_scale.1);
        let filtered = self.core.filter_at(mm, now);
        (filtered.0 / self.mm_scale.0, filtered.1 / self.mm_scale.1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn saved_settings() -> RadialFollowSettings {
        RadialFollowSettings {
            outer_radius: 0.7039,
            inner_radius: 0.302,
            smoothing_coefficient: 0.302,
            soft_knee_scale: 0.603,
            smoothing_leak_coefficient: 0.201,
        }
    }

    #[test]
    fn matches_original_csharp_curve_for_saved_profile() {
        // Reference generated by RadialFollowCore.cs from release 0.3.0.
        let core = RadialFollowCore::new(saved_settings());
        let points = [
            (0.0, 0.0),
            (0.1, 0.0),
            (0.302, 0.0),
            (0.5, 0.13855499),
            (0.7039, 0.28226689),
            (0.8, 0.35053164),
            (1.0, 0.49419802),
            (2.0, 1.2870909),
            (5.0, 4.091704),
            (20.0, 18.181173),
        ];
        for (distance, expected) in points {
            let actual = core.sample_radial_curve(distance);
            assert!(
                (actual - expected).abs() < 0.00001,
                "{distance}: {actual} != {expected}"
            );
        }
    }

    #[test]
    fn dead_zone_and_redetection_reset_match_core_behavior() {
        let mut core = RadialFollowCore::new(saved_settings());
        let start = Instant::now();
        assert_eq!(core.filter_at((0.0, 0.0), start), (0.0, 0.0));
        let near = core.filter_at((0.2, 0.0), start + Duration::from_millis(1));
        assert_eq!(near, (0.0, 0.0));
        let moved = core.filter_at((1.0, 0.0), start + Duration::from_millis(2));
        assert!(moved.0 > 0.0 && moved.0 < 1.0);
        assert_eq!(
            core.filter_at((20.0, 10.0), start + Duration::from_millis(52)),
            (20.0, 10.0)
        );
    }

    #[test]
    fn tablet_space_adapter_preserves_fractional_report_coordinates() {
        let mut filter = RadialFollowSmoothingTabletSpace::new(saved_settings());
        let start = Instant::now();
        assert_eq!(filter.filter_raw_at(0.0, 0.0, start), (0.0, 0.0));
        // PTH-660 has 200 report units per mm. The C# curve moves 0.49419802
        // mm toward a target one mm away, then converts back to report units.
        let moved = filter.filter_raw_at(200.0, 0.0, start + Duration::from_millis(1));
        assert!((moved.0 - 98.8396).abs() < 0.002);
        assert_eq!(moved.1, 0.0);
    }
}
