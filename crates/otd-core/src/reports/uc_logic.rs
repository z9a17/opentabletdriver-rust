//! Stateless UCLogic base/tilt generations and Huion tilt report dispatch.
//!
//! Current catalog parser updates: a126f7b241e417399be6c6a760c0a9d4b987ecfd.
//! Older unchanged fields retain the 0.6.7 reference below.
//!
//! Pinned source: OpenTabletDriver 0.6.7, commit
//! 736003ed72c8bbb28033b039d5a0bb76c344145c,
//! OpenTabletDriver.Configurations/Parsers/UCLogic/
//! {UCLogicReportParser,UCLogicTiltReportParser,UCLogicV1ReportParser,
//! UCLogicV2ReportParser,UCLogicAuxReport}.cs,
//! OpenTabletDriver.Configurations/Parsers/Huion/
//! {HuionTiltReportParser,InspiroyAuxReport,HuionWheelReport}.cs and
//! OpenTabletDriver.Plugin/Tablet/{TabletReport,TiltTabletReport,OutOfRangeReport}.cs.
//! https://github.com/OpenTabletDriver/OpenTabletDriver/tree/736003ed72c8bbb28033b039d5a0bb76c344145c
//!
//! Dispatch uses byte 1, without a report-ID whitelist: this is how all five
//! upstream parsers behave. Range loss is an explicit ReportKind, not invented
//! zero pressure or proximity fields. Raw-only reports remain ordinary Data
//! with no decoded capabilities. All reports borrow the entire input packet.
//! No device selection, initialization, binding, or output is enabled here.

use super::{
    AbsoluteAnalog, AbsoluteAnalogReport, AnalogKind, Buttons, ReportEnvelope, ReportError,
    ReportKind, ReportMetadata, ReportValues, WheelButtons, RelativeAnalog, RelativeAnalogReport,
};

/// Decode `OpenTabletDriver.Configurations.Parsers.UCLogic.UCLogicReportParser`.
/// Byte 1 equal to 0xC0 is OutOfRange; otherwise bit 6 chooses auxiliary over
/// standard pen. The pen layout has no proximity, eraser or tilt interface.
pub fn parse_uc_logic(
    raw: &[u8],
    metadata: ReportMetadata,
) -> Result<(ReportKind, ReportEnvelope<'_>), ReportError> {
    parse(raw, metadata, Parser::Base)
}

/// Decode `OpenTabletDriver.Configurations.Parsers.UCLogic.UCLogicTiltReportParser`.
/// Byte 1 bit 6 selects auxiliary, including 0xC0. Pen tilt negates Y only.
pub fn parse_uc_logic_tilt(
    raw: &[u8],
    metadata: ReportMetadata,
) -> Result<(ReportKind, ReportEnvelope<'_>), ReportError> {
    parse(raw, metadata, Parser::Tilt)
}

/// Decode `OpenTabletDriver.Configurations.Parsers.UCLogic.UCLogicV1ReportParser`.
/// Byte 1 equal to 0xE0 selects auxiliary; other values with bit 6 set select
/// standard pen. All remaining values mean OutOfRange, not raw-only Data.
pub fn parse_uc_logic_v1(
    raw: &[u8],
    metadata: ReportMetadata,
) -> Result<(ReportKind, ReportEnvelope<'_>), ReportError> {
    parse(raw, metadata, Parser::V1)
}

/// Decode `OpenTabletDriver.Configurations.Parsers.UCLogic.UCLogicV2ReportParser`.
/// Byte 1 equal to 0xE0 selects auxiliary and 0xF0 stays raw-only: upstream
/// explicitly leaves that wheel format undecoded. Other values select pen
/// tilt with neither axis inverted, including values other parsers reject.
pub fn parse_uc_logic_v2(
    raw: &[u8],
    metadata: ReportMetadata,
) -> Result<(ReportKind, ReportEnvelope<'_>), ReportError> {
    parse(raw, metadata, Parser::V2)
}

/// Decode `OpenTabletDriver.Configurations.Parsers.Huion.HuionTiltReportParser`.
/// Byte 1 equal to 0xE0 selects 19 auxiliary buttons plus a wheel button;
/// 0xF0 selects a nullable absolute wheel. Other values select uninverted tilt.
pub fn parse_huion_tilt(
    raw: &[u8],
    metadata: ReportMetadata,
) -> Result<(ReportKind, ReportEnvelope<'_>), ReportError> {
    parse(raw, metadata, Parser::HuionTilt)
}

enum Parser {
    Base,
    Tilt,
    V1,
    V2,
    HuionTilt,
}

enum Layout {
    Pen,
    Tilt { invert_y: bool },
    Auxiliary,
    HuionAuxiliary,
    Wheel,
    RelativeWheel,
    Raw,
    OutOfRange,
}

fn parse(
    raw: &[u8],
    metadata: ReportMetadata,
    parser: Parser,
) -> Result<(ReportKind, ReportEnvelope<'_>), ReportError> {
    require_length(raw, 2)?;
    let flags = raw[1];
    let layout = match parser {
        Parser::Base => {
            if flags == 0xc0 {
                Layout::OutOfRange
            } else if flags & 0x40 != 0 {
                Layout::Auxiliary
            } else {
                Layout::Pen
            }
        }
        Parser::Tilt => {
            if flags & 0x40 != 0 {
                Layout::Auxiliary
            } else {
                Layout::Tilt { invert_y: true }
            }
        }
        Parser::V1 => {
            if flags == 0xe0 { require_length(raw, 4)?; }
            if flags == 0xe0 && raw[3] == 1 {
                Layout::Auxiliary
            } else if flags == 0xe0 && raw[3] == 0x10 {
                Layout::RelativeWheel
            } else if flags & 0x40 != 0 {
                Layout::Pen
            } else {
                Layout::OutOfRange
            }
        }
        Parser::V2 => match flags {
            0xe0 => Layout::Auxiliary,
            0xf0 => Layout::Raw,
            _ => Layout::Tilt { invert_y: false },
        },
        Parser::HuionTilt => match flags {
            0xe0 => Layout::HuionAuxiliary,
            0xf0 => Layout::Wheel,
            _ => Layout::Tilt { invert_y: false },
        },
    };
    let kind = if matches!(layout, Layout::OutOfRange) {
        ReportKind::OutOfRange
    } else {
        ReportKind::Data
    };
    let values = match layout {
        Layout::Pen => pen(raw)?,
        Layout::Tilt { invert_y } => tilt_pen(raw, invert_y)?,
        Layout::Auxiliary => auxiliary(raw, false)?,
        Layout::HuionAuxiliary => auxiliary(raw, true)?,
        Layout::Wheel => wheel(raw)?,
        Layout::RelativeWheel => {
            require_length(raw, 5)?;
            ReportValues {
                relative_analog: Some(RelativeAnalogReport {
                    kind: AnalogKind::Wheel,
                    deltas: RelativeAnalog::from_slice(&[i32::from(raw[4] as i8)])?,
                }),
                ..ReportValues::default()
            }
        },
        Layout::Raw | Layout::OutOfRange => ReportValues::default(),
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

fn pen(raw: &[u8]) -> Result<ReportValues, ReportError> {
    require_length(raw, 8)?;
    Ok(ReportValues {
        position: Some([
            f32::from(u16::from_le_bytes([raw[2], raw[3]])),
            f32::from(u16::from_le_bytes([raw[4], raw[5]])),
        ]),
        pressure: Some(u32::from(u16::from_le_bytes([raw[6], raw[7]]))),
        pen_buttons: Some(Buttons::from_bits(u64::from(raw[1] >> 1), 3)?),
        ..ReportValues::default()
    })
}

fn tilt_pen(raw: &[u8], invert_y: bool) -> Result<ReportValues, ReportError> {
    require_length(raw, 12)?;
    let mut values = pen(raw)?;
    // Promote before negation: inverted -128 is +128 upstream, not overflow.
    // Integer negation also preserves the source's positive zero.
    let y = i16::from(raw[11] as i8);
    values.tilt = Some([
        f32::from(raw[10] as i8),
        f32::from(if invert_y { -y } else { y }),
    ]);
    Ok(values)
}

fn auxiliary(raw: &[u8], has_wheel_button: bool) -> Result<ReportValues, ReportError> {
    require_length(raw, 7)?;
    let bits = u64::from(raw[4]) | (u64::from(raw[5]) << 8) | (u64::from(raw[6] & 0x0f) << 16);
    let (bits, count, wheel_buttons) = if has_wheel_button {
        // InspiroyAuxReport removes bit 4 of byte 5 from the 20-button array,
        // closing the gap, and exposes it as the sole wheel button instead.
        let buttons = WheelButtons::from_slice(&[Buttons::from_bits(u64::from(raw[5] >> 4), 1)?])?;
        ((bits & 0x0fff) | ((bits >> 13) << 12), 19, Some(buttons))
    } else {
        (bits, 20, None)
    };
    Ok(ReportValues {
        aux_buttons: Some(Buttons::from_bits(bits, count)?),
        wheel_buttons,
        ..ReportValues::default()
    })
}

fn wheel(raw: &[u8]) -> Result<ReportValues, ReportError> {
    require_length(raw, 6)?;
    // Zero means no wheel reading; positive values are one-based upstream.
    let position = raw[5].checked_sub(1).map(u32::from);
    Ok(ReportValues {
        absolute_analog: Some(AbsoluteAnalogReport {
            kind: AnalogKind::Wheel,
            positions: AbsoluteAnalog::from_slice(&[position])?,
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
