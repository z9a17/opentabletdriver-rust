//! Complete stateless Veikk parser variants.
//!
//! Source pin: OpenTabletDriver 0.6.7, 736003ed72c8bbb28033b039d5a0bb76c344145c,
//! OpenTabletDriver.Configurations/Parsers/Veikk/
//! {VeikkReportParser,VeikkA15ReportParser,VeikkTiltReportParser,VeikkV1ReportParser,
//! VeikkTabletReport,VeikkA15TabletReport,VeikkTiltTabletReport,VeikkAuxReport,
//! VeikkAuxV1Report,VeikkRelativeWheelReport}.cs and Plugin/Tablet/TabletReport.cs.
//! https://github.com/OpenTabletDriver/OpenTabletDriver/tree/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.Configurations/Parsers/Veikk
//!
//! Unknown and ignored touchpad packets remain raw-only Data. Only the exact
//! Tilt/V1 range sentinel produces OutOfRange. Device/output gates are unchanged.

use super::generic::{TransportReport, parse_tablet};
use super::{
    AnalogKind, Buttons, RelativeAnalog, RelativeAnalogReport, ReportEnvelope, ReportError,
    ReportKind, ReportMetadata, ReportValues,
};

/// Decode `OpenTabletDriver.Configurations.Parsers.Veikk.VeikkReportParser`.
/// Byte 1 == 0x43 is raw-only; otherwise byte 2 bit 5 selects 24-bit pen,
/// value 1 auxiliary and value 3 relative wheel. Other values stay raw-only.
pub fn parse_veikk(
    raw: &[u8],
    metadata: ReportMetadata,
) -> Result<(ReportKind, ReportEnvelope<'_>), ReportError> {
    parse(raw, metadata, Parser::Base)
}

/// Decode `OpenTabletDriver.Configurations.Parsers.Veikk.VeikkA15ReportParser`.
/// Dispatch follows the base parser except pen coordinates are 16-bit and
/// byte 2 == 3 is raw-only: this parser does not dispatch a wheel report.
pub fn parse_veikk_a15(
    raw: &[u8],
    metadata: ReportMetadata,
) -> Result<(ReportKind, ReportEnvelope<'_>), ReportError> {
    parse(raw, metadata, Parser::A15)
}

/// Decode `OpenTabletDriver.Configurations.Parsers.Veikk.VeikkTiltReportParser`.
/// Byte 1 == 0x41 selects pen or range loss (byte 2 == 0xC0); 0x42 selects
/// auxiliary. All remaining values, including touchpad 0x43, are raw-only.
pub fn parse_veikk_tilt(
    raw: &[u8],
    metadata: ReportMetadata,
) -> Result<(ReportKind, ReportEnvelope<'_>), ReportError> {
    parse(raw, metadata, Parser::Tilt)
}

enum Parser {
    Base,
    A15,
    Tilt,
}

fn parse(
    raw: &[u8],
    metadata: ReportMetadata,
    parser: Parser,
) -> Result<(ReportKind, ReportEnvelope<'_>), ReportError> {
    require_length(raw, 2)?;
    let mut kind = ReportKind::Data;
    let values = match parser {
        Parser::Tilt => match raw[1] {
            0x41 => {
                require_length(raw, 3)?;
                if raw[2] == 0xc0 {
                    kind = ReportKind::OutOfRange;
                    ReportValues::default()
                } else {
                    pen(raw, false, true)?
                }
            }
            0x42 => auxiliary(raw)?,
            _ => ReportValues::default(),
        },
        Parser::Base | Parser::A15 => {
            if raw[1] == 0x43 {
                ReportValues::default()
            } else {
                require_length(raw, 3)?;
                if raw[2] & 0x20 != 0 {
                    pen(raw, matches!(parser, Parser::A15), false)?
                } else if raw[2] == 1 {
                    auxiliary(raw)?
                } else if raw[2] == 3 && matches!(parser, Parser::Base) {
                    wheel(raw)?
                } else {
                    ReportValues::default()
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

/// Decode `OpenTabletDriver.Configurations.Parsers.Veikk.VeikkV1ReportParser`.
/// ID 3 is a variable-length button-ID list. Other packets use bytes 1/2:
/// (0x41, 0xA0..=0xAF) decodes a generic pen after stripping one byte, and
/// (0x41, 0xC0) is OutOfRange. Only pen reports expose the sliced Raw payload;
/// auxiliary, range and unknown packets expose the whole transport packet.
pub fn parse_veikk_v1(
    raw: &[u8],
    metadata: ReportMetadata,
) -> Result<(ReportKind, TransportReport<'_>), ReportError> {
    require_length(raw, 1)?;
    let mut kind = ReportKind::Data;
    let values = if raw[0] == 0x03 {
        auxiliary_v1(raw)?
    } else {
        require_length(raw, 3)?;
        if raw[1] == 0x41 && raw[2] & 0xf0 == 0xa0 {
            let (kind, report) = parse_tablet(&raw[1..], metadata)?;
            return Ok((
                kind,
                TransportReport {
                    transport_raw: raw,
                    report,
                },
            ));
        }
        if raw[1] == 0x41 && raw[2] == 0xc0 {
            kind = ReportKind::OutOfRange;
        }
        ReportValues::default()
    };
    Ok((
        kind,
        TransportReport {
            transport_raw: raw,
            report: ReportEnvelope {
                metadata,
                raw,
                values,
            },
        },
    ))
}

fn pen(raw: &[u8], a15: bool, tilt: bool) -> Result<ReportValues, ReportError> {
    require_length(
        raw,
        if tilt {
            13
        } else if a15 {
            9
        } else {
            11
        },
    )?;
    let (position, pressure) = if a15 {
        (
            [
                f32::from(u16::from_le_bytes([raw[3], raw[4]])),
                f32::from(u16::from_le_bytes([raw[5], raw[6]])),
            ],
            u16::from_le_bytes([raw[7], raw[8]]),
        )
    } else {
        (
            [
                u32::from_le_bytes([raw[3], raw[4], raw[5], 0]) as f32,
                u32::from_le_bytes([raw[6], raw[7], raw[8], 0]) as f32,
            ],
            u16::from_le_bytes([raw[9], raw[10]]),
        )
    };
    Ok(ReportValues {
        position: Some(position),
        pressure: Some(u32::from(pressure)),
        pen_buttons: Some(Buttons::from_bits(u64::from(raw[2] >> 1), 2)?),
        tilt: tilt.then(|| [f32::from(raw[11] as i8), f32::from(raw[12] as i8)]),
        ..ReportValues::default()
    })
}

fn auxiliary(raw: &[u8]) -> Result<ReportValues, ReportError> {
    require_length(raw, 6)?;
    let bits = if raw[3] & 1 != 0 {
        u64::from(raw[4]) | (u64::from(raw[5]) << 8)
    } else {
        0
    };
    Ok(ReportValues {
        aux_buttons: Some(Buttons::from_bits(bits, 12)?),
        ..ReportValues::default()
    })
}

fn wheel(raw: &[u8]) -> Result<ReportValues, ReportError> {
    require_length(raw, 5)?;
    let delta = if raw[3] & 1 == 0 {
        0
    } else if raw[4] & 2 != 0 {
        1
    } else if raw[4] & 1 != 0 {
        -1
    } else {
        0
    };
    Ok(ReportValues {
        relative_analog: Some(RelativeAnalogReport {
            kind: AnalogKind::Wheel,
            deltas: RelativeAnalog::from_slice(&[delta])?,
        }),
        ..ReportValues::default()
    })
}

fn auxiliary_v1(raw: &[u8]) -> Result<ReportValues, ReportError> {
    let mut bits = 0u64;
    // A one- or two-byte ID-3 packet is valid upstream: its loop is empty and
    // returns twelve released buttons. Byte 1 is accessed only inside the loop.
    for &code in raw.iter().skip(2) {
        let index = match code {
            0x3e => 0,
            0x0c => 1,
            0x2c => 2,
            // The shared button code aliases indices 3 and 5. A nonzero byte 1
            // selects index 5, preserving upstream's documented shadowing.
            0x19 => {
                if raw[1] == 0 {
                    3
                } else {
                    5
                }
            }
            0x06 => 4,
            0x1d => 6,
            0x16 => 7,
            0x2e => 8,
            0x2d => 9,
            0x30 => 10,
            0x2f => 11,
            _ => continue,
        };
        bits |= 1 << index;
    }
    Ok(ReportValues {
        aux_buttons: Some(Buttons::from_bits(bits, 12)?),
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
