//! Complete stateless XP-Pen parser variants from the pinned baseline.
//!
//! Source: OpenTabletDriver 0.6.7, 736003ed72c8bbb28033b039d5a0bb76c344145c,
//! OpenTabletDriver.Configurations/Parsers/XP_Pen/
//! {XP_PenReportParser,XP_PenGen2ReportParser,XP_PenOffsetPressureReportParser,
//! XP_PenOffsetAuxReportParser,XP_PenDedicatedAuxReportParser,XP_PenTabletReport,
//! XP_PenTabletOverflowReport,XP_PenTabletGen2Report,XP_PenPressureOffsetTabletReport,
//! XP_PenPressureOffsetTiltTabletReport,XP_PenTabletPressureOffsetOverflowReport,
//! XP_PenAuxReport}.cs and OpenTabletDriver.Plugin/Tablet/TabletReport.cs.
//! https://github.com/OpenTabletDriver/OpenTabletDriver/tree/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.Configurations/Parsers/XP_Pen
//!
//! Actual packet length is part of upstream dispatch: callers must not pad or
//! trim packets to select a layout. The complete raw slice is borrowed. These
//! parsers inspect byte 1 without an ID whitelist and never manufacture eraser,
//! proximity, or other capabilities absent from the selected source report.
//! The stateful Deco03 parser is separate and is not implemented by this module.
//! No device selection, initialization, bindings or live output is enabled.

use super::{
    AnalogKind, Buttons, RelativeAnalog, RelativeAnalogReport, ReportEnvelope, ReportError,
    ReportKind, ReportMetadata, ReportValues,
};

/// Decode `OpenTabletDriver.Configurations.Parsers.XP_Pen.XP_PenReportParser`.
/// 0xC0 means OutOfRange; bit 4 selects auxiliary. Otherwise lengths >=12,
/// >=10 and >=8 select overflow tilt, ordinary tilt and standard pen layouts.
pub fn parse_xp_pen(
    raw: &[u8],
    metadata: ReportMetadata,
) -> Result<(ReportKind, ReportEnvelope<'_>), ReportError> {
    parse(raw, metadata, Parser::Base)
}

/// Decode `OpenTabletDriver.Configurations.Parsers.XP_Pen.XP_PenGen2ReportParser`.
/// 0xC0 means OutOfRange, 0xF0 is auxiliary and 0xA0..=0xAF selects the
/// fourteen-byte extended pen layout. Remaining flags are raw-only Data.
pub fn parse_xp_pen_gen2(
    raw: &[u8],
    metadata: ReportMetadata,
) -> Result<(ReportKind, ReportEnvelope<'_>), ReportError> {
    parse(raw, metadata, Parser::Gen2)
}

/// Decode `OpenTabletDriver.Configurations.Parsers.XP_Pen.XP_PenOffsetPressureReportParser`.
/// Dispatch matches the base parser, but all pen layouts expose eraser/two pen
/// buttons. Only layouts >=10 mask pressure; the short source report does not.
pub fn parse_xp_pen_offset_pressure(
    raw: &[u8],
    metadata: ReportMetadata,
) -> Result<(ReportKind, ReportEnvelope<'_>), ReportError> {
    parse(raw, metadata, Parser::OffsetPressure)
}

/// Decode `OpenTabletDriver.Configurations.Parsers.XP_Pen.XP_PenOffsetAuxReportParser`.
/// 0xC0 means OutOfRange; bit 5 selects auxiliary buttons starting at byte 4.
/// Pen packets >=10 use ordinary tilt, never overflow coordinates, even >=12.
pub fn parse_xp_pen_offset_auxiliary(
    raw: &[u8],
    metadata: ReportMetadata,
) -> Result<(ReportKind, ReportEnvelope<'_>), ReportError> {
    parse(raw, metadata, Parser::OffsetAuxiliary)
}

/// Decode `OpenTabletDriver.Configurations.Parsers.XP_Pen.XP_PenDedicatedAuxReportParser`.
/// Every packet is auxiliary with buttons starting at byte 1 and wheels at 7;
/// byte 1 equal to 0xC0 is button data here, never an OutOfRange sentinel.
pub fn parse_xp_pen_dedicated_auxiliary(
    raw: &[u8],
    metadata: ReportMetadata,
) -> Result<(ReportKind, ReportEnvelope<'_>), ReportError> {
    Ok((
        ReportKind::Data,
        ReportEnvelope {
            metadata,
            raw,
            values: auxiliary(raw, 1)?,
        },
    ))
}

enum Parser {
    Base,
    Gen2,
    OffsetPressure,
    OffsetAuxiliary,
}

fn parse(
    raw: &[u8],
    metadata: ReportMetadata,
    parser: Parser,
) -> Result<(ReportKind, ReportEnvelope<'_>), ReportError> {
    require_length(raw, 2)?;
    let kind = if raw[1] == 0xc0 {
        ReportKind::OutOfRange
    } else {
        ReportKind::Data
    };
    let values = if kind == ReportKind::OutOfRange {
        ReportValues::default()
    } else {
        match parser {
            Parser::Gen2 => match raw[1] {
                0xf0 => auxiliary(raw, 2)?,
                flags if flags & 0xf0 == 0xa0 => gen2_pen(raw)?,
                _ => ReportValues::default(),
            },
            Parser::OffsetAuxiliary => {
                if raw[1] & 0x20 != 0 {
                    auxiliary(raw, 4)?
                } else {
                    pen(raw, false, false)?
                }
            }
            Parser::Base | Parser::OffsetPressure => {
                if raw[1] & 0x10 != 0 {
                    auxiliary(raw, 2)?
                } else {
                    pen(raw, matches!(parser, Parser::OffsetPressure), true)?
                }
            }
        }
    };
    Ok((
        kind,
        ReportEnvelope {
            metadata,
            raw,
            values,
        },
    ))
}

fn pen(
    raw: &[u8],
    offset_pressure: bool,
    allow_overflow: bool,
) -> Result<ReportValues, ReportError> {
    require_length(raw, 8)?;
    let has_tilt = raw.len() >= 10;
    let has_eraser = has_tilt || offset_pressure;
    let mut x = u32::from(u16::from_le_bytes([raw[2], raw[3]]));
    let mut y = u32::from(u16::from_le_bytes([raw[4], raw[5]]));
    if allow_overflow && raw.len() >= 12 {
        x |= u32::from(raw[10]) << 16;
        y |= u32::from(raw[11]) << 16;
    }
    let mut pressure = u32::from(u16::from_le_bytes([raw[6], raw[7]]));
    if offset_pressure && has_tilt {
        pressure &= 0x1fff;
    }
    Ok(ReportValues {
        position: Some([x as f32, y as f32]),
        pressure: Some(pressure),
        tilt: has_tilt.then(|| [f32::from(raw[8] as i8), f32::from(raw[9] as i8)]),
        eraser: has_eraser.then_some(raw[1] & 0x08 != 0),
        // The generic short TabletReport treats bit 3 as a third button.
        // The XP-Pen-specific reports instead expose that bit as eraser.
        pen_buttons: Some(Buttons::from_bits(
            u64::from(raw[1] >> 1),
            if has_eraser { 2 } else { 3 },
        )?),
        ..ReportValues::default()
    })
}

fn gen2_pen(raw: &[u8]) -> Result<ReportValues, ReportError> {
    require_length(raw, 14)?;
    let mut values = pen(raw, false, true)?;
    let pressure = u32::from(u16::from_le_bytes([raw[6], raw[7]]));
    // Keep the exact pinned mask and OR: bit 15 survives, bit 14 is cleared,
    // and byte 13 bit 0 is ORed into bit 13 rather than replacing that bit.
    values.pressure = Some((pressure & 0xbfff) | (u32::from(raw[13] & 1) << 13));
    Ok(values)
}

fn auxiliary(raw: &[u8], buttons_at: usize) -> Result<ReportValues, ReportError> {
    // All supported button offsets (1, 2, 4) end before the wheel byte at 7.
    require_length(raw, 8)?;
    let buttons = u64::from(raw[buttons_at])
        | (u64::from(raw[buttons_at + 1]) << 8)
        | (u64::from(raw[buttons_at + 2]) << 16);
    let wheel = raw[7];
    // Clockwise wins if both direction bits are set. Both channels are always
    // present upstream, even if a particular device advertises only one wheel.
    let deltas = [
        if wheel & 1 != 0 {
            1
        } else if wheel & 2 != 0 {
            -1
        } else {
            0
        },
        if wheel & 0x10 != 0 {
            1
        } else if wheel & 0x20 != 0 {
            -1
        } else {
            0
        },
    ];
    Ok(ReportValues {
        aux_buttons: Some(Buttons::from_bits(buttons, 20)?),
        relative_analog: Some(RelativeAnalogReport {
            kind: AnalogKind::Wheel,
            deltas: RelativeAnalog::from_slice(&deltas)?,
        }),
        ..ReportValues::default()
    })
}

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
