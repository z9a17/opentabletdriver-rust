use crate::config::ContactPolicy;
use crate::protocol::PenReport;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Frame {
    pub position: Option<(u32, u32)>,
    pub contact: bool,
}

impl Frame {
    pub const NEUTRAL: Self = Self {
        position: None,
        contact: false,
    };
}

/// The built-in profile uses the captured tip flag; OpenTabletDriver profiles
/// use their pressure activation threshold and enabled tip/eraser binding.
pub fn frame(report: PenReport, policy: ContactPolicy) -> Frame {
    if !report.proximity {
        return Frame::NEUTRAL;
    }
    Frame {
        position: Some((report.x, report.y)),
        contact: if report.eraser {
            policy.eraser_enabled
                && policy
                    .eraser_threshold_raw
                    .map_or(report.tip_switch, |threshold| report.pressure >= threshold)
        } else {
            policy.tip_enabled
                && policy
                    .tip_threshold_raw
                    .map_or(report.tip_switch, |threshold| report.pressure >= threshold)
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report() -> PenReport {
        PenReport {
            id: 0x10,
            x: 100,
            y: 200,
            pressure: 0,
            proximity: true,
            tip_switch: false,
            eraser: false,
            tilt: [0; 2],
            rotation: Some(0),
            hover_distance: Some(0),
        }
    }

    #[test]
    fn hover_contact_and_proximity_loss() {
        let mut pen = report();
        let policy = ContactPolicy::default();
        assert!(!frame(pen, policy).contact);
        pen.pressure = 10;
        pen.tip_switch = true;
        assert!(frame(pen, policy).contact);
        pen.proximity = false;
        assert_eq!(frame(pen, policy), Frame::NEUTRAL);
    }

    #[test]
    fn open_tablet_driver_threshold_controls_tip_contact() {
        let mut pen = report();
        let policy = ContactPolicy {
            tip_threshold_raw: Some(83),
            ..ContactPolicy::default()
        };
        pen.tip_switch = true;
        pen.pressure = 82;
        assert!(!frame(pen, policy).contact);
        pen.pressure = 83;
        assert!(frame(pen, policy).contact);
    }
}
