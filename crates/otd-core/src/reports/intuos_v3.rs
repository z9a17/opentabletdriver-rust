//! Stateless decoding of the three IntuosV3 report variants.
//!
//! Current catalog parser updates: a126f7b241e417399be6c6a760c0a9d4b987ecfd.
//! Older unchanged fields retain the 0.6.7 reference below.
//!
//! Source: OpenTabletDriver 0.6.7, commit
//! 736003ed72c8bbb28033b039d5a0bb76c344145c,
//! OpenTabletDriver.Configurations/Parsers/Wacom/IntuosV3/
//! {IntuosV3ReportParser,IntuosV3Report,IntuosV3ExtendedReport,IntuosV3AuxReport}.cs.
//! https://github.com/OpenTabletDriver/OpenTabletDriver/tree/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.Configurations/Parsers/Wacom/IntuosV3
//!
//! These portable decoders do not select or initialize a device, enable bindings,
//! or imply live support for any tablet using this parser in the database.

use super::{
    AnalogKind, Buttons, RelativeAnalog, RelativeAnalogReport, ReportEnvelope, ReportError,
    ReportMetadata, ReportValues,
};

/// Decode `OpenTabletDriver.Configurations.Parsers.Wacom.IntuosV3.IntuosV3ReportParser`.
///
/// The complete transport slice is retained, including any trailing padding.
/// Unknown IDs and unknown 0x1F subtypes produce an envelope with no decoded
/// capabilities, matching upstream's raw-only DeviceReport. They never fall
/// back to a different pen layout or imply release of any held state.
///
/// Empty input and truncated known layouts return errors before any field read.
/// Lengths below are checked field extents, not endpoint descriptor lengths:
/// the pinned configurations include both 19-byte and 192-byte endpoints.
pub fn parse_intuos_v3(
    raw: &[u8],
    metadata: ReportMetadata,
) -> Result<ReportEnvelope<'_>, ReportError> {
    let Some(&id) = raw.first() else {
        return Err(ReportError::Empty);
    };
    let values = match id {
        0x11 => {
            require_length(raw, 6)?;
            auxiliary(raw)?
        }
        0x1e => {
            require_length(raw, 20)?;
            extended_pen(raw)?
        }
        0x1f => {
            require_length(raw, 2)?;
            if raw[1] == 0x01 {
                require_length(raw, 14)?;
                pen(raw)?
            } else {
                ReportValues::default()
            }
        }
        _ => ReportValues::default(),
    };
    Ok(ReportEnvelope {
        metadata,
        raw,
        values,
    })
}

fn require_length(raw: &[u8], need: usize) -> Result<(), ReportError> {
    if raw.len() < need {
        return Err(ReportError::Short {
            // Only called after parse_intuos_v3 has read the report ID.
            id: raw[0],
            got: raw.len(),
            need,
        });
    }
    Ok(())
}

// All helpers below receive slices checked for their complete field extents.
fn pen(raw: &[u8]) -> Result<ReportValues, ReportError> {
    Ok(ReportValues {
        position: Some([
            f32::from(u16::from_le_bytes([raw[3], raw[4]])),
            f32::from(u16::from_le_bytes([raw[5], raw[6]])),
        ]),
        pressure: Some(u32::from(u16::from_le_bytes([raw[7], raw[8]]))),
        // Signed bytes, as upstream #5215 corrects: subtracting 0xFF read
        // 0xFF as zero and put every negative tilt off by one.
        tilt: Some([f32::from(raw[9] as i8), f32::from(raw[11] as i8)]),
        eraser: Some(raw[2] & 0x20 != 0),
        near_proximity: Some(raw[2] & 0x40 != 0),
        hover_distance: Some(u32::from(raw[13])),
        pen_buttons: Some(Buttons::from_bits(u64::from(raw[2] >> 1), 2)?),
        ..ReportValues::default()
    })
}

fn extended_pen(raw: &[u8]) -> Result<ReportValues, ReportError> {
    Ok(ReportValues {
        rotation: Some(i32::from(i16::from_le_bytes([raw[15], raw[16]]))),
        position: Some([
            u32::from_le_bytes([raw[3], raw[4], raw[5], 0]) as f32,
            u32::from_le_bytes([raw[6], raw[7], raw[8], 0]) as f32,
        ]),
        pressure: Some(u32::from(u16::from_le_bytes([raw[9], raw[10]]))),
        tilt: Some([
            f32::from(i16::from_le_bytes([raw[11], raw[12]])),
            f32::from(i16::from_le_bytes([raw[13], raw[14]])),
        ]),
        eraser: Some(raw[2] & 0x20 != 0),
        near_proximity: Some(raw[2] & 0x40 != 0),
        hover_distance: Some(u32::from(raw[19])),
        pen_buttons: Some(Buttons::from_bits(u64::from(raw[2] >> 1), 3)?),
        ..ReportValues::default()
    })
}

fn auxiliary(raw: &[u8]) -> Result<ReportValues, ReportError> {
    let buttons = u64::from(raw[1]);
    let extra = u64::from(raw[3]);
    // Upstream inserts byte 3 bit 0 between byte 1's two four-button groups,
    // then appends byte 3 bit 1. Keep all ten indices even on smaller tablets.
    let bits = (buttons & 0x0f) | ((extra & 1) << 4) | ((buttons & 0xf0) << 1) | ((extra & 2) << 8);
    Ok(ReportValues {
        aux_buttons: Some(Buttons::from_bits(bits, 10)?),
        relative_analog: Some(RelativeAnalogReport {
            kind: AnalogKind::Wheel,
            deltas: RelativeAnalog::from_slice(&[wheel_delta(raw[4]), wheel_delta(raw[5])])?,
        }),
        ..ReportValues::default()
    })
}

fn wheel_delta(value: u8) -> i32 {
    // Signed seven-bit value: bit 6 is sign; bit 7 is ignored. The two
    // channels remain present even when the device exposes only one wheel.
    i32::from(value & 0x7f) - if value & 0x40 != 0 { 0x80 } else { 0 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reports::{DeviceId, EndpointId, SessionId};
    use std::time::Duration;

    #[test]
    fn pen_tilt_bytes_are_signed() {
        let metadata = ReportMetadata {
            device: DeviceId(1),
            session: SessionId(1),
            endpoint: EndpointId(0),
            received_at: Duration::ZERO,
            sequence: 0,
        };
        for (byte, expected) in [(0x00, 0.0), (0x7f, 127.0), (0x80, -128.0), (0xff, -1.0)] {
            let mut raw = [0; 14];
            raw[0] = 0x1f;
            raw[1] = 0x01;
            raw[9] = byte;
            raw[11] = byte;
            let report = parse_intuos_v3(&raw, metadata).unwrap();
            assert_eq!(report.values.tilt, Some([expected, expected]));
        }
    }
}
