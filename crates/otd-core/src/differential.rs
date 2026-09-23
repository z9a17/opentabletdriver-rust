//! Differential tests (F05): this driver against OpenTabletDriver 0.6.7 on
//! the fixtures in the repository's `tests/differential`. Their expected
//! outputs come from upstream's own pipeline (`bench/upstream --reference`);
//! `tests/differential/README.md` records where they come from and how to
//! regenerate them.
//!
//! Positions are compared in desktop pixels, button presses and releases
//! exactly, relative motion by its running sum. The tolerances below are the
//! documented differences, not slack.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::config::{Profile, activation_raw};
use crate::display::DisplaySnapshot;
use crate::mapping::Rect;
use crate::output::flags;
use crate::pipeline::ReportPipeline;
use crate::plugins::NoFilters;
use crate::protocol;

/// Rounding to `SendInput`'s 0..65535 coordinates moves a position by up to
/// half a unit, 0.032 px on the fixtures' 4160 px desktop. This driver's f64
/// arithmetic against upstream's float (BC-13) adds under 0.001 px.
const POSITION_TOLERANCE_PX: f64 = 0.05;
/// Upstream clamps a clipped position to the display area's far edge (2560);
/// this driver clamps to the last pixel inside it (2559), BC-11.
const EDGE_TOLERANCE_PX: f64 = 1.0 + POSITION_TOLERANCE_PX;
/// Area limiting decides inside or outside at the display area's edges. For a
/// pen exactly on an edge, upstream's single-precision transform can land a
/// few hundred-thousandths of a pixel outside while this driver's f64 lands
/// on the edge (BC-13), so reports this close to an edge may be dropped by
/// only one.
const LIMIT_EDGE_PX: f64 = 0.001;
/// Both drivers truncate each relative delta and carry the fraction to the
/// next (BC-18), so each one's running sum of motion is within one count of
/// the exact motion, and can come arbitrarily close to one count. Upstream's
/// single-precision arithmetic and the fixture's five decimals add less
/// than 0.0001 counts over the whole trace.
const RELATIVE_TOLERANCE: f64 = 1.0 + 1e-3;

fn directory() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/differential")
}

fn load(name: &str) -> Value {
    let path = directory().join(name);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// Upstream's output for one report: a position (desktop pixels, or a
/// relative delta) and its button presses (`D`) and releases (`U`).
struct Expected {
    position: Option<(f64, f64)>,
    events: String,
}

fn expected(text: &str) -> Expected {
    let mut parts = text.split(',');
    let x = parts.next().unwrap_or("");
    let y = parts.next().unwrap_or("");
    Expected {
        position: (!x.is_empty()).then(|| (x.parse().unwrap(), y.parse().unwrap())),
        events: parts.next().unwrap_or("").to_owned(),
    }
}

/// A report as the pen collection delivers it: 192 bytes, zero after the
/// fixture's prefix.
fn report(hex: &str) -> [u8; 192] {
    let mut bytes = [0; 192];
    for (byte, digits) in bytes.iter_mut().zip(hex.split_whitespace()) {
        *byte = u8::from_str_radix(digits, 16).unwrap();
    }
    bytes
}

fn rect(value: &Value) -> Rect {
    let n = |i: usize| value[i].as_i64().unwrap() as i32;
    Rect {
        left: n(0),
        top: n(1),
        right: n(2),
        bottom: n(3),
    }
}

/// Runs one fixture case through this driver's pipeline and returns every
/// report where it differs from upstream.
fn compare(fixture: &Value, case: &Value) -> Vec<String> {
    let name = case["name"].as_str().unwrap();
    let profile =
        Profile::from_toml_text(case["profile"].as_str().unwrap(), Path::new(name)).unwrap();
    let desktop = &fixture["desktop"];
    let snapshot = DisplaySnapshot {
        virtual_screen: rect(&desktop["virtual_screen"]),
        monitors: desktop["monitors"]
            .as_array()
            .unwrap()
            .iter()
            .map(rect)
            .collect(),
    };
    let relative = case["mode"] == "relative";
    let mapper = (!relative).then(|| snapshot.mapper(&profile).unwrap());
    let screen = snapshot.virtual_screen;
    let unit = |span: i32| f64::from(span - 1) / 65_535.0;
    // Upstream's clamp boundary: the display area's right and bottom edges.
    let area = profile.otd_mapping.map(|m| {
        (
            m.display.x + m.display.width / 2.0,
            m.display.y + m.display.height / 2.0,
        )
    });
    let limiting = profile.otd_mapping.is_some_and(|m| m.limiting);
    let on_edge = |x: f64, y: f64| {
        profile.otd_mapping.is_some_and(|m| {
            let (half_w, half_h) = (m.display.width / 2.0, m.display.height / 2.0);
            [
                m.display.x - half_w - x,
                x - m.display.x - half_w,
                m.display.y - half_h - y,
                y - m.display.y - half_h,
            ]
            .iter()
            .any(|distance| distance.abs() <= LIMIT_EDGE_PX)
        })
    };
    let interval = Duration::from_millis(fixture["interval_ms"].as_u64().unwrap());
    let reports = fixture["reports"].as_array().unwrap();
    let expectations = case["expected"]
        .as_array()
        .expect("run bench/upstream --reference first");
    assert_eq!(
        reports.len(),
        expectations.len(),
        "{name}: one expectation per report"
    );

    let mut pipeline = ReportPipeline::new(&profile).unwrap();
    let start = Instant::now();
    let mut cursor = None;
    let (mut ours, mut theirs) = ((0i64, 0i64), (0f64, 0f64));
    let mut failures = Vec::new();
    for (index, (hex, text)) in reports.iter().zip(expectations).enumerate() {
        let bytes = report(hex.as_str().unwrap());
        let want = expected(text.as_str().unwrap());
        let mut events = String::new();
        let mut moved = false;
        if let Ok(Some(pen)) = protocol::parse(&bytes) {
            let now = start + interval * index as u32;
            pipeline
                .process(pen, now, mapper, &mut NoFilters, |packet| {
                    if packet.flags & flags::MOVE != 0 {
                        moved = true;
                        if relative {
                            ours.0 += i64::from(packet.dx);
                            ours.1 += i64::from(packet.dy);
                        } else {
                            cursor = Some((packet.dx, packet.dy));
                        }
                    }
                    if packet.flags & flags::LEFTDOWN != 0 {
                        events.push('D');
                    }
                    if packet.flags & flags::LEFTUP != 0 {
                        events.push('U');
                    }
                    Ok(())
                })
                .unwrap();
        }
        let mut fail = |what: String| {
            failures.push(format!(
                "{name} report {index} ({}): {what}",
                hex.as_str().unwrap()
            ))
        };
        if events != want.events {
            fail(format!("buttons {events:?}, upstream {:?}", want.events));
        }
        match (relative, want.position) {
            (true, position) => {
                if let Some((dx, dy)) = position {
                    theirs.0 += dx;
                    theirs.1 += dy;
                }
                let drift = (
                    (ours.0 as f64 - theirs.0).abs(),
                    (ours.1 as f64 - theirs.1).abs(),
                );
                if drift.0 >= RELATIVE_TOLERANCE || drift.1 >= RELATIVE_TOLERANCE {
                    fail(format!(
                        "motion sum {ours:?}, upstream ({:.3}, {:.3})",
                        theirs.0, theirs.1
                    ));
                }
            }
            (false, None) => {
                if moved {
                    let (nx, ny) = cursor.unwrap();
                    let px = f64::from(screen.left) + f64::from(nx) * unit(screen.width());
                    let py = f64::from(screen.top) + f64::from(ny) * unit(screen.height());
                    // Within rounding of the edge, see LIMIT_EDGE_PX.
                    let at_edge = limiting && on_edge(px.round(), py.round());
                    if !at_edge {
                        fail("moved the cursor where upstream output nothing".into());
                    }
                }
            }
            (false, Some((x, y))) => {
                let Some((nx, ny)) = cursor else {
                    fail(format!("no position, upstream ({x:.3}, {y:.3})"));
                    continue;
                };
                if limiting && !moved && on_edge(x, y) {
                    continue;
                }
                let px = f64::from(screen.left) + f64::from(nx) * unit(screen.width());
                let py = f64::from(screen.top) + f64::from(ny) * unit(screen.height());
                let (edge_x, edge_y) = area.unwrap();
                let tolerance = |at_edge: bool| {
                    if at_edge {
                        EDGE_TOLERANCE_PX
                    } else {
                        POSITION_TOLERANCE_PX
                    }
                };
                if (px - x).abs() > tolerance(x >= edge_x - 1.0)
                    || (py - y).abs() > tolerance(y >= edge_y - 1.0)
                {
                    fail(format!(
                        "position ({px:.3}, {py:.3}), upstream ({x:.3}, {y:.3})"
                    ));
                }
            }
        }
    }
    failures
}

fn check(name: &str) {
    let fixture = load(name);
    let failures: Vec<String> = fixture["cases"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|case| compare(&fixture, case))
        .collect();
    assert!(
        failures.is_empty(),
        "{} differences from OpenTabletDriver, first ones:\n{}",
        failures.len(),
        failures
            .iter()
            .take(12)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn osu_play_matches_opentabletdriver() {
    check("osu-trace.json");
}

#[test]
fn edge_cases_match_opentabletdriver() {
    check("edges.json");
}

#[test]
fn tip_thresholds_press_at_the_same_raw_pressure() {
    let fixture = load("thresholds.json");
    for entry in fixture["thresholds"].as_array().unwrap() {
        let percent = entry["percent"].as_f64().unwrap();
        let upstream = entry["first_pressing_raw"]
            .as_i64()
            .expect("run bench/upstream --reference first");
        assert_eq!(
            i64::from(activation_raw(percent).unwrap()),
            upstream,
            "threshold {percent} %"
        );
    }
}

/// The comparison itself must notice a changed mapping or transition.
#[test]
fn a_changed_position_or_transition_fails() {
    let fixture = load("edges.json");
    let case = &fixture["cases"][0];
    assert!(compare(&fixture, case).is_empty());
    let expectations = case["expected"].as_array().unwrap();
    let index = expectations
        .iter()
        .position(|e| e.as_str().unwrap().ends_with(",D"))
        .unwrap();

    // A press moved to the next report.
    let mut changed = fixture.clone();
    let press = changed["cases"][0]["expected"][index]
        .as_str()
        .unwrap()
        .trim_end_matches(",D")
        .to_owned();
    changed["cases"][0]["expected"][index] = press.into();
    let next = format!(
        "{},D",
        changed["cases"][0]["expected"][index + 1].as_str().unwrap()
    );
    changed["cases"][0]["expected"][index + 1] = next.into();
    assert_eq!(compare(&changed, &changed["cases"][0]).len(), 2);

    // A position a tenth of a pixel off.
    let mut changed = fixture.clone();
    let text = changed["cases"][0]["expected"][0]
        .as_str()
        .unwrap()
        .to_owned();
    let (x, rest) = text.split_once(',').unwrap();
    let shifted = format!("{:.4},{rest}", x.parse::<f64>().unwrap() + 0.1);
    changed["cases"][0]["expected"][0] = shifted.into();
    assert_eq!(compare(&changed, &changed["cases"][0]).len(), 1);
}
