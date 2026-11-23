//! Report parser selection by OpenTabletDriver type name, and the pen
//! decoders a device session runs.
//!
//! `ReportParser` covers every parser type the pinned configurations name and
//! yields upstream's full report values (pen, auxiliary, touch, wheel, mouse).
//! Runtime decoding preserves that envelope for the pipeline. IntuosV2
//! tablets keep the compact checked PTH-660 decoder with their own ranges;
//! `PenDecoder::decode` remains the legacy position-only view for callers that
//! need it. Neither path allocates per report.

use std::time::Duration;

use crate::protocol::{self, ParseError, PenReport};
use crate::reports::*;
use crate::spec::TabletSpec;

type Stateless =
    for<'a> fn(&'a [u8], ReportMetadata) -> Result<(ReportKind, ReportEnvelope<'a>), ReportError>;
type Prefixed =
    for<'a> fn(&'a [u8], ReportMetadata) -> Result<(ReportKind, TransportReport<'a>), ReportError>;
type Envelope = for<'a> fn(&'a [u8], ReportMetadata) -> Result<ReportEnvelope<'a>, ReportError>;

pub const PASSTHROUGH: &str = "OpenTabletDriver.Plugin.Tablet.PassthroughReportParser";
const INTUOS_V2: &str =
    "OpenTabletDriver.Configurations.Parsers.Wacom.IntuosV2.IntuosV2ReportParser";
const WACOM_DRIVER_INTUOS_V2: &str =
    "OpenTabletDriver.Configurations.Parsers.Wacom.IntuosV2.WacomDriverIntuosV2ReportParser";

/// One upstream report parser with its state. One instance per endpoint.
#[derive(Clone, Debug)]
pub enum ReportParser {
    Passthrough,
    Stateless(Stateless),
    Prefixed(Prefixed),
    Envelope(Envelope),
    IntuosV2 {
        prefixed: bool,
        touch: IntuosV2TouchParser,
    },
    IntuosV1(IntuosV1Parser),
    IntuosPro(IntuosProParser),
    Intuos3(Intuos3Parser),
    Intuos4(Intuos4Parser),
    CintiqV1(CintiqV1Parser),
    Pl(PlParser),
    Wacom64bAux(Wacom64bAuxParser),
    Acepen(AcepenParser),
    Deco03(Deco03Parser),
}

/// Every parser type name this registry implements, for inventories.
pub const TYPE_NAMES: &[&str] = &[
    PASSTHROUGH,
    "OpenTabletDriver.Plugin.Tablet.TabletReportParser",
    "OpenTabletDriver.Plugin.Tablet.AuxReportParser",
    "OpenTabletDriver.Configurations.Parsers.SkipByteTabletReportParser",
    "OpenTabletDriver.Configurations.Parsers.Acepen.AcepenReportParser",
    "OpenTabletDriver.Configurations.Parsers.Bosto.BostoReportParser",
    "OpenTabletDriver.Configurations.Parsers.FlooGoo.FmaReportParser",
    "OpenTabletDriver.Configurations.Parsers.Genius.GeniusReportParser",
    "OpenTabletDriver.Configurations.Parsers.Genius.GeniusReportParserV2",
    "OpenTabletDriver.Configurations.Parsers.Huion.GianoReportParser",
    "OpenTabletDriver.Configurations.Parsers.Huion.KamvasOffsetReportParser",
    "OpenTabletDriver.Configurations.Parsers.Huion.HuionTiltReportParser",
    "OpenTabletDriver.Configurations.Parsers.Huion.InspiroyReportParser",
    "OpenTabletDriver.Configurations.Parsers.Lifetec.LifetecReportParser",
    "OpenTabletDriver.Configurations.Parsers.RobotPen.RobotPenReportParser",
    "OpenTabletDriver.Configurations.Parsers.Silvercrest.SilvercrestReportParser",
    "OpenTabletDriver.Configurations.Parsers.UCLogic.UCLogicReportParser",
    "OpenTabletDriver.Configurations.Parsers.UCLogic.UCLogicTiltReportParser",
    "OpenTabletDriver.Configurations.Parsers.UCLogic.UCLogicV1ReportParser",
    "OpenTabletDriver.Configurations.Parsers.UCLogic.UCLogicV2ReportParser",
    "OpenTabletDriver.Configurations.Parsers.Veikk.VeikkA15ReportParser",
    "OpenTabletDriver.Configurations.Parsers.Veikk.VeikkReportParser",
    "OpenTabletDriver.Configurations.Parsers.Veikk.VeikkTiltReportParser",
    "OpenTabletDriver.Configurations.Parsers.Veikk.VeikkV1ReportParser",
    "OpenTabletDriver.Configurations.Parsers.ViewSonic.WoodPadReportParser",
    "OpenTabletDriver.Configurations.Parsers.Wacom.Bamboo.BambooReportParser",
    "OpenTabletDriver.Configurations.Parsers.Wacom.BambooPad.BambooPadReportParser",
    "OpenTabletDriver.Configurations.Parsers.Wacom.BambooV2.BambooV2AuxReportParser",
    "OpenTabletDriver.Configurations.Parsers.Wacom.CintiqV1.CintiqV1ReportParser",
    "OpenTabletDriver.Configurations.Parsers.Wacom.Graphire.GraphireReportParser",
    "OpenTabletDriver.Configurations.Parsers.Wacom.Intuos.IntuosReportParser",
    "OpenTabletDriver.Configurations.Parsers.Wacom.Intuos.WacomDriverIntuosReportParser",
    "OpenTabletDriver.Configurations.Parsers.Wacom.Intuos3.Intuos3ExtraAuxReportParser",
    "OpenTabletDriver.Configurations.Parsers.Wacom.Intuos3.Intuos3ReportParser",
    "OpenTabletDriver.Configurations.Parsers.Wacom.Intuos3.WacomDriverIntuos3ReportParser",
    "OpenTabletDriver.Configurations.Parsers.Wacom.Intuos4.Intuos4ReportParser",
    "OpenTabletDriver.Configurations.Parsers.Wacom.Intuos4.WacomDriverIntuos4ReportParser",
    "OpenTabletDriver.Configurations.Parsers.Wacom.IntuosPro.IntuosProReportParser",
    "OpenTabletDriver.Configurations.Parsers.Wacom.IntuosPro.WacomDriverIntuosProReportParser",
    "OpenTabletDriver.Configurations.Parsers.Wacom.IntuosV1.IntuosV1ReportParser",
    "OpenTabletDriver.Configurations.Parsers.Wacom.IntuosV1.WacomDriverIntuosV1ReportParser",
    INTUOS_V2,
    WACOM_DRIVER_INTUOS_V2,
    "OpenTabletDriver.Configurations.Parsers.Wacom.IntuosV3.IntuosV3ReportParser",
    "OpenTabletDriver.Configurations.Parsers.Wacom.PL.PLReportParser",
    "OpenTabletDriver.Configurations.Parsers.Wacom.PTU.PTUReportParser",
    "OpenTabletDriver.Configurations.Parsers.Wacom.Wacom64bAuxReportParser",
    "OpenTabletDriver.Configurations.Parsers.XENX.XENXReportParser",
    "OpenTabletDriver.Configurations.Parsers.XP_Pen.XP_PenDeco03ReportParser",
    "OpenTabletDriver.Configurations.Parsers.XP_Pen.XP_PenDedicatedAuxReportParser",
    "OpenTabletDriver.Configurations.Parsers.XP_Pen.XP_PenGen2ReportParser",
    "OpenTabletDriver.Configurations.Parsers.XP_Pen.XP_PenOffsetAuxReportParser",
    "OpenTabletDriver.Configurations.Parsers.XP_Pen.XP_PenOffsetPressureReportParser",
    "OpenTabletDriver.Configurations.Parsers.XP_Pen.XP_PenReportParser",
    "OpenTabletDriver.Configurations.Parsers.XenceLabs.XenceLabsReportParser",
];

impl ReportParser {
    /// The parser for an upstream type name, or `None` when it is not known.
    /// An unknown parser never falls back to another layout.
    pub fn for_type(type_name: &str) -> Option<Self> {
        let short = type_name
            .strip_prefix("OpenTabletDriver.Configurations.Parsers.")
            .or_else(|| type_name.strip_prefix("OpenTabletDriver.Plugin.Tablet."))?;
        Some(match short {
            "PassthroughReportParser" => Self::Passthrough,
            "TabletReportParser" => Self::Stateless(parse_tablet),
            "AuxReportParser" => Self::Stateless(parse_auxiliary),
            "SkipByteTabletReportParser" => Self::Prefixed(parse_skip_byte_tablet),
            "Acepen.AcepenReportParser" => Self::Acepen(AcepenParser::default()),
            "Bosto.BostoReportParser" => Self::Stateless(parse_bosto),
            "FlooGoo.FmaReportParser" => Self::Stateless(parse_floogoo),
            "Genius.GeniusReportParser" => Self::Stateless(parse_genius),
            "Genius.GeniusReportParserV2" => Self::Stateless(parse_genius_v2),
            "Huion.GianoReportParser" => Self::Stateless(parse_huion_giano),
            "Huion.KamvasOffsetReportParser" => Self::Stateless(parse_huion_kamvas_offset),
            "Huion.HuionTiltReportParser" => Self::Stateless(parse_huion_tilt),
            "Huion.InspiroyReportParser" => Self::Stateless(parse_huion_inspiroy),
            "Lifetec.LifetecReportParser" => Self::Stateless(parse_lifetec),
            "RobotPen.RobotPenReportParser" => Self::Stateless(parse_robot_pen),
            "Silvercrest.SilvercrestReportParser" => Self::Stateless(parse_silvercrest),
            "UCLogic.UCLogicReportParser" => Self::Stateless(parse_uc_logic),
            "UCLogic.UCLogicTiltReportParser" => Self::Stateless(parse_uc_logic_tilt),
            "UCLogic.UCLogicV1ReportParser" => Self::Stateless(parse_uc_logic_v1),
            "UCLogic.UCLogicV2ReportParser" => Self::Stateless(parse_uc_logic_v2),
            "Veikk.VeikkA15ReportParser" => Self::Stateless(parse_veikk_a15),
            "Veikk.VeikkReportParser" => Self::Stateless(parse_veikk),
            "Veikk.VeikkTiltReportParser" => Self::Stateless(parse_veikk_tilt),
            "Veikk.VeikkV1ReportParser" => Self::Prefixed(parse_veikk_v1),
            "ViewSonic.WoodPadReportParser" => Self::Stateless(parse_wood_pad),
            "Wacom.Bamboo.BambooReportParser" => Self::Envelope(parse_bamboo),
            "Wacom.BambooPad.BambooPadReportParser" => Self::Envelope(parse_bamboo_pad),
            "Wacom.BambooV2.BambooV2AuxReportParser" => Self::Envelope(parse_bamboo_v2_auxiliary),
            "Wacom.CintiqV1.CintiqV1ReportParser" => Self::CintiqV1(CintiqV1Parser::default()),
            "Wacom.Graphire.GraphireReportParser" => Self::Stateless(parse_graphire),
            "Wacom.Intuos.IntuosReportParser" => Self::Stateless(parse_intuos),
            "Wacom.Intuos.WacomDriverIntuosReportParser" => {
                Self::Stateless(parse_wacom_driver_intuos)
            }
            "Wacom.Intuos3.Intuos3ExtraAuxReportParser" => {
                Self::Intuos3(Intuos3Parser::new(false, true))
            }
            "Wacom.Intuos3.Intuos3ReportParser" => Self::Intuos3(Intuos3Parser::new(false, false)),
            "Wacom.Intuos3.WacomDriverIntuos3ReportParser" => {
                Self::Intuos3(Intuos3Parser::new(true, false))
            }
            "Wacom.Intuos4.Intuos4ReportParser" => Self::Intuos4(Intuos4Parser::new(false)),
            "Wacom.Intuos4.WacomDriverIntuos4ReportParser" => {
                Self::Intuos4(Intuos4Parser::new(true))
            }
            "Wacom.IntuosPro.IntuosProReportParser" => Self::IntuosPro(IntuosProParser::new(false)),
            "Wacom.IntuosPro.WacomDriverIntuosProReportParser" => {
                Self::IntuosPro(IntuosProParser::new(true))
            }
            "Wacom.IntuosV1.IntuosV1ReportParser" => Self::IntuosV1(IntuosV1Parser::new(false)),
            "Wacom.IntuosV1.WacomDriverIntuosV1ReportParser" => {
                Self::IntuosV1(IntuosV1Parser::new(true))
            }
            "Wacom.IntuosV2.IntuosV2ReportParser" => Self::IntuosV2 {
                prefixed: false,
                touch: IntuosV2TouchParser::default(),
            },
            "Wacom.IntuosV2.WacomDriverIntuosV2ReportParser" => Self::IntuosV2 {
                prefixed: true,
                touch: IntuosV2TouchParser::default(),
            },
            "Wacom.IntuosV3.IntuosV3ReportParser" => Self::Envelope(parse_intuos_v3),
            "Wacom.PL.PLReportParser" => Self::Pl(PlParser::default()),
            "Wacom.PTU.PTUReportParser" => Self::Stateless(parse_ptu),
            "Wacom.Wacom64bAuxReportParser" => Self::Wacom64bAux(Wacom64bAuxParser::default()),
            "XENX.XENXReportParser" => Self::Stateless(parse_xenx),
            "XP_Pen.XP_PenDeco03ReportParser" => Self::Deco03(Deco03Parser::default()),
            "XP_Pen.XP_PenDedicatedAuxReportParser" => {
                Self::Stateless(parse_xp_pen_dedicated_auxiliary)
            }
            "XP_Pen.XP_PenGen2ReportParser" => Self::Stateless(parse_xp_pen_gen2),
            "XP_Pen.XP_PenOffsetAuxReportParser" => Self::Stateless(parse_xp_pen_offset_auxiliary),
            "XP_Pen.XP_PenOffsetPressureReportParser" => {
                Self::Stateless(parse_xp_pen_offset_pressure)
            }
            "XP_Pen.XP_PenReportParser" => Self::Stateless(parse_xp_pen),
            "XenceLabs.XenceLabsReportParser" => Self::Stateless(parse_xencelabs),
            _ => return None,
        })
    }

    /// Clears the parser's state when its endpoint's session ends.
    pub fn reset(&mut self) {
        match self {
            Self::IntuosV2 { touch, .. } => touch.reset(),
            Self::IntuosV1(parser) => parser.reset(),
            Self::IntuosPro(parser) => parser.reset(),
            Self::Intuos3(parser) => parser.reset(),
            Self::Intuos4(parser) => parser.reset(),
            Self::CintiqV1(parser) => parser.reset(),
            Self::Pl(parser) => parser.reset(),
            Self::Wacom64bAux(parser) => parser.reset(),
            Self::Acepen(parser) => parser.reset(),
            Self::Deco03(parser) => parser.reset(),
            Self::Passthrough | Self::Stateless(_) | Self::Prefixed(_) | Self::Envelope(_) => {}
        }
    }

    /// Decodes one transport packet. `report.raw` is upstream's `Raw`, which a
    /// prefixed parser shortens by its prefix byte.
    pub fn parse<'a>(
        &mut self,
        raw: &'a [u8],
        metadata: ReportMetadata,
    ) -> Result<(ReportKind, ReportEnvelope<'a>), ReportError> {
        let plain = |values| {
            Ok((
                ReportKind::Data,
                ReportEnvelope {
                    metadata,
                    raw,
                    values,
                },
            ))
        };
        match self {
            Self::Passthrough => plain(ReportValues::default()),
            Self::Stateless(parse) => parse(raw, metadata),
            Self::Prefixed(parse) => {
                parse(raw, metadata).map(|(kind, report)| (kind, report.report))
            }
            Self::Envelope(parse) => parse(raw, metadata).map(|report| (ReportKind::Data, report)),
            Self::IntuosV2 { prefixed, touch } => {
                let payload = if *prefixed {
                    match raw.split_first() {
                        Some((_, payload)) if !payload.is_empty() => payload,
                        _ => {
                            return Err(raw.first().map_or(ReportError::Empty, |&id| {
                                ReportError::Short {
                                    id,
                                    got: raw.len(),
                                    need: 2,
                                }
                            }));
                        }
                    }
                } else {
                    raw
                };
                intuos_v2(payload, metadata, touch)
            }
            Self::IntuosV1(parser) => parser.parse(raw, metadata),
            Self::IntuosPro(parser) => parser.parse(raw, metadata),
            Self::Intuos3(parser) => parser.parse(raw, metadata),
            Self::Intuos4(parser) => parser.parse(raw, metadata),
            Self::CintiqV1(parser) => parser.parse(raw, metadata),
            Self::Pl(parser) => parser.parse(raw, metadata),
            Self::Wacom64bAux(parser) => parser.parse(raw, metadata),
            Self::Acepen(parser) => parser.parse(raw, metadata),
            Self::Deco03(parser) => parser.parse(raw, metadata),
        }
    }
}

/// `IntuosV2ReportParser`: 0x10/0x1E pen, 0x11 auxiliary, 0x21/0xD2 touch.
/// Pen positions are not range-checked here; the pen decoder checks them
/// against the matched tablet.
fn intuos_v2<'a>(
    raw: &'a [u8],
    metadata: ReportMetadata,
    touch: &mut IntuosV2TouchParser,
) -> Result<(ReportKind, ReportEnvelope<'a>), ReportError> {
    let Some(&id) = raw.first() else {
        return Err(ReportError::Empty);
    };
    let envelope = match id {
        0x10 | 0x1e => {
            let pen = protocol::parse_within(raw, u32::MAX, u32::MAX, u16::MAX)
                .map_err(|error| match error {
                    ParseError::Short { id, got, need } => ReportError::Short { id, got, need },
                    _ => ReportError::Empty,
                })?
                .expect("0x10 and 0x1E are pen reports");
            Some(from_pth660(pen, raw, metadata)?)
        }
        0x11 => parse_intuos_auxiliary(raw, metadata)?,
        0x21 | 0xd2 => touch.parse(raw, metadata)?,
        _ => None,
    };
    Ok((
        ReportKind::Data,
        envelope.unwrap_or(ReportEnvelope {
            metadata,
            raw,
            values: ReportValues::default(),
        }),
    ))
}

/// Why a packet did not become a pen report.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DecodeError {
    Pen(ParseError),
    Report(ReportError),
    /// A trusted managed parser failed; preserve its exact setup/parse error.
    Managed(String),
}

/// One decoded pen and the parser's payload, borrowed from the same input.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DecodedPen<'a> {
    pub pen: PenReport,
    pub raw: &'a [u8],
    pub buttons: Option<Buttons>,
}

/// A compact IntuosV2 input, or the original capabilities of another parser.
/// The report owns inline values and borrows only its canonical raw payload.
#[derive(Clone, Copy, Debug, PartialEq)]
#[allow(clippy::large_enum_variant)] // Inline report values keep decoding allocation-free.
pub enum DecodedInput<'a> {
    Pen(DecodedPen<'a>),
    Report {
        kind: ReportKind,
        report: ReportEnvelope<'a>,
        /// Legacy pen view for capture and position-only filter callers. It
        /// never replaces the parser's values when dispatching the report.
        pen: Option<PenReport>,
    },
}

impl DecodedInput<'_> {
    pub fn pen(&self) -> Option<PenReport> {
        match self {
            Self::Pen(decoded) => Some(decoded.pen),
            Self::Report { pen, .. } => *pen,
        }
    }
}

/// Compact zero-copy pen sample for native passthrough fast paths (both
/// IntuosV2 and stateless non-IntuosV2 pen parsers). Avoids constructing or
/// copying the full ~1 KiB `ReportValues` envelope on every report.
#[doc(hidden)]
#[derive(Clone, Copy, Debug)]
pub struct CompactPen {
    pub x: u32,
    pub y: u32,
    pub position_exact: bool,
    pub pos_f32: [f32; 2],
    pub has_position: bool,
    pub pressure: u32,
    pub pen_buttons: Option<Buttons>,
    pub tilt: Option<[f32; 2]>,
    pub eraser: Option<bool>,
    pub rotation: Option<i32>,
    pub tip_switch: Option<bool>,
    pub physical_loss: bool,
}

/// Fast-path tag for common stateless non-IntuosV2 pen parsers so
/// `TabletDecoder::Values` can decode pen/OutOfRange packets into `CompactPen`
/// without zeroing a 1 KiB `ReportValues` struct or calling `roundf`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FastParser {
    None,
    GenericTablet,
    XpPenBase,
    XpPenGen2,
    XpPenOffsetPressure,
    XpPenOffsetAux,
    HuionGiano,
    HuionInspiroy,
    HuionTilt,
    HuionKamvasOffset,
    UcLogicBase,
    UcLogicTilt,
    UcLogicV1,
    UcLogicV2,
    VeikkBase,
    VeikkA15,
    VeikkTilt,
    IntuosV3,
}

impl FastParser {
    #[inline]
    #[allow(clippy::too_many_arguments)]
    fn compact_pen(
        x: u32,
        y: u32,
        pressure: u32,
        buttons: Buttons,
        tilt: Option<[f32; 2]>,
        eraser: Option<bool>,
        rotation: Option<i32>,
        spec: TabletSpec,
    ) -> CompactPen {
        let fx = x as f32;
        let fy = y as f32;
        let position_exact =
            x <= spec.max_x && y <= spec.max_y && (fx as u32) == x && (fy as u32) == y;
        CompactPen {
            x,
            y,
            position_exact,
            pos_f32: [fx, fy],
            has_position: true,
            pressure,
            pen_buttons: Some(buttons),
            tilt,
            eraser,
            rotation,
            tip_switch: None,
            physical_loss: false,
        }
    }

    #[inline]
    fn out_of_range() -> CompactPen {
        CompactPen {
            x: 0,
            y: 0,
            position_exact: false,
            pos_f32: [0.0, 0.0],
            has_position: false,
            pressure: 0,
            pen_buttons: None,
            tilt: None,
            eraser: None,
            rotation: None,
            tip_switch: None,
            physical_loss: true,
        }
    }

    #[inline]
    fn require(raw: &[u8], need: usize) -> Result<(), DecodeError> {
        let Some(&id) = raw.first() else {
            return Err(DecodeError::Report(ReportError::Empty));
        };
        if raw.len() < need {
            return Err(DecodeError::Report(ReportError::Short {
                id,
                got: raw.len(),
                need,
            }));
        }
        Ok(())
    }

    #[inline]
    fn decode(
        self,
        raw: &[u8],
        spec: TabletSpec,
    ) -> Option<Result<Option<CompactPen>, DecodeError>> {
        if self == Self::None {
            return None;
        }
        match self {
            Self::None => None,
            Self::GenericTablet => {
                if let Err(e) = Self::require(raw, 8) {
                    return Some(Err(e));
                }
                let x = u32::from(u16::from_le_bytes([raw[2], raw[3]]));
                let y = u32::from(u16::from_le_bytes([raw[4], raw[5]]));
                let pressure = u32::from(u16::from_le_bytes([raw[6], raw[7]]));
                let buttons = Buttons::from_bits_unchecked(u64::from(raw[1] >> 1), 3);
                Some(Ok(Some(Self::compact_pen(
                    x, y, pressure, buttons, None, None, None, spec,
                ))))
            }
            Self::XpPenBase | Self::XpPenOffsetPressure | Self::XpPenOffsetAux => {
                if let Err(e) = Self::require(raw, 2) {
                    return Some(Err(e));
                }
                let flags = raw[1];
                if flags == 0xc0 {
                    return Some(Ok(Some(Self::out_of_range())));
                }
                let (offset_pressure, allow_overflow) = match self {
                    Self::XpPenOffsetAux => {
                        if flags & 0x20 != 0 {
                            return None;
                        }
                        (false, false)
                    }
                    Self::XpPenBase | Self::XpPenOffsetPressure => {
                        if matches!(flags, 0xb4 | 0xf2 | 0xf8) || flags & 0x10 != 0 {
                            return None;
                        }
                        (self == Self::XpPenOffsetPressure, true)
                    }
                    _ => unreachable!(),
                };
                if let Err(e) = Self::require(raw, 8) {
                    return Some(Err(e));
                }
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
                let tilt = has_tilt.then(|| [f32::from(raw[8] as i8), f32::from(raw[9] as i8)]);
                let eraser = has_eraser.then_some(flags & 0x08 != 0);
                let buttons = Buttons::from_bits_unchecked(
                    u64::from(flags >> 1),
                    if has_eraser { 2 } else { 3 },
                );
                Some(Ok(Some(Self::compact_pen(
                    x, y, pressure, buttons, tilt, eraser, None, spec,
                ))))
            }
            Self::XpPenGen2 => {
                if let Err(e) = Self::require(raw, 2) {
                    return Some(Err(e));
                }
                let flags = raw[1];
                if flags == 0xc0 {
                    return Some(Ok(Some(Self::out_of_range())));
                }
                if flags & 0xf0 != 0xa0 {
                    return None;
                }
                if let Err(e) = Self::require(raw, 14) {
                    return Some(Err(e));
                }
                let x =
                    u32::from(u16::from_le_bytes([raw[2], raw[3]])) | (u32::from(raw[10]) << 16);
                let y =
                    u32::from(u16::from_le_bytes([raw[4], raw[5]])) | (u32::from(raw[11]) << 16);
                let pressure = (u32::from(u16::from_le_bytes([raw[6], raw[7]])) & 0x1fff)
                    | (u32::from(raw[13] & 1) << 13);
                let tilt = Some([f32::from(raw[8] as i8), f32::from(raw[9] as i8)]);
                let eraser = Some(flags & 0x08 != 0);
                let buttons = Buttons::from_bits_unchecked(u64::from(flags >> 1), 2);
                Some(Ok(Some(Self::compact_pen(
                    x, y, pressure, buttons, tilt, eraser, None, spec,
                ))))
            }
            Self::HuionGiano => {
                if let Err(e) = Self::require(raw, 2) {
                    return Some(Err(e));
                }
                let flags = raw[1];
                if flags == 0xf1 || flags == 0xf0 || flags & 0x60 == 0x60 {
                    return None;
                }
                if let Err(e) = Self::require(raw, 12) {
                    return Some(Err(e));
                }
                let x =
                    u32::from(u16::from_le_bytes([raw[2], raw[3]])) | (u32::from(raw[8] & 1) << 16);
                let y =
                    u32::from(u16::from_le_bytes([raw[4], raw[5]])) | (u32::from(raw[9] & 1) << 16);
                let pressure = u32::from(u16::from_le_bytes([raw[6], raw[7]]));
                let tilt = Some([
                    f32::from(-i16::from(raw[10] as i8)),
                    f32::from(-i16::from(raw[11] as i8)),
                ]);
                let buttons = Buttons::from_bits_unchecked(u64::from(flags >> 1), 3);
                Some(Ok(Some(Self::compact_pen(
                    x,
                    y,
                    pressure,
                    buttons,
                    tilt,
                    Some(flags & 0x20 != 0),
                    None,
                    spec,
                ))))
            }
            Self::HuionInspiroy => {
                if let Err(e) = Self::require(raw, 2) {
                    return Some(Err(e));
                }
                let flags = raw[1];
                if flags == 0x00 {
                    return Some(Ok(Some(Self::out_of_range())));
                }
                if matches!(flags, 0xe0 | 0xe3 | 0xf1) || flags & 0x80 == 0 {
                    return None;
                }
                if let Err(e) = Self::require(raw, 12) {
                    return Some(Err(e));
                }
                let x = u32::from(u16::from_le_bytes([raw[2], raw[3]]));
                let y = u32::from(u16::from_le_bytes([raw[4], raw[5]]));
                let pressure = u32::from(u16::from_le_bytes([raw[6], raw[7]]));
                let tilt = Some([
                    f32::from(i16::from(raw[10] as i8)),
                    f32::from(-i16::from(raw[11] as i8)),
                ]);
                let buttons = Buttons::from_bits_unchecked(u64::from(flags >> 1), 3);
                Some(Ok(Some(Self::compact_pen(
                    x, y, pressure, buttons, tilt, None, None, spec,
                ))))
            }
            Self::HuionKamvasOffset => {
                if let Err(e) = Self::require(raw, 2) {
                    return Some(Err(e));
                }
                let flags = raw[1];
                if flags == 0xf1 || flags == 0xf0 || flags & 0x60 == 0x60 {
                    return None;
                }
                if let Err(e) = Self::require(raw, 12) {
                    return Some(Err(e));
                }
                let x =
                    u32::from(u16::from_le_bytes([raw[2], raw[3]])) | (u32::from(raw[4] & 1) << 16);
                let y = u32::from(u16::from_le_bytes([raw[5], raw[6]]));
                let pressure = u32::from(u16::from_le_bytes([raw[7], raw[8]]));
                let tilt = Some([
                    -f32::from(i16::from(raw[10] as i8)),
                    -f32::from(i16::from(raw[11] as i8)),
                ]);
                let buttons = Buttons::from_bits_unchecked(u64::from(flags >> 1), 3);
                Some(Ok(Some(Self::compact_pen(
                    x, y, pressure, buttons, tilt, None, None, spec,
                ))))
            }
            Self::UcLogicBase
            | Self::UcLogicTilt
            | Self::UcLogicV1
            | Self::UcLogicV2
            | Self::HuionTilt => {
                if let Err(e) = Self::require(raw, 2) {
                    return Some(Err(e));
                }
                let flags = raw[1];
                let (is_tilt, invert_y) = match self {
                    Self::UcLogicBase => {
                        if flags == 0xc0 {
                            return Some(Ok(Some(Self::out_of_range())));
                        }
                        if flags & 0x40 != 0 {
                            return None;
                        }
                        (false, false)
                    }
                    Self::UcLogicTilt => {
                        if flags & 0x40 != 0 {
                            return None;
                        }
                        (true, true)
                    }
                    Self::UcLogicV1 => {
                        if flags == 0xe0 {
                            return None;
                        }
                        if flags & 0x40 == 0 {
                            return Some(Ok(Some(Self::out_of_range())));
                        }
                        (false, false)
                    }
                    Self::UcLogicV2 | Self::HuionTilt => {
                        if matches!(flags, 0xe0 | 0xf0) {
                            return None;
                        }
                        (true, false)
                    }
                    _ => unreachable!(),
                };
                if let Err(e) = Self::require(raw, if is_tilt { 12 } else { 8 }) {
                    return Some(Err(e));
                }
                let x = u32::from(u16::from_le_bytes([raw[2], raw[3]]));
                let y = u32::from(u16::from_le_bytes([raw[4], raw[5]]));
                let pressure = u32::from(u16::from_le_bytes([raw[6], raw[7]]));
                let buttons = Buttons::from_bits_unchecked(u64::from(flags >> 1), 3);
                let tilt = is_tilt.then(|| {
                    let ty = i16::from(raw[11] as i8);
                    [
                        f32::from(raw[10] as i8),
                        f32::from(if invert_y { -ty } else { ty }),
                    ]
                });
                Some(Ok(Some(Self::compact_pen(
                    x, y, pressure, buttons, tilt, None, None, spec,
                ))))
            }
            Self::VeikkBase | Self::VeikkA15 | Self::VeikkTilt => {
                if let Err(e) = Self::require(raw, 2) {
                    return Some(Err(e));
                }
                let (a15, tilt) = match self {
                    Self::VeikkTilt => {
                        if raw[1] != 0x41 {
                            return None;
                        }
                        if let Err(e) = Self::require(raw, 3) {
                            return Some(Err(e));
                        }
                        if raw[2] == 0xc0 {
                            return Some(Ok(Some(Self::out_of_range())));
                        }
                        (false, true)
                    }
                    Self::VeikkBase | Self::VeikkA15 => {
                        if raw[1] == 0x43 {
                            return None;
                        }
                        if let Err(e) = Self::require(raw, 3) {
                            return Some(Err(e));
                        }
                        if raw[2] & 0x20 == 0 {
                            return None;
                        }
                        (self == Self::VeikkA15, false)
                    }
                    _ => unreachable!(),
                };
                let need = if tilt {
                    13
                } else if a15 {
                    9
                } else {
                    11
                };
                if let Err(e) = Self::require(raw, need) {
                    return Some(Err(e));
                }
                let (x, y, pressure) = if a15 {
                    (
                        u32::from(u16::from_le_bytes([raw[3], raw[4]])),
                        u32::from(u16::from_le_bytes([raw[5], raw[6]])),
                        u32::from(u16::from_le_bytes([raw[7], raw[8]])),
                    )
                } else {
                    (
                        u32::from_le_bytes([raw[3], raw[4], raw[5], 0]),
                        u32::from_le_bytes([raw[6], raw[7], raw[8], 0]),
                        u32::from(u16::from_le_bytes([raw[9], raw[10]])),
                    )
                };
                let buttons = Buttons::from_bits_unchecked(u64::from(raw[2] >> 1), 2);
                let tilt_val = tilt.then(|| [f32::from(raw[11] as i8), f32::from(raw[12] as i8)]);
                Some(Ok(Some(Self::compact_pen(
                    x, y, pressure, buttons, tilt_val, None, None, spec,
                ))))
            }
            Self::IntuosV3 => {
                let Some(&id) = raw.first() else {
                    return Some(Err(DecodeError::Report(ReportError::Empty)));
                };
                match id {
                    0x1e => {
                        if let Err(e) = Self::require(raw, 20) {
                            return Some(Err(e));
                        }
                        let x = u32::from_le_bytes([raw[3], raw[4], raw[5], 0]);
                        let y = u32::from_le_bytes([raw[6], raw[7], raw[8], 0]);
                        let pressure = u32::from(u16::from_le_bytes([raw[9], raw[10]]));
                        let tilt = Some([
                            f32::from(i16::from_le_bytes([raw[11], raw[12]])),
                            f32::from(i16::from_le_bytes([raw[13], raw[14]])),
                        ]);
                        let rotation = Some(i32::from(i16::from_le_bytes([raw[15], raw[16]])));
                        let eraser = Some(raw[2] & 0x20 != 0);
                        let buttons = Buttons::from_bits_unchecked(u64::from(raw[2] >> 1), 3);
                        Some(Ok(Some(Self::compact_pen(
                            x, y, pressure, buttons, tilt, eraser, rotation, spec,
                        ))))
                    }
                    0x1f => {
                        if let Err(e) = Self::require(raw, 2) {
                            return Some(Err(e));
                        }
                        if raw[1] != 0x01 {
                            return None;
                        }
                        if let Err(e) = Self::require(raw, 14) {
                            return Some(Err(e));
                        }
                        let x = u32::from(u16::from_le_bytes([raw[3], raw[4]]));
                        let y = u32::from(u16::from_le_bytes([raw[5], raw[6]]));
                        let pressure = u32::from(u16::from_le_bytes([raw[7], raw[8]]));
                        let tilt = Some([f32::from(raw[9] as i8), f32::from(raw[11] as i8)]);
                        let eraser = Some(raw[2] & 0x20 != 0);
                        let buttons = Buttons::from_bits_unchecked(u64::from(raw[2] >> 1), 2);
                        Some(Ok(Some(Self::compact_pen(
                            x, y, pressure, buttons, tilt, eraser, None, spec,
                        ))))
                    }
                    _ => None,
                }
            }
        }
    }
}

/// Turns device packets into synchronous pipeline input.
pub trait PenDecoder {
    /// Legacy pen view: `Ok(None)` for auxiliary, touch, status and unknown
    /// packets without a pen position. Runtime callers use `decode_input`.
    fn decode<'a>(&mut self, raw: &'a [u8]) -> Result<Option<DecodedPen<'a>>, DecodeError>;
    /// Runtime decoding preserves complete parser capabilities. Existing pen
    /// sources keep their compact decoder and report construction path.
    #[inline]
    fn decode_input<'a>(&mut self, raw: &'a [u8]) -> Result<Option<DecodedInput<'a>>, DecodeError> {
        self.decode(raw)
            .map(|decoded| decoded.map(DecodedInput::Pen))
    }
    /// Internal compact zero-copy decode for native passthrough sessions.
    #[doc(hidden)]
    #[inline]
    fn decode_compact_pen(
        &mut self,
        _raw: &[u8],
    ) -> Option<Result<Option<CompactPen>, DecodeError>> {
        None
    }
    /// Clears parser state when a session ends.
    fn reset(&mut self) {}
}

/// IntuosV2 pen buttons, with the same checked byte/bit ordering as
/// `reports::from_pth660`: two buttons in 0x10 packets, three in 0x1e.
#[inline]
pub fn intuos_v2_pen_buttons(raw: &[u8]) -> Option<Buttons> {
    match raw {
        [0x10, flags, ..] if raw.len() >= 17 => {
            Some(Buttons::from_bits_unchecked(u64::from(flags >> 1), 2))
        }
        [0x1e, _, flags, ..] if raw.len() >= 13 => {
            Some(Buttons::from_bits_unchecked(u64::from(flags >> 1), 3))
        }
        _ => None,
    }
}

/// Session decoder for one tablet endpoint.
#[derive(Clone, Debug)]
pub enum TabletDecoder {
    /// The checked IntuosV2 pen layout (PTH-660 and other IntuosV2 tablets).
    IntuosV2 {
        spec: TabletSpec,
        prefixed: bool,
        fallback: Box<ReportParser>,
    },
    /// Any other parser, retaining complete report values at runtime.
    /// Boxed: stateful parsers such as the touch ones are large.
    Values {
        parser: Box<ReportParser>,
        spec: TabletSpec,
    },
}

impl TabletDecoder {
    /// The PTH-660 decoder the driver has always used.
    pub fn pth_660() -> Self {
        Self::IntuosV2 {
            spec: TabletSpec::PTH_660,
            prefixed: false,
            fallback: Box::new(ReportParser::IntuosV2 {
                prefixed: false,
                touch: IntuosV2TouchParser::default(),
            }),
        }
    }

    /// The decoder for a configured parser type, or `None` if it is unknown.
    pub fn for_parser(type_name: &str, spec: TabletSpec) -> Option<Self> {
        match type_name {
            INTUOS_V2 => Some(Self::IntuosV2 {
                spec,
                prefixed: false,
                fallback: Box::new(ReportParser::IntuosV2 {
                    prefixed: false,
                    touch: IntuosV2TouchParser::default(),
                }),
            }),
            WACOM_DRIVER_INTUOS_V2 => Some(Self::IntuosV2 {
                spec,
                prefixed: true,
                fallback: Box::new(ReportParser::IntuosV2 {
                    prefixed: true,
                    touch: IntuosV2TouchParser::default(),
                }),
            }),
            _ => ReportParser::for_type(type_name).map(|parser| Self::Values {
                parser: Box::new(parser),
                spec,
            }),
        }
    }
}

const SESSION_METADATA: ReportMetadata = ReportMetadata {
    device: DeviceId(0),
    session: SessionId(0),
    endpoint: EndpointId(0),
    received_at: Duration::ZERO,
    sequence: 0,
};

impl FastParser {
    #[inline]
    fn from_report_parser(parser: &ReportParser) -> Self {
        match parser {
            ReportParser::Stateless(f) => {
                if std::ptr::fn_addr_eq(*f, parse_tablet as Stateless) {
                    Self::GenericTablet
                } else if std::ptr::fn_addr_eq(*f, parse_xp_pen as Stateless) {
                    Self::XpPenBase
                } else if std::ptr::fn_addr_eq(*f, parse_xp_pen_gen2 as Stateless) {
                    Self::XpPenGen2
                } else if std::ptr::fn_addr_eq(*f, parse_xp_pen_offset_pressure as Stateless) {
                    Self::XpPenOffsetPressure
                } else if std::ptr::fn_addr_eq(*f, parse_xp_pen_offset_auxiliary as Stateless) {
                    Self::XpPenOffsetAux
                } else if std::ptr::fn_addr_eq(*f, parse_huion_giano as Stateless) {
                    Self::HuionGiano
                } else if std::ptr::fn_addr_eq(*f, parse_huion_inspiroy as Stateless) {
                    Self::HuionInspiroy
                } else if std::ptr::fn_addr_eq(*f, parse_huion_tilt as Stateless) {
                    Self::HuionTilt
                } else if std::ptr::fn_addr_eq(*f, parse_huion_kamvas_offset as Stateless) {
                    Self::HuionKamvasOffset
                } else if std::ptr::fn_addr_eq(*f, parse_uc_logic as Stateless) {
                    Self::UcLogicBase
                } else if std::ptr::fn_addr_eq(*f, parse_uc_logic_tilt as Stateless) {
                    Self::UcLogicTilt
                } else if std::ptr::fn_addr_eq(*f, parse_uc_logic_v1 as Stateless) {
                    Self::UcLogicV1
                } else if std::ptr::fn_addr_eq(*f, parse_uc_logic_v2 as Stateless) {
                    Self::UcLogicV2
                } else if std::ptr::fn_addr_eq(*f, parse_veikk as Stateless) {
                    Self::VeikkBase
                } else if std::ptr::fn_addr_eq(*f, parse_veikk_a15 as Stateless) {
                    Self::VeikkA15
                } else if std::ptr::fn_addr_eq(*f, parse_veikk_tilt as Stateless) {
                    Self::VeikkTilt
                } else {
                    Self::None
                }
            }
            ReportParser::Envelope(f) if std::ptr::fn_addr_eq(*f, parse_intuos_v3 as Envelope) => {
                Self::IntuosV3
            }
            _ => Self::None,
        }
    }
}

impl PenDecoder for TabletDecoder {
    #[inline]
    fn decode<'a>(&mut self, raw: &'a [u8]) -> Result<Option<DecodedPen<'a>>, DecodeError> {
        match self {
            Self::IntuosV2 { spec, prefixed, .. } => {
                let payload = if *prefixed {
                    raw.get(1..).unwrap_or_default()
                } else {
                    raw
                };
                protocol::parse_within_with_buttons(
                    payload,
                    spec.max_x,
                    spec.max_y,
                    spec.max_pressure,
                )
                .map(|decoded| {
                    decoded.map(|(pen, buttons)| DecodedPen {
                        pen,
                        raw: payload,
                        buttons: Some(buttons),
                    })
                })
                .map_err(DecodeError::Pen)
            }
            Self::Values { parser, spec } => {
                let (kind, report) = parser
                    .parse(raw, SESSION_METADATA)
                    .map_err(DecodeError::Report)?;
                Ok(
                    pen_from_values(kind, &report.values, report.raw, *spec).map(|pen| {
                        DecodedPen {
                            pen,
                            raw: report.raw,
                            buttons: report.values.pen_buttons,
                        }
                    }),
                )
            }
        }
    }

    #[inline]
    fn decode_compact_pen(
        &mut self,
        raw: &[u8],
    ) -> Option<Result<Option<CompactPen>, DecodeError>> {
        match self {
            Self::IntuosV2 { spec, prefixed, .. } => {
                let payload = if *prefixed {
                    raw.get(1..).unwrap_or_default()
                } else {
                    raw
                };
                if !matches!(payload.first(), Some(0x10 | 0x1e)) {
                    return None;
                }
                Some(
                    protocol::parse_within_with_buttons(
                        payload,
                        spec.max_x,
                        spec.max_y,
                        spec.max_pressure,
                    )
                    .map(|decoded| {
                        decoded.map(|(pen, buttons)| CompactPen {
                            x: pen.x,
                            y: pen.y,
                            position_exact: true,
                            pos_f32: [pen.x as f32, pen.y as f32],
                            has_position: true,
                            pressure: u32::from(pen.pressure),
                            pen_buttons: Some(buttons),
                            tilt: Some([f32::from(pen.tilt[0]), f32::from(pen.tilt[1])]),
                            eraser: Some(pen.eraser),
                            rotation: pen.rotation.map(i32::from),
                            tip_switch: Some(pen.tip_switch),
                            physical_loss: !pen.in_range && !pen.sense,
                        })
                    })
                    .map_err(DecodeError::Pen),
                )
            }
            Self::Values { parser, spec } => {
                FastParser::from_report_parser(parser).decode(raw, *spec)
            }
        }
    }

    #[inline]
    fn decode_input<'a>(&mut self, raw: &'a [u8]) -> Result<Option<DecodedInput<'a>>, DecodeError> {
        match self {
            Self::IntuosV2 {
                prefixed,
                fallback,
                spec,
            } => {
                let payload = if *prefixed {
                    raw.get(1..).unwrap_or_default()
                } else {
                    raw
                };
                if matches!(payload.first(), Some(0x10 | 0x1e)) {
                    return protocol::parse_within_with_buttons(
                        payload,
                        spec.max_x,
                        spec.max_y,
                        spec.max_pressure,
                    )
                    .map(|decoded| {
                        decoded.map(|(pen, buttons)| {
                            DecodedInput::Pen(DecodedPen {
                                pen,
                                raw: payload,
                                buttons: Some(buttons),
                            })
                        })
                    })
                    .map_err(DecodeError::Pen);
                }
                let (kind, report) = fallback
                    .parse(raw, SESSION_METADATA)
                    .map_err(DecodeError::Report)?;
                let pen = pen_from_values(kind, &report.values, report.raw, *spec);
                Ok(Some(DecodedInput::Report { kind, report, pen }))
            }
            Self::Values { parser, spec } => {
                let (kind, report) = parser
                    .parse(raw, SESSION_METADATA)
                    .map_err(DecodeError::Report)?;
                let pen = pen_from_values(kind, &report.values, report.raw, *spec);
                Ok(Some(DecodedInput::Report { kind, report, pen }))
            }
        }
    }

    fn reset(&mut self) {
        match self {
            Self::IntuosV2 { fallback, .. } => fallback.reset(),
            Self::Values { parser, .. } => parser.reset(),
        }
    }
}

#[inline]
fn round_coord_u32(value: f32, max: u32) -> u32 {
    if !value.is_finite() || value <= 0.0 {
        return 0;
    }
    let v = f64::from(value);
    let max_f = f64::from(max);
    if v >= max_f {
        return max;
    }
    let t = v as u64;
    (t + u64::from(v - (t as f64) >= 0.5)).min(u64::from(max)) as u32
}

#[inline]
fn round_tilt_i8(value: f32) -> i8 {
    if !value.is_finite() {
        return 0;
    }
    let clamped = value.clamp(-128.0, 127.0);
    let t = clamped as i32;
    let frac = clamped - (t as f32);
    let rounded = if frac >= 0.5 {
        t + 1
    } else if frac <= -0.5 {
        t - 1
    } else {
        t
    };
    rounded.clamp(-128, 127) as i8
}

/// The pipeline's pen report for one parsed packet, or `None` when it has no
/// position. OutOfRange reports lose the pen (both proximity flags clear).
/// A report with a position always counts as detected, since upstream moves
/// the cursor for every absolute position. The tip is pressure above zero,
/// as upstream's binding thresholds see it. Values are clamped to the
/// tablet's ranges instead of being rejected, because upstream passes them on.
pub fn pen_from_values(
    kind: ReportKind,
    values: &ReportValues,
    raw: &[u8],
    spec: TabletSpec,
) -> Option<PenReport> {
    let id = raw.first().copied().unwrap_or(0);
    if kind == ReportKind::OutOfRange {
        return Some(PenReport {
            id,
            x: 0,
            y: 0,
            pressure: 0,
            in_range: false,
            sense: false,
            tip_switch: false,
            eraser: false,
            tilt: [0, 0],
            rotation: None,
            hover_distance: None,
        });
    }
    let [x, y] = values.position?;
    let pressure = values
        .pressure
        .unwrap_or(0)
        .min(u32::from(spec.max_pressure)) as u16;
    let tilt = values.tilt.unwrap_or([0.0, 0.0]).map(round_tilt_i8);
    let eraser = values.eraser.unwrap_or(false)
        || values
            .tool
            .is_some_and(|tool| tool.tool == ToolType::Eraser);
    Some(PenReport {
        id,
        x: round_coord_u32(x, spec.max_x),
        y: round_coord_u32(y, spec.max_y),
        pressure,
        in_range: values.near_proximity.unwrap_or(true),
        sense: true,
        tip_switch: pressure > 0,
        eraser,
        tilt,
        rotation: values
            .rotation
            .and_then(|rotation| i16::try_from(rotation).ok()),
        hover_distance: values
            .hover_distance
            .map(|distance| distance.min(u32::from(u8::MAX)) as u8),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tablets::{Database, ParserSupport, parser_support};

    #[test]
    fn every_referenced_parser_resolves() {
        let database = Database::builtin();
        for parser in database.parsers().keys() {
            assert!(ReportParser::for_type(parser).is_some(), "{parser}");
            assert!(TYPE_NAMES.contains(parser), "{parser}");
            assert_ne!(parser_support(parser), ParserSupport::Missing, "{parser}");
        }
        assert!(ReportParser::for_type("Vendor.Unknown").is_none());
    }

    #[test]
    fn the_pth_660_decoder_is_unchanged() {
        let mut report = [0u8; 17];
        report[0] = 0x10;
        report[1] = 0x61;
        report[2..5].copy_from_slice(&[0x10, 0x27, 0]);
        report[8] = 0x80;
        let mut decoder = TabletDecoder::pth_660();
        assert_eq!(
            decoder
                .decode(&report)
                .map(|decoded| decoded.map(|decoded| decoded.pen)),
            protocol::parse(&report).map_err(DecodeError::Pen)
        );
        report[2..5].copy_from_slice(&[0, 0, 1]);
        assert!(decoder.decode(&report).is_err(), "PTH-660 range is kept");
        let mut larger = TabletDecoder::IntuosV2 {
            spec: TabletSpec {
                max_x: 100_000,
                ..TabletSpec::PTH_660
            },
            prefixed: false,
            fallback: Box::new(ReportParser::IntuosV2 {
                prefixed: false,
                touch: IntuosV2TouchParser::default(),
            }),
        };
        assert_eq!(larger.decode(&report).unwrap().unwrap().pen.x, 0x10000);
    }

    #[test]
    fn values_become_pen_reports() {
        let spec = TabletSpec {
            max_x: 100,
            max_y: 100,
            max_pressure: 1023,
            rotation_range: None,
            width_mm: 10.0,
            height_mm: 10.0,
            controls: Default::default(),
        };
        let mut decoder = TabletDecoder::for_parser(
            "OpenTabletDriver.Configurations.Parsers.XP_Pen.XP_PenReportParser",
            spec,
        )
        .unwrap();
        // XP-Pen pen report: position (50, 200), pressure 2000.
        let pen = decoder
            .decode(&[2, 0x80, 50, 0, 200, 0, 0xd0, 0x07])
            .unwrap()
            .unwrap()
            .pen;
        assert_eq!((pen.x, pen.y, pen.pressure), (50, 100, 1023));
        assert!(pen.sense && pen.tip_switch);
        let gone = decoder.decode(&[2, 0xc0]).unwrap().unwrap().pen;
        assert!(!gone.sense && !gone.in_range);
        // An auxiliary report has no position.
        assert_eq!(decoder.decode(&[2, 0xf0, 1, 0, 0, 0, 0, 0]).unwrap(), None);
    }

    fn buttons_json(buttons: Buttons) -> serde_json::Value {
        (0..buttons.len())
            .map(|index| buttons.get(index).unwrap_or(false))
            .collect::<Vec<_>>()
            .into()
    }

    /// The fields upstream's report interfaces expose, in the fixture's shape.
    fn values_json(kind: ReportKind, values: &ReportValues) -> serde_json::Value {
        use serde_json::{Value, json};
        let mut d = serde_json::Map::new();
        d.insert(
            "kind".into(),
            json!(if kind == ReportKind::OutOfRange {
                "out_of_range"
            } else {
                "data"
            }),
        );
        if let Some(position) = values.position {
            d.insert("position".into(), json!(position));
        }
        if let Some(pressure) = values.pressure {
            d.insert("pressure".into(), json!(pressure));
        }
        if let Some(buttons) = values.pen_buttons {
            d.insert("pen_buttons".into(), buttons_json(buttons));
        }
        if let Some(tilt) = values.tilt {
            d.insert("tilt".into(), json!(tilt));
        }
        if let Some(eraser) = values.eraser {
            d.insert("eraser".into(), json!(eraser));
        }
        if let Some(near) = values.near_proximity {
            d.insert("near_proximity".into(), json!(near));
        }
        if let Some(hover) = values.hover_distance {
            d.insert("hover_distance".into(), json!(hover));
        }
        if let Some(buttons) = values.aux_buttons {
            d.insert("aux_buttons".into(), buttons_json(buttons));
        }
        if let Some(buttons) = values.mouse_buttons {
            d.insert("mouse_buttons".into(), buttons_json(buttons));
        }
        if let Some(scroll) = values.mouse_scroll {
            d.insert("mouse_scroll".into(), json!(scroll));
        }
        if let Some(tool) = values.tool {
            d.insert(
                "tool".into(),
                json!({"serial": tool.serial, "raw_tool_id": tool.raw_tool_id,
                    "eraser": tool.tool == ToolType::Eraser}),
            );
        }
        if let Some(touches) = values.touches {
            let list: Vec<Value> = touches
                .as_slice()
                .iter()
                .map(|touch| {
                    touch.map_or(
                        Value::Null,
                        |t| json!({"id": t.id, "x": t.position[0], "y": t.position[1]}),
                    )
                })
                .collect();
            d.insert("touches".into(), list.into());
        }
        if let Some(analog) = values.absolute_analog {
            d.insert(
                "analog_positions".into(),
                json!(analog.positions.as_slice()),
            );
        }
        if let Some(analog) = values.relative_analog {
            d.insert("analog_deltas".into(), json!(analog.deltas.as_slice()));
        }
        if let Some(wheels) = values.wheel_buttons {
            let list: Vec<Value> = wheels.as_slice().iter().map(|b| buttons_json(*b)).collect();
            d.insert("wheel_buttons".into(), list.into());
        }
        Value::Object(d)
    }

    fn same(a: &serde_json::Value, b: &serde_json::Value) -> bool {
        use serde_json::Value;
        match (a, b) {
            (Value::Number(a), Value::Number(b)) => {
                let (a, b) = (a.as_f64().unwrap(), b.as_f64().unwrap());
                (a - b).abs() <= 1e-4 * a.abs().max(1.0)
            }
            (Value::Array(a), Value::Array(b)) => {
                a.len() == b.len() && a.iter().zip(b).all(|(a, b)| same(a, b))
            }
            (Value::Object(a), Value::Object(b)) => {
                a.len() == b.len() && a.iter().all(|(k, v)| b.get(k).is_some_and(|w| same(v, w)))
            }
            _ => a == b,
        }
    }

    /// Every parser decodes the fixture's seeded packets exactly as
    /// OpenTabletDriver's own parser classes did (bench/upstream --parsers),
    /// at the v0.6.8-rc1 revision whose catalog the driver embeds.
    #[test]
    fn parsers_match_upstream_on_seeded_packets() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/parsers/upstream.json");
        let fixture: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let mut failures = Vec::new();
        let mut compared = 0;
        for case in fixture["cases"].as_array().unwrap() {
            let name = case["parser"].as_str().unwrap();
            let intuos_v2 = name.contains(".IntuosV2.");
            let touch = intuos_v2 || name.ends_with("Wacom64bAuxReportParser");
            let mut parser = ReportParser::for_type(name).unwrap();
            // Touch updates apply all or nothing; upstream keeps the slots a
            // truncated packet updated before it threw (BC-31).
            let mut touch_state_diverged = false;
            for (index, entry) in case["reports"].as_array().unwrap().iter().enumerate() {
                let hex = entry["hex"].as_str().unwrap();
                let raw: Vec<u8> = (0..hex.len())
                    .step_by(2)
                    .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
                    .collect();
                let ours = parser.parse(&raw, SESSION_METADATA);
                compared += 1;
                let expected = &entry["report"];
                let mut expected = expected.clone();
                if let (Ok((_, report)), Some(object)) = (&ours, expected.as_object_mut()) {
                    // Upstream #5232, #5215, #5308 and #5311 are adopted ahead of the fixture.
                    if name.ends_with(".GianoReportParser") && object.contains_key("position") {
                        object.insert("eraser".into(), serde_json::json!(raw[1] & 0x20 != 0));
                    }
                    if name.ends_with(".IntuosV3ReportParser") && raw.starts_with(&[0x1f, 0x01]) {
                        let tilt = [raw[9] as i8, raw[11] as i8];
                        object.insert("tilt".into(), serde_json::json!(tilt));
                    }
                    if name.ends_with(".BostoReportParser") {
                        if raw.first() == Some(&0x04) {
                            *object = values_json(ReportKind::Data, &report.values)
                                .as_object()
                                .unwrap()
                                .clone();
                        } else if !(raw.first() == Some(&0x01)
                            && raw.get(1).is_some_and(|b| b & 0x10 != 0))
                        {
                            *object = serde_json::json!({"kind": "out_of_range"})
                                .as_object()
                                .unwrap()
                                .clone();
                        }
                    }
                    // The 0x1E hover distance overlaps tilt X upstream (BC-23).
                    if intuos_v2
                        && raw.get(usize::from(name.contains("WacomDriver"))) == Some(&0x1e)
                        && report.values.hover_distance.is_none()
                    {
                        object.remove("hover_distance");
                    }
                    if touch_state_diverged {
                        object.remove("touches");
                    }
                }
                let expected = &expected;
                let ours_json = |kind: ReportKind, values: &ReportValues| {
                    let mut value = values_json(kind, values);
                    if touch_state_diverged {
                        value.as_object_mut().unwrap().remove("touches");
                    }
                    value
                };
                let matched = match (&ours, entry.get("error")) {
                    (Err(_), Some(_)) => {
                        touch_state_diverged |= touch;
                        true
                    }
                    // Upstream #5311: 8-byte Bamboo reports now decode with a null wheel
                    // instead of throwing IndexOutOfRangeException on byte 8.
                    (Ok(_), Some(_)) if name.ends_with(".BambooReportParser") && raw.len() == 8 => {
                        true
                    }
                    // Upstream #5308: BostoReportParser treats non-0x01/0x04 packets as
                    // OutOfRange without reading past byte 1.
                    (Ok((ReportKind::OutOfRange, _)), Some(_))
                        if name.ends_with(".BostoReportParser") =>
                    {
                        true
                    }
                    // Upstream's Unsafe.ReadUnaligned reads up to three bytes
                    // past a short packet instead of throwing (BC-30).
                    (Err(ReportError::Short { got, need, .. }), None) if need - got <= 3 => true,
                    (Ok((kind, report)), None) => same(&ours_json(*kind, &report.values), expected),
                    _ => false,
                };
                if !matched {
                    failures.push(format!(
                        "{name} #{index} {hex}\n  upstream {}\n  ours     {}",
                        entry.get("error").unwrap_or(expected),
                        match &ours {
                            Ok((kind, report)) => values_json(*kind, &report.values).to_string(),
                            Err(error) => format!("{error:?}"),
                        }
                    ));
                }
            }
        }
        assert!(compared > 6000, "{compared}");
        assert!(
            failures.is_empty(),
            "{} of {compared} differ:\n{}",
            failures.len(),
            failures.join("\n")
        );
    }

    #[test]
    fn compact_pen_fast_parsers_match_generic_decode_without_allocating() {
        let parsers = [
            "OpenTabletDriver.Plugin.Tablet.TabletReportParser",
            "OpenTabletDriver.Configurations.Parsers.XP_Pen.XP_PenReportParser",
            "OpenTabletDriver.Configurations.Parsers.XP_Pen.XP_PenOffsetAuxReportParser",
            "OpenTabletDriver.Configurations.Parsers.XP_Pen.XP_PenOffsetPressureReportParser",
            "OpenTabletDriver.Configurations.Parsers.XP_Pen.XP_PenGen2ReportParser",
            "OpenTabletDriver.Configurations.Parsers.Huion.GianoReportParser",
            "OpenTabletDriver.Configurations.Parsers.Huion.InspiroyReportParser",
            "OpenTabletDriver.Configurations.Parsers.Huion.KamvasOffsetReportParser",
            "OpenTabletDriver.Configurations.Parsers.Huion.HuionTiltReportParser",
            "OpenTabletDriver.Configurations.Parsers.UCLogic.UCLogicReportParser",
            "OpenTabletDriver.Configurations.Parsers.UCLogic.UCLogicTiltReportParser",
            "OpenTabletDriver.Configurations.Parsers.UCLogic.UCLogicV1ReportParser",
            "OpenTabletDriver.Configurations.Parsers.UCLogic.UCLogicV2ReportParser",
            "OpenTabletDriver.Configurations.Parsers.Veikk.VeikkReportParser",
            "OpenTabletDriver.Configurations.Parsers.Veikk.VeikkTiltReportParser",
            "OpenTabletDriver.Configurations.Parsers.Veikk.VeikkA15ReportParser",
            "OpenTabletDriver.Configurations.Parsers.Wacom.IntuosV3.IntuosV3ReportParser",
            "OpenTabletDriver.Configurations.Parsers.Wacom.IntuosV2.IntuosV2ReportParser",
        ];
        let mut packets: Vec<[u8; 32]> = Vec::new();
        // Representative pen, hover, out-of-range, and aux/non-pen packets.
        let headers: &[(u8, u8)] = &[
            (0x02, 0x81),
            (0x02, 0xa3),
            (0x02, 0xc0),
            (0x02, 0xf0),
            (0x07, 0x80),
            (0x08, 0x81),
            (0x08, 0xe0),
            (0x09, 0x85),
            (0x10, 0x61),
            (0x10, 0x42),
            (0x10, 0x00),
            (0x11, 0x01),
            (0x1e, 0x81),
        ];
        for &(b0, b1) in headers {
            let mut pkt = [0u8; 32];
            pkt[0] = b0;
            pkt[1] = b1;
            for (i, slot) in pkt[2..].iter_mut().enumerate() {
                *slot = ((b0 as usize * 31 + b1 as usize * 17 + i * 13) & 0xff) as u8;
            }
            packets.push(pkt);
        }
        let spec = TabletSpec::PTH_660;
        for name in parsers {
            let mut fast_decoder = TabletDecoder::for_parser(name, spec).unwrap();
            let mut generic_decoder = TabletDecoder::for_parser(name, spec).unwrap();
            for pkt in &packets {
                let fast = fast_decoder.decode_compact_pen(pkt);
                let generic = generic_decoder.decode_input(pkt);
                match (fast, generic) {
                    (
                        Some(Ok(Some(compact))),
                        Ok(Some(DecodedInput::Report { kind, report, .. })),
                    ) => {
                        if compact.physical_loss {
                            assert_eq!(kind, ReportKind::OutOfRange, "loss mismatch for {name}");
                        } else {
                            assert_eq!(
                                Some(compact.pos_f32),
                                report.values.position,
                                "position mismatch for {name}"
                            );
                            assert_eq!(
                                Some(compact.pressure),
                                report.values.pressure,
                                "pressure mismatch for {name}"
                            );
                            assert_eq!(
                                compact.pen_buttons, report.values.pen_buttons,
                                "buttons mismatch for {name}"
                            );
                            assert_eq!(
                                compact.tilt, report.values.tilt,
                                "tilt mismatch for {name}"
                            );
                            assert_eq!(
                                compact.eraser, report.values.eraser,
                                "eraser mismatch for {name}"
                            );
                        }
                    }
                    (Some(Ok(Some(compact))), Ok(Some(DecodedInput::Pen(pen)))) => {
                        assert_eq!(compact.x, pen.pen.x, "x mismatch for {name}");
                        assert_eq!(compact.y, pen.pen.y, "y mismatch for {name}");
                        assert_eq!(
                            compact.pressure,
                            u32::from(pen.pen.pressure),
                            "pressure mismatch"
                        );
                        assert_eq!(compact.physical_loss, !pen.pen.in_range && !pen.pen.sense);
                    }
                    // Fallback to generic path (e.g. aux packet) or ignored/short packet.
                    (None, Ok(_)) | (Some(Ok(None)), Ok(None)) | (Some(Err(_)), Err(_)) => {}
                    (other_fast, other_generic) => panic!(
                        "compact/generic mismatch for {name}: fast={other_fast:?} generic={other_generic:?}"
                    ),
                }
            }
            // Steady-state zero-allocation check on a pen packet.
            let sample = &packets[0];
            crate::test_alloc::assert_no_allocations(|| {
                for _ in 0..1_000 {
                    let _ = std::hint::black_box(fast_decoder.decode_compact_pen(sample));
                }
            });
        }
    }
}
