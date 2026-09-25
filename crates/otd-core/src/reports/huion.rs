//! Stateless Giano and Inspiroy report decoding.
//!
//! Source pin: OpenTabletDriver 0.6.7, 736003ed72c8bbb28033b039d5a0bb76c344145c.
//! OpenTabletDriver.Configurations/Parsers/Huion/
//! {GianoReportParser,GianoReport,KamvasRelWheelReport,InspiroyReportParser,
//! InspiroyRelWheelReport}.cs, Parsers/UCLogic/UCLogicAuxReport.cs and
//! OpenTabletDriver.Plugin/Tablet/TiltTabletReport.cs.
//! https://github.com/OpenTabletDriver/OpenTabletDriver/tree/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.Configurations/Parsers/Huion
//!
//! Byte 1 controls dispatch regardless of report ID. The complete transport
//! slice is borrowed, and raw-only Data differs from explicit OutOfRange.
//! These functions do not enable additional devices or execute bindings.

use super::{
    AnalogKind, Buttons, RelativeAnalog, RelativeAnalogReport, ReportEnvelope, ReportError,
    ReportKind, ReportMetadata, ReportValues,
};

/// Decode `OpenTabletDriver.Configurations.Parsers.Huion.GianoReportParser`.
/// 0xF1 selects two relative wheels; flags with both bits 5/6 set select
/// auxiliary. Every other flag selects 17-bit position and inverted X/Y tilt.
pub fn parse_huion_giano(
    raw: &[u8],
    metadata: ReportMetadata,
) -> Result<(ReportKind, ReportEnvelope<'_>), ReportError> {
    require_length(raw, 2)?;
    let values = if raw[1] == 0xf1 {
        // For an unknown selector, upstream never reads byte 5 and returns
        // two zero deltas. Require only the fields its selected path reads.
        require_length(raw, 4)?;
        let mut deltas = [0, 0];
        if matches!(raw[3], 1 | 2) {
            require_length(raw, 6)?;
            deltas[usize::from(raw[3] - 1)] = wheel_delta(raw[5]);
        }
        relative_wheels(&deltas)?
    } else if raw[1] & 0x60 == 0x60 {
        auxiliary(raw)?
    } else {
        tilt_pen(raw, true)?
    };
    Ok((
        ReportKind::Data,
        ReportEnvelope {
            metadata,
            raw,
            values,
        },
    ))
}

/// Decode `OpenTabletDriver.Configurations.Parsers.Huion.InspiroyReportParser`.
/// 0xE0/0xE3 are identical auxiliary layouts, 0xF1 is a relative wheel, 0x00
/// means OutOfRange. Remaining bit-7 flags select pen; others stay raw-only.
pub fn parse_huion_inspiroy(
    raw: &[u8],
    metadata: ReportMetadata,
) -> Result<(ReportKind, ReportEnvelope<'_>), ReportError> {
    require_length(raw, 2)?;
    let values = match raw[1] {
        // Upstream treats group-button 0xE3 as ordinary auxiliary, without
        // providing separate group state. Preserve the same twenty indices.
        0xe0 | 0xe3 => auxiliary(raw)?,
        0xf1 => {
            require_length(raw, 6)?;
            relative_wheels(&[wheel_delta(raw[5])])?
        }
        0x00 => ReportValues::default(),
        flags if flags & 0x80 != 0 => tilt_pen(raw, false)?,
        _ => ReportValues::default(),
    };
    let kind = if raw[1] == 0 {
        ReportKind::OutOfRange
    } else {
        ReportKind::Data
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

fn tilt_pen(raw: &[u8], giano: bool) -> Result<ReportValues, ReportError> {
    require_length(raw, 12)?;
    let mut x = u32::from(u16::from_le_bytes([raw[2], raw[3]]));
    let mut y = u32::from(u16::from_le_bytes([raw[4], raw[5]]));
    if giano {
        x |= u32::from(raw[8] & 1) << 16;
        y |= u32::from(raw[9] & 1) << 16;
    }
    // Promote before negation so -128 becomes +128, as in C# int arithmetic.
    let tilt_x = i16::from(raw[10] as i8);
    let tilt_y = -i16::from(raw[11] as i8);
    Ok(ReportValues {
        position: Some([x as f32, y as f32]),
        pressure: Some(u32::from(u16::from_le_bytes([raw[6], raw[7]]))),
        tilt: Some([
            f32::from(if giano { -tilt_x } else { tilt_x }),
            f32::from(tilt_y),
        ]),
        pen_buttons: Some(Buttons::from_bits(u64::from(raw[1] >> 1), 3)?),
        ..ReportValues::default()
    })
}

fn auxiliary(raw: &[u8]) -> Result<ReportValues, ReportError> {
    require_length(raw, 7)?;
    let bits = u64::from(raw[4]) | (u64::from(raw[5]) << 8) | (u64::from(raw[6]) << 16);
    Ok(ReportValues {
        aux_buttons: Some(Buttons::from_bits(bits, 20)?),
        ..ReportValues::default()
    })
}

fn relative_wheels(deltas: &[i32]) -> Result<ReportValues, ReportError> {
    Ok(ReportValues {
        relative_analog: Some(RelativeAnalogReport {
            kind: AnalogKind::Wheel,
            deltas: RelativeAnalog::from_slice(deltas)?,
        }),
        ..ReportValues::default()
    })
}

fn wheel_delta(value: u8) -> i32 {
    match value {
        1 => 1,
        2 => -1,
        _ => 0,
    }
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
