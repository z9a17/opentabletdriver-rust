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

/// Bit 0 of the pen flags switches on during contact in real PTH-660 captures.
pub fn frame(report: PenReport) -> Frame {
    if !report.proximity {
        return Frame::NEUTRAL;
    }
    Frame {
        position: Some((report.x, report.y)),
        contact: report.tip_switch,
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
        assert!(!frame(pen).contact);
        pen.pressure = 10;
        pen.tip_switch = true;
        assert!(frame(pen).contact);
        pen.proximity = false;
        assert_eq!(frame(pen), Frame::NEUTRAL);
    }
}
