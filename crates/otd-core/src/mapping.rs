use crate::protocol::{MAX_X, MAX_Y};
use crate::spec::TabletSpec;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Rect {
    pub fn width(self) -> i32 {
        self.right.checked_sub(self.left).unwrap_or(0)
    }

    pub fn height(self) -> i32 {
        self.bottom.checked_sub(self.top).unwrap_or(0)
    }

    pub fn valid(self) -> bool {
        self.width() > 0 && self.height() > 0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Crop {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// OpenTabletDriver stores absolute areas by their center and size. Tablet
/// coordinates are millimeters; display coordinates are desktop pixels.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct OtdArea {
    pub width: f64,
    pub height: f64,
    pub x: f64,
    pub y: f64,
    pub rotation: f64,
}

impl OtdArea {
    fn valid(self) -> bool {
        [self.width, self.height, self.x, self.y, self.rotation]
            .into_iter()
            .all(f64::is_finite)
            && self.width > 0.0
            && self.height > 0.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Deserialize, Serialize)]
pub struct OtdMapping {
    pub display: OtdArea,
    pub tablet: OtdArea,
    pub clipping: bool,
    pub limiting: bool,
}

#[derive(Clone, Copy, Debug)]
struct OtdTransform {
    a: f64,
    b: f64,
    c: f64,
    d: f64,
    tx: f64,
    ty: f64,
    min_x: f64,
    max_x: f64,
    min_y: f64,
    max_y: f64,
    clip_max_x: f64,
    clip_max_y: f64,
    clipping: bool,
    limiting: bool,
}

impl Default for Crop {
    fn default() -> Self {
        Self {
            x: 0,
            y: 0,
            width: MAX_X,
            height: MAX_Y,
        }
    }
}

impl Crop {
    /// Within the PTH-660's range.
    pub fn valid(self) -> bool {
        self.valid_for(TabletSpec::PTH_660)
    }

    pub fn valid_for(self, spec: TabletSpec) -> bool {
        self.width > 0
            && self.height > 0
            && self
                .x
                .checked_add(self.width)
                .is_some_and(|v| v <= spec.max_x)
            && self
                .y
                .checked_add(self.height)
                .is_some_and(|v| v <= spec.max_y)
    }

    /// The whole digitizer of a tablet.
    pub fn full(spec: TabletSpec) -> Self {
        Self {
            x: 0,
            y: 0,
            width: spec.max_x,
            height: spec.max_y,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Mapper {
    crop: Crop,
    rotation: u16,
    destination: Rect,
    virtual_screen: Rect,
    screen_left: f64,
    screen_top: f64,
    screen_right_minus_1: f64,
    screen_bottom_minus_1: f64,
    norm_x: f64,
    norm_y: f64,
    inv_crop_w: f64,
    inv_crop_h: f64,
    dest_span_x: f64,
    dest_span_y: f64,
    otd: Option<OtdTransform>,
}

/// Branchless round-half-away-from-zero for finite `f64` clamped to `0..=65_535`.
/// Bit-identical to `value.round().clamp(0.0, 65_535.0) as i32` for every finite
/// input, without calling the C runtime `round()` function.
#[inline]
fn round_clamped_u16(value: f64) -> i32 {
    let clamped = value.clamp(0.0, 65_535.0);
    let truncated = clamped as i32;
    truncated + i32::from(clamped - f64::from(truncated) >= 0.5)
}

impl Mapper {
    fn norm_scales(virtual_screen: Rect) -> (f64, f64) {
        (
            65_535.0 / f64::from((virtual_screen.width() - 1).max(1)),
            65_535.0 / f64::from((virtual_screen.height() - 1).max(1)),
        )
    }

    pub fn new(crop: Crop, rotation: u16, destination: Rect, virtual_screen: Rect) -> Option<Self> {
        Self::new_for(
            crop,
            rotation,
            destination,
            virtual_screen,
            TabletSpec::PTH_660,
        )
    }

    pub fn new_for(
        crop: Crop,
        rotation: u16,
        destination: Rect,
        virtual_screen: Rect,
        spec: TabletSpec,
    ) -> Option<Self> {
        if !crop.valid_for(spec)
            || !matches!(rotation, 0 | 90 | 180 | 270)
            || !destination.valid()
            || !virtual_screen.valid()
        {
            return None;
        }
        let (norm_x, norm_y) = Self::norm_scales(virtual_screen);
        Some(Self {
            crop,
            rotation,
            destination,
            virtual_screen,
            screen_left: f64::from(virtual_screen.left),
            screen_top: f64::from(virtual_screen.top),
            screen_right_minus_1: f64::from(virtual_screen.right - 1),
            screen_bottom_minus_1: f64::from(virtual_screen.bottom - 1),
            norm_x,
            norm_y,
            inv_crop_w: 1.0 / f64::from(crop.width),
            inv_crop_h: 1.0 / f64::from(crop.height),
            dest_span_x: f64::from(destination.width() - 1),
            dest_span_y: f64::from(destination.height() - 1),
            otd: None,
        })
    }

    pub fn from_otd(settings: OtdMapping, virtual_screen: Rect) -> Option<Self> {
        Self::from_otd_for(settings, virtual_screen, TabletSpec::PTH_660)
    }

    /// OpenTabletDriver's absolute area transform for a tablet: areas are in
    /// millimetres, reports in the tablet's raw units.
    pub fn from_otd_for(
        settings: OtdMapping,
        virtual_screen: Rect,
        spec: TabletSpec,
    ) -> Option<Self> {
        if !virtual_screen.valid() || !settings.display.valid() || !settings.tablet.valid() {
            return None;
        }
        let (mm_x, mm_y) = spec.mm_per_unit();
        let angle = (-settings.tablet.rotation).to_radians();
        let (sin, cos) = angle.sin_cos();
        let sx = settings.display.width / settings.tablet.width;
        let sy = settings.display.height / settings.tablet.height;
        let a = cos * sx * mm_x;
        let b = -sin * sx * mm_y;
        let c = sin * sy * mm_x;
        let d = cos * sy * mm_y;
        let tx = settings.display.x - cos * sx * settings.tablet.x + sin * sx * settings.tablet.y;
        let ty = settings.display.y - sin * sy * settings.tablet.x - cos * sy * settings.tablet.y;
        let min_x = settings.display.x - settings.display.width / 2.0;
        let max_x = settings.display.x + settings.display.width / 2.0;
        let min_y = settings.display.y - settings.display.height / 2.0;
        let max_y = settings.display.y + settings.display.height / 2.0;
        let transform = OtdTransform {
            a,
            b,
            c,
            d,
            tx,
            ty,
            min_x,
            max_x,
            min_y,
            max_y,
            clip_max_x: (max_x - 1.0).max(min_x),
            clip_max_y: (max_y - 1.0).max(min_y),
            clipping: settings.clipping,
            limiting: settings.limiting,
        };
        if ![
            a,
            b,
            c,
            d,
            tx,
            ty,
            transform.min_x,
            transform.max_x,
            transform.min_y,
            transform.max_y,
        ]
        .into_iter()
        .all(f64::is_finite)
        {
            return None;
        }
        let (norm_x, norm_y) = Self::norm_scales(virtual_screen);
        let crop = Crop::default();
        Some(Self {
            crop,
            rotation: 0,
            destination: virtual_screen,
            virtual_screen,
            screen_left: f64::from(virtual_screen.left),
            screen_top: f64::from(virtual_screen.top),
            screen_right_minus_1: f64::from(virtual_screen.right - 1),
            screen_bottom_minus_1: f64::from(virtual_screen.bottom - 1),
            norm_x,
            norm_y,
            inv_crop_w: 1.0 / f64::from(crop.width),
            inv_crop_h: 1.0 / f64::from(crop.height),
            dest_span_x: f64::from(virtual_screen.width() - 1),
            dest_span_y: f64::from(virtual_screen.height() - 1),
            otd: Some(transform),
        })
    }

    #[inline]
    fn scale(value: u32, denom: u32, span: i32) -> i32 {
        let span = span as u64;
        ((u64::from(value) * span + u64::from(denom) / 2) / u64::from(denom)) as i32
    }

    /// Returns normalized virtual-desktop coordinates for SendInput.
    #[inline]
    pub fn map(self, x: u32, y: u32) -> Option<(i32, i32)> {
        self.map_ref(x, y)
    }

    #[inline]
    pub(crate) fn map_ref(&self, x: u32, y: u32) -> Option<(i32, i32)> {
        if let Some(t) = &self.otd {
            return self.map_otd_ref(t, f64::from(x), f64::from(y));
        }
        let crop = self.crop;
        let u = x.saturating_sub(crop.x).min(crop.width);
        let v = y.saturating_sub(crop.y).min(crop.height);
        let (u, v, w, h) = match self.rotation {
            90 => (v, crop.width - u, crop.height, crop.width),
            180 => (crop.width - u, crop.height - v, crop.width, crop.height),
            270 => (crop.height - v, u, crop.height, crop.width),
            _ => (u, v, crop.width, crop.height),
        };
        let px = self.destination.left + Self::scale(u, w, self.destination.width() - 1);
        let py = self.destination.top + Self::scale(v, h, self.destination.height() - 1);
        let nx = Self::scale(
            (i64::from(px) - i64::from(self.virtual_screen.left))
                .clamp(0, i64::from(self.virtual_screen.width() - 1)) as u32,
            (self.virtual_screen.width() - 1).max(1) as u32,
            65_535,
        );
        let ny = Self::scale(
            (i64::from(py) - i64::from(self.virtual_screen.top))
                .clamp(0, i64::from(self.virtual_screen.height() - 1)) as u32,
            (self.virtual_screen.height() - 1).max(1) as u32,
            65_535,
        );
        Some((nx.clamp(0, 65_535), ny.clamp(0, 65_535)))
    }

    /// The pre-transform filter returns fractional report coordinates. Keep
    /// those fractions through the area transform, as OpenTabletDriver does.
    #[inline]
    pub fn map_filtered(self, x: f32, y: f32) -> Option<(i32, i32)> {
        let (px, py) = self.map_filtered_pixels_ref(x, y)?;
        self.normalize_pixels_ref(px, py)
    }

    /// Returns desktop pixels before SendInput normalization, so a managed
    /// PostTransform filter sees the same units as OpenTabletDriver.
    #[inline]
    pub fn map_filtered_pixels(self, x: f32, y: f32) -> Option<(f64, f64)> {
        self.map_filtered_pixels_ref(x, y)
    }

    #[inline]
    pub(crate) fn map_filtered_pixels_ref(&self, x: f32, y: f32) -> Option<(f64, f64)> {
        if !x.is_finite() || !y.is_finite() {
            return None;
        }
        self.map_filtered_finite_pixels_ref(x, y)
    }

    #[inline]
    pub(crate) fn map_filtered_finite_pixels_ref(&self, x: f32, y: f32) -> Option<(f64, f64)> {
        if let Some(transform) = &self.otd {
            return Self::map_otd_pixels_ref(transform, f64::from(x), f64::from(y));
        }
        let u = (f64::from(x) - f64::from(self.crop.x)).clamp(0.0, f64::from(self.crop.width));
        let v = (f64::from(y) - f64::from(self.crop.y)).clamp(0.0, f64::from(self.crop.height));
        let (w, h) = (f64::from(self.crop.width), f64::from(self.crop.height));
        let (u, v, inv_w, inv_h) = match self.rotation {
            90 => (v, w - u, self.inv_crop_h, self.inv_crop_w),
            180 => (w - u, h - v, self.inv_crop_w, self.inv_crop_h),
            270 => (h - v, u, self.inv_crop_h, self.inv_crop_w),
            _ => (u, v, self.inv_crop_w, self.inv_crop_h),
        };
        let px = f64::from(self.destination.left) + (u * inv_w) * self.dest_span_x;
        let py = f64::from(self.destination.top) + (v * inv_h) * self.dest_span_y;
        Some((px, py))
    }

    /// The desktop area pixel positions are clamped to.
    #[inline]
    pub fn virtual_screen(self) -> Rect {
        self.virtual_screen
    }

    /// A desktop pixel position moved inside the virtual screen.
    #[inline]
    pub fn clamp_pixels(self, px: f64, py: f64) -> Option<(f64, f64)> {
        self.clamp_pixels_ref(px, py)
    }

    #[inline]
    pub(crate) fn clamp_pixels_ref(&self, px: f64, py: f64) -> Option<(f64, f64)> {
        if !px.is_finite() || !py.is_finite() {
            return None;
        }
        Some((
            px.clamp(self.screen_left, self.screen_right_minus_1),
            py.clamp(self.screen_top, self.screen_bottom_minus_1),
        ))
    }

    #[inline]
    pub fn normalize_pixels(self, px: f64, py: f64) -> Option<(i32, i32)> {
        self.normalize_pixels_ref(px, py)
    }

    #[inline]
    pub(crate) fn normalize_pixels_ref(&self, px: f64, py: f64) -> Option<(i32, i32)> {
        if !px.is_finite() || !py.is_finite() {
            return None;
        }
        Some(self.normalize_finite_pixels_ref(px, py))
    }

    #[inline]
    pub(crate) fn normalize_finite_pixels_ref(&self, px: f64, py: f64) -> (i32, i32) {
        (
            round_clamped_u16((px - self.screen_left) * self.norm_x),
            round_clamped_u16((py - self.screen_top) * self.norm_y),
        )
    }

    #[inline]
    fn map_otd_ref(&self, t: &OtdTransform, x: f64, y: f64) -> Option<(i32, i32)> {
        let (px, py) = Self::map_otd_pixels_ref(t, x, y)?;
        self.normalize_pixels_ref(px, py)
    }

    #[inline]
    fn map_otd_pixels_ref(t: &OtdTransform, x: f64, y: f64) -> Option<(f64, f64)> {
        let mut px = t.a * x + t.b * y + t.tx;
        let mut py = t.c * x + t.d * y + t.ty;
        if t.limiting && (px < t.min_x || px > t.max_x || py < t.min_y || py > t.max_y) {
            return None;
        }
        if t.clipping || t.limiting {
            px = px.clamp(t.min_x, t.clip_max_x);
            py = py.clamp(t.min_y, t.clip_max_y);
        }
        Some((px, py))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corners_reach_edges_with_negative_screen_origin() {
        let screen = Rect {
            left: -1920,
            top: -1080,
            right: 1920,
            bottom: 1080,
        };
        let m = Mapper::new(Crop::default(), 0, screen, screen).unwrap();
        assert_eq!(m.map(0, 0), Some((0, 0)));
        assert_eq!(m.map(MAX_X, MAX_Y), Some((65_535, 65_535)));
    }

    #[test]
    fn monitor_selection_and_rotation() {
        let screen = Rect {
            left: -1920,
            top: 0,
            right: 1920,
            bottom: 1080,
        };
        let left = Rect {
            left: -1920,
            top: 0,
            right: 0,
            bottom: 1080,
        };
        let m = Mapper::new(Crop::default(), 90, left, screen).unwrap();
        assert_eq!(m.map(0, MAX_Y), Some((32_759, 65_535)));
        assert_eq!(m.map(MAX_X, 0), Some((0, 0)));
    }

    #[test]
    fn invalid_crop_is_rejected() {
        let screen = Rect {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1080,
        };
        let crop = Crop {
            x: MAX_X,
            ..Crop::default()
        };
        assert!(Mapper::new(crop, 0, screen, screen).is_none());
    }

    #[test]
    fn invalid_rectangle_spans_are_rejected_without_overflow() {
        let screen = Rect {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1080,
        };
        for invalid in [
            Rect {
                left: i32::MIN,
                right: i32::MAX,
                ..screen
            },
            Rect {
                top: i32::MIN,
                bottom: i32::MAX,
                ..screen
            },
            Rect {
                left: i32::MAX,
                right: i32::MIN,
                ..screen
            },
            Rect {
                top: i32::MAX,
                bottom: i32::MIN,
                ..screen
            },
        ] {
            assert!(!invalid.valid());
            assert!(Mapper::new(Crop::default(), 0, invalid, screen).is_none());
            assert!(Mapper::new(Crop::default(), 0, screen, invalid).is_none());
        }
    }

    #[test]
    fn positions_outside_virtual_desktop_clamp_to_the_nearest_edge() {
        let screen = Rect {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1080,
        };
        let destination = Rect {
            left: -1920,
            top: -1080,
            right: 3840,
            bottom: 2160,
        };
        let mapper = Mapper::new(Crop::default(), 0, destination, screen).unwrap();
        assert_eq!(mapper.map(0, 0), Some((0, 0)));
        assert_eq!(mapper.map(MAX_X, MAX_Y), Some((65_535, 65_535)));
        assert_eq!(mapper.map(0, MAX_Y), Some((0, 65_535)));
        // Subtraction itself must also be safe when two individually valid
        // rectangles occupy opposite ends of the signed coordinate range.
        let far_screen = Rect {
            left: i32::MAX - 1920,
            right: i32::MAX,
            ..screen
        };
        let far_destination = Rect {
            left: i32::MIN,
            right: i32::MIN + 1920,
            ..screen
        };
        let far = Mapper::new(Crop::default(), 0, far_destination, far_screen).unwrap();
        assert_eq!(far.map(MAX_X, MAX_Y), Some((0, 65_535)));
    }

    #[test]
    fn overflowing_original_area_transform_is_rejected_during_setup() {
        let screen = Rect {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1080,
        };
        let area = OtdArea {
            width: 100.0,
            height: 100.0,
            x: 50.0,
            y: 50.0,
            rotation: 0.0,
        };
        for tablet in [
            OtdArea {
                width: f64::MIN_POSITIVE,
                ..area
            },
            OtdArea {
                x: f64::MAX,
                ..area
            },
        ] {
            assert!(
                Mapper::from_otd(
                    OtdMapping {
                        display: OtdArea {
                            width: 1920.0,
                            height: 1080.0,
                            ..area
                        },
                        tablet,
                        clipping: true,
                        limiting: false,
                    },
                    screen
                )
                .is_none()
            );
        }
    }

    #[test]
    fn filtered_positions_work_with_simple_profiles_and_keep_fractions() {
        let screen = Rect {
            left: -100,
            top: 0,
            right: 900,
            bottom: 1000,
        };
        let mapper = Mapper::new(Crop::default(), 0, screen, screen).unwrap();
        assert_eq!(mapper.map_filtered(0.0, 0.0), Some((0, 0)));
        assert_eq!(
            mapper.map_filtered(MAX_X as f32, MAX_Y as f32),
            Some((65_535, 65_535))
        );
        assert!(
            mapper.map_filtered(100.5, 100.5).unwrap().0
                > mapper.map_filtered(100.0, 100.0).unwrap().0
        );
    }

    #[test]
    fn otd_center_areas_map_tablet_crop_to_first_display() {
        let screen = Rect {
            left: 0,
            top: 0,
            right: 4160,
            bottom: 1440,
        };
        let settings = OtdMapping {
            display: OtdArea {
                width: 2560.0,
                height: 1440.0,
                x: 1280.0,
                y: 720.0,
                rotation: 0.0,
            },
            tablet: OtdArea {
                width: 85.0,
                height: 47.8125,
                x: 110.0,
                y: 23.90625,
                rotation: 0.0,
            },
            clipping: true,
            limiting: false,
        };
        let m = Mapper::from_otd(settings, screen).unwrap();
        assert_eq!(m.map(13_500, 0), Some((0, 0)));
        let (right, bottom) = m.map(30_500, 9_563).unwrap();
        assert!((40_300..40_400).contains(&right));
        assert_eq!(bottom, 65_535);
        assert_eq!(m.map(0, 0), Some((0, 0)));
        let limited = Mapper::from_otd(
            OtdMapping {
                limiting: true,
                ..settings
            },
            screen,
        )
        .unwrap();
        assert_eq!(limited.map(0, 0), None);
    }

    #[test]
    fn filtered_fractional_report_position_is_preserved() {
        let screen = Rect {
            left: 0,
            top: 0,
            right: 2560,
            bottom: 1440,
        };
        let settings = OtdMapping {
            display: OtdArea {
                width: 2560.0,
                height: 1440.0,
                x: 1280.0,
                y: 720.0,
                rotation: 0.0,
            },
            tablet: OtdArea {
                width: 85.0,
                height: 47.8125,
                x: 110.0,
                y: 23.90625,
                rotation: 0.0,
            },
            clipping: true,
            limiting: false,
        };
        let mapper = Mapper::from_otd(settings, screen).unwrap();
        let integer = mapper.map(22_000, 4_780).unwrap();
        let fractional = mapper.map_filtered(22_000.5, 4_780.5).unwrap();
        assert!(fractional.0 > integer.0);
        assert!(fractional.1 > integer.1);
    }

    #[test]
    fn round_clamped_u16_matches_f64_round_clamp_across_edges() {
        let cases = [
            f64::NEG_INFINITY,
            -1_000.0,
            -1.0,
            -0.5,
            -0.0,
            0.0,
            0.25,
            0.499_999_999_999_999_94,
            0.5,
            0.500_000_000_000_000_1,
            1.499_999_999_999_999_8,
            1.5,
            2.5,
            32_767.5,
            65_534.499_999_999_99,
            65_534.5,
            65_535.0,
            65_535.5,
            100_000.0,
            f64::INFINITY,
        ];
        for v in cases {
            let expected = v.round().clamp(0.0, 65_535.0) as i32;
            assert_eq!(round_clamped_u16(v), expected, "mismatch for {v:?}");
        }
        assert_eq!(round_clamped_u16(f64::NAN), 0);
    }
}
