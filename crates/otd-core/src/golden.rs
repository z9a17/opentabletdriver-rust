//! Golden traces: recorded report sequences replayed through the decoder and
//! `ReportPipeline`, compared with the output they produced when frozen.
//! Fixtures live in the repository's `tests/golden/*.toml`, shared by the
//! workspace; `docs/parity/BEHAVIOR_CONTRACTS.md`
//! describes the format, the behavior each trace pins down and how it differs
//! from upstream. A mismatch prints the whole actual trace in fixture syntax.
//! Update a fixture only for an intentional, documented behavior change.

use std::fmt::Write as _;
use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use serde::Deserialize;

use crate::config::Profile;
use crate::display::DisplaySnapshot;
use crate::mapping::Rect;
use crate::output::MousePacket;
use crate::output::flags::{
    ABSOLUTE as MOUSEEVENTF_ABSOLUTE, LEFTDOWN as MOUSEEVENTF_LEFTDOWN,
    LEFTUP as MOUSEEVENTF_LEFTUP, MOVE as MOUSEEVENTF_MOVE, VIRTUALDESK as MOUSEEVENTF_VIRTUALDESK,
};
use crate::pipeline::ReportPipeline;
use crate::plugins::NoFilters;
use crate::protocol;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Fixture {
    /// What the trace pins down.
    #[allow(dead_code)]
    description: String,
    /// `captured`, `synthetic`, or a mix, per report.
    #[allow(dead_code)]
    origin: String,
    #[serde(default = "default_screen")]
    virtual_screen: [i32; 4],
    /// Monitor rectangles for `monitor = N` profiles; the virtual screen alone
    /// when omitted.
    #[serde(default)]
    monitors: Vec<[i32; 4]>,
    /// A Rust driver profile in TOML.
    profile: String,
    /// Output when the session ends.
    #[serde(default = "no_output")]
    end: String,
    step: Vec<Step>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Step {
    /// Processing time relative to the start of the trace. The pipeline is
    /// created at 0, so a first report at 100 ms arrives well after the
    /// filters' 50 ms redetection window, as it would on a real device.
    at_ms: f64,
    /// Report bytes in hex; spaces are ignored.
    report: String,
    expect: String,
    #[serde(default)]
    #[allow(dead_code)]
    note: String,
}

fn default_screen() -> [i32; 4] {
    [0, 0, 1920, 1080]
}

fn no_output() -> String {
    "none".into()
}

fn rect([left, top, right, bottom]: [i32; 4]) -> Rect {
    Rect {
        left,
        top,
        right,
        bottom,
    }
}

fn bytes(hex: &str) -> Result<Vec<u8>, String> {
    let digits: String = hex.chars().filter(|c| !c.is_whitespace()).collect();
    if !digits.len().is_multiple_of(2) {
        return Err(format!("odd number of hex digits in {hex:?}"));
    }
    (0..digits.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&digits[i..i + 2], 16).map_err(|e| format!("{hex:?}: {e}")))
        .collect()
}

/// `move X Y` (absolute, normalized), `delta DX DY` (relative), then `down`
/// or `up` for a left-button change; `none` when nothing was sent.
fn describe(packet: Option<MousePacket>) -> String {
    let Some(packet) = packet else {
        return "none".into();
    };
    let mut words = Vec::new();
    let mut flags = packet.flags;
    let absolute = MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK;
    if flags & absolute == absolute {
        words.push(format!("move {} {}", packet.dx, packet.dy));
        flags &= !absolute;
    } else if flags & MOUSEEVENTF_MOVE != 0 {
        words.push(format!("delta {} {}", packet.dx, packet.dy));
        flags &= !MOUSEEVENTF_MOVE;
    }
    for (flag, word) in [(MOUSEEVENTF_LEFTDOWN, "down"), (MOUSEEVENTF_LEFTUP, "up")] {
        if flags & flag != 0 {
            words.push(word.into());
            flags &= !flag;
        }
    }
    if flags != 0 {
        words.push(format!("flags 0x{flags:x}"));
    }
    words.join(" ")
}

fn replay(path: &Path) -> Result<(), String> {
    let text = fs::read_to_string(path).map_err(|e| e.to_string())?;
    let fixture: Fixture = toml::from_str(&text).map_err(|e| e.to_string())?;
    let profile = Profile::from_toml_text(&fixture.profile, path)?;
    if !profile.plugins.is_empty() {
        return Err("golden traces cannot load plugin DLLs".into());
    }
    let screen = rect(fixture.virtual_screen);
    let snapshot = DisplaySnapshot {
        virtual_screen: screen,
        monitors: if fixture.monitors.is_empty() {
            vec![screen]
        } else {
            fixture.monitors.iter().copied().map(rect).collect()
        },
    };
    let mapper = match profile.relative {
        Some(_) => None,
        None => Some(snapshot.mapper(&profile)?),
    };
    let mut pipeline = ReportPipeline::new(&profile)?;
    let mut plugins = NoFilters;
    let start = Instant::now();
    let mut actual = Vec::new();
    for step in &fixture.step {
        let report = bytes(&step.report)?;
        let at = start + Duration::from_micros((step.at_ms * 1000.0).round() as u64);
        actual.push(match protocol::parse(&report) {
            Ok(None) => "ignored".to_owned(),
            Err(_) => "malformed".to_owned(),
            Ok(Some(pen)) => {
                let mut sent = None;
                pipeline
                    .process(pen, at, mapper, &mut plugins, |packet| {
                        sent = Some(packet);
                        Ok(())
                    })
                    .map_err(|e| e.to_string())?;
                describe(sent)
            }
        });
    }
    let mut released = None;
    pipeline
        .release_all(|packet| {
            released = Some(packet);
            Ok(())
        })
        .map_err(|e| e.to_string())?;
    let end = describe(released);

    let expected = fixture.step.iter().map(|step| step.expect.as_str());
    if expected.clone().eq(actual.iter().map(String::as_str)) && end == fixture.end {
        return Ok(());
    }
    let mut report = format!("{} does not match; actual trace:\n", path.display());
    for (index, (step, actual)) in fixture.step.iter().zip(&actual).enumerate() {
        let mark = if *actual == step.expect { " " } else { "!" };
        let _ = writeln!(
            report,
            "{mark} step {} at {} ms: expect = {:?} (fixture has {:?})",
            index + 1,
            step.at_ms,
            actual,
            step.expect
        );
    }
    let mark = if end == fixture.end { " " } else { "!" };
    let _ = writeln!(
        report,
        "{mark} end = {end:?} (fixture has {:?})",
        fixture.end
    );
    Err(report)
}

#[test]
fn golden_traces_match() {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/golden");
    let mut paths: Vec<_> = fs::read_dir(&directory)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|e| e == "toml"))
        .collect();
    paths.sort();
    assert!(
        paths.len() >= 8,
        "expected the golden fixtures in {directory:?}"
    );
    let failures: Vec<String> = paths.iter().filter_map(|path| replay(path).err()).collect();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn describe_names_each_packet_part() {
    let absolute = MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK;
    let packet = |dx, dy, flags| Some(MousePacket { dx, dy, flags });
    assert_eq!(describe(None), "none");
    assert_eq!(
        describe(packet(10, 20, absolute | MOUSEEVENTF_LEFTDOWN)),
        "move 10 20 down"
    );
    assert_eq!(describe(packet(-3, 4, MOUSEEVENTF_MOVE)), "delta -3 4");
    assert_eq!(describe(packet(0, 0, MOUSEEVENTF_LEFTUP)), "up");
    assert_eq!(bytes("10 6 0").unwrap(), [0x10, 0x60]);
    assert!(bytes("1").is_err());
}
