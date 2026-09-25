//! Generic and one-byte-prefix tablet report parsers.
//!
//! Source pin: OpenTabletDriver 0.6.7, 736003ed72c8bbb28033b039d5a0bb76c344145c,
//! OpenTabletDriver.Plugin/Tablet/{TabletReportParser,TabletReport,AuxReportParser,AuxReport}.cs
//! and OpenTabletDriver.Configurations/Parsers/SkipByteTabletReportParser.cs.
//! https://github.com/OpenTabletDriver/OpenTabletDriver/tree/736003ed72c8bbb28033b039d5a0bb76c344145c
//!
//! These parsers decode every packet, without an ID whitelist or range sentinel.
//! The wrapper below distinguishes original transport bytes from upstream Raw
//! when a parser deliberately slices its input. No live device is enabled.

use super::{Buttons, ReportEnvelope, ReportError, ReportKind, ReportMetadata, ReportValues};

/// A parser result retaining both transport bytes and the upstream Raw view.
/// `report.raw` may be a suffix of `transport_raw`; it must not be replaced by
/// the full transport packet when exposing a report to plugins. Metadata stays
/// with the transport read. Unprefixed variants may use the same slice for both.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TransportReport<'a> {
    pub transport_raw: &'a [u8],
    pub report: ReportEnvelope<'a>,
}

/// Decode `OpenTabletDriver.Plugin.Tablet.TabletReportParser`.
/// The eight-byte minimum carries position, pressure and three pen buttons;
/// no proximity, eraser or tilt capability is inferred from the flag byte.
pub fn parse_tablet(
    raw: &[u8],
    metadata: ReportMetadata,
) -> Result<(ReportKind, ReportEnvelope<'_>), ReportError> {
    require_length(raw, 8)?;
    Ok((
        ReportKind::Data,
        ReportEnvelope {
            metadata,
            raw,
            values: ReportValues {
                position: Some([
                    f32::from(u16::from_le_bytes([raw[2], raw[3]])),
                    f32::from(u16::from_le_bytes([raw[4], raw[5]])),
                ]),
                pressure: Some(u32::from(u16::from_le_bytes([raw[6], raw[7]]))),
                pen_buttons: Some(Buttons::from_bits(u64::from(raw[1] >> 1), 3)?),
                ..ReportValues::default()
            },
        },
    ))
}

/// Decode `OpenTabletDriver.Plugin.Tablet.AuxReportParser`.
/// Four auxiliary buttons come from byte 3; every packet requires four bytes.
pub fn parse_auxiliary(
    raw: &[u8],
    metadata: ReportMetadata,
) -> Result<(ReportKind, ReportEnvelope<'_>), ReportError> {
    require_length(raw, 4)?;
    Ok((
        ReportKind::Data,
        ReportEnvelope {
            metadata,
            raw,
            values: ReportValues {
                aux_buttons: Some(Buttons::from_bits(u64::from(raw[3]), 4)?),
                ..ReportValues::default()
            },
        },
    ))
}

/// Decode `OpenTabletDriver.Configurations.Parsers.SkipByteTabletReportParser`.
/// Exactly one transport byte is removed before generic pen decoding. The
/// minimum is nine transport bytes; decoding errors describe the eight-byte
/// payload requirement, consistent with other explicit prefix adapters.
pub fn parse_skip_byte_tablet(
    raw: &[u8],
    metadata: ReportMetadata,
) -> Result<(ReportKind, TransportReport<'_>), ReportError> {
    let Some((_, payload)) = raw.split_first() else {
        return Err(ReportError::Empty);
    };
    let (kind, report) = parse_tablet(payload, metadata)?;
    Ok((
        kind,
        TransportReport {
            transport_raw: raw,
            report,
        },
    ))
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
