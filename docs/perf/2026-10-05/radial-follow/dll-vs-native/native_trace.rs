// Native Radial Follow over trace.csv, written to native-out.csv, for
// comparison with the DLL's dotnet-out.csv. Copy to crates/otd-core/tests/ and run:
//   RF_DIR=<folder with trace.csv> cargo test --release -p otd-core --test native_trace -- --ignored
use otd_core::radial_follow::{RadialFollowSettings, RadialFollowSmoothingTabletSpace};
use otd_core::spec::TabletSpec;
use std::time::{Duration, Instant};
#[test]
#[ignore]
fn zz_rf() {
    let dir = std::env::var("RF_DIR").unwrap();
    let trace = std::fs::read_to_string(format!("{dir}/trace.csv")).unwrap();
    let settings = RadialFollowSettings { outer_radius: 0.7039, inner_radius: 0.302, smoothing_coefficient: 0.302, soft_knee_scale: 0.603, smoothing_leak_coefficient: 0.201 };
    let mut filter = RadialFollowSmoothingTabletSpace::new_for(settings, TabletSpec::PTH_660);
    let mut now = Instant::now() + Duration::from_secs(1);
    let mut out = String::new();
    for line in trace.lines() {
        let f: Vec<&str> = line.split(',').collect();
        let gap: u64 = f[2].parse().unwrap();
        now += Duration::from_millis(2 + gap);
        let (x, y) = filter.filter_raw_at(f[0].parse().unwrap(), f[1].parse().unwrap(), now);
        out.push_str(&format!("{x:?},{y:?}\n"));
    }
    std::fs::write(format!("{dir}/native-out.csv"), out).unwrap();
}
