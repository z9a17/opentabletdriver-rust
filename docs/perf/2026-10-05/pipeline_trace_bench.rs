// Offline pipeline benchmark for docs/PERFORMANCE_AUDIT_2026-10-05.md. Copy to
// crates/otd-core/tests/ and run:
//   cargo test --release -p otd-core --test pipeline_trace_bench -- --ignored --nocapture
use std::hint::black_box;
use std::path::Path;
use std::time::{Duration, Instant};
use otd_core::config::Profile;
use otd_core::display::DisplaySnapshot;
use otd_core::mapping::Rect;
use otd_core::pipeline::ReportPipeline;
use otd_core::protocol;

const ABS: &str = r#"
[absolute]
clipping = true
limiting = false
[absolute.display]
Width = 2560.0
Height = 1440.0
X = 1280.0
Y = 720.0
Rotation = 0.0
[absolute.tablet]
Width = 85.0
Height = 47.8125
X = 110.0
Y = 23.90625
Rotation = 0.0
[bindings]
tip_enabled = true
eraser_enabled = true
tip_threshold_raw = 82
eraser_threshold_raw = 82
"#;
const RF: &str = r#"
[[radial_follow]]
outer_radius = 0.7039
inner_radius = 0.302
smoothing_coefficient = 0.302
soft_knee_scale = 0.603
smoothing_leak_coefficient = 0.201
"#;
const REL: &str = r#"
[relative]
x_sensitivity = 10.0
y_sensitivity = 10.0
rotation = 0.0
reset_delay_ms = 100
"#;

fn trace(n: usize) -> Vec<[u8; 17]> {
    let base = [0x10u8, 0x61, 0x11, 0x55, 0x00, 0x8c, 0x12, 0x00, 0x0e, 0x11, 0x00, 0x07, 0, 0, 0, 0, 0x19];
    (0..n).map(|i| {
        let t = i as f64 * 0.002;
        // Small circular motion with jitter, inside the area, pen in contact half the time.
        let x = (22000.0 + 3000.0 * (t * 2.0).cos() + ((i * 7919) % 13) as f64) as u32;
        let y = (4800.0 + 1500.0 * (t * 3.0).sin() + ((i * 104729) % 11) as f64) as u32;
        let mut r = base;
        r[1] = if (i / 200) % 2 == 0 { 0x61 } else { 0x60 };
        r[2..4].copy_from_slice(&(x as u16).to_le_bytes()); r[4] = (x >> 16) as u8;
        r[5..7].copy_from_slice(&(y as u16).to_le_bytes()); r[7] = (y >> 16) as u8;
        let p: u16 = if r[1] & 1 != 0 { 300 + (i % 500) as u16 } else { 0 };
        r[8..10].copy_from_slice(&p.to_le_bytes());
        r
    }).collect()
}

fn run(name: &str, text: &str) {
    let profile = Profile::from_toml_text(text, Path::new("bench.toml")).unwrap();
    let screen = Rect { left: 0, top: 0, right: 2560, bottom: 1440 };
    let mapper = DisplaySnapshot { virtual_screen: screen, monitors: vec![screen] }.mapper(&profile).ok();
    let reports = trace(20_000);
    let start = Instant::now();
    let times: Vec<Instant> = (0..reports.len()).map(|i| start + Duration::from_micros(i as u64 * 2000)).collect();
    let mut best = f64::MAX; let mut all = Vec::new();
    for _round in 0..40 {
        let mut pipeline = ReportPipeline::new(&profile).unwrap();
        let mut plugins = otd_core::plugins::NoFilters;
        let mut sent = 0u64;
        let t0 = Instant::now();
        for (r, now) in reports.iter().zip(&times) {
            let pen = protocol::parse(black_box(r)).unwrap().unwrap();
            pipeline.process_with_raw(pen, r, *now, mapper, &mut plugins, |p| { black_box(p); sent += 1; Ok(()) }).unwrap();
        }
        let ns = t0.elapsed().as_nanos() as f64 / reports.len() as f64;
        black_box(sent);
        best = best.min(ns); all.push(ns);
    }
    all.sort_by(|a, b| a.partial_cmp(b).unwrap());
    println!("{name:28} min {best:6.1} ns  median {:6.1} ns", all[all.len() / 2]);
}

#[test]
#[ignore]
fn zz_bench() {
    run("absolute", ABS);
    run("absolute+radial_follow", &format!("{ABS}{RF}"));
    run("relative", REL);
    run("relative+radial_follow", &format!("{REL}{RF}"));
}
