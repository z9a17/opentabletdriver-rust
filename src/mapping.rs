use crate::protocol::{MAX_X, MAX_Y};
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
        self.right - self.left
    }

    pub fn height(self) -> i32 {
        self.bottom - self.top
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
    pub fn valid(self) -> bool {
        self.width > 0
            && self.height > 0
            && self.x.checked_add(self.width).is_some_and(|v| v <= MAX_X)
            && self.y.checked_add(self.height).is_some_and(|v| v <= MAX_Y)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Mapper {
    crop: Crop,
    rotation: u16,
    destination: Rect,
    virtual_screen: Rect,
    otd: Option<OtdTransform>,
}

impl Mapper {
    pub fn new(crop: Crop, rotation: u16, destination: Rect, virtual_screen: Rect) -> Option<Self> {
        if !crop.valid()
            || !matches!(rotation, 0 | 90 | 180 | 270)
            || !destination.valid()
            || !virtual_screen.valid()
        {
            return None;
        }
        Some(Self {
            crop,
            rotation,
            destination,
            virtual_screen,
            otd: None,
        })
    }

    pub fn from_otd(settings: OtdMapping, virtual_screen: Rect) -> Option<Self> {
        if !virtual_screen.valid() || !settings.display.valid() || !settings.tablet.valid() {
            return None;
        }
        // The PTH-660 reports 44,800 x 29,600 units over 224 x 148 mm.
        let mm_x = 224.0 / f64::from(MAX_X);
        let mm_y = 148.0 / f64::from(MAX_Y);
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
        let transform = OtdTransform {
            a,
            b,
            c,
            d,
            tx,
            ty,
            min_x: settings.display.x - settings.display.width / 2.0,
            max_x: settings.display.x + settings.display.width / 2.0,
            min_y: settings.display.y - settings.display.height / 2.0,
            max_y: settings.display.y + settings.display.height / 2.0,
            clipping: settings.clipping,
            limiting: settings.limiting,
        };
        Some(Self {
            crop: Crop::default(),
            rotation: 0,
            destination: virtual_screen,
            virtual_screen,
            otd: Some(transform),
        })
    }

    #[inline]
    fn scale(value: u32, denom: u32, span: i32) -> i32 {
        let span = span as u64;
        ((u64::from(value) * span + u64::from(denom) / 2) / u64::from(denom)) as i32
    }

    /// Returns normalized virtual-desktop coordinates for SendInput.
    pub fn map(self, x: u32, y: u32) -> Option<(i32, i32)> {
        if let Some(t) = self.otd {
            return self.map_otd(t, f64::from(x), f64::from(y));
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
            (px - self.virtual_screen.left) as u32,
            (self.virtual_screen.width() - 1).max(1) as u32,
            65_535,
        );
        let ny = Self::scale(
            (py - self.virtual_screen.top) as u32,
            (self.virtual_screen.height() - 1).max(1) as u32,
            65_535,
        );
        Some((nx.clamp(0, 65_535), ny.clamp(0, 65_535)))
    }

    /// The pre-transform filter returns fractional report coordinates. Keep
    /// those fractions through the area transform, as OpenTabletDriver does.
    pub fn map_filtered(self, x: f32, y: f32) -> Option<(i32, i32)> {
        if !x.is_finite() || !y.is_finite() {
            return None;
        }
        if let Some(transform) = self.otd {
            return self.map_otd(transform, f64::from(x), f64::from(y));
        }
        let u = (f64::from(x) - f64::from(self.crop.x)).clamp(0.0, f64::from(self.crop.width));
        let v = (f64::from(y) - f64::from(self.crop.y)).clamp(0.0, f64::from(self.crop.height));
        let (w, h) = (f64::from(self.crop.width), f64::from(self.crop.height));
        let (u, v, w, h) = match self.rotation {
            90 => (v, w - u, h, w),
            180 => (w - u, h - v, w, h),
            270 => (h - v, u, h, w),
            _ => (u, v, w, h),
        };
        let px = f64::from(self.destination.left) + u / w * f64::from(self.destination.width() - 1);
        let py = f64::from(self.destination.top) + v / h * f64::from(self.destination.height() - 1);
        let screen = self.virtual_screen;
        Some((
            ((px - f64::from(screen.left)) * 65_535.0 / f64::from((screen.width() - 1).max(1)))
                .round()
                .clamp(0.0, 65_535.0) as i32,
            ((py - f64::from(screen.top)) * 65_535.0 / f64::from((screen.height() - 1).max(1)))
                .round()
                .clamp(0.0, 65_535.0) as i32,
        ))
    }

    fn map_otd(self, t: OtdTransform, x: f64, y: f64) -> Option<(i32, i32)> {
        let mut px = t.a * x + t.b * y + t.tx;
        let mut py = t.c * x + t.d * y + t.ty;
        if t.limiting && (px < t.min_x || px > t.max_x || py < t.min_y || py > t.max_y) {
            return None;
        }
        if t.clipping || t.limiting {
            px = px.clamp(t.min_x, (t.max_x - 1.0).max(t.min_x));
            py = py.clamp(t.min_y, (t.max_y - 1.0).max(t.min_y));
        }
        let screen = self.virtual_screen;
        let nx = ((px - f64::from(screen.left)) * 65_535.0 / f64::from((screen.width() - 1).max(1)))
            .round()
            .clamp(0.0, 65_535.0) as i32;
        let ny = ((py - f64::from(screen.top)) * 65_535.0 / f64::from((screen.height() - 1).max(1)))
            .round()
            .clamp(0.0, 65_535.0) as i32;
        Some((nx, ny))
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
}
