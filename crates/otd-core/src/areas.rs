//! Shared area editing and conversion services; no device or platform calls.
use crate::mapping::{OtdArea, OtdMapping, Rect};
use crate::protocol::{HEIGHT_MM as TABLET_HEIGHT_MM, WIDTH_MM as TABLET_WIDTH_MM};

/// Exact converter formulas from OpenTabletDriver 0.6.7, commit
/// 736003ed72c8bbb28033b039d5a0bb76c344145c, Desktop/Conversion/*.cs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Conversion {
    /// Up, Left, Down, Right as fractions (1 means 100 percent).
    Percentage,
    /// Top, Left, Bottom, Right in tablet units, using the X conversion factor.
    WacomVeikk,
    /// Width, Height, X, Y in XP Pen units; the pinned constant is 3.937f.
    XpPen,
    /// Width, Height, X, Y in tablet units. Pinned upstream uses X for both offsets.
    GaomonV2Otd067,
    /// Explicit correction of upstream's X-for-Y formula; never selected implicitly.
    GaomonV2Corrected,
}

impl std::str::FromStr for Conversion {
    type Err = String;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "percentage" => Ok(Self::Percentage),
            "wacom-veikk" => Ok(Self::WacomVeikk),
            "xp-pen" => Ok(Self::XpPen),
            "gaomon-v2-otd067" => Ok(Self::GaomonV2Otd067),
            "gaomon-v2-corrected" => Ok(Self::GaomonV2Corrected),
            _ => Err("unknown converter; choose percentage, wacom-veikk, xp-pen, gaomon-v2-otd067 or gaomon-v2-corrected".into()),
        }
    }
}

impl Conversion {
    pub fn labels(self) -> [&'static str; 4] {
        match self {
            Self::Percentage => ["Up", "Left", "Down", "Right"],
            Self::WacomVeikk => ["Top", "Left", "Bottom", "Right"],
            Self::XpPen => ["W", "H", "X", "Y"],
            Self::GaomonV2Otd067 | Self::GaomonV2Corrected => ["Width", "Height", "X", "Y"],
        }
    }

    pub fn input_units(self) -> &'static str {
        match self {
            Self::Percentage => "fraction (1 = 100%)",
            Self::XpPen => "XP Pen driver units",
            _ => "tablet report units",
        }
    }

    pub fn notice(self) -> Option<&'static str> {
        match self {
            Self::GaomonV2Otd067 => Some(
                "Matches OTD 0.6.7, including its use of X for the Y offset; the supplied Y is ignored. Choose gaomon-v2-corrected to use Y.",
            ),
            Self::GaomonV2Corrected => Some(
                "Uses Y for the Y offset, correcting the pinned OTD 0.6.7 Gaomon converter formula.",
            ),
            Self::WacomVeikk => Some(
                "Matches OTD 0.6.7: both axes use MaxX / Width, even if tablet resolutions differ.",
            ),
            _ => None,
        }
    }

    pub fn convert(
        self,
        tablet: &crate::tablets::DigitizerSpecifications,
        values: [f64; 4],
    ) -> Result<OtdArea, String> {
        if !values.into_iter().all(f64::is_finite) {
            return Err("all four conversion inputs must be finite numbers".into());
        }
        let [a, b, c, d] = values;
        let (width, height, x, y) = match self {
            Self::Percentage => {
                let width = positive_spec(tablet.width, "Width")?;
                let height = positive_spec(tablet.height, "Height")?;
                let w = (d - b) * width;
                let h = (c - a) * height;
                (w, h, w / 2.0 + b * width, h / 2.0 + a * height)
            }
            Self::WacomVeikk => {
                // The upstream helper divides two float specification properties
                // before widening its return value to double.
                let factor = f64::from(
                    positive_spec(tablet.max_x, "MaxX")? as f32
                        / positive_spec(tablet.width, "Width")? as f32,
                );
                let w = (d - b) / factor;
                let h = (c - a) / factor;
                (w, h, w / 2.0 + b / factor, h / 2.0 + a / factor)
            }
            Self::XpPen => {
                let factor = f64::from(3.937_f32);
                let w = a / factor;
                let h = b / factor;
                (w, h, w / 2.0 + c / factor, h / 2.0 + d / factor)
            }
            Self::GaomonV2Otd067 | Self::GaomonV2Corrected => {
                let width = positive_spec(tablet.width, "Width")?;
                let height = positive_spec(tablet.height, "Height")?;
                let max_x = positive_spec(tablet.max_x, "MaxX")?;
                let max_y = positive_spec(tablet.max_y, "MaxY")?;
                let w = (a / max_x) * width;
                let h = (b / max_y) * height;
                let offset_y = if self == Self::GaomonV2Otd067 { c } else { d };
                (
                    w,
                    h,
                    (c / max_x) * width + w / 2.0,
                    (offset_y / max_y) * height + h / 2.0,
                )
            }
        };
        // Upstream Area/Vector2 values are floats, despite double input formulas.
        let area = OtdArea {
            width: f64::from(width as f32),
            height: f64::from(height as f32),
            x: f64::from(x as f32),
            y: f64::from(y as f32),
            rotation: 0.0,
        };
        validate_area(area)?;
        Ok(area)
    }
}

fn positive_spec(value: Option<f64>, name: &str) -> Result<f64, String> {
    value
        // Pinned DigitizerSpecifications exposes these values as CLR Single.
        .map(|value| f64::from(value as f32))
        .filter(|value| value.is_finite() && *value > 0.0)
        .ok_or_else(|| {
            format!("tablet digitizer needs a finite, positive {name} for this conversion")
        })
}

pub fn validate_area(area: OtdArea) -> Result<(), String> {
    if ![area.width, area.height, area.x, area.y, area.rotation]
        .into_iter()
        .all(f64::is_finite)
        || area.width <= 0.0
        || area.height <= 0.0
    {
        Err("area dimensions must be positive and all area values must be finite; check input order and units".into())
    } else {
        Ok(())
    }
}

pub fn full_area(tablet: &crate::tablets::DigitizerSpecifications) -> Result<OtdArea, String> {
    let width = positive_spec(tablet.width, "Width")?;
    let height = positive_spec(tablet.height, "Height")?;
    Ok(OtdArea {
        width,
        height,
        x: width / 2.0,
        y: height / 2.0,
        rotation: 0.0,
    })
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bounds {
    pub left: f64,
    pub top: f64,
    pub right: f64,
    pub bottom: f64,
}

impl Bounds {
    /// The PTH-660's active area.
    pub fn tablet() -> Self {
        Self {
            left: 0.0,
            top: 0.0,
            right: TABLET_WIDTH_MM,
            bottom: TABLET_HEIGHT_MM,
        }
    }

    /// A tablet's active area in millimetres.
    pub fn tablet_for(spec: crate::spec::TabletSpec) -> Self {
        Self {
            left: 0.0,
            top: 0.0,
            right: spec.width_mm,
            bottom: spec.height_mm,
        }
    }

    pub fn from_rect(rect: Rect) -> Self {
        Self {
            left: f64::from(rect.left),
            top: f64::from(rect.top),
            right: f64::from(rect.right),
            bottom: f64::from(rect.bottom),
        }
    }

    pub fn width(self) -> f64 {
        self.right - self.left
    }

    pub fn height(self) -> f64 {
        self.bottom - self.top
    }

    pub fn center(self) -> (f64, f64) {
        (
            (self.left + self.right) / 2.0,
            (self.top + self.bottom) / 2.0,
        )
    }

    pub fn valid(self) -> bool {
        self.width() > 0.0 && self.height() > 0.0
    }
}

/// Half extents of an area's bounding box after rotation about its center.
pub fn rotated_extent(area: &OtdArea) -> (f64, f64) {
    let (sin, cos) = area.rotation.to_radians().sin_cos();
    let (half_width, half_height) = (area.width / 2.0, area.height / 2.0);
    (
        half_width * cos.abs() + half_height * sin.abs(),
        half_width * sin.abs() + half_height * cos.abs(),
    )
}

/// OpenTabletDriver's "Lock to usable area": shrink unrotated areas to the
/// bounds, then move the rotated area back inside them.
pub fn constrain(area: &mut OtdArea, bounds: Bounds) {
    if !(area.width > 0.0 && area.height > 0.0) || !bounds.valid() {
        return;
    }
    if area.rotation.rem_euclid(180.0) == 0.0 {
        area.width = area.width.min(bounds.width());
        area.height = area.height.min(bounds.height());
    }
    let (extent_x, extent_y) = rotated_extent(area);
    area.x = clamp_center(area.x, extent_x, bounds.left, bounds.right);
    area.y = clamp_center(area.y, extent_y, bounds.top, bounds.bottom);
}

fn clamp_center(center: f64, extent: f64, low: f64, high: f64) -> f64 {
    if 2.0 * extent >= high - low {
        (low + high) / 2.0
    } else {
        center.clamp(low + extent, high - extent)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Left,
    Right,
    Top,
    Bottom,
    Center,
}

pub fn align(area: &mut OtdArea, bounds: Bounds, align: Align) {
    let (extent_x, extent_y) = rotated_extent(area);
    match align {
        Align::Left => area.x = bounds.left + extent_x,
        Align::Right => area.x = bounds.right - extent_x,
        Align::Top => area.y = bounds.top + extent_y,
        Align::Bottom => area.y = bounds.bottom - extent_y,
        Align::Center => (area.x, area.y) = bounds.center(),
    }
}

pub fn flip_horizontal(area: &mut OtdArea, bounds: Bounds) {
    area.x = bounds.left + bounds.right - area.x;
}

pub fn flip_vertical(area: &mut OtdArea, bounds: Bounds) {
    area.y = bounds.top + bounds.bottom - area.y;
}

/// Rotates the area by 180 degrees for the other hand, as upstream does.
pub fn flip_handedness(area: &mut OtdArea, bounds: Bounds) {
    area.rotation = (area.rotation + 180.0).rem_euclid(360.0);
    flip_horizontal(area, bounds);
    flip_vertical(area, bounds);
}

/// Largest size with the given width/height ratio that fits the bounds.
pub fn fit_aspect(bounds: Bounds, ratio: f64) -> (f64, f64) {
    if ratio <= 0.0 || !ratio.is_finite() {
        return (bounds.width(), bounds.height());
    }
    if bounds.width() / bounds.height() > ratio {
        (bounds.height() * ratio, bounds.height())
    } else {
        (bounds.width(), bounds.width() / ratio)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AspectSource {
    TabletWidth,
    TabletHeight,
    DisplayWidth,
    DisplayHeight,
    /// A completed Full/Quarter/Set to display operation, with both sizes set.
    DisplayArea,
}

/// OpenTabletDriver's "Lock aspect ratio": the tablet area follows the
/// display area's shape.
pub fn lock_aspect(mapping: &mut OtdMapping, source: AspectSource, bounds: Bounds) {
    let display = mapping.display;
    if !(display.width > 0.0 && display.height > 0.0) || !bounds.valid() {
        return;
    }
    let ratio = display.width / display.height;
    let tablet = &mut mapping.tablet;
    // RC #5101 derives the other dimension from the current ratio, rather
    // than scaling by the previous display size. Clamp and recompute together
    // so the subsequent usable-area constraint cannot break the ratio.
    let height_first = matches!(
        source,
        AspectSource::TabletWidth | AspectSource::DisplayHeight
    ) || matches!(source, AspectSource::DisplayArea) && ratio <= 1.0;
    if height_first {
        tablet.height = tablet.width / ratio;
        if tablet.height > bounds.height() {
            tablet.height = bounds.height();
            tablet.width = tablet.height * ratio;
        }
    } else {
        tablet.width = tablet.height * ratio;
        if tablet.width > bounds.width() {
            tablet.width = bounds.width();
            tablet.height = tablet.width / ratio;
        }
    }
}

/// RC #5102's largest freely sized rectangle at this rotation. The result
/// describes the area before rotation, in the tablet's millimetres.
pub fn fit_rotated(bounds: Bounds, rotation: f64) -> (f64, f64) {
    let (width, height) = (bounds.width(), bounds.height());
    if !bounds.valid() || !rotation.is_finite() {
        return (0.0, 0.0);
    }
    let (sin, cos) = rotation.to_radians().sin_cos();
    let (sin, cos) = (sin.abs(), cos.abs());
    let width_is_longer = width >= height;
    let (long, short) = if width_is_longer {
        (width, height)
    } else {
        (height, width)
    };
    if short <= 2.0 * sin * cos * long || (sin - cos).abs() < 1e-10 {
        let half = short / 2.0;
        if width_is_longer {
            (half / sin, half / cos)
        } else {
            (half / cos, half / sin)
        }
    } else {
        let cos_two = cos * cos - sin * sin;
        (
            (width * cos - height * sin) / cos_two,
            (height * cos - width * sin) / cos_two,
        )
    }
}

/// Largest rectangle with a fixed width/height ratio that fits after rotation.
/// These two linear bounds are the same edge intersections used by RC #5102,
/// without infinite slopes at 0/90/180/270 degrees.
pub fn fit_rotated_aspect(bounds: Bounds, rotation: f64, ratio: f64) -> (f64, f64) {
    if !bounds.valid() || !rotation.is_finite() || !ratio.is_finite() || ratio <= 0.0 {
        return (0.0, 0.0);
    }
    let (sin, cos) = rotation.to_radians().sin_cos();
    let (sin, cos) = (sin.abs(), cos.abs());
    let height = (bounds.width() / (ratio * cos + sin)).min(bounds.height() / (ratio * sin + cos));
    let width = height * ratio;
    // Upstream rounds whichever dimension is closer to a whole millimetre
    // to four decimal places, then derives the other to preserve the ratio.
    if (width - width.round()).abs() <= (height - height.round()).abs() {
        let width = (width * 10_000.0).round_ties_even() / 10_000.0;
        (width, width / ratio)
    } else {
        let height = (height * 10_000.0).round_ties_even() / 10_000.0;
        (height * ratio, height)
    }
}

#[cfg(test)]
mod rc_area_tests {
    use super::*;

    fn mapping() -> OtdMapping {
        OtdMapping {
            display: OtdArea {
                width: 1920.0,
                height: 1080.0,
                x: 960.0,
                y: 540.0,
                rotation: 0.0,
            },
            tablet: OtdArea {
                width: 200.0,
                height: 112.5,
                x: 112.0,
                y: 74.0,
                rotation: 0.0,
            },
            clipping: true,
            limiting: false,
        }
    }

    #[test]
    fn changing_display_width_keeps_ratio_when_tablet_hits_its_width_limit() {
        let mut mapping = mapping();
        mapping.display.width = 3840.0;
        lock_aspect(&mut mapping, AspectSource::DisplayWidth, Bounds::tablet());
        constrain(&mut mapping.tablet, Bounds::tablet());
        assert_eq!((mapping.tablet.width, mapping.tablet.height), (224.0, 63.0));
    }

    #[test]
    fn complete_display_resize_uses_final_ratio_and_preserves_existing_scale() {
        let mut mapping = mapping();
        mapping.display.width /= 2.0;
        mapping.display.height /= 2.0;
        lock_aspect(&mut mapping, AspectSource::DisplayArea, Bounds::tablet());
        assert_eq!(
            (mapping.tablet.width, mapping.tablet.height),
            (200.0, 112.5)
        );
        mapping.display.width = 1080.0;
        mapping.display.height = 1920.0;
        lock_aspect(&mut mapping, AspectSource::DisplayArea, Bounds::tablet());
        assert_eq!(mapping.tablet.height, 148.0);
        assert_eq!(mapping.tablet.width, 83.25);
    }

    #[test]
    fn rotated_full_area_and_extreme_portrait_ratios_fit_inside_tablet() {
        let bounds = Bounds::tablet();
        assert_eq!(fit_rotated_aspect(bounds, 90.0, 16.0 / 9.0), (148.0, 83.25));
        for rotation in [0.0, 13.0, 45.0, 89.0, 90.0, 180.0, 270.0, -23.0] {
            for ratio in [0.05, 9.0 / 16.0, 1.0, 16.0 / 9.0, 20.0] {
                let (width, height) = fit_rotated_aspect(bounds, rotation, ratio);
                assert!((width / height - ratio).abs() < 1e-10);
                let area = OtdArea {
                    width,
                    height,
                    rotation,
                    x: 112.0,
                    y: 74.0,
                };
                let (x, y) = rotated_extent(&area);
                // RC rounds one dimension to 0.0001 mm; allow that precision.
                assert!(
                    2.0 * x <= bounds.width() + 0.001,
                    "{rotation} degrees, ratio {ratio}"
                );
                assert!(
                    2.0 * y <= bounds.height() + 0.001,
                    "{rotation} degrees, ratio {ratio}"
                );
            }
            let (width, height) = fit_rotated(bounds, rotation);
            let area = OtdArea {
                width,
                height,
                rotation,
                x: 112.0,
                y: 74.0,
            };
            let (x, y) = rotated_extent(&area);
            assert!(width > 0.0 && height > 0.0);
            assert!(2.0 * x <= bounds.width() + 1e-9 && 2.0 * y <= bounds.height() + 1e-9);
        }
    }
}
