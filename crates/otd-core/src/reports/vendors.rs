//! Parsers for the smaller vendors in the pinned database: Acepen, Bosto,
//! FlooGoo, Genius, Lifetec, RobotPen, ViewSonic, XENX and XenceLabs.
//!
//! Source: OpenTabletDriver 0.6.7, commit
//! 736003ed72c8bbb28033b039d5a0bb76c344145c,
//! OpenTabletDriver.Configurations/Parsers/{Acepen,Bosto,FlooGoo,Genius,Lifetec,
//! RobotPen,ViewSonic,XENX,XenceLabs}/*.cs.
//! https://github.com/OpenTabletDriver/OpenTabletDriver/tree/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.Configurations/Parsers
//!
//! Upstream throws on a short packet and drops the report; a known layout
//! that is too short is an error here. Tilt keeps upstream's signedness.

use super::{
    Buttons, ReportEnvelope, ReportError, ReportKind, ReportMetadata, ReportValues, parse_tablet,
    xp_pen,
};

type Parsed<'a> = Result<(ReportKind, ReportEnvelope<'a>), ReportError>;

fn require_length(raw: &[u8], need: usize) -> Result<(), ReportError> {
    let Some(&id) = raw.first() else {
        return Err(ReportError::Empty);
    };
    if raw.len() < need {
        return Err(ReportError::Short {
            id,
            got: raw.len(),
            need,
        });
    }
    Ok(())
}

fn bit(value: u8, index: u32) -> bool {
    value & (1 << index) != 0
}

fn u16_at(raw: &[u8], at: usize) -> u32 {
    u32::from(u16::from_le_bytes([raw[at], raw[at + 1]]))
}

fn position(raw: &[u8], x: usize, y: usize) -> Option<[f32; 2]> {
    Some([u16_at(raw, x) as f32, u16_at(raw, y) as f32])
}

/// Buttons from the listed bits of one byte, in list order.
fn buttons(value: u8, bits: &[u32]) -> Result<Buttons, ReportError> {
    let packed = bits
        .iter()
        .enumerate()
        .fold(0u64, |packed, (index, &source)| {
            packed | (u64::from(bit(value, source)) << index)
        });
    Buttons::from_bits(packed, bits.len())
}

fn report(
    raw: &[u8],
    metadata: ReportMetadata,
    kind: ReportKind,
    values: ReportValues,
) -> Parsed<'_> {
    Ok((
        kind,
        ReportEnvelope {
            metadata,
            raw,
            values,
        },
    ))
}

fn data(raw: &[u8], metadata: ReportMetadata, values: ReportValues) -> Parsed<'_> {
    report(raw, metadata, ReportKind::Data, values)
}

fn out_of_range(raw: &[u8], metadata: ReportMetadata) -> Parsed<'_> {
    report(
        raw,
        metadata,
        ReportKind::OutOfRange,
        ReportValues::default(),
    )
}

/// `AcepenReportParser`. Aux packets carry one button each, so the parser
/// keeps all eight button states.
#[derive(Clone, Debug, Default)]
pub struct AcepenParser {
    aux: u8,
}

impl AcepenParser {
    pub fn reset(&mut self) {
        self.aux = 0;
    }

    pub fn parse<'a>(&mut self, raw: &'a [u8], metadata: ReportMetadata) -> Parsed<'a> {
        require_length(raw, 2)?;
        match raw[1] {
            0x41 => {
                require_length(raw, 3)?;
                if raw[2] & 0xf0 != 0xa0 {
                    return data(raw, metadata, ReportValues::default());
                }
                require_length(raw, 11)?;
                data(
                    raw,
                    metadata,
                    ReportValues {
                        position: position(raw, 3, 5),
                        pressure: Some(u16_at(raw, 7)),
                        pen_buttons: Some(buttons(raw[2], &[1, 2])?),
                        // Upstream reads the tilt bytes unsigned.
                        tilt: Some([f32::from(raw[9]), f32::from(raw[10])]),
                        ..ReportValues::default()
                    },
                )
            }
            0x42 => {
                require_length(raw, 5)?;
                // BitOperations.Log2(0) is 0.
                let index = raw[4].checked_ilog2().unwrap_or(0);
                if bit(raw[3], 0) {
                    self.aux |= 1 << index;
                } else {
                    self.aux &= !(1 << index);
                }
                data(
                    raw,
                    metadata,
                    ReportValues {
                        aux_buttons: Some(Buttons::from_bits(u64::from(self.aux), 8)?),
                        ..ReportValues::default()
                    },
                )
            }
            _ => data(raw, metadata, ReportValues::default()),
        }
    }
}

/// `BostoReportParser`.
pub fn parse_bosto(raw: &[u8], metadata: ReportMetadata) -> Parsed<'_> {
    require_length(raw, 2)?;
    if raw[1] == 0 {
        return out_of_range(raw, metadata);
    }
    require_length(raw, 8)?;
    data(
        raw,
        metadata,
        ReportValues {
            position: position(raw, 2, 4),
            pressure: Some(u16_at(raw, 6)),
            pen_buttons: Some(buttons(raw[1], &[5, 1])?),
            ..ReportValues::default()
        },
    )
}

/// `FmaReportParser` (FlooGoo). An empty packet is ignored, as upstream's
/// null report is.
pub fn parse_floogoo(raw: &[u8], metadata: ReportMetadata) -> Parsed<'_> {
    if raw.len() < 12 || raw[0] != 0x01 {
        return data(raw, metadata, ReportValues::default());
    }
    if !bit(raw[1], 5) {
        return out_of_range(raw, metadata);
    }
    let tilt = |at: usize| f32::from(i16::from_le_bytes([raw[at], raw[at + 1]])) * 0.01;
    data(
        raw,
        metadata,
        ReportValues {
            position: position(raw, 2, 4),
            pressure: Some(u16_at(raw, 6)),
            tilt: Some([tilt(8), tilt(10)]),
            pen_buttons: Some(buttons(raw[1], &[1, 2])?),
            eraser: Some(bit(raw[1], 3)),
            ..ReportValues::default()
        },
    )
}

/// `GeniusReportParser`: 0x10 is the generic tablet report, 0x11 the mouse.
pub fn parse_genius(raw: &[u8], metadata: ReportMetadata) -> Parsed<'_> {
    let Some(&id) = raw.first() else {
        return Err(ReportError::Empty);
    };
    match id {
        0x10 => parse_tablet(raw, metadata),
        0x11 => {
            require_length(raw, 7)?;
            data(
                raw,
                metadata,
                ReportValues {
                    position: position(raw, 2, 4),
                    mouse_buttons: Some(buttons(raw[1], &[0, 1, 2])?),
                    mouse_scroll: Some([0.0, f32::from(raw[6] as i8)]),
                    ..ReportValues::default()
                },
            )
        }
        _ => data(raw, metadata, ReportValues::default()),
    }
}

/// `GeniusReportParserV2`: 0x02 pen, 0x05 button strip.
pub fn parse_genius_v2(raw: &[u8], metadata: ReportMetadata) -> Parsed<'_> {
    let Some(&id) = raw.first() else {
        return Err(ReportError::Empty);
    };
    match id {
        0x02 => {
            require_length(raw, 8)?;
            let flags = raw[5];
            data(
                raw,
                metadata,
                ReportValues {
                    position: position(raw, 1, 3),
                    pressure: Some(if bit(flags, 2) { u16_at(raw, 6) } else { 0 }),
                    pen_buttons: Some(buttons(flags, &[3, 4])?),
                    ..ReportValues::default()
                },
            )
        }
        0x05 => {
            require_length(raw, 4)?;
            // C# integer division truncates toward zero, so byte 0 selects button 0.
            let index = (i32::from(raw[3]) - 1) / 2;
            if index >= 12 {
                return Err(ReportError::Index {
                    index: index as usize,
                    length: 12,
                });
            }
            data(
                raw,
                metadata,
                ReportValues {
                    aux_buttons: Some(Buttons::from_bits(1 << index, 12)?),
                    ..ReportValues::default()
                },
            )
        }
        _ => data(raw, metadata, ReportValues::default()),
    }
}

/// `LifetecReportParser`.
pub fn parse_lifetec(raw: &[u8], metadata: ReportMetadata) -> Parsed<'_> {
    if raw.len() < 8 || raw[0] != 0x02 {
        return data(raw, metadata, ReportValues::default());
    }
    data(
        raw,
        metadata,
        ReportValues {
            position: position(raw, 1, 3),
            pressure: Some(u16_at(raw, 6)),
            pen_buttons: Some(buttons(raw[5], &[3, 4])?),
            ..ReportValues::default()
        },
    )
}

/// `RobotPenReportParser`: anything but 0x42 in byte 1 is out of range.
pub fn parse_robot_pen(raw: &[u8], metadata: ReportMetadata) -> Parsed<'_> {
    require_length(raw, 2)?;
    if raw[1] != 0x42 {
        return out_of_range(raw, metadata);
    }
    require_length(raw, 12)?;
    data(
        raw,
        metadata,
        ReportValues {
            position: position(raw, 6, 8),
            pressure: Some(u16_at(raw, 10)),
            pen_buttons: Some(buttons(raw[11], &[1])?),
            ..ReportValues::default()
        },
    )
}

/// `WoodPadReportParser` (ViewSonic).
pub fn parse_wood_pad(raw: &[u8], metadata: ReportMetadata) -> Parsed<'_> {
    require_length(raw, 10)?;
    let flags = raw[9];
    if flags & 0b11 != 0b11 {
        return data(raw, metadata, ReportValues::default());
    }
    require_length(raw, 14)?;
    data(
        raw,
        metadata,
        ReportValues {
            position: position(raw, 1, 5),
            pressure: Some(if bit(flags, 2) { u16_at(raw, 10) } else { 0 }),
            tilt: Some([f32::from(raw[12]), f32::from(raw[13])]),
            pen_buttons: Some(buttons(flags, &[3, 4])?),
            ..ReportValues::default()
        },
    )
}

/// `XENXReportParser`.
pub fn parse_xenx(raw: &[u8], metadata: ReportMetadata) -> Parsed<'_> {
    let Some(&id) = raw.first() else {
        return Err(ReportError::Empty);
    };
    match id {
        0x01 => {
            require_length(raw, 2)?;
            if raw[1] == 0 {
                return out_of_range(raw, metadata);
            }
            require_length(raw, 8)?;
            data(
                raw,
                metadata,
                ReportValues {
                    position: position(raw, 2, 4),
                    pressure: Some(u16_at(raw, 6)),
                    eraser: Some(bit(raw[1], 6)),
                    pen_buttons: Some(buttons(raw[1], &[1, 2])?),
                    ..ReportValues::default()
                },
            )
        }
        0x02 => {
            require_length(raw, 12)?;
            let packed = raw[2..12]
                .iter()
                .enumerate()
                .fold(0u64, |packed, (index, &value)| {
                    packed | (u64::from(value != 0) << index)
                });
            data(
                raw,
                metadata,
                ReportValues {
                    aux_buttons: Some(Buttons::from_bits(packed, 10)?),
                    ..ReportValues::default()
                },
            )
        }
        _ => data(raw, metadata, ReportValues::default()),
    }
}

/// `XenceLabsReportParser`: aux packets use the XP-Pen aux layout.
pub fn parse_xencelabs(raw: &[u8], metadata: ReportMetadata) -> Parsed<'_> {
    require_length(raw, 2)?;
    let flags = raw[1];
    if flags & 0xf0 == 0xf0 {
        return data(raw, metadata, xp_pen::auxiliary(raw, 2)?);
    }
    if !bit(flags, 5) {
        return data(raw, metadata, ReportValues::default());
    }
    require_length(raw, 10)?;
    data(
        raw,
        metadata,
        ReportValues {
            position: position(raw, 2, 4),
            pressure: Some(u16_at(raw, 6)),
            eraser: Some(bit(flags, 6)),
            pen_buttons: Some(buttons(flags, &[1, 2, 3])?),
            tilt: Some([f32::from(raw[8] as i8), f32::from(raw[9] as i8)]),
            ..ReportValues::default()
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reports::{DeviceId, EndpointId, SessionId};
    use std::time::Duration;

    fn metadata() -> ReportMetadata {
        ReportMetadata {
            device: DeviceId(1),
            session: SessionId(1),
            endpoint: EndpointId(0),
            received_at: Duration::ZERO,
            sequence: 0,
        }
    }

    #[test]
    fn pen_layouts() {
        let (_, r) = parse_bosto(&[0, 0x22, 1, 0, 2, 0, 3, 0], metadata()).unwrap();
        assert_eq!(r.values.position, Some([1.0, 2.0]));
        assert_eq!(r.values.pen_buttons.unwrap().bits(), 0b11);
        assert_eq!(
            parse_bosto(&[0, 0], metadata()).unwrap().0,
            ReportKind::OutOfRange
        );
        let fma = [1, 0x28, 1, 0, 2, 0, 3, 0, 0x10, 0x27, 0xf0, 0xd8];
        let (_, r) = parse_floogoo(&fma, metadata()).unwrap();
        assert_eq!(r.values.tilt, Some([100.0, -100.0]));
        assert_eq!(r.values.eraser, Some(true));
        let (_, r) = parse_genius_v2(&[2, 1, 0, 2, 0, 0x04, 9, 0], metadata()).unwrap();
        assert_eq!(r.values.pressure, Some(9));
        let (_, r) = parse_genius_v2(&[5, 0, 0, 0], metadata()).unwrap();
        assert_eq!(r.values.aux_buttons.unwrap().bits(), 1);
        assert!(parse_genius_v2(&[5, 0, 0, 30], metadata()).is_err());
        let (_, r) = parse_robot_pen(&[0, 0x42, 0, 0, 0, 0, 5, 0, 6, 0, 7, 2], metadata()).unwrap();
        assert_eq!(
            (r.values.position, r.values.pressure),
            (Some([5.0, 6.0]), Some(7 | 2 << 8))
        );
        let wood = [0, 1, 0, 0, 0, 2, 0, 0, 0, 0b111, 8, 0, 200, 10];
        let (_, r) = parse_wood_pad(&wood, metadata()).unwrap();
        assert_eq!(r.values.tilt, Some([200.0, 10.0]));
        let (_, r) = parse_xenx(&[1, 0x42, 1, 0, 1, 0, 1, 0], metadata()).unwrap();
        assert_eq!(r.values.eraser, Some(true));
        let (_, r) = parse_xencelabs(&[0, 0x20, 1, 0, 1, 0, 1, 0, 0xff, 2], metadata()).unwrap();
        assert_eq!(r.values.tilt, Some([-1.0, 2.0]));
        let (_, r) = parse_lifetec(&[2, 1, 0, 2, 0, 0x08, 3, 0], metadata()).unwrap();
        assert_eq!(r.values.pen_buttons.unwrap().bits(), 1);
    }

    #[test]
    fn acepen_keeps_aux_state() {
        let mut parser = AcepenParser::default();
        let (_, r) = parser.parse(&[0, 0x42, 0, 1, 0x04], metadata()).unwrap();
        assert_eq!(r.values.aux_buttons.unwrap().bits(), 0b100);
        let (_, r) = parser.parse(&[0, 0x42, 0, 1, 0x01], metadata()).unwrap();
        assert_eq!(r.values.aux_buttons.unwrap().bits(), 0b101);
        let (_, r) = parser.parse(&[0, 0x42, 0, 0, 0x04], metadata()).unwrap();
        assert_eq!(r.values.aux_buttons.unwrap().bits(), 0b001);
        let pen = [0, 0x41, 0xa2, 1, 0, 2, 0, 3, 0, 4, 5];
        let (_, r) = parser.parse(&pen, metadata()).unwrap();
        assert_eq!(r.values.pressure, Some(3));
    }
}
