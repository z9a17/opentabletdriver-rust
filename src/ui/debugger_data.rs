//! Statistics and bounded background recording for the debugger's sampled IPC.
//! Sequence gaps are observable; this never claims to capture every HID report.
use crate::control::DebugReport;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::io::{BufWriter, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, mpsc};

#[derive(Default)]
pub(super) struct Statistics {
    pub samples: u64,
    pub skipped: u64,
    previous: Option<(Option<String>, u64)>,
    pub ranges: BTreeMap<String, (f64, f64)>,
}

impl Statistics {
    /// Duplicate polls must neither inflate statistics nor create recordings.
    pub fn observe(&mut self, report: &DebugReport) -> bool {
        if report.raw_hex.is_empty() { return false; }
        if let Some((tablet, sequence)) = &self.previous {
            if tablet == &report.tablet && *sequence == report.sequence { return false; }
            if tablet == &report.tablet && report.sequence > *sequence {
                self.skipped = self.skipped.saturating_add(report.sequence - *sequence - 1);
            }
        }
        self.previous = Some((report.tablet.clone(), report.sequence));
        self.samples = self.samples.saturating_add(1);
        self.collect("", report.values.get("values").unwrap_or(&report.values), 0);
        true
    }

    fn collect(&mut self, path: &str, value: &Value, depth: usize) {
        if depth > 4 { return; }
        if let Some(number) = value.as_f64().or_else(|| value.as_bool().map(|v| u8::from(v) as f64)) {
            if !number.is_finite() { return; }
            if self.ranges.len() < 128 || self.ranges.contains_key(path) {
                let range = self.ranges.entry(path.into()).or_insert((number, number));
                range.0 = range.0.min(number);
                range.1 = range.1.max(number);
            }
        } else if let Some(fields) = value.as_object() {
            for (key, value) in fields { self.collect(&format!("{path}{key}."), value, depth + 1); }
        } else if let Some(values) = value.as_array() {
            for (index, value) in values.iter().take(64).enumerate() {
                self.collect(&format!("{path}{index}"), value, depth + 1);
            }
        }
    }

    pub fn lines(&self) -> Vec<String> {
        let mut lines = vec![format!("Samples: {}   Sequence gaps: {}", self.samples, self.skipped)];
        lines.extend(self.ranges.iter().map(|(name, (min, max))|
            format!("{}: {min:.2} .. {max:.2}", name.trim_end_matches('.'))));
        lines
    }
}

pub(super) struct Recorder {
    sender: Option<mpsc::SyncSender<(u64, DebugReport)>>,
    completed: mpsc::Receiver<Result<(), String>>,
    written: Arc<AtomicU64>,
    dropped: Arc<AtomicU64>,
}

/// Both modes share the debugger's completion/close path. Only the sampled
/// mode accepts GUI updates; full capture owns its independent daemon reader.
pub(super) enum Recording {
    Sampled(Recorder),
    Capture(super::debugger_capture::Recorder),
}

impl Recording {
    pub fn start(path: &Path, full_rate: bool) -> Result<Self, String> {
        if full_rate { super::debugger_capture::Recorder::start(path).map(Self::Capture) }
        else { Recorder::start(path).map(Self::Sampled) }
    }
    pub fn push(&self, elapsed_us: u64, report: DebugReport) {
        if let Self::Sampled(recorder) = self { recorder.push(elapsed_us, report); }
    }
    pub fn stop(&mut self) {
        match self { Self::Sampled(recorder) => recorder.stop(), Self::Capture(recorder) => recorder.stop() }
    }
    pub fn active(&self) -> bool {
        match self { Self::Sampled(recorder) => recorder.active(), Self::Capture(recorder) => recorder.active() }
    }
    pub fn status(&self) -> String {
        match self { Self::Sampled(recorder) => recorder.status(), Self::Capture(recorder) => recorder.status() }
    }
    pub fn result(&self) -> Option<Result<(), String>> {
        match self { Self::Sampled(recorder) => recorder.result(), Self::Capture(recorder) => recorder.result() }
    }
}

impl Recorder {
    pub fn start(path: &Path) -> Result<Self, String> {
        // Never silently replace an existing capture, like profile Save As.
        let file = std::fs::OpenOptions::new().write(true).create_new(true).open(path)
            .map_err(|error| format!("Cannot create recording {}: {error}", path.display()))?;
        let (sender, receive) = mpsc::sync_channel::<(u64, DebugReport)>(256);
        let (complete, completed) = mpsc::channel();
        let written = Arc::new(AtomicU64::new(0));
        let dropped = Arc::new(AtomicU64::new(0));
        let count = Arc::clone(&written);
        let losses = Arc::clone(&dropped);
        let worker = std::thread::Builder::new().name("debugger-recording".into()).spawn(move || {
            let result = (|| -> std::io::Result<()> {
                let mut writer = BufWriter::new(file);
                writeln!(writer, "{}", json!({"format":"otd-rust-sampled-reports", "version":1,
                    "poll_ms":33, "complete_hid_stream":false, "tap_losses_measured":false}))?;
                let mut statistics = Statistics::default();
                for (elapsed_us, report) in receive {
                    if !statistics.observe(&report) { continue; }
                    writeln!(writer, "{}", json!({"elapsed_us":elapsed_us, "report":report}))?;
                    count.fetch_add(1, Ordering::Relaxed);
                }
                writeln!(writer, "{}", json!({"summary":{"written":count.load(Ordering::Relaxed),
                    "queue_dropped":losses.load(Ordering::Relaxed), "sequence_gaps":statistics.skipped,
                    "statistics":statistics.ranges}}))?;
                writer.flush()
            })().map_err(|error| format!("Recording failed: {error}"));
            let _ = complete.send(result);
        });
        if let Err(error) = worker {
            let _ = std::fs::remove_file(path);
            return Err(error.to_string());
        }
        Ok(Self { sender: Some(sender), completed, written, dropped })
    }

    pub fn push(&self, elapsed_us: u64, report: DebugReport) {
        if let Some(sender) = &self.sender {
            match sender.try_send((elapsed_us, report)) {
                Ok(()) => {},
                Err(mpsc::TrySendError::Full(_)) => { self.dropped.fetch_add(1, Ordering::Relaxed); },
                Err(mpsc::TrySendError::Disconnected(_)) => {},
            }
        }
    }

    pub fn stop(&mut self) { self.sender.take(); }
    pub fn active(&self) -> bool { self.sender.is_some() }
    pub fn status(&self) -> String {
        format!("{}: {} saved, {} queue drops", if self.active() { "Recording samples" } else { "Finishing" },
            self.written.load(Ordering::Relaxed), self.dropped.load(Ordering::Relaxed))
    }
    pub fn result(&self) -> Option<Result<(), String>> {
        match self.completed.try_recv() {
            Ok(result) => Some(result),
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => Some(Err("Recording worker exited without a completion result.".into())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn report(sequence: u64) -> DebugReport {
        DebugReport { tablet: Some("fixture".into()), parser: None, sequence, raw_hex: "1001".into(),
            values: json!({"kind":"data", "values":{"position":[sequence, -2], "pressure":sequence}}) }
    }
    #[test]
    fn samples_distinguish_duplicate_polls_gaps_and_restart() {
        let mut stats = Statistics::default();
        assert!(stats.observe(&report(10)));
        assert!(!stats.observe(&report(10)));
        assert!(stats.observe(&report(15)));
        assert!(stats.observe(&report(1)));
        assert_eq!((stats.samples, stats.skipped), (3, 4));
        assert_eq!(stats.ranges["pressure."], (1.0, 15.0));
        assert_eq!(stats.ranges["position.1"], (-2.0, -2.0));
    }
    #[test]
    fn recording_drains_on_stop_and_refuses_overwrite() {
        let directory = std::path::PathBuf::from("E:/AgentWork/tmp")
            .join(format!("otd-recording-fixture-{}-{}", std::process::id(),
                std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("sample.jsonl");
        let mut recorder = Recorder::start(&path).unwrap();
        recorder.push(123, report(7));
        // A UI statistics reset may offer the same last packet again.
        recorder.push(456, report(7));
        recorder.stop();
        recorder.completed.recv_timeout(std::time::Duration::from_secs(3)).unwrap().unwrap();
        let lines: Vec<Value> = std::fs::read_to_string(&path).unwrap().lines()
            .map(|line| serde_json::from_str(line).unwrap()).collect();
        assert_eq!(lines[0]["complete_hid_stream"], false);
        assert_eq!(lines[1]["report"]["sequence"], 7);
        assert_eq!(lines[1]["elapsed_us"], 123);
        assert_eq!(lines[2]["summary"]["written"], 1);
        assert!(Recorder::start(&path).is_err());
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(directory).unwrap();
    }
}
