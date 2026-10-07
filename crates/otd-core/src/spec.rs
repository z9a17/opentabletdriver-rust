//! The physical and raw ranges of the tablet a session drives. Mapping,
//! relative output, Radial Follow and pressure thresholds convert between
//! report units and millimetres with these values; OpenTabletDriver takes
//! them from the matched configuration's `Specifications`.

use crate::protocol::{HEIGHT_MM, MAX_PRESSURE, MAX_X, MAX_Y, WIDTH_MM};
use crate::reports::{MAX_BUTTONS, MAX_WHEELS};
use crate::tablets::{TabletConfiguration, TabletSpecifications};

/// The buttons and wheels a configuration declares, which size its binding
/// lists as upstream's `BindingSettings.MatchSpecifications` does.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Controls {
    pub pen_buttons: u8,
    pub aux_buttons: u8,
    pub mouse_buttons: u8,
    wheels: [Wheel; MAX_WHEELS],
    wheel_count: u8,
}

/// One wheel, ring or dial.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Wheel {
    /// Device steps in a full turn: `AbsoluteWheelMax + 1` for an absolute
    /// wheel, `RelativeWheelSteps` for a relative one (upstream's
    /// `WheelSpecifications.StepCount`). Zero when the configuration has
    /// neither; such a wheel has no rotation bindings.
    pub steps: u32,
    pub buttons: u8,
}

impl Wheel {
    /// Degrees of rotation per device step, or `None` without a step count.
    pub fn degrees_per_step(self) -> Option<f64> {
        (self.steps != 0).then(|| 360.0 / f64::from(self.steps))
    }
}

impl Controls {
    /// Wheels past `MAX_WHEELS` are dropped; no report can carry them.
    pub const fn new(pen_buttons: u8, aux_buttons: u8, wheels: &[Wheel]) -> Self {
        let mut all = [Wheel {
            steps: 0,
            buttons: 0,
        }; MAX_WHEELS];
        let mut index = 0;
        while index < wheels.len() && index < MAX_WHEELS {
            all[index] = wheels[index];
            index += 1;
        }
        Self {
            pen_buttons,
            aux_buttons,
            mouse_buttons: 0,
            wheels: all,
            wheel_count: index as u8,
        }
    }

    pub fn wheels(&self) -> &[Wheel] {
        &self.wheels[..usize::from(self.wheel_count)]
    }

    fn from_specifications(specifications: &TabletSpecifications) -> Self {
        let count = |value: Option<u32>| value.unwrap_or(0).min(MAX_BUTTONS as u32) as u8;
        let wheels: Vec<Wheel> = specifications
            .wheels
            .iter()
            .flatten()
            .take(MAX_WHEELS)
            .map(|wheel| Wheel {
                steps: match (wheel.absolute_wheel_max, wheel.relative_wheel_steps) {
                    (Some(max), None) => max.saturating_add(1),
                    (None, Some(steps)) => steps,
                    _ => 0,
                },
                buttons: count(wheel.button_count),
            })
            .collect();
        let mut controls = Self::new(
            count(specifications.pen.as_ref().and_then(|pen| pen.buttons())),
            count(
                specifications
                    .auxiliary_buttons
                    .as_ref()
                    .and_then(|buttons| buttons.button_count),
            ),
            &wheels,
        );
        controls.mouse_buttons = count(specifications.mouse_buttons.as_ref().and_then(|buttons| buttons.button_count));
        controls
    }
}

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
    /// Declared buttons and wheels.
    pub controls: Controls,
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
        // Two side buttons, eight express keys and the 72-position touch
        // ring with its center button.
        controls: Controls::new(
            2,
            8,
            &[Wheel {
                steps: 72,
                buttons: 1,
            }],
        ),
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
            controls: Controls::from_specifications(specifications),
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
