//! Checked decoding of the USB IntuosV2 pen reports used by the PTH-660.

pub const MAX_X: u32 = 44_800;
pub const MAX_Y: u32 = 29_600;
pub const MAX_PRESSURE: u16 = 8_191;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PenReport {
    pub id: u8,
    pub x: u32,
    pub y: u32,
    pub pressure: u16,
    pub proximity: bool,
    pub tip_switch: bool,
    pub eraser: bool,
    pub tilt: [i8; 2],
    pub rotation: Option<i16>,
    pub hover_distance: Option<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParseError {
    Empty,
    Short { id: u8, got: usize, need: usize },
    Position { x: u32, y: u32 },
    Pressure(u16),
}

#[inline]
fn u24(data: &[u8], at: usize) -> u32 {
    u32::from(data[at]) | (u32::from(data[at + 1]) << 8) | (u32::from(data[at + 2]) << 16)
}

#[inline]
fn u16le(data: &[u8], at: usize) -> u16 {
    u16::from(data[at]) | (u16::from(data[at + 1]) << 8)
}

/// Unknown and touch IDs are deliberately ignored; known short reports are errors.
pub fn parse(data: &[u8]) -> Result<Option<PenReport>, ParseError> {
    let Some(&id) = data.first() else {
        return Err(ParseError::Empty);
    };
    let need = match id {
        0x10 => 17,
        0x1e => 13,
        _ => return Ok(None),
    };
    if data.len() < need {
        return Err(ParseError::Short {
            id,
            got: data.len(),
            need,
        });
    }
    let report = if id == 0x10 {
        let flags = data[1];
        PenReport {
            id,
            x: u24(data, 2),
            y: u24(data, 5),
            pressure: u16le(data, 8),
            proximity: flags & (1 << 5) != 0,
            tip_switch: flags & 1 != 0,
            eraser: flags & (1 << 4) != 0,
            tilt: [data[10] as i8, data[11] as i8],
            rotation: Some(i16::from_le_bytes([data[12], data[13]])),
            hover_distance: Some(data[16]),
        }
    } else {
        let flags = data[2];
        PenReport {
            id,
            x: u24(data, 3),
            y: u24(data, 6),
            pressure: u16le(data, 9),
            proximity: data[1] & (1 << 5) != 0,
            tip_switch: flags & 1 != 0,
            eraser: flags & (1 << 4) != 0,
            tilt: [data[11] as i8, data[12] as i8],
            rotation: None,
            hover_distance: None,
        }
    };
    if report.x > MAX_X || report.y > MAX_Y {
        return Err(ParseError::Position {
            x: report.x,
            y: report.y,
        });
    }
    if report.pressure > MAX_PRESSURE {
        return Err(ParseError::Pressure(report.pressure));
    }
    Ok(Some(report))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_pen_report_and_signed_values() {
        let mut data = [0u8; 192];
        data[0] = 0x10;
        data[1] = 0b0011_0110;
        data[2..5].copy_from_slice(&[0x00, 0xaf, 0x00]); // 44800
        data[5..8].copy_from_slice(&[0xa0, 0x73, 0x00]); // 29600
        data[8..10].copy_from_slice(&[0xff, 0x1f]); // 8191
        data[10] = (-12i8) as u8;
        data[11] = 9;
        data[12..14].copy_from_slice(&(-250i16).to_le_bytes());
        data[16] = 7;
        let report = parse(&data).unwrap().unwrap();
        assert_eq!(
            (report.x, report.y, report.pressure),
            (MAX_X, MAX_Y, MAX_PRESSURE)
        );
        assert!(report.eraser && report.proximity);
        assert!(!report.tip_switch);
        assert_eq!(report.tilt, [-12, 9]);
        assert_eq!(report.rotation, Some(-250));
        assert_eq!(report.hover_distance, Some(7));
    }

    #[test]
    fn offset_report_has_different_layout() {
        let mut data = [0u8; 13];
        data[0] = 0x1e;
        data[1] = 0x20;
        data[2] = 0x02;
        data[3..6].copy_from_slice(&[0x34, 0x12, 0]);
        data[6..9].copy_from_slice(&[0x78, 0x56, 0]);
        data[9..11].copy_from_slice(&[0x05, 0]);
        data[11] = (-3i8) as u8;
        data[12] = 4;
        let report = parse(&data).unwrap().unwrap();
        assert_eq!((report.x, report.y, report.pressure), (0x1234, 0x5678, 5));
        assert_eq!(report.tilt, [-3, 4]);
    }

    #[test]
    fn malformed_and_unwanted_reports_do_not_decode() {
        assert_eq!(parse(&[]), Err(ParseError::Empty));
        assert!(matches!(
            parse(&[0x10, 0x20]),
            Err(ParseError::Short { need: 17, .. })
        ));
        assert_eq!(parse(&[0x21]), Ok(None));
        let mut data = [0u8; 17];
        data[0] = 0x10;
        data[8..10].copy_from_slice(&8192u16.to_le_bytes());
        assert_eq!(parse(&data), Err(ParseError::Pressure(8192)));
        data[8..10].fill(0);
        data[2..5].copy_from_slice(&(MAX_X + 1).to_le_bytes()[..3]);
        assert_eq!(
            parse(&data),
            Err(ParseError::Position { x: MAX_X + 1, y: 0 })
        );
    }

    #[test]
    fn captured_pth660_hover_report() {
        // First 17 bytes of a real 192-byte USB report captured on Windows 11.
        let data = [
            0x10, 0x60, 0x14, 0x56, 0x00, 0xa3, 0x16, 0x00, 0x00, 0x00, 0x07, 0x04, 0x00, 0x00,
            0x00, 0x00, 0x28,
        ];
        let pen = parse(&data).unwrap().unwrap();
        assert_eq!((pen.x, pen.y, pen.pressure), (22_036, 5_795, 0));
        assert!(pen.proximity);
        assert_eq!(pen.tilt, [7, 4]);
        assert_eq!(pen.hover_distance, Some(40));
        assert!(!pen.tip_switch);
    }

    #[test]
    fn captured_contact_lift_and_proximity_loss() {
        // Independent report prefixes from an actual tap/lift capture.
        let contact = [
            0x10, 0x61, 0x11, 0x55, 0x00, 0x8c, 0x12, 0x00, 0x0e, 0x11, 0x00, 0x07, 0x00, 0x00,
            0x00, 0x00, 0x19,
        ];
        let lift = [
            0x10, 0x60, 0x49, 0x54, 0x00, 0xe1, 0x0f, 0x00, 0x00, 0x00, 0xe4, 0x10, 0x00, 0x00,
            0x00, 0x00, 0x14,
        ];
        let out_of_range = [
            0x10, 0x40, 0xb8, 0x51, 0x00, 0x7e, 0x12, 0x00, 0x00, 0x00, 0x00, 0x04, 0x00, 0x00,
            0x00, 0x00, 0x3f,
        ];
        let tap = parse(&contact).unwrap().unwrap();
        let up = parse(&lift).unwrap().unwrap();
        let away = parse(&out_of_range).unwrap().unwrap();
        assert_eq!(tap.pressure, 4_366);
        assert!(tap.tip_switch && tap.proximity);
        assert_eq!(up.pressure, 0);
        assert!(!up.tip_switch && up.proximity);
        assert!(!away.tip_switch && !away.proximity);
    }
}
