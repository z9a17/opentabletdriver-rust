//! Offline inspection of explicitly supplied bytes. Never opens devices/plugins.
use otd_core::{protocol, reports::*};
use serde_json::{Value, json};
use std::{fs::File, io::Read, path::PathBuf, time::Duration};

const MAX_INPUT_BYTES: u64 = 4 * 1024 * 1024;
const MAX_REPORTS: usize = 4096;
const MAX_PACKET_BYTES: usize = otd_core::debug::MAX_BYTES;

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
65535 bytes per packet. Each output line is one JSON snapshot; a later error does
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
    parse_hex_within(text, MAX_PACKET_BYTES)
}

fn parse_hex_within(text: &str, maximum: usize) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::with_capacity(maximum.min(MAX_PACKET_BYTES).min(text.len() / 2));
    let mut high = None;
    for character in text.chars() {
        if character.is_ascii_whitespace() || character == ':' {
            continue;
        }
        let digit = character
            .to_digit(16)
            .ok_or("expected hexadecimal byte pairs")? as u8;
        if let Some(value) = high.take() {
            if bytes.len() == maximum {
                return Err(format!("packet exceeds {maximum} bytes"));
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

/// The tablet debugger's view of the latest packet, for the daemon to send.
/// The daemon never decodes it: a decoding fault in the debugger must not be
/// able to stop the process that owns the tablet and the output. Clients
/// decode with [`decode_debug_report`].
pub(crate) fn debug_report() -> crate::control::DebugReport {
    let snapshot = otd_core::debug::poll();
    let (tablet, parser) = snapshot
        .device
        .map(|device| (Some(device.name), Some(device.parser)))
        .unwrap_or((None, None));
    crate::control::DebugReport {
        tablet,
        parser,
        sequence: snapshot.sequence,
        raw_hex: encode_hex(&snapshot.bytes),
        values: Value::Null,
    }
}

/// Hex encoding and metadata allocation belong to the daemon control thread;
/// the report tap stores only fixed-size raw bytes and timestamps.
pub(crate) fn debug_capture_status(instance: &str, capture: otd_core::debug::CaptureStatus)
    -> crate::control::DebugCaptureStatus {
    use crate::control::{DebugCaptureStatus, DebugCaptureStopReason, DebugCaptureToken};
    use otd_core::debug::CaptureStopReason;
    DebugCaptureStatus {
        token: DebugCaptureToken { instance: instance.to_owned(), epoch: capture.epoch, session: capture.session },
        tablet: capture.device.name, parser: capture.device.parser,
        auxiliary_parser: capture.auxiliary_parser,
        report_length: capture.report_length as u32, capacity_reports: capture.capacity_reports as u32,
        started_unix_ms: capture.started_unix_ms, active: capture.active,
        last_sequence: capture.last_sequence, resolved_reports: capture.resolved_reports,
        pending_reports: capture.pending_reports, lost_tap: capture.lost_tap,
        overflow: capture.overflow, oversized: capture.oversized,
        acknowledged_sequence: capture.acknowledged_sequence,
        stop_reason: capture.stop_reason.map(|reason| match reason {
            CaptureStopReason::Requested => DebugCaptureStopReason::Requested,
            CaptureStopReason::LeaseExpired => DebugCaptureStopReason::LeaseExpired,
            CaptureStopReason::SessionEnded => DebugCaptureStopReason::SessionEnded,
            CaptureStopReason::SequenceLimit => DebugCaptureStopReason::SequenceLimit,
        }),
        lease_remaining_ms: capture.lease_remaining_ms,
    }
}
pub(crate) fn debug_capture_batch(instance: &str, batch: otd_core::debug::CaptureBatch)
    -> crate::control::DebugCaptureBatch {
    crate::control::DebugCaptureBatch {
        capture: debug_capture_status(instance, batch.capture),
        packets: batch.packets.into_iter().map(|packet| crate::control::DebugCapturePacket {
            sequence: packet.sequence, elapsed_us: packet.elapsed_us, auxiliary: packet.auxiliary,
            raw_hex: encode_hex(&packet.bytes),
        }).collect(),
        next_sequence: batch.next_sequence,
    }
}

/// Fills in the decoded values of a debugger snapshot in the client's own
/// process, with a fresh parser of the session's type. Stateful parsers
/// therefore show only what this one packet carries. A snapshot a daemon
/// already decoded (earlier versions did) is left as it is.
pub(crate) fn decode_debug_report(report: &mut crate::control::DebugReport) {
    if !report.values.is_null() {
        return;
    }
    let Some(parser) = report.parser.as_deref() else {
        return;
    };
    let Ok(bytes) = parse_hex_within(&report.raw_hex, otd_core::debug::MAX_BYTES) else {
        return;
    };
    let Some(mut parser) = otd_core::decoders::ReportParser::for_type(parser) else {
        report.values = json!({"error": "this driver has no decoder for the parser"});
        return;
    };
    let metadata = ReportMetadata {
        device: DeviceId(0),
        session: SessionId(0),
        endpoint: EndpointId(0),
        received_at: Duration::ZERO,
        sequence: report.sequence,
    };
    report.values = match parser.parse(&bytes, metadata) {
        Ok((kind, decoded)) => json!({
            "kind": match kind { ReportKind::Data => "data", ReportKind::OutOfRange => "out_of_range" },
            "values": values_json(decoded.values),
        }),
        Err(error) => json!({"error": format!("{error:?}")}),
    };
}

/// One endpoint's decoder in a full-rate capture reader. This parser starts at
/// the capture boundary, which can be mid-session: its initial retained state
/// is unknown, and cannot be recovered from the daemon's live decoder. Feed
/// every packet in endpoint order, and reset both endpoint decoders whenever
/// the shared capture sequence has a gap. This never runs on the report tap.
pub(crate) struct DebugCaptureDecoder {
    parser_type: Option<String>,
    parser: Option<otd_core::decoders::ReportParser>,
    session: u64,
    endpoint: u32,
    generation: u64,
}
impl DebugCaptureDecoder {
    pub(crate) fn new(parser: Option<&str>, session: u64, endpoint: u32) -> Self {
        Self {
            parser_type: parser.map(str::to_owned),
            parser: parser.and_then(otd_core::decoders::ReportParser::for_type),
            session, endpoint, generation: 0,
        }
    }
    /// Reconstruct rather than relying on a parser-specific partial reset.
    /// Values before this boundary remain unknown even if later packets can
    /// rebuild particular touch slots, pressure, rotation or tool state.
    pub(crate) fn reset(&mut self) {
        self.parser = self.parser_type.as_deref()
            .and_then(otd_core::decoders::ReportParser::for_type);
        self.generation = self.generation.saturating_add(1);
    }
    /// Unlike the sampled snapshot helper, this always feeds the owned parser
    /// and rewrites decoded values. Session/endpoint are fixed by construction;
    /// sequence and read-completion offset are explicit packet metadata.
    pub(crate) fn decode(&mut self, report: &mut crate::control::DebugReport, elapsed_us: u64) {
        report.values = self.decode_values(report, elapsed_us);
    }
    fn decode_values(&mut self, report: &crate::control::DebugReport, elapsed_us: u64) -> Value {
        let result = if report.parser.as_deref() != self.parser_type.as_deref() {
            Err("capture packet parser changed within one endpoint".to_owned())
        } else {
            parse_hex_within(&report.raw_hex, otd_core::debug::MAX_BYTES).and_then(|bytes| {
                let parser = self.parser.as_mut().ok_or_else(|| {
                    "this capture endpoint has no supported decoder".to_owned()
                })?;
                let metadata = ReportMetadata {
                    device: DeviceId(0), session: SessionId(self.session), endpoint: EndpointId(self.endpoint),
                    received_at: Duration::from_micros(elapsed_us), sequence: report.sequence,
                };
                parser.parse(&bytes, metadata).map(|(kind, decoded)| json!({
                    "kind": match kind { ReportKind::Data => "data", ReportKind::OutOfRange => "out_of_range" },
                    "values": values_json(decoded.values),
                })).map_err(|error| format!("{error:?}"))
            })
        };
        let mut decoded = match result {
            Ok(decoded) => decoded,
            Err(error) => {
                // An invalid packet cannot establish continuity for retained
                // parser state. Raw capture still preserves it and its error.
                self.reset();
                // Keep numeric decoder metadata out of report-value ranges:
                // the statistics reader explicitly selects this values field.
                json!({"error": error, "values": Value::Null})
            }
        };
        decoded["decoder_state"] = json!({
            "scope": if self.generation == 0 { "since_capture_start" } else { "since_last_reset" },
            "generation": self.generation, "pre_capture_state_known": false,
            "state_before_boundary_known": false, "source_session": self.session,
            "endpoint": self.endpoint, "elapsed_us": elapsed_us, "sequence": report.sequence,
        });
        decoded
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
    fn capture_packet(parser: &str, sequence: u64, bytes: &[u8]) -> crate::control::DebugReport {
        crate::control::DebugReport { tablet: Some("capture fixture".into()), parser: Some(parser.into()),
            sequence, raw_hex: encode_hex(bytes), values: Value::Null }
    }
    #[test]
    fn capture_decoder_preserves_touch_updates_and_resets_unknown_gap_state() {
        let parser = "OpenTabletDriver.Configurations.Parsers.Wacom.IntuosV2.IntuosV2ReportParser";
        let mut decoder = DebugCaptureDecoder::new(Some(parser), 41, 0);
        let mut first = [0u8; 40];
        first[0] = 0x21;
        first[2] = 1;
        first[3] = 1;
        first[4] = 10;
        let mut report = capture_packet(parser, 1, &first);
        decoder.decode(&mut report, 123);
        assert_eq!(report.values["values"]["touches"][0]["position"][0], 10.0);
        assert_eq!(report.values["decoder_state"]["scope"], "since_capture_start");
        assert_eq!(report.values["decoder_state"]["pre_capture_state_known"], false);
        assert_eq!(report.values["decoder_state"]["source_session"], 41);
        assert_eq!(report.values["decoder_state"]["elapsed_us"], 123);
        let mut second = first;
        second[2] = 2;
        second[4] = 20;
        let mut report = capture_packet(parser, 2, &second);
        decoder.decode(&mut report, 456);
        assert_eq!(report.values["values"]["touches"][0]["position"][0], 10.0);
        assert_eq!(report.values["values"]["touches"][1]["position"][0], 20.0);
        let mut independent = DebugCaptureDecoder::new(Some(parser), 41, 1);
        independent.decode(&mut report, 456);
        assert!(report.values["values"]["touches"][0].is_null(), "another endpoint cannot inherit touches");
        assert_eq!(report.values["decoder_state"]["endpoint"], 1);
        decoder.reset();
        let mut report = capture_packet(parser, 4, &second);
        decoder.decode(&mut report, 789);
        assert!(report.values["values"]["touches"][0].is_null());
        assert_eq!(report.values["decoder_state"]["scope"], "since_last_reset");
        assert_eq!(report.values["decoder_state"]["generation"], 1);
        assert_eq!(report.values["decoder_state"]["sequence"], 4);
        // Sampled snapshots continue to use a fresh parser for each packet.
        report.values = Value::Null;
        decode_debug_report(&mut report);
        assert!(report.values["values"]["touches"][0].is_null());
        assert!(report.values.get("decoder_state").is_none());
    }
    #[test]
    fn capture_decoder_preserves_pen_values_across_rotation_packets() {
        let parser = "OpenTabletDriver.Configurations.Parsers.Wacom.IntuosV1.IntuosV1ReportParser";
        let tablet = [0x02, 0xe3, 0x12, 0x34, 0x05, 0x06, 0x80, 0xc0, 0x40, 0x03];
        let rotation = [0x02, 0xea, 0, 0, 0, 0, 0, 0, 0, 0];
        let mut decoder = DebugCaptureDecoder::new(Some(parser), 57, 0);
        let mut first = capture_packet(parser, 1, &tablet);
        decoder.decode(&mut first, 1_000);
        let mut second = capture_packet(parser, 2, &rotation);
        decoder.decode(&mut second, 2_000);
        assert_eq!(second.values["values"]["pressure"], 1_031);
        assert_eq!(second.values["values"]["pressure"], first.values["values"]["pressure"]);
        assert_eq!(second.values["values"]["tilt"], first.values["values"]["tilt"]);
        assert_eq!(second.values["values"]["pen_buttons"], first.values["values"]["pen_buttons"]);
        decoder.reset();
        decoder.decode(&mut second, 2_000);
        assert_eq!(second.values["values"]["pressure"], 0);
        assert_eq!(second.values["decoder_state"]["state_before_boundary_known"], false);
        assert_eq!(second.values["decoder_state"]["generation"], 1);
    }
    #[test]
    fn capture_decode_errors_reset_retained_state_and_preserve_raw_packet() {
        let parser = "OpenTabletDriver.Configurations.Parsers.Wacom.IntuosV1.IntuosV1ReportParser";
        let mut decoder = DebugCaptureDecoder::new(Some(parser), 7, 0);
        let mut report = capture_packet(parser, 3, &[0x02, 0xe3]);
        let raw = report.raw_hex.clone();
        decoder.decode(&mut report, 8);
        assert!(report.values["error"].is_string());
        assert!(report.values.get("values").unwrap().is_null());
        assert_eq!(report.raw_hex, raw);
        assert_eq!(report.values["decoder_state"]["generation"], 1);
        let mut changed = capture_packet("Unknown.Parser", 4, &[1]);
        decoder.decode(&mut changed, 9);
        assert!(changed.values["error"].as_str().unwrap().contains("parser changed"));
        assert_eq!(changed.values["decoder_state"]["generation"], 2);
    }

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
        let mut report = debug_report();
        drop(registration);
        assert_eq!(report.tablet.as_deref(), Some("Wacom PTH-660"));
        assert!(report.raw_hex.starts_with("106110"), "{}", report.raw_hex);
        // The daemon side sends bytes only; the client decodes them.
        assert!(report.values.is_null());
        decode_debug_report(&mut report);
        assert_eq!(report.values["values"]["position"][0], 16.0);
        assert_eq!(report.values["values"]["pressure"], 32);
    }

    #[test]
    fn client_decoding_keeps_daemon_values_and_handles_odd_snapshots() {
        let mut report = crate::control::DebugReport {
            tablet: Some("Tablet".into()),
            parser: Some("OpenTabletDriver.Plugin.Tablet.TabletReportParser".into()),
            sequence: 3,
            raw_hex: "ff".repeat(otd_core::debug::MAX_BYTES),
            values: serde_json::Value::Null,
        };
        // A full-size capture decodes; it is longer than `decode` accepts.
        decode_debug_report(&mut report);
        assert!(!report.values.is_null());
        let decoded = report.values.clone();
        decode_debug_report(&mut report);
        assert_eq!(report.values, decoded, "already decoded values stay");
        for (parser, raw_hex) in [
            (Some("Unknown.Parser"), "10"),
            (
                Some("OpenTabletDriver.Plugin.Tablet.TabletReportParser"),
                "",
            ),
            (
                Some("OpenTabletDriver.Plugin.Tablet.TabletReportParser"),
                "1",
            ),
            (
                Some("OpenTabletDriver.Plugin.Tablet.TabletReportParser"),
                "zz",
            ),
            (None, "10"),
        ] {
            let mut report = crate::control::DebugReport {
                tablet: None,
                parser: parser.map(str::to_owned),
                sequence: 0,
                raw_hex: raw_hex.into(),
                values: serde_json::Value::Null,
            };
            decode_debug_report(&mut report);
            if parser == Some("Unknown.Parser") {
                assert!(report.values["error"].is_string());
            } else {
                assert!(report.values.is_null(), "{parser:?} {raw_hex:?}");
            }
        }
    }
}
