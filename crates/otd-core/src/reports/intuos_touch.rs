//! Stateful touch snapshots, with no gesture interpretation or output.
//!
//! Exact reference at 736003ed72c8bbb28033b039d5a0bb76c344145c:
//! OpenTabletDriver.Configurations/Parsers/Wacom/IntuosV2/
//! {IntuosV2ReportParser,IntuosV2TouchReport,WacomDriverIntuosV2ReportParser}.cs

use super::{ReportEnvelope, ReportError, ReportMetadata, ReportValues, TouchPoint, Touches};

const TOUCH_SLOTS: usize = 16;
const TOUCH_RECORDS: usize = 5;
// Five records start at 2 + 8*i; the last coordinate ends at byte 39.
// Trailing transport padding is retained but is not required for decoding.
const MINIMUM_TOUCH_LENGTH: usize = 40;

/// One parser belongs to one input endpoint/session. IntuosV2 packets update
/// only five contacts, while the plugin-visible snapshot has sixteen slots.
/// A new packet must not implicitly release contacts absent from those updates.
#[derive(Clone, Debug, Default)]
pub struct IntuosV2TouchParser {
    slots: [Option<TouchPoint>; TOUCH_SLOTS],
}

impl IntuosV2TouchParser {
    /// Clear only this parser's state when its endpoint/session is lost. This
    /// does not emit a gesture, button release, or change another report category.
    pub fn reset(&mut self) {
        self.slots.fill(None);
    }

    pub fn slots(&self) -> &[Option<TouchPoint>; TOUCH_SLOTS] {
        &self.slots
    }

    /// Unknown IDs return None without changing state. A malformed known packet
    /// fails before applying any updates, so truncation never releases contacts.
    pub fn parse<'a>(
        &mut self,
        raw: &'a [u8],
        metadata: ReportMetadata,
    ) -> Result<Option<ReportEnvelope<'a>>, ReportError> {
        let Some(&id) = raw.first() else {
            return Err(ReportError::Empty);
        };
        if !matches!(id, 0x21 | 0xd2) {
            return Ok(None);
        }
        if raw.len() < MINIMUM_TOUCH_LENGTH {
            return Err(ReportError::Short {
                id,
                got: raw.len(),
                need: MINIMUM_TOUCH_LENGTH,
            });
        }

        let mut next = self.slots;
        for record in 0..TOUCH_RECORDS {
            let offset = 2 + 8 * record;
            // Wire IDs 1..16 correspond to upstream TouchID/array index 0..15.
            // ID 0 means no update; IDs beyond 16 are ignored by upstream.
            let Some(slot) = raw[offset].checked_sub(1) else {
                continue;
            };
            let Some(value) = next.get_mut(usize::from(slot)) else {
                continue;
            };
            *value = if raw[offset + 1] == 0 {
                None
            } else {
                Some(TouchPoint {
                    id: slot,
                    position: [
                        f32::from(u16::from_le_bytes([raw[offset + 2], raw[offset + 3]])),
                        f32::from(u16::from_le_bytes([raw[offset + 4], raw[offset + 5]])),
                    ],
                })
            };
        }
        let touches = Touches::from_slice(&next)?;
        self.slots = next;
        Ok(Some(ReportEnvelope {
            metadata,
            raw,
            values: ReportValues {
                touches: Some(touches),
                ..ReportValues::default()
            },
        }))
    }
}

/// The Wacom-driver parser calls its base parser with data[1..]. Keep the exact
/// transport packet alongside that payload: `report.raw` matches upstream Raw,
/// while `transport_raw` retains the prefix and actual transport byte count.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WacomDriverReport<'a> {
    pub transport_raw: &'a [u8],
    pub report: ReportEnvelope<'a>,
}

/// Explicit prefix variant; do not auto-strip a byte on normal USB reports.
/// This bounded slice implements touch reports only. Pen/auxiliary IDs return
/// None for their own decoders, rather than claiming whole-family support.
#[derive(Clone, Debug, Default)]
pub struct WacomDriverIntuosV2TouchParser {
    inner: IntuosV2TouchParser,
}

impl WacomDriverIntuosV2TouchParser {
    pub fn reset(&mut self) {
        self.inner.reset();
    }

    pub fn slots(&self) -> &[Option<TouchPoint>; TOUCH_SLOTS] {
        self.inner.slots()
    }

    pub fn parse<'a>(
        &mut self,
        raw: &'a [u8],
        metadata: ReportMetadata,
    ) -> Result<Option<WacomDriverReport<'a>>, ReportError> {
        let Some((_, payload)) = raw.split_first() else {
            return Err(ReportError::Empty);
        };
        self.inner.parse(payload, metadata).map(|report| {
            report.map(|report| WacomDriverReport {
                transport_raw: raw,
                report,
            })
        })
    }
}
