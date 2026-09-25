//! Portable report values, independent of transport and output state.
//!
//! The optional fields mirror the composable interfaces in upstream 0.6.7:
//! https://github.com/OpenTabletDriver/OpenTabletDriver/tree/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.Plugin/Tablet
//! `None` means this report does not carry that field, not that its value is zero
//! or that an earlier held value should be released. In particular, an auxiliary
//! report must not release a pen button. Session teardown is a separate event.
//!
//! Native envelopes borrow the complete transport packet and keep decoded values
//! inline. Retaining a report requires the explicit owned snapshot operation.
//! These bounds are implementation capacities, not claims of device support;
//! device setup must reject capacities exceeding them instead of truncating.

use crate::protocol::PenReport;
use std::time::Duration;

mod intuos_touch;
pub use intuos_touch::{IntuosV2TouchParser, WacomDriverIntuosV2TouchParser, WacomDriverReport};

pub const MAX_BUTTONS: usize = 64;
pub const MAX_ANALOG_CHANNELS: usize = 16;
pub const MAX_WHEELS: usize = 8;
pub const MAX_TOUCH_POINTS: usize = 32;

/// An explicit upstream OutOfRangeReport is not an empty ordinary report.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ReportKind {
    #[default]
    Data,
    OutOfRange,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReportError {
    Empty,
    UnsupportedPenId(u8),
    ReportIdMismatch {
        parsed: u8,
        raw: u8,
    },
    Short {
        id: u8,
        got: usize,
        need: usize,
    },
    Capacity {
        field: &'static str,
        got: usize,
        maximum: usize,
    },
    Index {
        index: usize,
        length: usize,
    },
}

/// A fixed allocation with an explicit meaningful length. Unused entries are
/// never exposed, and a full collection fails rather than dropping input.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct InlineValues<T: Copy + Default, const N: usize> {
    values: [T; N],
    len: usize,
}

impl<T: Copy + Default, const N: usize> Default for InlineValues<T, N> {
    fn default() -> Self {
        Self {
            values: [T::default(); N],
            len: 0,
        }
    }
}

impl<T: Copy + Default, const N: usize> InlineValues<T, N> {
    pub fn from_slice(values: &[T]) -> Result<Self, ReportError> {
        if values.len() > N {
            return Err(ReportError::Capacity {
                field: "inline values",
                got: values.len(),
                maximum: N,
            });
        }
        let mut result = Self::default();
        result.values[..values.len()].copy_from_slice(values);
        result.len = values.len();
        Ok(result)
    }

    pub fn push(&mut self, value: T) -> Result<(), ReportError> {
        if self.len == N {
            return Err(ReportError::Capacity {
                field: "inline values",
                got: self.len + 1,
                maximum: N,
            });
        }
        self.values[self.len] = value;
        self.len += 1;
        Ok(())
    }

    pub fn as_slice(&self) -> &[T] {
        &self.values[..self.len]
    }

    pub fn as_mut_slice(&mut self) -> &mut [T] {
        &mut self.values[..self.len]
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

/// Button index ordering is the parser's upstream bool-array ordering.
/// A present empty button array differs from an absent button capability.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Buttons {
    bits: u64,
    count: u8,
}

impl Buttons {
    pub fn from_bits(bits: u64, count: usize) -> Result<Self, ReportError> {
        if count > MAX_BUTTONS {
            return Err(ReportError::Capacity {
                field: "buttons",
                got: count,
                maximum: MAX_BUTTONS,
            });
        }
        let mask = if count == MAX_BUTTONS {
            u64::MAX
        } else {
            (1u64 << count) - 1
        };
        Ok(Self {
            bits: bits & mask,
            count: count as u8,
        })
    }

    pub fn len(self) -> usize {
        self.count as usize
    }

    pub fn is_empty(self) -> bool {
        self.count == 0
    }

    pub fn bits(self) -> u64 {
        self.bits
    }

    /// `None` is an absent index; `Some(false)` is a released button.
    pub fn get(self, index: usize) -> Option<bool> {
        (index < self.len()).then(|| self.bits & (1u64 << index) != 0)
    }

    pub fn set(&mut self, index: usize, pressed: bool) -> Result<(), ReportError> {
        if index >= self.len() {
            return Err(ReportError::Index {
                index,
                length: self.len(),
            });
        }
        if pressed {
            self.bits |= 1u64 << index;
        } else {
            self.bits &= !(1u64 << index);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolType {
    Pen,
    Eraser,
}

/// Upstream IToolReport fields. Eraser capability alone does not supply a tool
/// serial or raw tool ID, so parsers must not invent a ToolIdentity from it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ToolIdentity {
    pub serial: u64,
    pub raw_tool_id: u32,
    pub tool: ToolType,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TouchPoint {
    pub id: u8,
    pub position: [f32; 2],
}

pub type AbsoluteAnalog = InlineValues<Option<u32>, MAX_ANALOG_CHANNELS>;
pub type RelativeAnalog = InlineValues<i32, MAX_ANALOG_CHANNELS>;
pub type WheelButtons = InlineValues<Buttons, MAX_WHEELS>;
/// A null slot mirrors upstream ITouchReport.Touches, including released slots.
/// This is a complete decoded snapshot, not a list of inferred touch gestures.
pub type Touches = InlineValues<Option<TouchPoint>, MAX_TOUCH_POINTS>;

/// Marker interfaces distinguish a wheel from another analog input upstream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnalogKind {
    Generic,
    Wheel,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AbsoluteAnalogReport {
    pub kind: AnalogKind,
    /// A null channel is explicitly no relevant reading (e.g. ring not touched).
    pub positions: AbsoluteAnalog,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RelativeAnalogReport {
    pub kind: AnalogKind,
    pub deltas: RelativeAnalog,
}

/// The values that report consumers/plugins may observe and modify. Independent
/// fields allow reports implementing several upstream interfaces simultaneously.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ReportValues {
    pub position: Option<[f32; 2]>,
    pub pressure: Option<u32>,
    pub tilt: Option<[f32; 2]>,
    pub eraser: Option<bool>,
    pub tool: Option<ToolIdentity>,
    pub pen_buttons: Option<Buttons>,
    pub near_proximity: Option<bool>,
    pub hover_distance: Option<u32>,
    /// PTH-660 transport extensions, not invented upstream interfaces.
    pub sense: Option<bool>,
    pub tip_switch: Option<bool>,
    pub rotation: Option<i16>,
    pub aux_buttons: Option<Buttons>,
    /// IMouseReport/puck: absolute position plus buttons and 2D scroll.
    pub mouse_buttons: Option<Buttons>,
    pub mouse_scroll: Option<[f32; 2]>,
    pub absolute_analog: Option<AbsoluteAnalogReport>,
    pub relative_analog: Option<RelativeAnalogReport>,
    pub wheel_buttons: Option<WheelButtons>,
    pub touches: Option<Touches>,
}

/// Allocated by the session owner, not derived from plugin-mutable raw bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DeviceId(pub u64);

/// Changes on reconnect, even when the physical DeviceId is unchanged.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SessionId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct EndpointId(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReportMetadata {
    pub device: DeviceId,
    pub session: SessionId,
    pub endpoint: EndpointId,
    /// Monotonic time since the session's clock origin; never wall-clock time.
    pub received_at: Duration,
    /// Session-owner sequence shared by all endpoints, in dispatch order.
    pub sequence: u64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ReportEnvelope<'a> {
    pub metadata: ReportMetadata,
    /// Exactly the bytes received, without padding or a guessed report length.
    pub raw: &'a [u8],
    pub values: ReportValues,
}

impl ReportEnvelope<'_> {
    /// Explicit allocation for asynchronous/retained consumers. Neither the
    /// native envelope nor its borrowed raw slice may outlive the transport read.
    pub fn snapshot(&self) -> OwnedReport {
        OwnedReport {
            metadata: self.metadata,
            raw: self.raw.into(),
            values: self.values,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct OwnedReport {
    pub metadata: ReportMetadata,
    pub raw: Box<[u8]>,
    pub values: ReportValues,
}

impl OwnedReport {
    pub fn as_borrowed(&self) -> ReportEnvelope<'_> {
        ReportEnvelope {
            metadata: self.metadata,
            raw: &self.raw,
            values: self.values,
        }
    }
}

/// Adapt the existing checked PTH-660 decoder, without changing its report type
/// or cursor behavior. `pen` must have been parsed from this same transport read.
/// ID and minimum length are checked before touching the additional button bits.
///
/// Pinned source: OpenTabletDriver.Configurations/Parsers/Wacom/IntuosV2/
/// IntuosV2Report.cs and IntuosV2OffsetReport.cs at
/// 736003ed72c8bbb28033b039d5a0bb76c344145c. The first uses byte 1 bits 1/2;
/// the offset variant uses byte 2 bits 1/2/3, in that order.
pub fn from_pth660(
    pen: PenReport,
    raw: &[u8],
    metadata: ReportMetadata,
) -> Result<ReportEnvelope<'_>, ReportError> {
    let Some(&id) = raw.first() else {
        return Err(ReportError::Empty);
    };
    if id != pen.id {
        return Err(ReportError::ReportIdMismatch {
            parsed: pen.id,
            raw: id,
        });
    }
    let (need, flags_at, button_count) = match id {
        0x10 => (17, 1, 2),
        0x1e => (13, 2, 3),
        _ => return Err(ReportError::UnsupportedPenId(id)),
    };
    if raw.len() < need {
        return Err(ReportError::Short {
            id,
            got: raw.len(),
            need,
        });
    }
    let values = ReportValues {
        position: Some([pen.x as f32, pen.y as f32]),
        pressure: Some(u32::from(pen.pressure)),
        tilt: Some([f32::from(pen.tilt[0]), f32::from(pen.tilt[1])]),
        eraser: Some(pen.eraser),
        pen_buttons: Some(Buttons::from_bits(
            u64::from(raw[flags_at] >> 1),
            button_count,
        )?),
        near_proximity: Some(pen.in_range),
        // Existing decoder has no offset hover distance. Upstream's offset
        // parser reads byte 11, overlapping tilt X. Do not guess its meaning or
        // manufacture a zero reading; that parser parity question remains open.
        hover_distance: pen.hover_distance.map(u32::from),
        sense: Some(pen.sense),
        tip_switch: Some(pen.tip_switch),
        rotation: pen.rotation,
        ..ReportValues::default()
    };
    Ok(ReportEnvelope {
        metadata,
        raw,
        values,
    })
}

/// Decode the IntuosV2 auxiliary packet without changing pen/touch state.
/// Unknown IDs are left to the other parser; a known truncated packet is an
/// error rather than an all-released report.
///
/// Pinned source: OpenTabletDriver.Configurations/Parsers/Wacom/IntuosV2/
/// IntuosV2ReportParser.cs and IntuosV2AuxReport.cs at
/// 736003ed72c8bbb28033b039d5a0bb76c344145c: ID 0x11, eight aux buttons in
/// byte 1, ring button in byte 3 bit 0, absolute ring position in byte 4 bits
/// 0..6 only when bit 7 is set. Ring-not-touched is null, not position zero.
/// This parser only supplies values; it does not enable bindings or OS output.
pub fn parse_intuos_auxiliary(
    raw: &[u8],
    metadata: ReportMetadata,
) -> Result<Option<ReportEnvelope<'_>>, ReportError> {
    let Some(&id) = raw.first() else {
        return Err(ReportError::Empty);
    };
    if id != 0x11 {
        return Ok(None);
    }
    if raw.len() < 5 {
        return Err(ReportError::Short {
            id,
            got: raw.len(),
            need: 5,
        });
    }
    let ring_position = (raw[4] & 0x80 != 0).then_some(u32::from(raw[4] & 0x7f));
    let values = ReportValues {
        aux_buttons: Some(Buttons::from_bits(u64::from(raw[1]), 8)?),
        absolute_analog: Some(AbsoluteAnalogReport {
            kind: AnalogKind::Wheel,
            positions: AbsoluteAnalog::from_slice(&[ring_position])?,
        }),
        wheel_buttons: Some(WheelButtons::from_slice(&[Buttons::from_bits(
            u64::from(raw[3] & 1),
            1,
        )?])?),
        ..ReportValues::default()
    };
    Ok(Some(ReportEnvelope {
        metadata,
        raw,
        values,
    }))
}
