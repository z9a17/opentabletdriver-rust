//! Report parser selection by OpenTabletDriver type name, and the pen
//! decoders a device session runs.
//!
//! `ReportParser` covers every parser type the pinned configurations name and
//! yields upstream's full report values (pen, auxiliary, touch, wheel, mouse).
//! `PenDecoder` turns one transport packet into the `PenReport` the report
//! pipeline consumes. IntuosV2 tablets keep the checked PTH-660 decoder with
//! their own ranges; every other parser goes through `ReportParser` and
//! `pen_from_values`. Neither allocates per report.

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
    "OpenTabletDriver.Configurations.Parsers.Huion.HuionTiltReportParser",
    "OpenTabletDriver.Configurations.Parsers.Huion.InspiroyReportParser",
    "OpenTabletDriver.Configurations.Parsers.Lifetec.LifetecReportParser",
    "OpenTabletDriver.Configurations.Parsers.RobotPen.RobotPenReportParser",
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
            "Huion.HuionTiltReportParser" => Self::Stateless(parse_huion_tilt),
            "Huion.InspiroyReportParser" => Self::Stateless(parse_huion_inspiroy),
            "Lifetec.LifetecReportParser" => Self::Stateless(parse_lifetec),
            "RobotPen.RobotPenReportParser" => Self::Stateless(parse_robot_pen),
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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecodeError {
    Pen(ParseError),
    Report(ReportError),
}

/// Turns device packets into pen reports for the report pipeline.
pub trait PenDecoder {
    /// `Ok(None)` for packets that carry no pen position (auxiliary, touch,
    /// status and unknown reports), which the session ignores.
    fn decode(&mut self, raw: &[u8]) -> Result<Option<PenReport>, DecodeError>;
    /// Clears parser state when a session ends.
    fn reset(&mut self) {}
}

/// Session decoder for one tablet endpoint.
#[derive(Clone, Debug)]
pub enum TabletDecoder {
    /// The checked IntuosV2 pen layout (PTH-660 and other IntuosV2 tablets).
    IntuosV2 { spec: TabletSpec, prefixed: bool },
    /// Any other parser, adapted from its report values.
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
        }
    }

    /// The decoder for a configured parser type, or `None` if it is unknown.
    pub fn for_parser(type_name: &str, spec: TabletSpec) -> Option<Self> {
        match type_name {
            INTUOS_V2 => Some(Self::IntuosV2 {
                spec,
                prefixed: false,
            }),
            WACOM_DRIVER_INTUOS_V2 => Some(Self::IntuosV2 {
                spec,
                prefixed: true,
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

impl PenDecoder for TabletDecoder {
    #[inline]
    fn decode(&mut self, raw: &[u8]) -> Result<Option<PenReport>, DecodeError> {
        match self {
            Self::IntuosV2 { spec, prefixed } => {
                let payload = if *prefixed {
                    raw.get(1..).unwrap_or_default()
                } else {
                    raw
                };
                protocol::parse_within(payload, spec.max_x, spec.max_y, spec.max_pressure)
                    .map_err(DecodeError::Pen)
            }
            Self::Values { parser, spec } => {
                let (kind, report) = parser
                    .parse(raw, SESSION_METADATA)
                    .map_err(DecodeError::Report)?;
                Ok(pen_from_values(kind, &report.values, report.raw, *spec))
            }
        }
    }

    fn reset(&mut self) {
        if let Self::Values { parser, .. } = self {
            parser.reset();
        }
    }
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
    let coordinate = |value: f32, max: u32| {
        if value.is_finite() {
            (value.round().max(0.0) as u64).min(u64::from(max)) as u32
        } else {
            0
        }
    };
    let pressure = values
        .pressure
        .unwrap_or(0)
        .min(u32::from(spec.max_pressure)) as u16;
    let tilt = values.tilt.unwrap_or([0.0, 0.0]).map(|value| {
        if value.is_finite() {
            value.round().clamp(-128.0, 127.0) as i8
        } else {
            0
        }
    });
    let eraser = values.eraser.unwrap_or(false)
        || values
            .tool
            .is_some_and(|tool| tool.tool == ToolType::Eraser);
    Some(PenReport {
        id,
        x: coordinate(x, spec.max_x),
        y: coordinate(y, spec.max_y),
        pressure,
        in_range: values.near_proximity.unwrap_or(true),
        sense: true,
        tip_switch: pressure > 0,
        eraser,
        tilt,
        rotation: values.rotation,
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
            decoder.decode(&report),
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
        };
        assert_eq!(larger.decode(&report).unwrap().unwrap().x, 0x10000);
    }

    #[test]
    fn values_become_pen_reports() {
        let spec = TabletSpec {
            max_x: 100,
            max_y: 100,
            max_pressure: 1023,
            width_mm: 10.0,
            height_mm: 10.0,
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
            .unwrap();
        assert_eq!((pen.x, pen.y, pen.pressure), (50, 100, 1023));
        assert!(pen.sense && pen.tip_switch);
        let gone = decoder.decode(&[2, 0xc0]).unwrap().unwrap();
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
    /// OpenTabletDriver's own parser classes did (bench/upstream --parsers).
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
        assert!(compared > 8000, "{compared}");
        assert!(
            failures.is_empty(),
            "{} of {compared} differ:\n{}",
            failures.len(),
            failures.join("\n")
        );
    }
}
