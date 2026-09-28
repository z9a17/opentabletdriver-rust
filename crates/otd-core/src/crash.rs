//! Crash records. The daemon and the panel run without a console, so a panic
//! or a fatal error would otherwise leave no trace. A panic hook and fatal
//! errors append one JSON line to `crash.log` in the data directory; the
//! panel reads the daemon's record back when the daemon stops unexpectedly.
//!
//! Release builds abort on panic. The hook runs before the abort, but code
//! that crashes without a Rust panic (an access violation in a native plugin,
//! a stack overflow) leaves only its process exit code.

use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub const FILE_NAME: &str = "crash.log";
/// The previous file after rotation; together they keep the latest records.
pub const OLD_FILE_NAME: &str = "crash.log.old";
/// A file larger than this is rotated before the next record is added.
pub const MAX_FILE_BYTES: u64 = 256 * 1024;
const MAX_MESSAGE_BYTES: usize = 4096;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CrashRecord {
    /// Seconds since the Unix epoch.
    pub time: u64,
    pub version: String,
    /// `daemon`, `panel` or `cli`.
    pub role: String,
    pub pid: u32,
    /// `panic` for a Rust panic, `error` for a fatal error that ended the
    /// process.
    pub kind: String,
    pub thread: Option<String>,
    pub message: String,
    /// Source file, line and column of a panic.
    pub location: Option<String>,
}

impl CrashRecord {
    pub fn new(version: &str, role: &str, kind: &str, message: impl Into<String>) -> Self {
        let mut message = message.into();
        truncate(&mut message, MAX_MESSAGE_BYTES);
        Self {
            time: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |elapsed| elapsed.as_secs()),
            version: version.to_owned(),
            role: role.to_owned(),
            pid: std::process::id(),
            kind: kind.to_owned(),
            thread: std::thread::current().name().map(str::to_owned),
            message,
            location: None,
        }
    }

    /// One line for a log or console: what stopped, where and why.
    pub fn summary(&self) -> String {
        let mut text = match self.kind.as_str() {
            "panic" => format!("{} {} panicked", self.role, self.version),
            _ => format!("{} {} stopped with an error", self.role, self.version),
        };
        if let Some(thread) = &self.thread {
            text.push_str(&format!(" on thread '{thread}'"));
        }
        if let Some(location) = &self.location {
            text.push_str(&format!(" at {location}"));
        }
        text.push_str(": ");
        text.push_str(&self.message);
        text
    }
}

/// Records every panic of this process before the default hook prints it.
pub fn install(version: &'static str, role: &'static str) {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let message = if let Some(text) = info.payload().downcast_ref::<&str>() {
            (*text).to_owned()
        } else if let Some(text) = info.payload().downcast_ref::<String>() {
            text.clone()
        } else {
            "non-string panic payload".to_owned()
        };
        let mut record = CrashRecord::new(version, role, "panic", message);
        record.location = info.location().map(|location| {
            format!(
                "{}:{}:{}",
                location.file(),
                location.line(),
                location.column()
            )
        });
        // Nothing can be done about a failed write while panicking.
        let _ = append(&record);
        previous(info);
    }));
}

/// Records a fatal error before the process exits with it.
pub fn record_error(version: &str, role: &str, message: &str) {
    let _ = append(&CrashRecord::new(version, role, "error", message));
}

/// `crash.log` in the data directory.
pub fn path() -> Result<PathBuf, String> {
    crate::storage::data_directory().map(|directory| directory.join(FILE_NAME))
}

/// Appends one record, rotating a full file first.
pub fn append(record: &CrashRecord) -> Result<PathBuf, String> {
    let path = path()?;
    append_to(&path, record).map_err(|error| format!("{}: {error}", path.display()))?;
    Ok(path)
}

fn append_to(path: &Path, record: &CrashRecord) -> io::Result<()> {
    if let Some(directory) = path.parent() {
        fs::create_dir_all(directory)?;
    }
    if fs::metadata(path).is_ok_and(|metadata| metadata.len() > MAX_FILE_BYTES) {
        let _ = fs::rename(path, path.with_file_name(OLD_FILE_NAME));
    }
    let mut line = serde_json::to_vec(record).map_err(io::Error::other)?;
    line.push(b'\n');
    // One write per record keeps concurrent writers' lines whole.
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?
        .write_all(&line)
}

/// The newest record written by process `pid` at or after `since` (seconds
/// since the Unix epoch). Unreadable lines are skipped.
pub fn latest_for(pid: u32, since: u64) -> Option<CrashRecord> {
    let path = path().ok()?;
    latest_in(&path, pid, since)
}

fn latest_in(path: &Path, pid: u32, since: u64) -> Option<CrashRecord> {
    // The current file holds the newest records; the rotated one is older.
    [path.to_path_buf(), path.with_file_name(OLD_FILE_NAME)]
        .iter()
        .find_map(|file| {
            let text = read_bounded(file)?;
            text.lines()
                .rev()
                .filter_map(|line| serde_json::from_str::<CrashRecord>(line).ok())
                .find(|record| record.pid == pid && record.time >= since)
        })
}

/// Up to `limit` of the newest records of every process, newest first.
pub fn recent(limit: usize) -> Vec<CrashRecord> {
    path().map_or_else(|_| Vec::new(), |path| recent_in(&path, limit))
}

fn recent_in(path: &Path, limit: usize) -> Vec<CrashRecord> {
    [path.to_path_buf(), path.with_file_name(OLD_FILE_NAME)]
        .iter()
        .filter_map(|file| read_bounded(file))
        .flat_map(|text| {
            text.lines()
                .rev()
                .filter_map(|line| serde_json::from_str::<CrashRecord>(line).ok())
                .collect::<Vec<_>>()
        })
        .take(limit)
        .collect()
}

fn read_bounded(path: &Path) -> Option<String> {
    use std::io::Read;
    let mut bytes = Vec::new();
    fs::File::open(path)
        .ok()?
        .take(MAX_FILE_BYTES * 2)
        .read_to_end(&mut bytes)
        .ok()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

fn truncate(text: &mut String, maximum: usize) {
    if text.len() > maximum {
        let mut end = maximum;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn directory(name: &str) -> PathBuf {
        let directory = std::env::temp_dir().join(format!(
            "otd-crash-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&directory);
        directory
    }

    #[test]
    fn records_round_trip_and_select_the_newest_for_a_process() {
        let directory = directory("round-trip");
        let path = directory.join(FILE_NAME);
        let mut first = CrashRecord::new("1.0", "daemon", "panic", "first");
        first.pid = 7;
        first.time = 100;
        first.location = Some("src/a.rs:1:2".into());
        let mut second = first.clone();
        second.message = "second".into();
        second.time = 200;
        let mut other = first.clone();
        other.pid = 8;
        other.time = 300;
        for record in [&first, &second, &other] {
            append_to(&path, record).unwrap();
        }
        fs::write(
            &path,
            format!("{}not json\n", fs::read_to_string(&path).unwrap()),
        )
        .unwrap();
        assert_eq!(latest_in(&path, 7, 0), Some(second.clone()));
        assert_eq!(latest_in(&path, 7, 150).unwrap().message, "second");
        assert_eq!(latest_in(&path, 7, 250), None);
        assert_eq!(latest_in(&path, 8, 0), Some(other));
        assert_eq!(latest_in(&path, 9, 0), None);
        assert_eq!(
            second.summary(),
            format!(
                "daemon 1.0 panicked on thread '{}' at src/a.rs:1:2: second",
                second.thread.clone().unwrap()
            )
        );
        fs::remove_dir_all(&directory).unwrap();
    }

    #[test]
    fn a_full_file_rotates_and_both_files_are_searched() {
        let directory = directory("rotate");
        let path = directory.join(FILE_NAME);
        let mut old = CrashRecord::new("1.0", "panel", "error", "old");
        old.pid = 1;
        append_to(&path, &old).unwrap();
        // Pad the file past the limit; the next record starts a new file.
        let padding = "x".repeat(MAX_FILE_BYTES as usize + 1);
        fs::write(
            &path,
            format!("{}{padding}\n", fs::read_to_string(&path).unwrap()),
        )
        .unwrap();
        let mut new = CrashRecord::new("1.0", "panel", "error", "new");
        new.pid = 2;
        append_to(&path, &new).unwrap();
        assert!(fs::metadata(&path).unwrap().len() < 4096);
        assert!(directory.join(OLD_FILE_NAME).exists());
        assert_eq!(latest_in(&path, 2, 0).unwrap().message, "new");
        assert_eq!(latest_in(&path, 1, 0).unwrap().message, "old");
        let recent: Vec<_> = recent_in(&path, 5)
            .into_iter()
            .map(|record| record.message)
            .collect();
        assert_eq!(recent, ["new", "old"]);
        assert_eq!(recent_in(&path, 1).len(), 1);
        fs::remove_dir_all(&directory).unwrap();
    }

    #[test]
    fn long_messages_are_truncated_on_a_character_boundary() {
        let record = CrashRecord::new("1.0", "cli", "error", "é".repeat(MAX_MESSAGE_BYTES));
        assert!(record.message.len() <= MAX_MESSAGE_BYTES);
        assert!(record.message.chars().all(|c| c == 'é'));
    }
}
