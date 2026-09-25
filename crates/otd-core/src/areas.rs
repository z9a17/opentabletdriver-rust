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
    pub fn tablet() -> Self {
        Self {
            left: 0.0,
            top: 0.0,
            right: TABLET_WIDTH_MM,
            bottom: TABLET_HEIGHT_MM,
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
    DisplayWidth { previous: f64 },
    DisplayHeight { previous: f64 },
}

/// OpenTabletDriver's "Lock aspect ratio": the tablet area follows the
/// display area's shape.
pub fn lock_aspect(mapping: &mut OtdMapping, source: AspectSource) {
    let display = mapping.display;
    if !(display.width > 0.0 && display.height > 0.0) {
        return;
    }
    let tablet = &mut mapping.tablet;
    match source {
        AspectSource::TabletWidth => tablet.height = display.height / display.width * tablet.width,
        AspectSource::TabletHeight => tablet.width = display.width / display.height * tablet.height,
        AspectSource::DisplayWidth { previous } if previous > 0.0 => {
            tablet.width *= display.width / previous;
        }
        AspectSource::DisplayHeight { previous } if previous > 0.0 => {
            tablet.height *= display.height / previous;
        }
        _ => {}
    }
}
