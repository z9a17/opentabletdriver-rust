//! Wacom parser families other than IntuosV2, IntuosV3 and Bamboo.
//!
//! Current catalog parser updates: a126f7b241e417399be6c6a760c0a9d4b987ecfd.
//! Older unchanged fields retain the 0.6.7 reference below.
//!
//! Source: OpenTabletDriver 0.6.7, commit
//! 736003ed72c8bbb28033b039d5a0bb76c344145c,
//! OpenTabletDriver.Configurations/Parsers/Wacom/{Intuos,IntuosV1,Intuos3,
//! Intuos4,IntuosPro,CintiqV1,Graphire,PL,PTU}/*.cs,
//! Wacom64bAuxReportParser.cs and WacomTouchReport.cs.
//! https://github.com/OpenTabletDriver/OpenTabletDriver/tree/736003ed72c8bbb28033b039d5a0bb76c344145c/OpenTabletDriver.Configurations/Parsers/Wacom
//!
//! Stateful parsers keep what upstream keeps in parser fields (the last
//! pressure, tilt and pen buttons for IntuosV1 rotation reports, the PL eraser
//! latch and Wacom touch slots). One parser instance belongs to one endpoint.
//! Upstream reads past a short packet and throws, dropping the report; here a
//! known layout that is too short is an error returned before any state change.

use super::{
    AbsoluteAnalog, AbsoluteAnalogReport, AnalogKind, Buttons, ReportEnvelope, ReportError,
    ReportKind, ReportMetadata, ReportValues, ToolIdentity, ToolType, TouchPoint, Touches,
    WheelButtons,
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

fn envelope(
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
    envelope(raw, metadata, ReportKind::Data, values)
}

fn out_of_range(raw: &[u8], metadata: ReportMetadata) -> Parsed<'_> {
    envelope(
        raw,
        metadata,
        ReportKind::OutOfRange,
        ReportValues::default(),
    )
}

/// The WacomDriver* parsers drop the first transport byte, then parse as the
/// base parser. Upstream throws on an empty packet.
fn strip_prefix(raw: &[u8]) -> Result<&[u8], ReportError> {
    match raw.split_first() {
        Some((_, payload)) if !payload.is_empty() => Ok(payload),
        Some(_) => Err(ReportError::Short {
            id: raw[0],
            got: raw.len(),
            need: 2,
        }),
        None => Err(ReportError::Empty),
    }
}

/// `(report[3] | report[2] << 8) << 1 | ((report[9] >> 1) & 1)` and its Y twin.
fn high_resolution_position(raw: &[u8]) -> [f32; 2] {
    let x = ((u32::from(raw[3]) | u32::from(raw[2]) << 8) << 1) | u32::from((raw[9] >> 1) & 1);
    let y = ((u32::from(raw[5]) | u32::from(raw[4]) << 8) << 1) | u32::from(raw[9] & 1);
    [x as f32, y as f32]
}

/// Pressure, tilt and pen buttons kept between IntuosV1 reports, because the
/// rotation report repeats the previous tablet report's values.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct IntuosV1State {
    pressure: u32,
    tilt: [f32; 2],
    buttons: Buttons,
    rotation: i16,
}

fn v1_tablet(raw: &[u8], state: &mut IntuosV1State) -> Result<ReportValues, ReportError> {
    require_length(raw, 10)?;
    let tilt = [
        (((i32::from(raw[7]) << 1) & 0x7e) | i32::from(raw[8] >> 7)) as f32 - 64.0,
        f32::from(raw[8] & 0x7f) - 64.0,
    ];
    let pressure =
        (u32::from(raw[6]) << 3) | (u32::from(raw[7] & 0xc0) >> 5) | u32::from(raw[1] & 1);
    let buttons = Buttons::from_bits(u64::from(raw[1] >> 1), 2)?;
    *state = IntuosV1State {
        pressure,
        tilt,
        buttons,
        rotation: state.rotation,
    };
    Ok(ReportValues {
        rotation: Some(state.rotation),
        position: Some(high_resolution_position(raw)),
        tilt: Some(tilt),
        pressure: Some(pressure),
        pen_buttons: Some(buttons),
        near_proximity: Some(bit(raw[1], 6)),
        hover_distance: Some(u32::from(raw[9])),
        ..ReportValues::default()
    })
}

fn v1_rotation(raw: &[u8], state: &mut IntuosV1State) -> Result<ReportValues, ReportError> {
    require_length(raw, 10)?;
    let magnitude = (i16::from(raw[6]) << 2) | i16::from(raw[7] >> 6);
    state.rotation = if raw[7] & 0x20 != 0 { magnitude } else { -magnitude };
    Ok(ReportValues {
        rotation: Some(state.rotation),
        position: Some(high_resolution_position(raw)),
        tilt: Some(state.tilt),
        pressure: Some(state.pressure),
        pen_buttons: Some(state.buttons),
        near_proximity: Some(bit(raw[1], 6)),
        hover_distance: Some(u32::from(raw[9])),
        ..ReportValues::default()
    })
}

fn v1_tool(raw: &[u8]) -> Result<ReportValues, ReportError> {
    require_length(raw, 10)?;
    // Upstream sums in 32-bit signed arithmetic, then casts to ulong, so a set
    // bit 31 sign-extends. Reproduce that exactly.
    let serial = (i32::from(raw[3] & 0x0f) << 28)
        .wrapping_add(i32::from(raw[4]) << 20)
        .wrapping_add(i32::from(raw[5]) << 12)
        .wrapping_add(i32::from(raw[6]) << 4)
        .wrapping_add(i32::from(raw[7] >> 4));
    let raw_tool_id = (u32::from(raw[2]) << 4)
        | u32::from(raw[3] >> 4)
        | (u32::from(raw[7] & 0x0f) << 20)
        | (u32::from(raw[8] & 0xf0) << 8);
    let tool = if bit(raw[3], 7) {
        ToolType::Eraser
    } else {
        ToolType::Pen
    };
    Ok(ReportValues {
        tool: Some(ToolIdentity {
            serial: i64::from(serial) as u64,
            raw_tool_id,
            tool,
        }),
        eraser: Some(tool == ToolType::Eraser),
        near_proximity: Some(bit(raw[1], 6)),
        hover_distance: Some(u32::from(raw[9]) >> 2),
        ..ReportValues::default()
    })
}

fn v1_aux(raw: &[u8]) -> Result<ReportValues, ReportError> {
    require_length(raw, 5)?;
    Ok(ReportValues {
        aux_buttons: Some(Buttons::from_bits(u64::from(raw[4]), 8)?),
        ..ReportValues::default()
    })
}

/// `IntuosV1ReportParser.GetToolReport`, shared by IntuosV1, IntuosPro,
/// CintiqV1 and the pen path of Intuos4.
fn v1_tool_report<'a>(
    raw: &'a [u8],
    metadata: ReportMetadata,
    state: &mut IntuosV1State,
) -> Parsed<'a> {
    require_length(raw, 2)?;
    let flags = raw[1];
    if raw[0] == 0x10 && flags == 0x20 {
        return data(raw, metadata, ReportValues::default());
    }
    if flags == 0x80 {
        return out_of_range(raw, metadata);
    }
    let values = if bit(flags, 1) && bit(flags, 3) {
        v1_rotation(raw, state)?
    } else if bit(flags, 5) {
        v1_tablet(raw, state)?
    } else if flags == 0xc2 {
        v1_tool(raw)?
    } else {
        ReportValues::default()
    };
    data(raw, metadata, values)
}

/// `IntuosV1ReportParser` and `WacomDriverIntuosV1ReportParser`.
#[derive(Clone, Debug, Default)]
pub struct IntuosV1Parser {
    state: IntuosV1State,
    prefixed: bool,
}

impl IntuosV1Parser {
    pub fn new(prefixed: bool) -> Self {
        Self {
            prefixed,
            ..Self::default()
        }
    }

    pub fn reset(&mut self) {
        self.state = IntuosV1State::default();
    }

    pub fn parse<'a>(&mut self, raw: &'a [u8], metadata: ReportMetadata) -> Parsed<'a> {
        let raw = if self.prefixed {
            strip_prefix(raw)?
        } else {
            raw
        };
        let Some(&id) = raw.first() else {
            return Err(ReportError::Empty);
        };
        match id {
            0x02 | 0x10 => v1_tool_report(raw, metadata, &mut self.state),
            0x03 => data(raw, metadata, v1_aux(raw)?),
            _ => data(raw, metadata, ReportValues::default()),
        }
    }
}

/// `IntuosProReportParser` and `WacomDriverIntuosProReportParser`.
#[derive(Clone, Debug, Default)]
pub struct IntuosProParser {
    state: IntuosV1State,
    prefixed: bool,
}

impl IntuosProParser {
    pub fn new(prefixed: bool) -> Self {
        Self {
            prefixed,
            ..Self::default()
        }
    }

    pub fn reset(&mut self) {
        self.state = IntuosV1State::default();
    }

    pub fn parse<'a>(&mut self, raw: &'a [u8], metadata: ReportMetadata) -> Parsed<'a> {
        let raw = if self.prefixed {
            strip_prefix(raw)?
        } else {
            raw
        };
        let Some(&id) = raw.first() else {
            return Err(ReportError::Empty);
        };
        match id {
            0x02 | 0x10 => v1_tool_report(raw, metadata, &mut self.state),
            0x03 => {
                require_length(raw, 5)?;
                data(raw, metadata, wheel_aux(raw[4], raw[2], raw[3])?)
            }
            _ => data(raw, metadata, ReportValues::default()),
        }
    }
}

/// Eight aux buttons, a touch ring whose bit 7 means "touched" and a ring
/// button (Intuos4 and IntuosPro auxiliary reports).
fn wheel_aux(buttons: u8, wheel: u8, wheel_button: u8) -> Result<ReportValues, ReportError> {
    let position = bit(wheel, 7).then_some(u32::from(wheel & 0x7f));
    Ok(ReportValues {
        aux_buttons: Some(Buttons::from_bits(u64::from(buttons), 8)?),
        absolute_analog: Some(AbsoluteAnalogReport {
            kind: AnalogKind::Wheel,
            positions: AbsoluteAnalog::from_slice(&[position])?,
        }),
        wheel_buttons: Some(WheelButtons::from_slice(&[Buttons::from_bits(
            u64::from(wheel_button & 1),
            1,
        )?])?),
        ..ReportValues::default()
    })
}

/// `CintiqV1ReportParser`.
#[derive(Clone, Debug, Default)]
pub struct CintiqV1Parser {
    state: IntuosV1State,
}

impl CintiqV1Parser {
    pub fn reset(&mut self) {
        self.state = IntuosV1State::default();
    }

    pub fn parse<'a>(&mut self, raw: &'a [u8], metadata: ReportMetadata) -> Parsed<'a> {
        let Some(&id) = raw.first() else {
            return Err(ReportError::Empty);
        };
        match id {
            0x02 | 0x10 => v1_tool_report(raw, metadata, &mut self.state),
            0x0c => {
                require_length(raw, 10)?;
                // Upstream also tests bit 8 of the left and right button bytes,
                // which is never set in a byte; those two entries stay false.
                let bits = u64::from(raw[5] & 1)
                    | (u64::from(raw[6]) << 1)
                    | (u64::from(raw[7] & 1) << 10)
                    | (u64::from(raw[8]) << 11)
                    | (u64::from(raw[9] & 3) << 20);
                data(
                    raw,
                    metadata,
                    ReportValues {
                        aux_buttons: Some(Buttons::from_bits(bits, 22)?),
                        ..ReportValues::default()
                    },
                )
            }
            _ => data(raw, metadata, ReportValues::default()),
        }
    }
}

/// `IntuosReportParser` and `WacomDriverIntuosReportParser` (the original Intuos).
pub fn parse_intuos(raw: &[u8], metadata: ReportMetadata) -> Parsed<'_> {
    let Some(&id) = raw.first() else {
        return Err(ReportError::Empty);
    };
    if id != 0x02 {
        return data(raw, metadata, ReportValues::default());
    }
    require_length(raw, 2)?;
    let flags = raw[1];
    if bit(flags, 6) {
        require_length(raw, 9)?;
        return data(
            raw,
            metadata,
            ReportValues {
                position: Some([u16_at(raw, 2) as f32, u16_at(raw, 4) as f32]),
                pressure: Some(u16_at(raw, 6)),
                pen_buttons: Some(Buttons::from_bits(u64::from(flags >> 1), 2)?),
                eraser: Some(bit(flags, 3)),
                near_proximity: Some(bit(flags, 7)),
                hover_distance: Some(u32::from(raw[8])),
                ..ReportValues::default()
            },
        );
    }
    if flags == 0x80 {
        return out_of_range(raw, metadata);
    }
    data(raw, metadata, ReportValues::default())
}

pub fn parse_wacom_driver_intuos(raw: &[u8], metadata: ReportMetadata) -> Parsed<'_> {
    parse_intuos(strip_prefix(raw)?, metadata)
}

/// `Intuos3ReportParser`, `WacomDriverIntuos3ReportParser` and
/// `Intuos3ExtraAuxReportParser`, which differ only in the 0x0C aux report.
#[derive(Clone, Debug, Default)]
pub struct Intuos3Parser {
    state: IntuosV1State,
    prefixed: bool,
    extra_aux: bool,
}

impl Intuos3Parser {
    pub fn new(prefixed: bool, extra_aux: bool) -> Self {
        Self {
            prefixed,
            extra_aux,
            ..Self::default()
        }
    }

    pub fn reset(&mut self) {
        self.state = IntuosV1State::default();
    }

    pub fn parse<'a>(&mut self, raw: &'a [u8], metadata: ReportMetadata) -> Parsed<'a> {
        let raw = if self.prefixed {
            strip_prefix(raw)?
        } else {
            raw
        };
        let Some(&id) = raw.first() else {
            return Err(ReportError::Empty);
        };
        let values = match id {
            0x02 => {
                require_length(raw, 2)?;
                let flags = raw[1];
                if flags == 0xea || flags == 0xaa {
                    v1_rotation(raw, &mut self.state)?
                } else if matches!(flags & 0xf0, 0xe0 | 0xa0) {
                    v1_tablet(raw, &mut self.state)?
                } else if matches!(flags & 0xf0, 0xf0 | 0xb0) {
                    intuos3_mouse(raw)?
                } else if flags == 0xc2 {
                    v1_tool(raw)?
                } else {
                    ReportValues::default()
                }
            }
            0x10 => v1_tablet(raw, &mut self.state)?,
            0x03 => v1_aux(raw)?,
            0x0c => {
                require_length(raw, 7)?;
                let (mask, count) = if self.extra_aux { (0x1f, 5) } else { (0x0f, 4) };
                let bits = u64::from(raw[5] & mask) | (u64::from(raw[6] & mask) << count);
                ReportValues {
                    aux_buttons: Some(Buttons::from_bits(bits, count * 2)?),
                    ..ReportValues::default()
                }
            }
            _ => ReportValues::default(),
        };
        data(raw, metadata, values)
    }
}

fn scroll(up: bool, down: bool) -> [f32; 2] {
    [
        0.0,
        if up {
            1.0
        } else if down {
            -1.0
        } else {
            0.0
        },
    ]
}

fn intuos3_mouse(raw: &[u8]) -> Result<ReportValues, ReportError> {
    require_length(raw, 10)?;
    let b = raw[8];
    // Primary, secondary, middle, forward, backward.
    let bits = [2, 4, 3, 5, 6]
        .iter()
        .enumerate()
        .fold(0u64, |bits, (index, &source)| {
            bits | (u64::from(bit(b, source)) << index)
        });
    Ok(ReportValues {
        position: Some(high_resolution_position(raw)),
        mouse_buttons: Some(Buttons::from_bits(bits, 5)?),
        mouse_scroll: Some(scroll(bit(b, 0), bit(b, 1))),
        near_proximity: Some(bit(raw[1], 6)),
        hover_distance: Some(u32::from(raw[9])),
        ..ReportValues::default()
    })
}

fn intuos4_mouse(raw: &[u8]) -> Result<ReportValues, ReportError> {
    require_length(raw, 10)?;
    let b = raw[6];
    let bits = [0, 2, 1, 3, 4]
        .iter()
        .enumerate()
        .fold(0u64, |bits, (index, &source)| {
            bits | (u64::from(bit(b, source)) << index)
        });
    Ok(ReportValues {
        position: Some(high_resolution_position(raw)),
        mouse_buttons: Some(Buttons::from_bits(bits, 5)?),
        mouse_scroll: Some(scroll(bit(raw[7], 7), bit(raw[7], 6))),
        near_proximity: Some(bit(raw[1], 6)),
        hover_distance: Some(u32::from(raw[9]) >> 2),
        ..ReportValues::default()
    })
}

/// `Intuos4ReportParser` and `WacomDriverIntuos4ReportParser`. Pen reports go
/// to an owned IntuosV1 parser, as upstream's private field does.
#[derive(Clone, Debug, Default)]
pub struct Intuos4Parser {
    pen: IntuosV1Parser,
    prefixed: bool,
}

impl Intuos4Parser {
    pub fn new(prefixed: bool) -> Self {
        Self {
            prefixed,
            ..Self::default()
        }
    }

    pub fn reset(&mut self) {
        self.pen.reset();
    }

    pub fn parse<'a>(&mut self, raw: &'a [u8], metadata: ReportMetadata) -> Parsed<'a> {
        let raw = if self.prefixed {
            strip_prefix(raw)?
        } else {
            raw
        };
        let Some(&id) = raw.first() else {
            return Err(ReportError::Empty);
        };
        match id {
            0x02 => {
                require_length(raw, 2)?;
                if matches!(raw[1], 0xec | 0xac) {
                    data(raw, metadata, intuos4_mouse(raw)?)
                } else {
                    self.pen.parse(raw, metadata)
                }
            }
            0x10 => self.pen.parse(raw, metadata),
            0x0c => {
                require_length(raw, 4)?;
                data(raw, metadata, wheel_aux(raw[3], raw[1], raw[2])?)
            }
            _ => data(raw, metadata, ReportValues::default()),
        }
    }
}

/// `GraphireReportParser`.
pub fn parse_graphire(raw: &[u8], metadata: ReportMetadata) -> Parsed<'_> {
    let Some(&id) = raw.first() else {
        return Err(ReportError::Empty);
    };
    if id != 0x02 {
        return data(raw, metadata, ReportValues::default());
    }
    require_length(raw, 8)?;
    let flags = raw[1];
    let aux = Buttons::from_bits(u64::from(raw[7] >> 6), 2)?;
    let raw_pressure = u32::from(raw[6]) | (u32::from(raw[7] & 0x03) << 8);
    let has_position = bit(flags, 7)
        || u32::from_le_bytes([raw[2], raw[3], raw[4], raw[5]]) != 0
        || raw_pressure != 0;
    let position = Some([u16_at(raw, 2) as f32, u16_at(raw, 4) as f32]);
    let values = if !has_position {
        ReportValues {
            aux_buttons: Some(aux),
            ..ReportValues::default()
        }
    } else if bit(flags, 6) {
        let wheel = raw[7];
        let delta = f32::from(wheel & 1);
        ReportValues {
            position,
            mouse_scroll: Some([0.0, if bit(wheel, 1) { -delta } else { delta }]),
            mouse_buttons: Some(Buttons::from_bits(u64::from(flags & 7), 3)?),
            aux_buttons: Some(aux),
            ..ReportValues::default()
        }
    } else {
        ReportValues {
            position,
            pressure: Some(if bit(flags, 0) { raw_pressure } else { 0 }),
            eraser: Some(bit(flags, 5)),
            pen_buttons: Some(Buttons::from_bits(u64::from(flags >> 1), 2)?),
            aux_buttons: Some(aux),
            near_proximity: Some(bit(flags, 7)),
            hover_distance: Some(0),
            ..ReportValues::default()
        }
    };
    data(raw, metadata, values)
}

/// `PLReportParser`. Eraser and the second pen button share a bit; the
/// state latched when the pen enters range decides which it is.
#[derive(Clone, Debug)]
pub struct PlParser {
    initial_eraser: bool,
    last_out_of_range: bool,
}

impl Default for PlParser {
    fn default() -> Self {
        Self {
            initial_eraser: false,
            last_out_of_range: true,
        }
    }
}

impl PlParser {
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    pub fn parse<'a>(&mut self, raw: &'a [u8], metadata: ReportMetadata) -> Parsed<'a> {
        require_length(raw, 2)?;
        if !bit(raw[1], 6) {
            self.last_out_of_range = true;
            return out_of_range(raw, metadata);
        }
        // Upstream latches the eraser state before building the report, so a
        // packet too short for the report still updates it.
        require_length(raw, 5)?;
        if self.last_out_of_range {
            self.initial_eraser = bit(raw[4], 5);
            self.last_out_of_range = false;
        }
        require_length(raw, 8)?;
        let x = (u32::from(raw[1] & 0x03) << 14) + (u32::from(raw[2]) << 7) + u32::from(raw[3]);
        let y = (u32::from(raw[4] & 0x03) << 14) + (u32::from(raw[5]) << 7) + u32::from(raw[6]);
        let pressure = (u32::from(raw[7] ^ 0x40) << 2)
            + (u32::from(raw[4] & 0x40) >> 5)
            + (u32::from(raw[4] & 0x04) >> 2);
        let shared = bit(raw[4], 5);
        let buttons = u64::from(bit(raw[4], 4)) | (u64::from(shared && !self.initial_eraser) << 1);
        data(
            raw,
            metadata,
            ReportValues {
                position: Some([x as f32, y as f32]),
                pressure: Some(pressure),
                pen_buttons: Some(Buttons::from_bits(buttons, 2)?),
                eraser: Some(shared && self.initial_eraser),
                ..ReportValues::default()
            },
        )
    }
}

/// `PTUReportParser`: every packet is a pen report.
pub fn parse_ptu(raw: &[u8], metadata: ReportMetadata) -> Parsed<'_> {
    require_length(raw, 8)?;
    let flags = raw[1];
    let buttons = u64::from(bit(flags, 1)) | (u64::from(bit(flags, 4)) << 1);
    data(
        raw,
        metadata,
        ReportValues {
            position: Some([u16_at(raw, 2) as f32, u16_at(raw, 4) as f32]),
            pressure: Some(u16_at(raw, 6)),
            pen_buttons: Some(Buttons::from_bits(buttons, 2)?),
            eraser: Some(bit(flags, 2)),
            near_proximity: Some(bit(flags, 5)),
            hover_distance: Some(0),
            ..ReportValues::default()
        },
    )
}

const WACOM_TOUCH_POINTS: usize = 16;

/// `Wacom64bAuxReportParser` with `WacomTouchReport`: touch slots persist
/// between packets, and a chunk with ID 0x80 carries four aux buttons.
#[derive(Clone, Debug, Default)]
pub struct Wacom64bAuxParser {
    slots: [Option<TouchPoint>; WACOM_TOUCH_POINTS],
}

impl Wacom64bAuxParser {
    pub fn reset(&mut self) {
        self.slots = [None; WACOM_TOUCH_POINTS];
    }

    pub fn parse<'a>(&mut self, raw: &'a [u8], metadata: ReportMetadata) -> Parsed<'a> {
        require_length(raw, 3)?;
        let mut aux = Buttons::from_bits(0, 0)?;
        if raw[2] == 0x81 {
            require_length(raw, 5)?;
            let mut mask = u16::from_le_bytes([raw[3], raw[4]]);
            for slot in &mut self.slots {
                if mask & 1 == 0 {
                    *slot = None;
                }
                mask >>= 1;
            }
        } else {
            let chunks = usize::from(raw[1]);
            // Check the bytes each chunk actually reads, as upstream reads
            // them, before applying any chunk: a truncated packet fails
            // without releasing or moving a contact.
            for chunk in 0..chunks {
                let offset = (chunk << 3) + 2;
                require_length(raw, offset + 1)?;
                let id = raw[offset];
                if id == 0x80 || usize::from(id.wrapping_sub(2)) < WACOM_TOUCH_POINTS {
                    require_length(raw, offset + 2)?;
                    if id != 0x80 && raw[offset + 1] != 0x20 {
                        require_length(raw, offset + 5)?;
                    }
                }
            }
            for chunk in 0..chunks {
                let offset = (chunk << 3) + 2;
                let id = raw[offset];
                if id == 0x80 {
                    aux = Buttons::from_bits(u64::from(raw[offset + 1] & 0x0f), 4)?;
                    continue;
                }
                let id = id.wrapping_sub(2);
                let Some(slot) = self.slots.get_mut(usize::from(id)) else {
                    continue;
                };
                *slot = if raw[offset + 1] == 0x20 {
                    None
                } else {
                    let low = raw[offset + 4];
                    Some(TouchPoint {
                        id,
                        position: [
                            f32::from(u16::from(raw[offset + 2]) << 4 | u16::from(low >> 4)),
                            f32::from(u16::from(raw[offset + 3]) << 4 | u16::from(low & 0x0f)),
                        ],
                    })
                };
            }
        }
        data(
            raw,
            metadata,
            ReportValues {
                aux_buttons: Some(aux),
                touches: Some(Touches::from_slice(&self.slots)?),
                ..ReportValues::default()
            },
        )
    }
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
    fn intuos_v1_tablet_rotation_and_tool_reports() {
        let mut parser = IntuosV1Parser::new(false);
        // Tablet report: flags 0xE3 (bit5 tablet, bit0 pressure LSB, bit1 button 1).
        let tablet = [0x02, 0xe3, 0x12, 0x34, 0x05, 0x06, 0x80, 0xc0, 0x40, 0x03];
        let (kind, report) = parser.parse(&tablet, metadata()).unwrap();
        assert_eq!(kind, ReportKind::Data);
        let v = report.values;
        assert_eq!(
            v.position,
            Some([((0x1234 << 1) | 1) as f32, ((0x0506 << 1) | 1) as f32])
        );
        assert_eq!(v.pressure, Some((0x80 << 3) | 6 | 1));
        assert_eq!(v.tilt, Some([(((0xc0 << 1) & 0x7e) as f32) - 64.0, 0.0]));
        assert_eq!(v.pen_buttons.unwrap().get(0), Some(true));
        assert_eq!(v.near_proximity, Some(true));
        // A rotation report repeats pressure, tilt and buttons.
        let rotation = [0x02, 0xea, 0, 0, 0, 0, 0, 0, 0, 0];
        let (_, report) = parser.parse(&rotation, metadata()).unwrap();
        assert_eq!(report.values.pressure, v.pressure);
        assert_eq!(report.values.tilt, v.tilt);
        assert_eq!(
            parser.parse(&[0x02, 0x80], metadata()).unwrap().0,
            ReportKind::OutOfRange
        );
        let tool = [0x02, 0xc2, 0x80, 0x8f, 0xff, 0xff, 0xff, 0xff, 0xff, 0x08];
        let (_, report) = parser.parse(&tool, metadata()).unwrap();
        let identity = report.values.tool.unwrap();
        assert_eq!(identity.tool, ToolType::Eraser);
        assert_eq!(identity.serial >> 32, 0xffff_ffff, "upstream sign-extends");
        assert_eq!(report.values.hover_distance, Some(2));
        assert!(parser.parse(&[0x02, 0xe0, 0], metadata()).is_err());
    }

    #[test]
    fn prefixed_parsers_drop_one_byte() {
        let mut parser = IntuosV1Parser::new(true);
        let (kind, report) = parser.parse(&[0xaa, 0x02, 0x80], metadata()).unwrap();
        assert_eq!(kind, ReportKind::OutOfRange);
        assert_eq!(report.raw, &[0x02, 0x80]);
        assert!(parser.parse(&[0xaa], metadata()).is_err());
    }

    #[test]
    fn intuos3_intuos4_and_pro_aux_reports() {
        let mut i3 = Intuos3Parser::new(false, false);
        let (_, r) = i3
            .parse(&[0x0c, 0, 0, 0, 0, 0x05, 0x0a], metadata())
            .unwrap();
        assert_eq!(r.values.aux_buttons.unwrap().bits(), 0x05 | (0x0a << 4));
        let mut extra = Intuos3Parser::new(false, true);
        let (_, r) = extra
            .parse(&[0x0c, 0, 0, 0, 0, 0x10, 0x10], metadata())
            .unwrap();
        assert_eq!(r.values.aux_buttons.unwrap().bits(), 0x10 | (0x10 << 5));
        let mut i4 = Intuos4Parser::new(false);
        let (_, r) = i4.parse(&[0x0c, 0x85, 0x01, 0x81], metadata()).unwrap();
        assert_eq!(
            r.values.absolute_analog.unwrap().positions.as_slice(),
            &[Some(5)]
        );
        assert_eq!(r.values.aux_buttons.unwrap().bits(), 0x81);
        let (_, r) = i4.parse(&[0x0c, 0x05, 0, 0], metadata()).unwrap();
        assert_eq!(
            r.values.absolute_analog.unwrap().positions.as_slice(),
            &[None]
        );
        let mut pro = IntuosProParser::new(false);
        let (_, r) = pro.parse(&[0x03, 0, 0x90, 1, 0x0f], metadata()).unwrap();
        assert_eq!(
            r.values.wheel_buttons.unwrap().as_slice()[0].get(0),
            Some(true)
        );
        let mouse = [0x02, 0xf0, 0, 1, 0, 1, 0, 0, 0b0000_0101, 0];
        let (_, r) = i3.parse(&mouse, metadata()).unwrap();
        assert_eq!(r.values.mouse_buttons.unwrap().get(0), Some(true));
        assert_eq!(r.values.mouse_scroll, Some([0.0, 1.0]));
    }

    #[test]
    fn graphire_pl_ptu_and_touch() {
        let pen = [0x02, 0x81, 0x10, 0x00, 0x20, 0x00, 0xff, 0x41];
        let (_, r) = parse_graphire(&pen, metadata()).unwrap();
        assert_eq!(r.values.pressure, Some(0x1ff));
        assert_eq!(r.values.aux_buttons.unwrap().get(0), Some(true));
        let idle = [0x02, 0, 0, 0, 0, 0, 0, 0x80];
        let (_, r) = parse_graphire(&idle, metadata()).unwrap();
        assert!(r.values.position.is_none());
        let mut pl = PlParser::default();
        assert_eq!(
            pl.parse(&[0, 0x00], metadata()).unwrap().0,
            ReportKind::OutOfRange
        );
        let entering = [0, 0x41, 0x01, 0x02, 0x20, 0x03, 0x04, 0x40];
        let (_, r) = pl.parse(&entering, metadata()).unwrap();
        assert_eq!(r.values.eraser, Some(true));
        assert_eq!(
            r.values.position,
            Some([((1 << 14) + (1 << 7) + 2) as f32, ((3 << 7) + 4) as f32])
        );
        assert_eq!(r.values.pressure, Some(0));
        let (_, r) = parse_ptu(&[0, 0x26, 1, 0, 2, 0, 3, 0], metadata()).unwrap();
        assert_eq!(
            (r.values.eraser, r.values.near_proximity),
            (Some(true), Some(true))
        );
        let mut touch = Wacom64bAuxParser::default();
        let packet = [
            0, 2, 2, 0x81, 0x10, 0x20, 0x35, 0, 0, 0, 0x80, 0x05, 0, 0, 0,
        ];
        let (_, r) = touch.parse(&packet, metadata()).unwrap();
        let slots = r.values.touches.unwrap();
        assert_eq!(
            slots.as_slice()[0].unwrap().position,
            [0x103 as f32, 0x205 as f32]
        );
        assert_eq!(r.values.aux_buttons.unwrap().bits(), 5);
        let (_, r) = touch.parse(&[0, 0, 0x81, 0, 0], metadata()).unwrap();
        assert!(r.values.touches.unwrap().as_slice()[0].is_none());
        assert!(touch.parse(&[0, 3, 2, 0, 0, 0, 0], metadata()).is_err());
        // A chunk whose ID is out of range reads nothing more.
        assert!(touch.parse(&[0, 1, 0x40], metadata()).is_ok());
    }
}
