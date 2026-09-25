//! The physical and raw ranges of the tablet a session drives. Mapping,
//! relative output, Radial Follow and pressure thresholds convert between
//! report units and millimetres with these values; OpenTabletDriver takes
//! them from the matched configuration's `Specifications`.

use crate::protocol::{HEIGHT_MM, MAX_PRESSURE, MAX_X, MAX_Y, WIDTH_MM};
use crate::tablets::TabletConfiguration;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TabletSpec {
    /// Largest raw X and Y coordinate the digitizer reports.
    pub max_x: u32,
    pub max_y: u32,
    /// Largest raw pressure value.
    pub max_pressure: u16,
    /// Active area in millimetres.
    pub width_mm: f64,
    pub height_mm: f64,
}

impl Default for TabletSpec {
    fn default() -> Self {
        Self::PTH_660
    }
}

impl TabletSpec {
    /// The Wacom PTH-660, which the report path was first built for.
    pub const PTH_660: Self = Self {
        max_x: MAX_X,
        max_y: MAX_Y,
        max_pressure: MAX_PRESSURE,
        width_mm: WIDTH_MM,
        height_mm: HEIGHT_MM,
    };

    /// The digitizer and pen specifications of a configuration.
    pub fn from_configuration(configuration: &TabletConfiguration) -> Result<Self, String> {
        let name = &configuration.name;
        let specifications = configuration
            .specifications
            .as_ref()
            .ok_or_else(|| format!("{name} has no specifications"))?;
        let digitizer = specifications
            .digitizer
            .as_ref()
            .ok_or_else(|| format!("{name} has no digitizer specification"))?;
        let pen = specifications
            .pen
            .as_ref()
            .ok_or_else(|| format!("{name} has no pen specification"))?;
        let positive = |value: Option<f64>, field: &str| {
            value
                .filter(|value| value.is_finite() && *value > 0.0)
                .ok_or_else(|| format!("{name} has no positive digitizer {field}"))
        };
        let whole = |value: f64, field: &str| {
            if value.fract() == 0.0 && value <= f64::from(u32::MAX) {
                Ok(value as u32)
            } else {
                Err(format!(
                    "{name} digitizer {field} {value} is not a whole report unit"
                ))
            }
        };
        let max_pressure = pen
            .max_pressure
            .ok_or_else(|| format!("{name} has no pen MaxPressure"))?;
        Ok(Self {
            max_x: whole(positive(digitizer.max_x, "MaxX")?, "MaxX")?,
            max_y: whole(positive(digitizer.max_y, "MaxY")?, "MaxY")?,
            max_pressure: u16::try_from(max_pressure)
                .map_err(|_| format!("{name} pen MaxPressure {max_pressure} exceeds 65535"))?,
            width_mm: positive(digitizer.width, "Width")?,
            height_mm: positive(digitizer.height, "Height")?,
        })
    }

    /// Millimetres per raw unit on each axis.
    pub fn mm_per_unit(self) -> (f64, f64) {
        (
            self.width_mm / f64::from(self.max_x),
            self.height_mm / f64::from(self.max_y),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tablets::Database;

    #[test]
    fn the_pth_660_configuration_matches_the_built_in_spec() {
        let pth = Database::builtin()
            .entries()
            .iter()
            .filter_map(|entry| entry.usable())
            .find(|configuration| configuration.name == "Wacom PTH-660")
            .unwrap();
        assert_eq!(
            TabletSpec::from_configuration(pth).unwrap(),
            TabletSpec::PTH_660
        );
    }

    #[test]
    fn every_pinned_configuration_with_a_pen_has_a_spec() {
        let mut failures = Vec::new();
        for configuration in Database::builtin()
            .entries()
            .iter()
            .filter_map(|e| e.usable())
        {
            if let Err(error) = TabletSpec::from_configuration(configuration) {
                failures.push(error);
            }
        }
        assert!(failures.is_empty(), "{failures:#?}");
    }
}
