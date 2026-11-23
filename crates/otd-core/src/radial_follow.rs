// SPDX-License-Identifier: GPL-3.0-only
// Rust port of AbstractQbit's RadialFollow 0.3.0 tablet-space filter.
// Original source: https://github.com/AbstractQbit/AbstractOTDPlugins/tree/0.3.0/RadialFollow

use std::time::{Duration, Instant};

use crate::spec::TabletSpec;

pub const FILTER_PATH: &str = "RadialFollow.RadialFollowSmoothingTabletSpace";
pub const FILTER_NAME: &str = "AbstractQbit's Radial Follow Smoothing (Tablet coordinates)";
/// SHA-256 of the RadialFollow 0.3.0 release DLL this module ports. The
/// differential fixture `tests/differential/osu-trace.json` was generated
/// with this DLL.
pub const DLL_SHA256: &str = "830d61f29c07b109398f1e001df3c7fbd359b1ad2491a6d12ccc243dacf16383";

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
    /// Settings saved for the .NET filter, when they give every property.
    /// The bridge leaves a missing property at the C# constructor's zero,
    /// which this port does not model, so that returns `None`. A null selects
    /// the property's default, as the bridge does. Unknown keys are ignored
    /// like upstream.
    pub fn from_dotnet_json(text: &str) -> Option<Self> {
        let settings: serde_json::Map<String, serde_json::Value> =
            serde_json::from_str(text).ok()?;
        let defaults = Self::default();
        let property = |name: &str, default: f64| match settings.get(name)? {
            serde_json::Value::Null => Some(default),
            value => value.as_f64().filter(|value| value.is_finite()),
        };
        Some(Self {
            outer_radius: property("OuterRadius", defaults.outer_radius)?,
            inner_radius: property("InnerRadius", defaults.inner_radius)?,
            smoothing_coefficient: property(
                "SmoothingCoefficient",
                defaults.smoothing_coefficient,
            )?,
            soft_knee_scale: property("SoftKneeScale", defaults.soft_knee_scale)?,
            smoothing_leak_coefficient: property(
                "SmoothingLeakCoefficient",
                defaults.smoothing_leak_coefficient,
            )?,
        })
    }

    pub fn clamped(mut self) -> Self {
        self.outer_radius = self.outer_radius.clamp(0.0, 1_000_000.0);
        self.inner_radius = self.inner_radius.clamp(0.0, 1_000_000.0);
        self.smoothing_coefficient = self.smoothing_coefficient.clamp(f64::from(0.0001_f32), 1.0);
        self.soft_knee_scale = self.soft_knee_scale.clamp(0.0, 100.0);
        self.smoothing_leak_coefficient = self.smoothing_leak_coefficient.clamp(0.0, 1.0);
        self
    }
}

/// 16-segment degree-7 Chebyshev polynomial table for `ln(tanh(exp(a)))` on
/// `a in [-3.0, 3.0)`, aligned to 64-byte cache lines (one segment = one line).
/// Verified max absolute error < 1.15e-10 across [-3.0, 3.0).
#[repr(align(64))]
struct KneePolyTable([[f64; 8]; 16]);

static KNEE_POLY: KneePolyTable = KneePolyTable([
    [
        -2.813_701_177_058_026,
        0.187_049_937_214_549_5,
        -8.424_493_910_030_861e-5,
        -1.049_521_213_014_603_7e-5,
        -9.772_913_363_015_334e-7,
        -7.230_730_680_873_876e-8,
        -4.414_509_646_721_854e-9,
        -2.229_665_341_246_800_4e-10,
    ],
    [
        -2.440_040_507_087_785_3,
        0.186_549_002_828_323_2,
        -1.776_784_625_384_664_2e-4,
        -2.205_193_824_733_609e-5,
        -2.037_893_870_987_517e-6,
        -1.484_564_500_220_386_7e-7,
        -8.764_128_089_921_996e-9,
        -4.111_591_067_612_607_7e-10,
    ],
    [
        -2.067_867_603_314_665_5,
        0.185_494_700_046_910_96,
        -3.731_748_046_922_334e-4,
        -4.594_689_004_061_569_5e-5,
        -4.177_603_653_676_698e-6,
        -2.941_339_294_348_921_4e-7,
        -1.605_413_846_306_191_8e-8,
        -6.141_913_644_341_912e-10,
    ],
    [
        -1.698_815_838_119_41,
        0.183_290_064_417_531_26,
        -7.769_184_281_326_558e-4,
        -9.404_800_013_956_827e-5,
        -8.254_348_899_061_625e-6,
        -5.373_847_007_383_858e-7,
        -2.373_833_485_336_263e-8,
        -2.985_673_930_311_35e-10,
    ],
    [
        -1.336_246_577_807_555_4,
        0.178_742_052_843_034_85,
        -1.588_077_202_264_948e-3,
        -1.854_017_576_065_07e-4,
        -1.503_380_637_912_776_8e-5,
        -7.990_540_124_014_345e-7,
        -1.239_088_298_632_395_9e-8,
        2.664_147_569_220_176_6e-9,
    ],
    [
        -0.986_864_497_667_587_8,
        0.169_619_012_375_831_38,
        -3.125_840_422_749_687e-3,
        -3.374_828_060_486_168e-4,
        -2.257_267_497_807_191_6e-5,
        -5.227_037_498_389_819e-7,
        8.339_452_994_832_186e-8,
        1.227_795_509_706_197_7e-8,
    ],
    [
        -0.663_200_559_167_899,
        0.152_323_771_731_100_62,
        -5.704_964_382_496_93e-3,
        -5.175_880_031_410_701e-4,
        -1.858_702_715_629_157e-5,
        1.821_315_493_588_66e-6,
        3.260_168_333_074_631e-7,
        1.861_224_507_848_646_7e-8,
    ],
    [
        -0.385_729_623_744_053_56,
        0.122_913_820_925_700_97,
        -9.022_002_116_418_626e-3,
        -5.330_974_558_064_167e-4,
        2.270_723_733_766_022_4e-5,
        6.459_323_446_533_771e-6,
        3.037_583_354_847_939e-7,
        -3.679_178_317_561_593e-8,
    ],
    [
        -0.179_670_935_849_054,
        0.081_709_664_460_269_9,
        -1.111_893_116_578_247e-2,
        -7.407_979_229_822_129e-5,
        8.925_085_700_881_019e-5,
        4.588_820_590_900_6e-6,
        -7.700_180_953_973_401e-7,
        -9.073_249_335_189_715e-8,
    ],
    [
        -0.059_805_111_831_755_89,
        0.039_383_932_257_025_814,
        -9.291_115_835_922_535e-3,
        6.599_750_174_630_216e-4,
        7.120_477_111_591_944e-5,
        -8.661_029_389_782_36e-6,
        -9.408_012_546_230_36e-7,
        9.354_531_435_579_183e-8,
    ],
    [
        -0.012_106_418_278_403_933,
        0.011_593_341_588_054_38,
        -4.464_403_492_093_317e-3,
        7.994_147_218_215_162e-4,
        -3.674_828_442_078_604_4e-5,
        -8.882_168_484_776_031e-6,
        9.177_270_454_147_762e-7,
        9.724_570_484_460_904e-8,
    ],
    [
        -0.001_185_306_135_037_713_1,
        0.001_651_480_359_811_977_2,
        -9.956_740_648_832_266e-4,
        3.282_876_352_667_819_6e-4,
        -5.897_779_565_418_537_4e-5,
        3.669_975_437_492_072e-6,
        6.044_117_200_504_915e-7,
        -1.105_333_142_032_349_4e-7,
    ],
    [
        -4.031_652_180_147_979e-5,
        8.173_078_304_654_763e-5,
        -7.517_942_276_922_08e-5,
        4.092_653_762_739_338e-5,
        -1.431_292_092_540_335_4e-5,
        3.189_238_304_738_429_7e-6,
        -3.724_159_057_635_464_3e-7,
        -1.621_614_731_770_360_4e-9,
    ],
    [
        -2.944_586_951_430_074_7e-7,
        8.685_844_319_828_323e-7,
        -1.200_085_990_900_765e-6,
        1.024_386_606_084_190_4e-6,
        -5.979_052_710_464_623e-7,
        2.527_901_788_223_273e-7,
        -8.142_162_432_013_891e-8,
        1.664_231_489_129_046_4e-8,
    ],
    [
        -2.282_331_726_125_83e-10,
        9.841_302_084_301_831e-10,
        -2.056_746_017_221_565_3e-9,
        2.640_783_619_994_267_3e-9,
        -2.253_597_959_294_049e-9,
        1.650_506_013_545_063_2e-9,
        -1.203_166_191_845_107_5e-9,
        4.651_862_736_043_168_4e-10,
    ],
    [
        -5.155_776_201_263_392_5e-15,
        4.217_138_377_816_712e-14,
        -1.851_177_319_429_323_7e-13,
        2.789_916_866_727_352e-13,
        -9.461_295_122_229_388e-14,
        2.731_762_246_072_309_7e-13,
        -7.351_597_051_812_302e-13,
        4.268_589_276_645_816_5e-13,
    ],
]);

#[inline]
fn approx_ln_tanh_exp(argument: f64) -> f64 {
    let scaled = (argument + 3.0) * (16.0 / 6.0);
    let index = (scaled as usize).min(15);
    let x = (scaled - index as f64) * 2.0 - 1.0;
    let c = &KNEE_POLY.0[index];
    let x2 = x * x;
    let x4 = x2 * x2;
    let p01 = c[0] + c[1] * x;
    let p23 = c[2] + c[3] * x;
    let p45 = c[4] + c[5] * x;
    let p67 = c[6] + c[7] * x;
    (p01 + p23 * x2) + (p45 + p67 * x2) * x4
}

/// Original radial curve and position state. Math is evaluated in f64 and
/// report-space vectors in f32, matching the C# double/Vector2 split.
pub struct RadialFollowCore {
    settings: RadialFollowSettings,
    x_offset: f64,
    span: f64,
    scaled_factor: f64,
    leak_times_scale_comp: f64,
    one_minus_leak: f64,
    inv_knee: f64,
    inner_sq: f32,
    has_knee: bool,
    cursor: (f32, f32),
    last_sample: Instant,
}

impl RadialFollowCore {
    pub fn new(settings: RadialFollowSettings) -> Self {
        let settings = settings.clamped();
        let has_knee = settings.soft_knee_scale > f64::from(0.0001_f32);
        let (x_offset, scale_comp, inv_knee) = if has_knee {
            let knee = settings.soft_knee_scale;
            let exponential = ((0.0 - 1.0) / knee).exp();
            let inverse_tanh = ((1.0 + exponential) / (1.0 - exponential)).ln() / 2.0;
            let offset = knee * inverse_tanh.ln();
            let e = (offset / knee).exp();
            let tanh = e.tanh();
            (offset, (e - e * tanh * tanh) / tanh, 1.0 / knee)
        } else {
            (-1.0, 1.0, 0.0)
        };
        let outer = settings
            .outer_radius
            .max(settings.inner_radius + f64::from(0.0001_f32));
        let span = outer - settings.inner_radius;
        let scaled_factor = settings.smoothing_coefficient / (span * scale_comp);
        let leak_times_scale_comp = settings.smoothing_leak_coefficient * scale_comp;
        let one_minus_leak = 1.0 - settings.smoothing_leak_coefficient;
        let inner_f32 = settings.inner_radius as f32;
        Self {
            settings,
            x_offset,
            span,
            scaled_factor,
            leak_times_scale_comp,
            one_minus_leak,
            inv_knee,
            inner_sq: inner_f32 * inner_f32,
            has_knee,
            cursor: (0.0, 0.0),
            last_sample: Instant::now(),
        }
    }

    #[inline]
    fn knee_scaled(&self, x: f64) -> f64 {
        if self.has_knee {
            let argument = x * self.inv_knee;
            let value = if argument < -3.0 {
                argument
            } else if argument < 3.0 {
                approx_ln_tanh_exp(argument)
            } else {
                0.0
            };
            self.settings.soft_knee_scale * value + 1.0
        } else if x > 0.0 {
            1.0
        } else {
            1.0 + x
        }
    }

    /// Distance moved toward the latest report, in millimetres.
    #[inline]
    pub fn sample_radial_curve(&self, distance: f32) -> f32 {
        let inner = self.settings.inner_radius;
        let x = f64::from(distance);
        if x <= inner {
            return 0.0;
        }
        let scaled = (x - inner) * self.scaled_factor;
        let leaked = self.knee_scaled(scaled + self.x_offset) * self.one_minus_leak
            + scaled * self.leak_times_scale_comp;
        (x - self.span * leaked - inner) as f32
    }

    #[inline]
    pub fn filter_at(&mut self, target: (f32, f32), now: Instant) -> (f32, f32) {
        let dx = target.0 - self.cursor.0;
        let dy = target.1 - self.cursor.1;
        let dist_sq = dx * dx + dy * dy;
        let timed_out =
            now.saturating_duration_since(self.last_sample) >= Duration::from_millis(50);
        self.last_sample = now;
        if timed_out || !dist_sq.is_finite() {
            self.cursor = target;
            return self.cursor;
        }
        // Early-exit inside the dead zone: skips sqrt, curve evaluation, and
        // avoids the 0.0 / 0.0 NaN trap on stationary samples.
        if dist_sq <= self.inner_sq {
            return self.cursor;
        }
        let distance = dist_sq.sqrt();
        let move_distance = self.sample_radial_curve(distance);
        if move_distance != 0.0 {
            let ratio = move_distance / distance;
            let nx = self.cursor.0 + dx * ratio;
            let ny = self.cursor.1 + dy * ratio;
            if nx.is_finite() && ny.is_finite() {
                self.cursor = (nx, ny);
            } else {
                self.cursor = target;
            }
        }
        self.cursor
    }
}

/// The original filter runs at PipelinePosition.PreTransform and converts
/// PTH-660 report units to mm before invoking RadialFollowCore.
pub struct RadialFollowSmoothingTabletSpace {
    core: RadialFollowCore,
    mm_scale: (f32, f32),
    inv_mm_scale: (f64, f64),
}

impl RadialFollowSmoothingTabletSpace {
    pub fn new(settings: RadialFollowSettings) -> Self {
        Self::new_for(settings, TabletSpec::PTH_660)
    }

    /// Report units are converted with the tablet's size, as the original
    /// filter reads it from the tablet's digitizer specification.
    pub fn new_for(settings: RadialFollowSettings, spec: TabletSpec) -> Self {
        let mm_scale = (
            spec.width_mm as f32 / spec.max_x as f32,
            spec.height_mm as f32 / spec.max_y as f32,
        );
        Self {
            core: RadialFollowCore::new(settings),
            mm_scale,
            inv_mm_scale: (1.0 / f64::from(mm_scale.0), 1.0 / f64::from(mm_scale.1)),
        }
    }

    /// Filters one position in report units. `now` is the report's
    /// processing time, which drives the 50 ms redetection reset.
    #[inline]
    pub fn filter_raw_at(&mut self, x: f32, y: f32, now: Instant) -> (f32, f32) {
        let mm = (x * self.mm_scale.0, y * self.mm_scale.1);
        let filtered = self.core.filter_at(mm, now);
        (
            (f64::from(filtered.0) * self.inv_mm_scale.0) as f32,
            (f64::from(filtered.1) * self.inv_mm_scale.1) as f32,
        )
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

    #[test]
    fn knee_polynomial_matches_exact_transcendental_and_dead_zone_skips_nan() {
        let mut max_err = 0.0_f64;
        for i in 0..=60_000 {
            let a = -3.0 + (i as f64 / 60_000.0) * 5.999_999;
            let exact = a.exp().tanh().ln();
            let approx = approx_ln_tanh_exp(a);
            let err = (approx - exact).abs();
            if err > max_err {
                max_err = err;
            }
        }
        assert!(max_err < 2e-10, "max_err = {max_err:e}");

        // Stationary point inside inner_radius must return finite cursor with no NaN.
        let mut core = RadialFollowCore::new(saved_settings());
        let start = Instant::now() + Duration::from_millis(60);
        assert_eq!(core.filter_at((12.5, -3.25), start), (12.5, -3.25));
        let same = core.filter_at((12.5, -3.25), start + Duration::from_millis(1));
        assert_eq!(same, (12.5, -3.25));
    }
}
