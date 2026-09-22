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
///
/// Like OpenTabletDriver, which uses the position of every IntuosV2 pen
/// report, the cursor follows the pen through the whole Sense range, not only
/// the lower In Range band. A report with neither bit is not a detected pen.
pub fn frame(report: PenReport, policy: ContactPolicy) -> Frame {
    if !report.in_range && !report.sense {
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
            in_range: true,
            sense: true,
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
        pen.in_range = false;
        pen.sense = false;
        assert_eq!(frame(pen, policy), Frame::NEUTRAL);
    }

    #[test]
    fn sense_without_in_range_still_tracks_the_pen() {
        let mut pen = report();
        pen.in_range = false;
        assert_eq!(
            frame(pen, ContactPolicy::default()),
            Frame {
                position: Some((100, 200)),
                contact: false,
            }
        );
        pen.sense = false;
        pen.in_range = true;
        assert_eq!(
            frame(pen, ContactPolicy::default()).position,
            Some((100, 200))
        );
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
