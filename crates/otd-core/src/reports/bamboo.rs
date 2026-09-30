//! Complete stateless Bamboo, BambooPad and BambooV2 auxiliary dispatch.
//!
//! Current catalog parser updates: a126f7b241e417399be6c6a760c0a9d4b987ecfd.
//! Older unchanged fields retain the 0.6.7 reference below.
//!
//! Pinned source: OpenTabletDriver 0.6.7, commit
//! 736003ed72c8bbb28033b039d5a0bb76c344145c,
//! OpenTabletDriver.Configurations/Parsers/Wacom/
//! Bamboo/{BambooReportParser,BambooTabletReport,BambooMouseReport,BambooAuxReport}.cs,
//! BambooPad/{BambooPadReportParser,BambooPadTabletReport,BambooPadAuxReport}.cs,
//! BambooV2/BambooV2AuxReportParser.cs and IntuosV2/IntuosV2AuxReport.cs.
//! https://github.com/OpenTabletDriver/OpenTabletDriver/tree/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.Configurations/Parsers/Wacom
//!
//! Unknown reports retain their raw data with no decoded capabilities, as in
//! upstream DeviceReport. None of these parsers emits OutOfRangeReport. These
//! functions do not select devices, interpret bindings, or enable live output.

use super::{
    AbsoluteAnalog, AbsoluteAnalogReport, AnalogKind, Buttons, ReportEnvelope, ReportError,
    ReportMetadata, ReportValues, WheelButtons,
};

/// Decode `OpenTabletDriver.Configurations.Parsers.Wacom.Bamboo.BambooReportParser`.
/// ID 0x02 needs eight bytes for all three dispatched layouts; other IDs are
/// raw-only reports. Pen and mouse packets also carry the auxiliary buttons.
pub fn parse_bamboo(
    raw: &[u8],
    metadata: ReportMetadata,
) -> Result<ReportEnvelope<'_>, ReportError> {
    require_length(raw, 1)?;
    let values = if raw[0] == 0x02 {
        require_length(raw, 8)?;
        bamboo_tool(raw)?
    } else {
        ReportValues::default()
    };
    Ok(ReportEnvelope {
        metadata,
        raw,
        values,
    })
}

fn bamboo_wheel(raw: &[u8]) -> Result<AbsoluteAnalogReport, ReportError> {
    require_length(raw, 9)?;
    Ok(AbsoluteAnalogReport {
        kind: AnalogKind::Wheel,
        positions: AbsoluteAnalog::from_slice(&[(raw[8] & 0x80 != 0).then_some(u32::from(raw[8] & 0x7f))])?,
    })
}

fn bamboo_tool(raw: &[u8]) -> Result<ReportValues, ReportError> {
    let pressure = u32::from(raw[6]) | (u32::from(raw[7] & 0x03) << 8);
    let mut values = ReportValues {
        aux_buttons: Some(Buttons::from_bits(u64::from(raw[7] >> 3), 4)?),
        ..ReportValues::default()
    };
    // Upstream's dispatch checks raw position/pressure even outside proximity,
    // and before its pen tip flag gates the decoded pressure. Do not simplify
    // this to proximity alone or use the eventual gated pressure here.
    let has_position =
        raw[1] & 0x80 != 0 || raw[2..6].iter().any(|&byte| byte != 0) || pressure != 0;
    if !has_position {
        values.absolute_analog = Some(bamboo_wheel(raw)?);
        return Ok(values);
    }
    values.position = Some([
        f32::from(u16::from_le_bytes([raw[2], raw[3]])),
        f32::from(u16::from_le_bytes([raw[4], raw[5]])),
    ]);
    if raw[1] & 0x40 != 0 {
        let scroll = i16::from(raw[7] & 1);
        values.mouse_scroll = Some([
            0.0,
            f32::from(if raw[7] & 2 != 0 { -scroll } else { scroll }),
        ]);
        values.mouse_buttons = Some(Buttons::from_bits(u64::from(raw[1]), 3)?);
        // BambooMouseReport exposes neither pressure nor proximity, even on
        // Graphire WACOM_MO devices that might encode a mouse hover distance.
    } else {
        values.absolute_analog = Some(bamboo_wheel(raw)?);
        values.pressure = Some(if raw[1] & 1 != 0 { pressure } else { 0 });
        values.eraser = Some(raw[1] & 0x20 != 0);
        values.pen_buttons = Some(Buttons::from_bits(u64::from(raw[1] >> 1), 2)?);
        values.near_proximity = Some(raw[1] & 0x80 != 0);
        // This interface is present with a constant zero in the pinned parser.
        values.hover_distance = Some(0);
    }
    Ok(values)
}

/// Decode `OpenTabletDriver.Configurations.Parsers.Wacom.BambooPad.BambooPadReportParser`.
/// ID 0x10 subtype 1 is a nine-byte pen layout; subtype 6 is a 24-byte auxiliary
/// layout. Other subtypes and IDs are raw-only. No touch decoding is inferred.
pub fn parse_bamboo_pad(
    raw: &[u8],
    metadata: ReportMetadata,
) -> Result<ReportEnvelope<'_>, ReportError> {
    require_length(raw, 1)?;
    let values = if raw[0] == 0x10 {
        require_length(raw, 2)?;
        match raw[1] {
            0x01 => {
                require_length(raw, 9)?;
                ReportValues {
                    position: Some([
                        f32::from(u16::from_le_bytes([raw[3], raw[4]])),
                        f32::from(u16::from_le_bytes([raw[5], raw[6]])),
                    ]),
                    pressure: Some(u32::from(u16::from_le_bytes([raw[7], raw[8]]))),
                    pen_buttons: Some(Buttons::from_bits(u64::from(raw[2] >> 1), 1)?),
                    eraser: Some(raw[2] & 0x08 != 0),
                    ..ReportValues::default()
                }
            }
            0x06 => {
                require_length(raw, 24)?;
                // These are equality tests, not independent bits. Value 3
                // means both buttons released in the upstream constructor.
                let bits = u64::from(raw[23] == 1) | (u64::from(raw[23] == 2) << 1);
                ReportValues {
                    aux_buttons: Some(Buttons::from_bits(bits, 2)?),
                    ..ReportValues::default()
                }
            }
            _ => ReportValues::default(),
        }
    } else {
        ReportValues::default()
    };
    Ok(ReportEnvelope {
        metadata,
        raw,
        values,
    })
}

/// Decode `OpenTabletDriver.Configurations.Parsers.Wacom.BambooV2.BambooV2AuxReportParser`.
/// Its ID 0x02 uses IntuosV2AuxReport's five-byte layout, including all eight
/// auxiliary buttons, the wheel button, and a nullable absolute wheel position.
/// The original ID/raw bytes are retained; no prefix or ID rewrite is applied.
pub fn parse_bamboo_v2_auxiliary(
    raw: &[u8],
    metadata: ReportMetadata,
) -> Result<ReportEnvelope<'_>, ReportError> {
    require_length(raw, 1)?;
    let values = if raw[0] == 0x02 {
        require_length(raw, 5)?;
        let position = (raw[4] & 0x80 != 0).then_some(u32::from(raw[4] & 0x7f));
        ReportValues {
            aux_buttons: Some(Buttons::from_bits(u64::from(raw[1]), 8)?),
            absolute_analog: Some(AbsoluteAnalogReport {
                kind: AnalogKind::Wheel,
                positions: AbsoluteAnalog::from_slice(&[position])?,
            }),
            wheel_buttons: Some(WheelButtons::from_slice(&[Buttons::from_bits(
                u64::from(raw[3] & 1),
                1,
            )?])?),
            ..ReportValues::default()
        }
    } else {
        ReportValues::default()
    };
    Ok(ReportEnvelope {
        metadata,
        raw,
        values,
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
