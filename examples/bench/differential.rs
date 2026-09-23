//! Differential fixture skeletons (F05). Each file holds report bytes, the
//! desktop, and cases that describe one profile twice: as this driver's TOML
//! and as the same settings in OpenTabletDriver's terms. The upstream harness
//! (`bench/upstream --reference`) fills each case's `expected` outputs by
//! running OpenTabletDriver's own pipeline; `crates/otd-core/src/differential.rs`
//! compares this driver against them.

use std::path::Path;

use serde_json::{Value, json};

use otd_core::config::Profile;
use otd_core::mapping::{OtdArea, OtdMapping, Rect};
use otd_core::relative::RelativeSettings;

use crate::trace::{REPORT_BYTES, Trace};
use crate::{
    OSU_PROFILE, RELATIVE_PROFILE, TIP_PERCENT, TRACE_NAME, TRACE_SEED, radial_follow_settings,
};

/// Activation thresholds around the usual 1 % and the ends of the range.
const THRESHOLD_PERCENTS: [f64; 16] = [
    0.0, 0.001, 0.5, 0.98, 0.99, 1.0, 1.001, 1.0011, 2.0, 5.0, 10.0, 33.3, 50.0, 99.0, 99.99, 100.0,
];

/// Reports are 5 ms apart for time-based filters; upstream's filters see
/// back-to-back reports, so neither side reaches a reset timeout.
const INTERVAL_MS: u64 = 5;

/// The first 17 bytes of a report: all that either parser reads.
fn hex(report: &[u8]) -> String {
    report[..17]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// A `0x10` pen report. `flags` is byte 1: Sense `0x40`, In Range `0x20`,
/// Invert (eraser) `0x10`, tip switch `0x01`.
fn pen(flags: u8, x: u32, y: u32, pressure: u16) -> [u8; REPORT_BYTES] {
    let mut report = [0u8; REPORT_BYTES];
    report[0] = 0x10;
    report[1] = flags;
    report[2..5].copy_from_slice(&x.to_le_bytes()[..3]);
    report[5..8].copy_from_slice(&y.to_le_bytes()[..3]);
    report[8..10].copy_from_slice(&pressure.to_le_bytes());
    report[16] = 12;
    report
}

/// Boundaries a random trace rarely hits: the tip threshold, full pressure,
/// the eraser, the tablet's corners, the area's exact edges, each hover flag
/// alone, and report IDs that carry no position.
fn edge_reports() -> Vec<[u8; REPORT_BYTES]> {
    const HOVER: u8 = 0x60;
    let center = (22_000, 4_781);
    let mut reports = vec![
        pen(HOVER, center.0, center.1, 0),
        pen(HOVER | 1, center.0 + 3, center.1, 81),
        pen(HOVER | 1, center.0 + 6, center.1, 82),
        pen(HOVER | 1, center.0 + 9, center.1, 83),
        pen(HOVER | 1, center.0 + 12, center.1, 81),
        pen(HOVER, center.0 + 15, center.1, 0),
        pen(HOVER | 1, center.0 + 18, center.1, 8_191),
        pen(HOVER, center.0 + 21, center.1, 0),
        pen(HOVER | 0x10, center.0, center.1 + 40, 0),
        pen(HOVER | 0x10 | 1, center.0, center.1 + 45, 100),
        pen(HOVER | 0x10, center.0, center.1 + 50, 0),
        pen(HOVER, center.0, center.1 + 55, 0),
        pen(HOVER, 0, 0, 0),
        pen(HOVER, 44_800, 29_600, 0),
        pen(HOVER, 30_500, 4_781, 0),
        pen(HOVER, 13_500, 0, 0),
        pen(HOVER, 22_000, 9_562, 0),
        pen(HOVER, 22_000, 9_563, 0),
        pen(0x40, 20_000, 3_000, 0),
        pen(0x20, 20_100, 3_100, 0),
    ];
    // A report ID without a position, and the auxiliary report ID.
    let mut unknown = [0u8; REPORT_BYTES];
    unknown[0] = 0x13;
    unknown[1] = 0x64;
    reports.push(unknown);
    let mut auxiliary = [0u8; REPORT_BYTES];
    auxiliary[0] = 0x11;
    reports.push(auxiliary);
    // Pressed while crossing the area's edge and back.
    for step in 0..12u32 {
        reports.push(pen(
            HOVER | 1,
            29_000 + step * 250,
            2_000 + step * 40,
            2_000,
        ));
    }
    reports.push(pen(HOVER, 30_000, 2_500, 0));
    reports
}

/// The same absolute settings in OpenTabletDriver's terms.
fn upstream_absolute(mapping: OtdMapping, radial_follow: Option<Value>) -> Value {
    let area = |a: OtdArea| json!({"width": a.width, "height": a.height, "x": a.x, "y": a.y, "rotation": a.rotation});
    json!({
        "display": area(mapping.display),
        "tablet": area(mapping.tablet),
        "clipping": mapping.clipping,
        "limiting": mapping.limiting,
        "tip_threshold_percent": TIP_PERCENT,
        "eraser_threshold_percent": TIP_PERCENT,
        "radial_follow": radial_follow,
    })
}

fn upstream_relative(settings: RelativeSettings) -> Value {
    json!({
        "sensitivity": [settings.sensitivity.0, settings.sensitivity.1],
        "rotation": settings.rotation,
        "reset_ms": settings.reset_delay.as_secs_f64() * 1e3,
        "tip_threshold_percent": TIP_PERCENT,
        "eraser_threshold_percent": TIP_PERCENT,
    })
}

fn rect(r: Rect) -> Value {
    json!([r.left, r.top, r.right, r.bottom])
}

fn fixture(description: &str, reports_from: &str, reports: &[&[u8]], cases: Vec<Value>) -> Value {
    let desktop = crate::desktop();
    json!({
        "schema": "otd-differential/1",
        "description": description,
        "origin": "upstream-generated",
        "provenance": {
            "reports_from": reports_from,
            "generator": "bench/upstream --reference",
        },
        "interval_ms": INTERVAL_MS,
        "desktop": {
            "virtual_screen": rect(desktop.virtual_screen),
            "monitors": desktop.monitors.iter().map(|&m| rect(m)).collect::<Vec<_>>(),
        },
        "reports": reports.iter().map(|report| hex(report)).collect::<Vec<_>>(),
        "cases": cases,
    })
}

/// The osu! profile without its Radial Follow entry.
fn without_radial_follow(profile: &str) -> String {
    let start = profile
        .find("[[radial_follow]]")
        .expect("the osu! profile has Radial Follow");
    profile[..start].trim_end().to_owned() + "\n"
}

/// `profile` with the first `old` after `section` replaced by `new`.
fn replaced(profile: &str, section: &str, old: &str, new: &str) -> String {
    let start = profile.find(section).expect("section") + section.len();
    let at = start + profile[start..].find(old).expect("setting");
    format!("{}{new}{}", &profile[..at], &profile[at + old.len()..])
}

fn parse(profile: &str) -> Result<Profile, String> {
    Profile::from_toml_text(profile, Path::new("differential profile"))
}

pub fn export(
    dir: &Path,
    trace: &Trace,
    osu: &Profile,
    relative: &Profile,
    reports: usize,
) -> Result<(), String> {
    let follow = osu
        .radial_follow
        .first()
        .ok_or("the osu! profile has no Radial Follow")?;
    relative
        .relative
        .ok_or("the relative profile has no relative settings")?;
    let osu_reports: Vec<&[u8]> = (0..reports.min(trace.len()))
        .map(|i| trace.report(i))
        .collect();
    let edges = edge_reports();
    let edge_refs: Vec<&[u8]> = edges.iter().map(|report| &report[..]).collect();
    let plain = without_radial_follow(OSU_PROFILE);
    // An arbitrary area rotation, area limiting, and unequal relative
    // sensitivities with a rotation.
    let rotated = replaced(
        &plain,
        "[absolute.tablet]",
        "Rotation = 0.0",
        "Rotation = 30.0",
    );
    let limited = replaced(&plain, "[absolute]", "limiting = false", "limiting = true");
    let skewed = replaced(
        &replaced(
            &replaced(
                RELATIVE_PROFILE,
                "[relative]",
                "x_sensitivity = 10.0",
                "x_sensitivity = 12.0",
            ),
            "[relative]",
            "y_sensitivity = 10.0",
            "y_sensitivity = 8.0",
        ),
        "[relative]",
        "rotation = 0.0",
        "rotation = 15.0",
    );
    let absolute = |name: &str, profile: &str, filter: Option<Value>| -> Result<Value, String> {
        let mapping = parse(profile)?.otd_mapping.ok_or("no absolute mapping")?;
        Ok(json!({
            "name": name,
            "mode": "absolute",
            "profile": profile,
            "upstream": upstream_absolute(mapping, filter),
            "expected": null,
        }))
    };
    let relative = |name: &str, profile: &str| -> Result<Value, String> {
        let settings = parse(profile)?.relative.ok_or("no relative settings")?;
        Ok(json!({
            "name": name,
            "mode": "relative",
            "profile": profile,
            "upstream": upstream_relative(settings),
            "expected": null,
        }))
    };
    let thresholds: Vec<Value> = THRESHOLD_PERCENTS
        .iter()
        .map(|percent| json!({"percent": percent, "first_pressing_raw": null}))
        .collect();
    let files = [
        (
            "osu-trace.json",
            fixture(
                "The osu! profile with and without Radial Follow, and relative mode, over synthetic osu!-style play.",
                &format!(
                    "examples/bench {TRACE_NAME}, seed {TRACE_SEED}, first {} reports",
                    osu_reports.len()
                ),
                &osu_reports,
                vec![
                    absolute(
                        "absolute-radial-follow",
                        OSU_PROFILE,
                        Some(radial_follow_settings(follow)),
                    )?,
                    absolute("absolute", &plain, None)?,
                    absolute("absolute-rotated", &rotated, None)?,
                    absolute("absolute-limiting", &limited, None)?,
                    relative("relative", RELATIVE_PROFILE)?,
                    relative("relative-skewed", &skewed)?,
                ],
            ),
        ),
        (
            "edges.json",
            fixture(
                "Tip and eraser thresholds, full pressure, tablet corners, area edges, single hover flags and reports without a position.",
                "handwritten in examples/bench/differential.rs",
                &edge_refs,
                vec![
                    absolute("absolute", &plain, None)?,
                    absolute("absolute-limiting", &limited, None)?,
                    relative("relative", RELATIVE_PROFILE)?,
                ],
            ),
        ),
        (
            "thresholds.json",
            json!({
                "schema": "otd-differential-thresholds/1",
                "description": "The first raw pressure that presses the tip for each OpenTabletDriver activation threshold.",
                "origin": "upstream-generated",
                "provenance": {"generator": "bench/upstream --reference"},
                "max_pressure": 8191,
                "thresholds": thresholds,
            }),
        ),
    ];
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    for (name, value) in files {
        let text = serde_json::to_string_pretty(&value).map_err(|e| e.to_string())?;
        std::fs::write(dir.join(name), text + "\n").map_err(|e| e.to_string())?;
    }
    Ok(())
}
