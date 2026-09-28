//! Offline inspection of explicitly supplied bytes. Never opens devices/plugins.
use otd_core::{protocol, reports::*};
use serde_json::{Value, json};
use std::{fs::File, io::Read, path::PathBuf, time::Duration};

const MAX_INPUT_BYTES: u64 = 4 * 1024 * 1024;
const MAX_REPORTS: usize = 4096;
const MAX_PACKET_BYTES: usize = 192;

const USAGE: &str = "Offline report decoding:
  decode --parser NAME --hex HEX_BYTES
  decode --parser NAME --input HEX_LINES_FILE

Parsers: pth660-pen, intuos-v2-aux, intuos-v2-touch,
         wacom-driver-intuos-v2-touch, intuos-v3, bamboo, bamboo-pad,
         bamboo-v2-aux, uc-logic, uc-logic-tilt, uc-logic-v1, uc-logic-v2,
         huion-tilt, huion-giano, huion-inspiroy, xp-pen, xp-pen-gen2,
         xp-pen-offset-pressure, xp-pen-offset-aux, xp-pen-dedicated-aux,
         tablet, auxiliary, skip-byte-tablet, veikk, veikk-a15, veikk-tilt,
         veikk-v1
or any OpenTabletDriver parser type, such as Wacom.IntuosV1.IntuosV1ReportParser
or OpenTabletDriver.Plugin.Tablet.TabletReportParser.
Hex may contain ASCII whitespace or colons. Files contain one packet per line;
blank lines and lines starting with # are ignored. Limits: 4 MiB, 4096 reports,
192 bytes per packet. Each output line is one JSON snapshot; a later error does
not retract earlier lines. Touch state lasts only for this command.
This command never opens a tablet, loads a plugin, or injects input.";

type StatelessParser =
    for<'a> fn(&'a [u8], ReportMetadata) -> Result<(ReportKind, ReportEnvelope<'a>), ReportError>;

type PrefixedParser =
    for<'a> fn(&'a [u8], ReportMetadata) -> Result<(ReportKind, TransportReport<'a>), ReportError>;

enum Decoder {
    Stateless(StatelessParser),
    Prefixed(PrefixedParser),
    Pen,
    Aux,
    Touch(IntuosV2TouchParser),
    PrefixedTouch(WacomDriverIntuosV2TouchParser),
    IntuosV3,
    Bamboo,
    BambooPad,
    BambooV2Aux,
    Registry(otd_core::decoders::ReportParser),
}

impl Decoder {
    fn parse<'a>(
        &mut self,
        raw: &'a [u8],
        metadata: ReportMetadata,
    ) -> Result<(ReportKind, Option<ReportEnvelope<'a>>), String> {
        let report = match self {
            Self::Prefixed(parser) => {
                return parser(raw, metadata)
                    .map(|(kind, report)| (kind, Some(report.report)))
                    .map_err(|e| format!("{e:?}"));
            }
            Self::Stateless(parser) => {
                return parser(raw, metadata)
                    .map(|(kind, report)| (kind, Some(report)))
                    .map_err(|e| format!("{e:?}"));
            }
            Self::Pen => protocol::parse(raw)
                .map_err(|e| format!("{e:?}"))?
                .map(|pen| from_pth660(pen, raw, metadata).map_err(|e| format!("{e:?}")))
                .transpose(),
            Self::Aux => parse_intuos_auxiliary(raw, metadata).map_err(|e| format!("{e:?}")),
            Self::Touch(parser) => parser.parse(raw, metadata).map_err(|e| format!("{e:?}")),
            Self::PrefixedTouch(parser) => parser
                .parse(raw, metadata)
                .map(|report| report.map(|report| report.report))
                .map_err(|e| format!("{e:?}")),
            Self::IntuosV3 => parse_intuos_v3(raw, metadata)
                .map(Some)
                .map_err(|e| format!("{e:?}")),
            Self::Bamboo => parse_bamboo(raw, metadata)
                .map(Some)
                .map_err(|e| format!("{e:?}")),
            Self::BambooPad => parse_bamboo_pad(raw, metadata)
                .map(Some)
                .map_err(|e| format!("{e:?}")),
            Self::BambooV2Aux => parse_bamboo_v2_auxiliary(raw, metadata)
                .map(Some)
                .map_err(|e| format!("{e:?}")),
            Self::Registry(parser) => {
                return parser
                    .parse(raw, metadata)
                    .map(|(kind, report)| (kind, Some(report)))
                    .map_err(|e| format!("{e:?}"));
            }
        }?;
        Ok((ReportKind::Data, report))
    }
}

pub fn run(args: Vec<String>) -> Result<(), String> {
    let mut args = args.into_iter();
    let (mut parser, mut hex, mut input) = (None, None, None::<PathBuf>);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--help" | "-h" => {
                println!("{USAGE}");
                return Ok(());
            }
            "--parser" if parser.is_none() => {
                parser = Some(args.next().ok_or("--parser needs a name")?)
            }
            "--hex" if hex.is_none() => hex = Some(args.next().ok_or("--hex needs bytes")?),
            "--input" if input.is_none() => {
                input = Some(args.next().ok_or("--input needs a file")?.into())
            }
            _ => return Err(format!("unknown or repeated option: {arg}\n{USAGE}")),
        }
    }
    let parser = parser.ok_or(USAGE)?;
    let mut decoder = match parser.as_str() {
        "pth660-pen" => Decoder::Pen,
        "intuos-v2-aux" => Decoder::Aux,
        "intuos-v2-touch" => Decoder::Touch(IntuosV2TouchParser::default()),
        "wacom-driver-intuos-v2-touch" => {
            Decoder::PrefixedTouch(WacomDriverIntuosV2TouchParser::default())
        }
        "intuos-v3" => Decoder::IntuosV3,
        "bamboo" => Decoder::Bamboo,
        "bamboo-pad" => Decoder::BambooPad,
        "bamboo-v2-aux" => Decoder::BambooV2Aux,
        "uc-logic" => Decoder::Stateless(parse_uc_logic),
        "uc-logic-tilt" => Decoder::Stateless(parse_uc_logic_tilt),
        "uc-logic-v1" => Decoder::Stateless(parse_uc_logic_v1),
        "uc-logic-v2" => Decoder::Stateless(parse_uc_logic_v2),
        "huion-tilt" => Decoder::Stateless(parse_huion_tilt),
        "huion-giano" => Decoder::Stateless(parse_huion_giano),
        "huion-inspiroy" => Decoder::Stateless(parse_huion_inspiroy),
        "xp-pen" => Decoder::Stateless(parse_xp_pen),
        "xp-pen-gen2" => Decoder::Stateless(parse_xp_pen_gen2),
        "xp-pen-offset-pressure" => Decoder::Stateless(parse_xp_pen_offset_pressure),
        "xp-pen-offset-aux" => Decoder::Stateless(parse_xp_pen_offset_auxiliary),
        "xp-pen-dedicated-aux" => Decoder::Stateless(parse_xp_pen_dedicated_auxiliary),
        "tablet" => Decoder::Stateless(parse_tablet),
        "auxiliary" => Decoder::Stateless(parse_auxiliary),
        "skip-byte-tablet" => Decoder::Prefixed(parse_skip_byte_tablet),
        "veikk" => Decoder::Stateless(parse_veikk),
        "veikk-a15" => Decoder::Stateless(parse_veikk_a15),
        "veikk-tilt" => Decoder::Stateless(parse_veikk_tilt),
        "veikk-v1" => Decoder::Prefixed(parse_veikk_v1),
        name => {
            // Any OpenTabletDriver parser type, with or without its namespace.
            let full = if name.starts_with("OpenTabletDriver.") {
                name.to_owned()
            } else {
                format!("OpenTabletDriver.Configurations.Parsers.{name}")
            };
            match otd_core::decoders::ReportParser::for_type(&full) {
                Some(parser) => Decoder::Registry(parser),
                None => return Err(format!("unknown parser {parser:?}\n{USAGE}")),
            }
        }
    };
    let source = match (hex, input) {
        (Some(hex), None) => hex,
        (None, Some(path)) => {
            let file =
                File::open(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
            let mut bytes = Vec::new();
            file.take(MAX_INPUT_BYTES + 1)
                .read_to_end(&mut bytes)
                .map_err(|e| e.to_string())?;
            if bytes.len() as u64 > MAX_INPUT_BYTES {
                return Err("input exceeds 4 MiB".into());
            }
            String::from_utf8(bytes).map_err(|_| "input must be UTF-8 hex text")?
        }
        _ => return Err(format!("choose exactly one of --hex or --input\n{USAGE}")),
    };
    if source.len() as u64 > MAX_INPUT_BYTES {
        return Err("input exceeds 4 MiB".into());
    }
    let mut sequence = 0;
    for (line_index, text) in source.lines().enumerate() {
        let text = text.trim();
        if text.is_empty() || text.starts_with('#') {
            continue;
        }
        sequence += 1;
        if sequence > MAX_REPORTS {
            return Err("input exceeds 4096 reports".into());
        }
        let raw = parse_hex(text).map_err(|e| format!("line {}: {e}", line_index + 1))?;
        let metadata = ReportMetadata {
            device: DeviceId(0),
            session: SessionId(0),
            endpoint: EndpointId(0),
            received_at: Duration::ZERO,
            sequence: sequence as u64,
        };
        let (kind, report) = decoder
            .parse(&raw, metadata)
            .map_err(|e| format!("line {}: {e}", line_index + 1))?;
        let values = report.as_ref().map(|r| r.values).unwrap_or_default();
        let output = json!({
            "schema_version": 1, "parser": parser, "sequence": sequence,
            "kind": match kind { ReportKind::Data => "data", ReportKind::OutOfRange => "out_of_range" },
            "line": line_index + 1, "raw_hex": encode_hex(&raw),
            "report_raw_hex": report.as_ref().map(|r| encode_hex(r.raw)),
            "has_capabilities": values != ReportValues::default(),
            "values": values_json(values)
        });
        println!(
            "{}",
            serde_json::to_string(&output).map_err(|e| e.to_string())?
        );
    }
    if sequence == 0 {
        return Err("input contains no reports".into());
    }
    Ok(())
}

fn parse_hex(text: &str) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::with_capacity(MAX_PACKET_BYTES);
    let mut high = None;
    for character in text.chars() {
        if character.is_ascii_whitespace() || character == ':' {
            continue;
        }
        let digit = character
            .to_digit(16)
            .ok_or("expected hexadecimal byte pairs")? as u8;
        if let Some(value) = high.take() {
            if bytes.len() == MAX_PACKET_BYTES {
                return Err("packet exceeds 192 bytes".into());
            }
            bytes.push((value << 4) | digit);
        } else {
            high = Some(digit);
        }
    }
    if high.is_some() {
        return Err("hex input has an incomplete byte".into());
    }
    if bytes.is_empty() {
        return Err("empty packet".into());
    }
    Ok(bytes)
}

/// The tablet debugger's view of the latest packet, decoded with the
/// session's parser type. A fresh parser decodes it, so stateful parsers show
/// only what this one packet carries.
pub(crate) fn debug_report() -> crate::control::DebugReport {
    let snapshot = otd_core::debug::poll();
    let (tablet, parser) = snapshot
        .device
        .map(|device| (Some(device.name), Some(device.parser)))
        .unwrap_or((None, None));
    let values = parser
        .as_deref()
        .and_then(otd_core::decoders::ReportParser::for_type)
        .filter(|_| !snapshot.bytes.is_empty())
        .map(|mut parser| {
            let metadata = ReportMetadata {
                device: DeviceId(0),
                session: SessionId(0),
                endpoint: EndpointId(0),
                received_at: Duration::ZERO,
                sequence: snapshot.sequence,
            };
            match parser.parse(&snapshot.bytes, metadata) {
                Ok((kind, report)) => json!({
                    "kind": match kind { ReportKind::Data => "data", ReportKind::OutOfRange => "out_of_range" },
                    "values": values_json(report.values),
                }),
                Err(error) => json!({"error": format!("{error:?}")}),
            }
        })
        .unwrap_or(Value::Null);
    crate::control::DebugReport {
        tablet,
        parser,
        sequence: snapshot.sequence,
        raw_hex: encode_hex(&snapshot.bytes),
        values,
    }
}

fn encode_hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(encoded, "{byte:02x}");
    }
    encoded
}

fn buttons(value: Buttons) -> Vec<bool> {
    (0..value.len())
        .map(|index| value.get(index).unwrap_or(false))
        .collect()
}

fn analog_kind(kind: AnalogKind) -> &'static str {
    match kind {
        AnalogKind::Generic => "generic",
        AnalogKind::Wheel => "wheel",
    }
}

fn values_json(values: ReportValues) -> Value {
    json!({
        "position": values.position, "pressure": values.pressure,
        "tilt": values.tilt, "eraser": values.eraser,
        "tool": values.tool.map(|v| json!({"serial": v.serial, "raw_tool_id": v.raw_tool_id,
            "kind": match v.tool { ToolType::Pen => "pen", ToolType::Eraser => "eraser" }})),
        "pen_buttons": values.pen_buttons.map(buttons),
        "near_proximity": values.near_proximity, "hover_distance": values.hover_distance,
        "sense": values.sense, "tip_switch": values.tip_switch, "rotation": values.rotation,
        "aux_buttons": values.aux_buttons.map(buttons),
        "mouse_buttons": values.mouse_buttons.map(buttons), "mouse_scroll": values.mouse_scroll,
        "absolute_analog": values.absolute_analog.map(|v| json!({"kind": analog_kind(v.kind), "positions": v.positions.as_slice()})),
        "relative_analog": values.relative_analog.map(|v| json!({"kind": analog_kind(v.kind), "deltas": v.deltas.as_slice()})),
        "wheel_buttons": values.wheel_buttons.map(|v| v.as_slice().iter().copied().map(buttons).collect::<Vec<_>>()),
        "touches": values.touches.map(|v| v.as_slice().iter().map(|p| p.map(|p| json!({"id": p.id, "position": p.position}))).collect::<Vec<_>>())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_report_decodes_the_latest_packet_with_the_session_parser() {
        let registration = otd_core::debug::Registration::new(otd_core::debug::Device {
            name: "Wacom PTH-660".into(),
            parser: "OpenTabletDriver.Configurations.Parsers.Wacom.IntuosV2.IntuosV2ReportParser"
                .into(),
        });
        // The first poll arms the tap; the report thread then keeps packets.
        let _ = debug_report();
        let mut packet = [0u8; 17];
        packet[0] = 0x10;
        packet[1] = 0x61;
        packet[2] = 0x10;
        packet[8] = 0x20;
        otd_core::debug::record(&packet);
        let report = debug_report();
        drop(registration);
        assert_eq!(report.tablet.as_deref(), Some("Wacom PTH-660"));
        assert!(report.raw_hex.starts_with("106110"), "{}", report.raw_hex);
        assert_eq!(report.values["values"]["position"][0], 16.0);
        assert_eq!(report.values["values"]["pressure"], 32);
    }
}
