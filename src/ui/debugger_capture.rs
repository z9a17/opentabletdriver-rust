//! Full-rate raw capture consumer. No report traverses the sampled GUI channel.
//! Each acknowledged cursor has already reached a flushed, synced capture file.
use crate::control::{self, Command, DebugCaptureStatus, DebugCaptureStopReason,
    DebugCaptureToken, DebugReport, Reply, Request};
use super::debugger_data::Statistics;
use serde_json::json;
use std::io::{BufWriter, Write};
use std::path::Path;
use std::sync::{Arc, mpsc};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

const LEASE_MS: u32 = 15_000;
const CAPACITY_BYTES: u32 = 8 * 1024 * 1024;

/// Each endpoint owns its parser's touch/tool history for this capture only.
/// A global gap can belong to either endpoint, so invalidate both histories.
struct CaptureDecoders {
    primary: crate::decode_cli::DebugCaptureDecoder,
    auxiliary: crate::decode_cli::DebugCaptureDecoder,
    sequence: u64,
    continuity_lost: bool,
    reset_pending: bool,
}

impl CaptureDecoders {
    fn new(capture: &DebugCaptureStatus) -> Self {
        Self {
            primary: crate::decode_cli::DebugCaptureDecoder::new(Some(&capture.parser), capture.token.session, 0),
            auxiliary: crate::decode_cli::DebugCaptureDecoder::new(capture.auxiliary_parser.as_deref(), capture.token.session, 1),
            sequence: 0, continuity_lost: false, reset_pending: false,
        }
    }
    fn reset_for_gap(&mut self) {
        self.primary.reset();
        self.auxiliary.reset();
        self.continuity_lost = true;
        self.reset_pending = true;
    }
    fn decode(&mut self, report: &mut DebugReport, elapsed_us: u64, auxiliary: bool) -> bool {
        if report.sequence != self.sequence.saturating_add(1) { self.reset_for_gap(); }
        let decoder = if auxiliary { &mut self.auxiliary } else { &mut self.primary };
        decoder.decode(report, elapsed_us);
        // The helper also resets its endpoint after a decoding failure. Raw
        // capture can remain complete while decoded history becomes unknown.
        if report.values.get("error").is_some() { self.continuity_lost = true; }
        self.sequence = report.sequence;
        std::mem::take(&mut self.reset_pending)
    }
    fn advance(&mut self, cursor: u64) {
        // Read cursors may also advance across a trailing explicit loss gap
        // with no later packet. Reset before the next batch's first decode.
        if cursor > self.sequence { self.reset_for_gap(); self.sequence = cursor; }
    }
}

pub(super) struct Recorder {
    stop: Arc<AtomicBool>,
    written: Arc<AtomicU64>,
    completed: mpsc::Receiver<Result<(), String>>,
}

impl Recorder {
    pub fn start(path: &Path) -> Result<Self, String> {
        let file = std::fs::OpenOptions::new().write(true).create_new(true).open(path)
            .map_err(|error| format!("Cannot create recording {}: {error}", path.display()))?;
        let stop = Arc::new(AtomicBool::new(false));
        let written = Arc::new(AtomicU64::new(0));
        let (complete, completed) = mpsc::sync_channel(1);
        let cancel = Arc::clone(&stop);
        let count = Arc::clone(&written);
        let worker = std::thread::Builder::new().name("debugger-full-capture".into()).spawn(move || {
            let mut transport = |command| {
                control::request(&Request::new(1, command), Duration::from_secs(2))
                    .map(|response| response.reply).map_err(|error| error.to_string())
            };
            let result = record(file, &cancel, &count, &mut transport);
            let _ = complete.send(result);
        });
        if let Err(error) = worker {
            let _ = std::fs::remove_file(path);
            return Err(error.to_string());
        }
        Ok(Self { stop, written, completed })
    }
    pub fn stop(&self) { self.stop.store(true, Ordering::Release); }
    pub fn active(&self) -> bool { !self.stop.load(Ordering::Acquire) }
    pub fn status(&self) -> String {
        format!("{}: {} saved", if self.active() { "Capturing raw reports" } else { "Draining capture" },
            self.written.load(Ordering::Relaxed))
    }
    pub fn result(&self) -> Option<Result<(), String>> {
        match self.completed.try_recv() {
            Ok(result) => Some(result),
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => Some(Err("Capture worker exited without a completion result.".into())),
        }
    }
}

impl Drop for Recorder {
    fn drop(&mut self) { self.stop(); }
}

fn request(transport: &mut impl FnMut(Command) -> Result<Reply, String>, command: Command) -> Result<Reply, String> {
    let mut failure = String::new();
    for attempt in 0..3 {
        match transport(command.clone()) {
            Ok(Reply::Error { error }) => return Err(error.message),
            Ok(reply) => return Ok(reply),
            Err(error) => failure = error,
        }
        if attempt < 2 { std::thread::sleep(Duration::from_millis(50)); }
    }
    Err(format!("Capture request failed after three attempts: {failure}"))
}

fn same_token(left: &DebugCaptureToken, right: &DebugCaptureToken) -> bool {
    left.instance == right.instance && left.epoch == right.epoch && left.session == right.session
}

fn loss(status: &DebugCaptureStatus) -> Option<String> {
    (status.lost_tap != 0 || status.overflow != 0 || status.oversized != 0).then(|| format!(
        "Capture is incomplete: lost tap {}, overflow {}, oversized {}.",
        status.lost_tap, status.overflow, status.oversized))
}

fn unexpected_end(status: &DebugCaptureStatus) -> Option<String> {
    (!status.active && !matches!(status.stop_reason, Some(DebugCaptureStopReason::Requested)))
        .then(|| format!("Capture ended unexpectedly: {:?}.", status.stop_reason))
}

fn durable(writer: &mut BufWriter<std::fs::File>) -> Result<(), String> {
    writer.flush().and_then(|_| writer.get_ref().sync_data())
        .map_err(|error| format!("Cannot persist recording: {error}"))
}

fn record(file: std::fs::File, stop: &AtomicBool, written: &AtomicU64,
    transport: &mut impl FnMut(Command) -> Result<Reply, String>) -> Result<(), String>
{
    let mut writer = BufWriter::new(file);
    let mut status: Option<DebugCaptureStatus> = None;
    let mut cursor = 0;
    let mut statistics = Statistics::default();
    let mut decoded_continuity_lost = false;
    let result = (|| -> Result<(), String> {
        let Reply::Status { status: daemon } = request(transport, Command::Status)? else {
            return Err("The daemon did not return its capture identity.".into());
        };
        // The same nonzero ID is reused for a lost Start reply, never a new session.
        let start_id = ((std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default().as_nanos() as u64) ^ (u64::from(std::process::id()) << 32)).max(1);
        let started = request(transport, Command::DebugCaptureStart {
            expected: daemon.identity(), start_id, capacity_bytes: CAPACITY_BYTES, lease_ms: LEASE_MS,
        }).map_err(|error| {
            if error.starts_with("Capture request failed after") {
                format!("{error}. Start could not be confirmed; any armed capture expires after its 15-second lease.")
            } else { error }
        })?;
        let Reply::DebugCaptureStarted { capture } = started else {
            return Err("The daemon does not support full-rate capture. Use Record Sampled Reports for the fallback.".into());
        };
        status = Some(capture);
        writeln!(writer, "{}", json!({"format":"otd-rust-captured-reports", "version":1,
            "capture_mode":"full_rate", "complete_hid_stream":false, "tap_losses_measured":true,
            "capture_boundary":"selected_session_read_completions", "hardware_or_transport_losses_measured":false,
            "decoded_state_scope":"capture_local", "initial_prior_session_state_available":false,
            "capture":status.as_ref().unwrap()})).map_err(|error| error.to_string())?;
        durable(&mut writer)?;
        let token = status.as_ref().unwrap().token.clone();
        let mut decoders = CaptureDecoders::new(status.as_ref().unwrap());
        let mut frozen = None;
        let mut failure = None;
        let mut draining_since = None;
        loop {
            let current = status.as_ref().unwrap();
            if let Some(error) = loss(current) { failure = Some(error); }
            if let Some(error) = unexpected_end(current) { failure.get_or_insert(error); }
            if frozen.is_none() && (stop.load(Ordering::Acquire) || failure.is_some() || !current.active) {
                stop.store(true, Ordering::Release);
                let Reply::DebugCaptureStopped { capture } = request(transport, Command::DebugCaptureStop { capture: token.clone() })?
                    else { return Err("The daemon did not acknowledge capture stop.".into()); };
                if !same_token(&token, &capture.token) || capture.active {
                    return Err("Capture stop returned a different session or an active capture.".into());
                }
                frozen = Some(capture.last_sequence);
                draining_since = Some(Instant::now());
                status = Some(capture);
            }
            if draining_since.is_some_and(|start| start.elapsed() > Duration::from_secs(20)) {
                return Err("Capture drain timed out; the file is incomplete.".into());
            }
            // Only the last durable cursor is acknowledged. A lost Read reply
            // retries this exact command and cannot skip a not-yet-written batch.
            let reply = request(transport, Command::DebugCaptureRead { capture: token.clone(),
                after_sequence: cursor, acknowledge_through: cursor, limit: 512, lease_ms: LEASE_MS });
            let batch = match reply {
                Ok(Reply::DebugCaptureRead { batch }) => batch,
                Ok(_) => return Err("The daemon returned an unexpected capture batch.".into()),
                Err(error) if frozen.is_none() => {
                    failure = Some(error);
                    stop.store(true, Ordering::Release);
                    continue; // Freeze first, then make one bounded drain attempt.
                }
                Err(error) => return Err(error),
            };
            if !same_token(&token, &batch.capture.token) || batch.next_sequence < cursor
                || batch.next_sequence > batch.capture.last_sequence
                || frozen.is_some_and(|last| batch.capture.last_sequence != last) {
                return Err("Capture cursor or session changed unexpectedly; the file is incomplete.".into());
            }
            if batch.next_sequence - cursor != batch.packets.len() as u64 && loss(&batch.capture).is_none() {
                failure = Some("Capture contains a sequence gap without a loss counter; the file is incomplete.".into());
            }
            let mut previous = cursor;
            for packet in &batch.packets {
                if packet.sequence <= previous || packet.sequence > batch.next_sequence {
                    return Err("Capture batch contained duplicate or out-of-order reports.".into());
                }
                previous = packet.sequence;
                let mut report = DebugReport { tablet: Some(batch.capture.tablet.clone()),
                    parser: if packet.auxiliary { batch.capture.auxiliary_parser.clone() } else { Some(batch.capture.parser.clone()) },
                    sequence: packet.sequence, raw_hex: packet.raw_hex.clone(), values: serde_json::Value::Null };
                let reset_before = decoders.decode(&mut report, packet.elapsed_us, packet.auxiliary);
                decoded_continuity_lost = decoders.continuity_lost;
                statistics.observe(&report);
                // Preserve the sampled recorder's elapsed_us/report row shape.
                writeln!(writer, "{}", json!({"elapsed_us":packet.elapsed_us, "report":report,
                    "auxiliary":packet.auxiliary, "session":token.session, "epoch":token.epoch,
                    "decoded_reset_before":reset_before, "decoded_continuity_lost":decoded_continuity_lost}))
                    .map_err(|error| error.to_string())?;
            }
            decoders.advance(batch.next_sequence);
            decoded_continuity_lost = decoders.continuity_lost;
            // A gap cursor is durable too, including the explicit loss counters.
            if batch.next_sequence != cursor || !batch.capture.active || loss(&batch.capture).is_some() {
                writeln!(writer, "{}", json!({"capture_batch":{"next_sequence":batch.next_sequence,
                    "decoded_continuity_lost":decoded_continuity_lost,
                    "capture":batch.capture}})).map_err(|error| error.to_string())?;
                durable(&mut writer)?;
                written.fetch_add(batch.packets.len() as u64, Ordering::Relaxed);
            }
            cursor = batch.next_sequence;
            status = Some(batch.capture);
            if frozen == Some(cursor) && status.as_ref().unwrap().pending_reports == 0 {
                if let Some(error) = loss(status.as_ref().unwrap()) { failure = Some(error); }
                if let Some(error) = unexpected_end(status.as_ref().unwrap()) { failure.get_or_insert(error); }
                return failure.map_or(Ok(()), Err);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    })();
    // Even transport failure produces an explicit incomplete footer if the file
    // remains writable. Do not hide the primary failure behind a cleanup error.
    if let Some(capture) = &status {
        if let Ok(Reply::DebugCaptureStopped { capture: final_status }) =
            request(transport, Command::DebugCaptureStop { capture: capture.token.clone() })
            && same_token(&capture.token, &final_status.token)
        { status = Some(final_status); }
    }
    // Preserve both the primary failure and every known tap-loss counter on
    // the visible completion path, even when Stop/drain fails afterward.
    let result = match (result, status.as_ref().and_then(loss)) {
        (Ok(()), Some(detail)) => Err(detail),
        (Err(error), Some(detail)) if !error.contains(&detail) => Err(format!("{error} {detail}")),
        (result, _) => result,
    };
    let footer = writeln!(writer, "{}", json!({"summary":{"written":written.load(Ordering::Relaxed),
        "queue_dropped":0, "sequence_gaps":cursor.saturating_sub(written.load(Ordering::Relaxed)), "statistics":statistics.ranges,
        "complete_tap_stream":result.is_ok(), "complete_hid_stream":false,
        "decoded_continuity_lost":decoded_continuity_lost,
        "error":result.as_ref().err(), "final_sequence":cursor,
        "capture":status}})).map_err(|error| format!("Cannot write recording summary: {error}"))
        .and_then(|_| durable(&mut writer));
    let mut outcome = result.and(footer);
    if let Some(capture) = status {
        let released = request(transport, Command::DebugCaptureRelease { capture: capture.token });
        if !matches!(released, Ok(Reply::DebugCaptureReleased)) {
            let cleanup = released.err().unwrap_or_else(|| "Unexpected capture release reply.".into());
            outcome = Err(format!("{} Capture cleanup failed: {cleanup}. Its lease will expire.",
                outcome.err().unwrap_or_else(|| "Recording saved.".into())));
        }
    }
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::{DebugCaptureBatch, DebugCapturePacket};

    fn status(active: bool, pending: u64, lost: u64, reason: &str) -> DebugCaptureStatus {
        serde_json::from_value(json!({"token":{"instance":"fixture","epoch":7,"session":9},
            "tablet":"fixture","parser":"unsupported-fixture","auxiliary_parser":null,
            "report_length":2,"capacity_reports":64,"started_unix_ms":1234,
            "active":active,"last_sequence":2,"resolved_reports":2-pending,"pending_reports":pending,
            "lost_tap":lost,"overflow":0,"oversized":0,"acknowledged_sequence":0,
            "stop_reason":if active { serde_json::Value::Null } else { json!(reason) },
            "lease_remaining_ms":15_000})).unwrap()
    }
    fn daemon() -> Reply {
        Reply::Status { status: serde_json::from_value(json!({"instance":"fixture","state":"running",
            "generation":1,"profile":null,"last_error":null,"logs":[],"log_sequence":0})).unwrap() }
    }
    fn packet(sequence: u64) -> DebugCapturePacket {
        DebugCapturePacket { sequence, elapsed_us: sequence * 211, auxiliary: sequence == 2, raw_hex:"1001".into() }
    }

    fn touch_report(sequence: u64, contact: u8, x: u8) -> DebugReport {
        let mut raw = [0u8; 40];
        raw[0] = 0x21;
        raw[2] = contact;
        raw[3] = 1;
        raw[4] = x;
        DebugReport { tablet:Some("fixture".into()), parser:Some("Wacom.IntuosV2.IntuosV2ReportParser".into()),
            sequence, raw_hex:raw.iter().map(|byte| format!("{byte:02x}")).collect(), values:serde_json::Value::Null }
    }

    #[test]
    fn capture_touch_decoders_keep_endpoint_state_and_clear_both_after_gaps() {
        let mut capture = status(true, 0, 0, "requested");
        capture.parser = "Wacom.IntuosV2.IntuosV2ReportParser".into();
        capture.auxiliary_parser = Some(capture.parser.clone());
        let mut decoders = CaptureDecoders::new(&capture);
        let mut first = touch_report(1, 1, 10);
        assert!(!decoders.decode(&mut first, 211, false));
        let mut auxiliary = touch_report(2, 3, 30);
        assert!(!decoders.decode(&mut auxiliary, 422, true));
        assert!(auxiliary.values["values"]["touches"][0].is_null());
        let mut second = touch_report(3, 2, 20);
        assert!(!decoders.decode(&mut second, 633, false));
        assert_eq!(second.values["values"]["touches"][0]["position"][0], 10.0);
        assert_eq!(second.values["values"]["touches"][1]["position"][0], 20.0);
        assert!(second.values["values"]["touches"][2].is_null());
        assert_eq!(second.values["decoder_state"]["source_session"], 9);
        assert_eq!(second.values["decoder_state"]["endpoint"], 0);
        assert_eq!(second.values["decoder_state"]["elapsed_us"], 633);
        assert_eq!(auxiliary.values["decoder_state"]["endpoint"], 1);
        assert!(!decoders.continuity_lost);
        // Missing global sequence 4 could have changed either endpoint.
        let mut after_gap = touch_report(5, 4, 40);
        assert!(decoders.decode(&mut after_gap, 1055, true));
        assert!(after_gap.values["values"]["touches"][2].is_null());
        assert_eq!(after_gap.values["decoder_state"]["scope"], "since_last_reset");
        assert_eq!(after_gap.values["decoder_state"]["generation"], 1);
        let mut primary_after_gap = touch_report(6, 5, 50);
        assert!(!decoders.decode(&mut primary_after_gap, 1266, false));
        assert!(primary_after_gap.values["values"]["touches"][0].is_null());
        assert!(primary_after_gap.values["values"]["touches"][1].is_null());
        assert!(decoders.continuity_lost);
        // A cursor can resolve a loss gap without returning another packet.
        decoders.advance(8);
        let mut after_trailing_gap = touch_report(9, 1, 90);
        assert!(decoders.decode(&mut after_trailing_gap, 1899, false));
        assert!(after_trailing_gap.values["values"]["touches"][4].is_null());
    }
    fn recording_fixture(name: &str, stop_immediately: bool, transport: &mut impl FnMut(Command) -> Result<Reply, String>)
        -> (Result<(), String>, Vec<serde_json::Value>)
    {
        let directory = std::path::PathBuf::from("E:/AgentWork/tmp").join(format!(
            "otd-capture-{name}-{}-{}", std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("capture.jsonl");
        let file = std::fs::OpenOptions::new().write(true).create_new(true).open(&path).unwrap();
        let result = record(file, &AtomicBool::new(stop_immediately), &AtomicU64::new(0), transport);
        let lines = std::fs::read_to_string(&path).unwrap().lines()
            .map(|line| serde_json::from_str(line).unwrap()).collect();
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(directory).unwrap();
        (result, lines)
    }
    #[test]
    fn capture_retries_same_start_and_read_then_drains_frozen_tail() {
        let mut start_ids = Vec::new();
        let mut cursors = Vec::new();
        let mut failed_read = false;
        let (result, lines) = recording_fixture("retry-drain", true, &mut |command| match command {
            Command::Status => Ok(daemon()),
            Command::DebugCaptureStart { start_id, .. } => {
                start_ids.push(start_id);
                if start_ids.len() == 1 { Err("lost Start reply".into()) }
                else { Ok(Reply::DebugCaptureStarted { capture:status(true, 2, 0, "requested") }) }
            }
            Command::DebugCaptureStop { .. } => Ok(Reply::DebugCaptureStopped { capture:status(false, 0, 0, "requested") }),
            Command::DebugCaptureRead { after_sequence, acknowledge_through, .. } => {
                cursors.push((after_sequence, acknowledge_through));
                if !failed_read { failed_read = true; return Err("lost batch reply".into()); }
                Ok(Reply::DebugCaptureRead { batch:DebugCaptureBatch {
                    capture:status(false, u64::from(after_sequence == 0), 0, "requested"),
                    packets:vec![packet(after_sequence + 1)], next_sequence:after_sequence + 1,
                } })
            }
            Command::DebugCaptureRelease { .. } => Ok(Reply::DebugCaptureReleased),
            _ => panic!("unexpected command"),
        });
        result.unwrap();
        assert_eq!(start_ids.len(), 2);
        assert_eq!(start_ids[0], start_ids[1]);
        assert_eq!(cursors, [(0,0), (0,0), (1,1)]);
        let rows: Vec<_> = lines.iter().filter(|line| line.get("report").is_some()).collect();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["elapsed_us"], 211);
        assert_eq!(rows[1]["auxiliary"], true);
        assert_eq!(rows[1]["epoch"], 7);
        assert_eq!(rows[1]["session"], 9);
        assert_eq!(lines[0]["capture_boundary"], "selected_session_read_completions");
        assert_eq!(lines[0]["hardware_or_transport_losses_measured"], false);
        assert_eq!(lines[0]["decoded_state_scope"], "capture_local");
        assert_eq!(lines[0]["initial_prior_session_state_available"], false);
        assert_eq!(lines.last().unwrap()["summary"]["complete_tap_stream"], true);
        assert_eq!(lines.last().unwrap()["summary"]["complete_hid_stream"], false);
    }
    #[test]
    fn capture_loss_and_session_end_remain_explicit_in_saved_summary() {
        for (lost, reason) in [(1, "requested"), (0, "session_ended")] {
            let (result, lines) = recording_fixture(reason, true, &mut |command| match command {
                Command::Status => Ok(daemon()),
                Command::DebugCaptureStart { .. } => Ok(Reply::DebugCaptureStarted { capture:status(true, 0, 0, "requested") }),
                Command::DebugCaptureStop { .. } => Ok(Reply::DebugCaptureStopped { capture:status(false, 0, lost, reason) }),
                Command::DebugCaptureRead { .. } => Ok(Reply::DebugCaptureRead { batch:DebugCaptureBatch {
                    capture:status(false, 0, lost, reason), packets:vec![packet(2)], next_sequence:2,
                } }),
                Command::DebugCaptureRelease { .. } => Ok(Reply::DebugCaptureReleased),
                _ => panic!("unexpected command"),
            });
            assert!(result.is_err());
            let summary = &lines.last().unwrap()["summary"];
            assert_eq!(summary["complete_hid_stream"], false);
            assert_eq!(summary["complete_tap_stream"], false);
            assert_eq!(summary["capture"]["lost_tap"], lost);
            assert!(summary["error"].as_str().is_some());
        }
    }
    #[test]
    fn known_loss_survives_a_failed_final_drain_in_ui_and_footer() {
        let mut frozen = false;
        let mut reads_after_stop = 0;
        let (result, lines) = recording_fixture("loss-then-disconnect", false, &mut |command| match command {
            Command::Status => Ok(daemon()),
            Command::DebugCaptureStart { .. } => Ok(Reply::DebugCaptureStarted { capture:status(true, 2, 0, "requested") }),
            Command::DebugCaptureRead { .. } if !frozen => Ok(Reply::DebugCaptureRead { batch:DebugCaptureBatch {
                capture:status(true, 1, 1, "requested"), packets:vec![packet(1)], next_sequence:1,
            } }),
            Command::DebugCaptureStop { .. } => {
                frozen = true;
                Ok(Reply::DebugCaptureStopped { capture:status(false, 0, 1, "requested") })
            }
            Command::DebugCaptureRead { .. } => {
                reads_after_stop += 1;
                Err("fixture transport disconnected during final drain".into())
            }
            Command::DebugCaptureRelease { .. } => Ok(Reply::DebugCaptureReleased),
            _ => panic!("unexpected command"),
        });
        assert_eq!(reads_after_stop, 3);
        let error = result.unwrap_err();
        assert!(error.contains("transport disconnected during final drain"));
        assert!(error.contains("lost tap 1, overflow 0, oversized 0"));
        let summary = &lines.last().unwrap()["summary"];
        assert_eq!(summary["error"], error);
        assert_eq!(summary["complete_tap_stream"], false);
        assert_eq!(summary["capture"]["lost_tap"], 1);
    }
}
