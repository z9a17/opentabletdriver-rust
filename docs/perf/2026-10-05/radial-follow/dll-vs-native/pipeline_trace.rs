// The whole report pipeline over trace.csv twice: with the built-in Radial
// Follow, and as the managed path with the DLL's recorded outputs from
// dotnet-out.csv. Prints how many output packets differ. Copy to
// crates/otd-core/tests/ and run:
//   RF_DIR=<folder> cargo test --release -p otd-core --test pipeline_trace -- --ignored --nocapture
use otd_core::config::Profile;
use otd_core::display::DisplaySnapshot;
use otd_core::mapping::Rect;
use otd_core::pipeline::ReportPipeline;
use otd_core::plugins::{DispatchInput, Filters, PipelineRuntime};
use otd_core::protocol::{self, PenReport};
use std::io;
use std::path::Path;
use std::time::{Duration, Instant};

const ABS: &str = "[absolute]\nclipping = true\nlimiting = false\n[absolute.display]\nWidth = 2560.0\nHeight = 1440.0\nX = 1280.0\nY = 720.0\nRotation = 0.0\n[absolute.tablet]\nWidth = 85.0\nHeight = 47.8125\nX = 110.0\nY = 23.90625\nRotation = 0.0\n";
const RF: &str = "[[radial_follow]]\nouter_radius = 0.7039\ninner_radius = 0.302\nsmoothing_coefficient = 0.302\nsoft_knee_scale = 0.603\nsmoothing_leak_coefficient = 0.201\n";

/// The managed fused path's host side, with the DLL's recorded output.
struct Replay { out: Vec<[f32; 2]>, next: usize }
impl Filters for Replay {
    fn uses_managed_graph(&self) -> bool { true }
    fn dispatch(&mut self, input: DispatchInput<'_>, runtime: &mut dyn PipelineRuntime) -> io::Result<()> {
        let mut values = input.values;
        runtime.builtins(&mut values)?;
        values.position = Some(self.out[self.next]);
        self.next += 1;
        if runtime.transform(input.kind, &mut values)? { runtime.output(input.kind, &values, input.raw)?; }
        Ok(())
    }
    fn has_pre(&self) -> bool { true }
    fn process_pre(&mut self, p: (f32, f32), _: PenReport, _: Instant) -> (f32, f32) { p }
    fn has_pixels(&self) -> bool { false }
    fn process_pixels(&mut self, p: (f32, f32), _: PenReport, _: Instant) -> (f32, f32) { p }
    fn reset(&mut self) {}
    fn take_failure(&mut self) -> Option<&str> { None }
}

fn report(x: u32, y: u32) -> [u8; 17] {
    let mut r = [0x10u8, 0x60, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x19];
    r[2..4].copy_from_slice(&(x as u16).to_le_bytes()); r[4] = (x >> 16) as u8;
    r[5..7].copy_from_slice(&(y as u16).to_le_bytes()); r[7] = (y >> 16) as u8;
    r
}

fn run(profile_text: &str, filters: &mut impl Filters, trace: &[(u32, u32, u64)]) -> Vec<(i32, i32, u32)> {
    let profile = Profile::from_toml_text(profile_text, Path::new("p.toml")).unwrap();
    let screen = Rect { left: 0, top: 0, right: 2560, bottom: 1440 };
    let mapper = DisplaySnapshot { virtual_screen: screen, monitors: vec![screen] }.mapper(&profile).ok();
    let mut pipeline = ReportPipeline::new(&profile).unwrap();
    let mut now = Instant::now() + Duration::from_secs(1);
    let mut packets = Vec::new();
    for &(x, y, gap) in trace {
        now += Duration::from_millis(2 + gap);
        let raw = report(x, y);
        let pen = protocol::parse(&raw).unwrap().unwrap();
        let before = packets.len();
        pipeline.process_with_raw(pen, &raw, now, mapper, filters, |p| { packets.push((p.dx, p.dy, p.flags)); Ok(()) }).unwrap();
        if packets.len() == before { packets.push((i32::MIN, 0, 0)); }
    }
    packets
}

#[test]
#[ignore]
fn zz_rf_e2e() {
    let dir = std::env::var("RF_DIR").unwrap();
    let trace: Vec<(u32, u32, u64)> = std::fs::read_to_string(format!("{dir}/trace.csv")).unwrap().lines().map(|l| {
        let f: Vec<&str> = l.split(',').collect(); (f[0].parse().unwrap(), f[1].parse().unwrap(), f[2].parse().unwrap()) }).collect();
    let out: Vec<[f32; 2]> = std::fs::read_to_string(format!("{dir}/dotnet-out.csv")).unwrap().lines().map(|l| {
        let f: Vec<&str> = l.split(',').collect(); [f[0].parse().unwrap(), f[1].parse().unwrap()] }).collect();
    let native = run(&format!("{ABS}{RF}"), &mut otd_core::plugins::NoFilters, &trace);
    let managed = run(ABS, &mut Replay { out, next: 0 }, &trace);
    let mut worst = 0; let mut differ = 0;
    for (i, (a, b)) in native.iter().zip(&managed).enumerate() {
        if a != b { print!("#{i} "); }
        if a != b { differ += 1; println!("diff native {a:?} managed {b:?}"); if a.0 != i32::MIN && b.0 != i32::MIN { worst = worst.max((a.0 - b.0).abs().max((a.1 - b.1).abs())); } }
    }
    println!("reports {} native packets {} managed packets {} differing {} worst {} (of 65535)", trace.len(), native.len(), managed.len(), differ, worst);
}
