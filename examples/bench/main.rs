//! Repeatable timing of the driver's report path (F04). Every case feeds the
//! same synthetic PTH-660 trace through the code the driver runs and records
//! per-report time, thread CPU time and Rust allocations. Cases that call
//! `SendInput` move the cursor, never click, and run only when asked. See
//! `docs/PERFORMANCE.md`.
//!
//!     cargo run --release --locked --example bench -- --help

// The driver's Windows modules, compiled here unchanged.
#[allow(dead_code)]
#[path = "../../src/dotnet.rs"]
mod dotnet;
#[allow(dead_code)]
#[path = "../../src/hid.rs"]
mod hid;
#[allow(dead_code)]
#[path = "../../src/output.rs"]
mod output;
#[allow(dead_code)]
#[path = "../../src/plugins.rs"]
mod plugins;
#[allow(dead_code)]
#[path = "../../src/priority.rs"]
mod priority;

mod clock;
mod differential;
mod replay;
mod stats;
mod trace;

// The portable core, at the crate paths the driver's modules use.
use otd_core::{config, protocol};

use std::hint::black_box;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use windows_sys::Win32::Foundation::POINT;
use windows_sys::Win32::UI::WindowsAndMessaging::{GetCursorPos, SetCursorPos};

use otd_core::config::Profile;
use otd_core::display::{DisplayFingerprint, DisplaySnapshot};
use otd_core::mapping::{Mapper, OtdArea, Rect};
use otd_core::output::{MousePacket, flags};
use otd_core::pipeline::ReportPipeline;
use otd_core::plugins::{Filters, NoFilters, PluginConfig, PluginKind};
use otd_core::session::{self, Displays, Mode, Read, ReportSource};
use otd_core::test_alloc::Count;

use crate::plugins::PluginChain;
use crate::stats::Distribution;
use crate::trace::Trace;

const USAGE: &str = "Usage: bench [options]

  --out FILE              write the JSON results to FILE instead of stdout
  --rounds N              timed passes per case (default 7)
  --reports N             trace length in reports (default 20000)
  --warmup-ms N           warm-up per round before the timed pass (default 1000)
  --rate HZ               report rate for filter timestamps and replay (default 200)
  --ema DLL               native sample filter (default target/release/otd_ema_filter.dll)
  --radialfollow DLL      unchanged RadialFollow.dll; adds the managed case
  --compat DIR            .NET bridge directory (default: OTD_COMPAT_DIR)
  --send-input            adds SendInput cases; moves the cursor, never clicks
  --replay-seconds N      paced replay length; 0 skips it (default 0)
  --only TEXT             runs only cases whose name contains TEXT
  --export-workload DIR   writes trace.bin, workload.json and osu-profile.toml
                          for the upstream harness and the daemon, then exits
  --export-differential DIR
                          writes differential fixture skeletons for the first
                          --reports reports, then exits";

/// The development machine's osu! profile, as in
/// `tests/golden/absolute-radial-follow.toml`: an 85 x 47.8125 mm area on the
/// 2560 x 1440 primary monitor, the tip at 1 % pressure and AbstractQbit's
/// tablet-space Radial Follow.
const OSU_PROFILE: &str = r#"
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

[[radial_follow]]
outer_radius = 0.7039
inner_radius = 0.302
smoothing_coefficient = 0.302
soft_knee_scale = 0.603
smoothing_leak_coefficient = 0.201
"#;

/// Relative mode at 10 counts per millimetre with a 100 ms reset, as in
/// `tests/golden/relative.toml`.
const RELATIVE_PROFILE: &str = r#"
[relative]
x_sensitivity = 10.0
y_sensitivity = 10.0
rotation = 0.0
reset_delay_ms = 100

[bindings]
tip_enabled = true
eraser_enabled = true
tip_threshold_raw = 82
eraser_threshold_raw = 82
"#;

/// The same tip threshold in OpenTabletDriver's percent.
const TIP_PERCENT: f64 = 1.0;
const TRACE_SEED: u64 = 20_260_923;
const TRACE_NAME: &str = "osu-synthetic-v1";
const EMA_SETTINGS: &str = r#"{"alpha":0.35,"reset_ms":50}"#;

/// The development machine's desktop: 2560 x 1440 with 1600 x 900 to its right.
fn desktop() -> DisplaySnapshot {
    DisplaySnapshot {
        virtual_screen: Rect {
            left: 0,
            top: 0,
            right: 4160,
            bottom: 1440,
        },
        monitors: vec![
            Rect {
                left: 0,
                top: 0,
                right: 2560,
                bottom: 1440,
            },
            Rect {
                left: 2560,
                top: 0,
                right: 4160,
                bottom: 900,
            },
        ],
    }
}

/// Benchmarks move the cursor but never press a button.
pub(crate) fn moves_only(packet: MousePacket) -> Option<MousePacket> {
    (packet.flags & flags::MOVE != 0).then_some(MousePacket {
        flags: packet.flags & !(flags::LEFTDOWN | flags::LEFTUP),
        ..packet
    })
}

struct Options {
    out: Option<PathBuf>,
    rounds: usize,
    reports: usize,
    warmup: Duration,
    rate_hz: f64,
    ema: PathBuf,
    radial_follow: Option<PathBuf>,
    send_input: bool,
    replay_seconds: f64,
    only: Option<String>,
    export: Option<PathBuf>,
    differential: Option<PathBuf>,
}

fn number<T: std::str::FromStr>(name: &str, text: &str) -> Result<T, String> {
    text.parse()
        .map_err(|_| format!("{name} needs a number, not {text:?}"))
}

fn options() -> Result<Options, String> {
    let mut options = Options {
        out: None,
        rounds: 7,
        reports: 20_000,
        warmup: Duration::from_millis(1_000),
        rate_hz: 200.0,
        ema: PathBuf::from("target/release/otd_ema_filter.dll"),
        radial_follow: None,
        send_input: false,
        replay_seconds: 0.0,
        only: None,
        export: None,
        differential: None,
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or(format!("{arg} needs a value"));
        match arg.as_str() {
            "--out" => options.out = Some(value()?.into()),
            "--rounds" => options.rounds = number(&arg, &value()?)?,
            "--reports" => options.reports = number(&arg, &value()?)?,
            "--warmup-ms" => options.warmup = Duration::from_millis(number(&arg, &value()?)?),
            "--rate" => options.rate_hz = number(&arg, &value()?)?,
            "--ema" => options.ema = value()?.into(),
            "--radialfollow" => options.radial_follow = Some(value()?.into()),
            // SAFETY: options are read before any other thread starts.
            "--compat" => unsafe { std::env::set_var("OTD_COMPAT_DIR", value()?) },
            "--send-input" => options.send_input = true,
            "--replay-seconds" => options.replay_seconds = number(&arg, &value()?)?,
            "--only" => options.only = Some(value()?),
            "--export-workload" => options.export = Some(value()?.into()),
            "--export-differential" => options.differential = Some(value()?.into()),
            "--help" | "-h" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            other => return Err(format!("unknown option {other}\n\n{USAGE}")),
        }
    }
    if options.rounds == 0 || options.reports < 1_000 {
        return Err("use at least one round and 1000 reports".into());
    }
    if !(options.rate_hz.is_finite() && options.rate_hz >= 1.0) {
        return Err("--rate must be at least 1 Hz".into());
    }
    Ok(options)
}

/// Processes one report, as the driver's session loop would.
type Process = Box<dyn FnMut(&[u8], Instant)>;

struct Round {
    per_report: Distribution,
    thread_ns: f64,
    wall: Duration,
    allocations: usize,
}

struct Bench<'a> {
    trace: &'a Trace,
    options: &'a Options,
    ns_per_tick: f64,
    /// Nanoseconds per `clock::thread_cycles` cycle.
    ns_per_cycle: f64,
    interval: Duration,
    samples: Vec<u64>,
}

impl Bench<'_> {
    fn wanted(&self, name: &str) -> bool {
        self.options
            .only
            .as_deref()
            .is_none_or(|text| name.contains(text))
    }

    /// Feeds `count` reports, with timestamps that advance at the report rate
    /// so time-based filters behave as they do on a device.
    fn feed(&self, process: &mut Process, sequence: &mut u64, count: usize, base: Instant) {
        for _ in 0..count {
            let index = (*sequence % self.trace.len() as u64) as usize;
            process(
                self.trace.report(index),
                base + self.interval * *sequence as u32,
            );
            *sequence += 1;
        }
    }

    /// Warms up for the configured time, pauses so a managed runtime can
    /// finish tiered compilation, warms up briefly again, then times every
    /// report of the trace once.
    fn round(&mut self, process: &mut Process, base: Instant) -> Round {
        let reports = self.trace.len();
        let mut sequence = 0;
        let warming = Instant::now();
        while warming.elapsed() < self.options.warmup || sequence < reports as u64 {
            self.feed(process, &mut sequence, 1_000, base);
        }
        std::thread::sleep(Duration::from_millis(250));
        self.feed(process, &mut sequence, 2_000, base);

        self.samples.clear();
        let count = Count::start();
        let cycles = clock::thread_cycles();
        let wall = Instant::now();
        for index in 0..reports {
            let now = base + self.interval * sequence as u32;
            sequence += 1;
            let report = self.trace.report(index);
            let begin = clock::start();
            process(report, now);
            self.samples.push(clock::stop() - begin);
        }
        let wall = wall.elapsed();
        let cycles = clock::thread_cycles() - cycles;
        let allocations = count.finish();
        Round {
            per_report: Distribution::of(&mut self.samples, self.ns_per_tick),
            thread_ns: cycles as f64 * self.ns_per_cycle / reports as f64,
            wall,
            allocations,
        }
    }

    fn case(
        &mut self,
        name: &str,
        description: &str,
        make: impl Fn() -> Result<Process, String>,
    ) -> Result<Option<Value>, String> {
        if !self.wanted(name) {
            return Ok(None);
        }
        eprintln!("case {name}");
        let (mut rounds, mut setup_ms, mut first_us) = (Vec::new(), Vec::new(), Vec::new());
        for _ in 0..self.options.rounds {
            let created = Instant::now();
            let mut process = make()?;
            setup_ms.push(created.elapsed().as_secs_f64() * 1e3);
            // The first report includes lazy work such as JIT compilation.
            let base = Instant::now();
            let begin = clock::start();
            process(self.trace.report(0), base);
            first_us.push((clock::stop() - begin) as f64 * self.ns_per_tick / 1e3);
            rounds.push(self.round(&mut process, base));
        }
        Ok(Some(self.summary(
            name,
            description,
            &rounds,
            &setup_ms,
            &first_us,
        )))
    }

    fn summary(
        &self,
        name: &str,
        description: &str,
        rounds: &[Round],
        setup_ms: &[f64],
        first_us: &[f64],
    ) -> Value {
        let reports = self.trace.len() as f64;
        let distributions: Vec<Distribution> = rounds.iter().map(|r| r.per_report).collect();
        let thread: Vec<f64> = rounds.iter().map(|r| r.thread_ns).collect();
        let throughput: Vec<f64> = rounds
            .iter()
            .map(|r| reports / r.wall.as_secs_f64())
            .collect();
        let allocations = rounds.iter().map(|r| r.allocations).max().unwrap_or(0);
        json!({
            "name": name,
            "description": description,
            "reports": self.trace.len(),
            "rounds": rounds.len(),
            "per_report_ns": stats::across(&distributions),
            "thread_cpu_ns_per_report": stats::round(stats::median_of(&thread)),
            "throughput_per_s": stats::median_of(&throughput).round(),
            "allocations": allocations,
            "allocations_per_report": allocations as f64 / reports,
            "setup_ms": setup_ms.iter().map(|&v| stats::round(v)).collect::<Vec<_>>(),
            "first_report_us": first_us.iter().map(|&v| stats::round(v)).collect::<Vec<_>>(),
        })
    }
}

fn pipeline(
    profile: &Profile,
    mapper: Option<Mapper>,
    mut filters: impl Filters + 'static,
    send: fn(MousePacket) -> io::Result<()>,
) -> Result<Process, String> {
    let mut pipeline = ReportPipeline::new(profile)?;
    Ok(Box::new(move |report, now| {
        if let Ok(Some(pen)) = protocol::parse(report) {
            let _ = black_box(pipeline.process(pen, now, mapper, &mut filters, send));
        }
    }))
}

fn discard(packet: MousePacket) -> io::Result<()> {
    black_box(packet);
    Ok(())
}

fn send_moves(packet: MousePacket) -> io::Result<()> {
    moves_only(packet).map_or(Ok(()), output::send_input)
}

fn chain(config: PluginConfig) -> Result<PluginChain, String> {
    PluginChain::load(std::slice::from_ref(&config))
}

/// Serves the trace to the real session loop without waiting, recording when
/// the loop asks for each report.
struct TraceSource<'a> {
    trace: &'a Trace,
    next: usize,
    end: usize,
    stamps: Vec<u64>,
    count: Option<Count>,
    allocations: Option<usize>,
}

impl ReportSource for TraceSource<'_> {
    fn label(&self) -> &str {
        "synthetic trace"
    }

    fn now(&self) -> Instant {
        Instant::now()
    }

    fn next(&mut self, _timeout: Duration) -> io::Result<Read<'_>> {
        self.stamps.push(clock::stop());
        if self.next == 100 {
            self.count = Some(Count::start());
        }
        if self.next == self.end {
            self.allocations = self.count.take().map(Count::finish);
            return Ok(Read::Ended);
        }
        let report = self.trace.report(self.next % self.trace.len());
        self.next += 1;
        Ok(Read::Report {
            bytes: report,
            ready: Instant::now(),
            queued: false,
        })
    }
}

struct FixedDisplays(DisplaySnapshot);

impl Displays for FixedDisplays {
    fn fingerprint(&mut self) -> DisplayFingerprint {
        self.0.fingerprint()
    }

    fn snapshot(&mut self) -> Result<DisplaySnapshot, String> {
        Ok(self.0.clone())
    }
}

/// The whole session loop over the trace: decoding, the pipeline, counters,
/// timing, the once-a-second display check and the output call, without the
/// HID read. Per-report time runs from one request for a report to the next.
fn session_case(bench: &mut Bench, profile: &Profile) -> Result<Option<Value>, String> {
    let name = "session/absolute+radial_follow";
    if !bench.wanted(name) {
        return Ok(None);
    }
    eprintln!("case {name}");
    let reports = bench.trace.len();
    let mut rounds = Vec::new();
    let mut allocations = 0;
    for _ in 0..bench.options.rounds {
        let run = |end: usize| -> Result<TraceSource, String> {
            // Touch the stamp buffer's pages first, so page faults do not land
            // inside timed reports.
            let mut stamps = vec![0; end + 1];
            stamps.clear();
            let mut source = TraceSource {
                trace: bench.trace,
                next: 0,
                end,
                stamps,
                count: None,
                allocations: None,
            };
            let mut displays = FixedDisplays(desktop());
            session::run(
                &mut source,
                &mut displays,
                profile,
                Mode::Driver,
                &mut NoFilters,
                discard,
                &|_: &str| {},
            )
            .map_err(|e| e.to_string())?;
            Ok(source)
        };
        run(reports * 5)?;
        let cycles = clock::thread_cycles();
        let wall = Instant::now();
        let source = run(reports)?;
        let wall = wall.elapsed();
        let cycles = clock::thread_cycles() - cycles;
        allocations = allocations.max(source.allocations.unwrap_or(usize::MAX));
        let mut samples: Vec<u64> = source.stamps.windows(2).map(|w| w[1] - w[0]).collect();
        // The first request comes before the loop has started.
        samples.remove(0);
        rounds.push(Round {
            per_report: Distribution::of(&mut samples, bench.ns_per_tick),
            thread_ns: cycles as f64 * bench.ns_per_cycle / reports as f64,
            wall,
            allocations: source.allocations.unwrap_or(usize::MAX),
        });
    }
    let mut value = bench.summary(
        name,
        "session::run over the trace with the osu! profile: decoding, pipeline, counters, timing and display checks; no HID read or SendInput",
        &rounds,
        &[],
        &[],
    );
    value["allocations"] = json!(allocations);
    value["allocations_note"] = json!("counted from the 100th report to the end of each session");
    Ok(Some(value))
}

/// Tries one `SendInput` that moves the cursor where it already is. It fails
/// when another desktop, such as the lock screen, has the input.
fn send_input_works() -> Result<POINT, String> {
    let mut point = POINT { x: 0, y: 0 };
    if unsafe { GetCursorPos(&mut point) } == 0 {
        return Err(format!(
            "GetCursorPos failed: {}",
            io::Error::last_os_error()
        ));
    }
    output::send_input(MousePacket {
        dx: 0,
        dy: 0,
        flags: flags::MOVE,
    })
    .map_err(|e| format!("SendInput failed ({e}); is the desktop locked?"))?;
    Ok(point)
}

fn export(
    dir: &Path,
    trace: &Trace,
    osu: &Profile,
    relative: &Profile,
    rate_hz: f64,
) -> Result<(), String> {
    let mapping = osu
        .otd_mapping
        .ok_or("the osu! profile has no absolute mapping")?;
    let follow = osu
        .radial_follow
        .first()
        .ok_or("the osu! profile has no Radial Follow")?;
    let relative = relative
        .relative
        .ok_or("the relative profile has no relative settings")?;
    if Some(config::activation_raw(TIP_PERCENT)?) != osu.contact.tip_threshold_raw {
        return Err("TIP_PERCENT does not match the profile's raw tip threshold".into());
    }
    let area = |a: OtdArea| json!({"width": a.width, "height": a.height, "x": a.x, "y": a.y, "rotation": a.rotation});
    let workload = json!({
        "schema": "otd-bench-workload/1",
        "trace": {
            "file": "trace.bin",
            "generator": TRACE_NAME,
            "seed": TRACE_SEED,
            "reports": trace.len(),
            "report_bytes": trace::REPORT_BYTES,
            "fnv1a64": format!("{:016x}", trace.fnv1a64()),
            "rate_hz": rate_hz,
        },
        "tablet": {
            "name": "Wacom PTH-660",
            "width_mm": 224.0,
            "height_mm": 148.0,
            "max_x": protocol::MAX_X,
            "max_y": protocol::MAX_Y,
            "max_pressure": protocol::MAX_PRESSURE,
            "pen_buttons": 2,
        },
        "absolute": {
            "display": area(mapping.display),
            "tablet": area(mapping.tablet),
            "clipping": mapping.clipping,
            "limiting": mapping.limiting,
        },
        "tip_threshold_percent": TIP_PERCENT,
        "radial_follow": radial_follow_settings(follow),
        "relative": {
            "x_sensitivity": relative.sensitivity.0,
            "y_sensitivity": relative.sensitivity.1,
            "rotation": relative.rotation,
            "reset_ms": relative.reset_delay.as_secs_f64() * 1e3,
        },
    });
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("trace.bin"), trace.bytes()).map_err(|e| e.to_string())?;
    // The same profile for the daemon when idle processes are sampled.
    std::fs::write(dir.join("osu-profile.toml"), OSU_PROFILE).map_err(|e| e.to_string())?;
    let text = serde_json::to_string_pretty(&workload).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("workload.json"), text).map_err(|e| e.to_string())
}

/// The settings as the unchanged plugin names them.
fn radial_follow_settings(settings: &otd_core::radial_follow::RadialFollowSettings) -> Value {
    json!({
        "OuterRadius": settings.outer_radius,
        "InnerRadius": settings.inner_radius,
        "SmoothingCoefficient": settings.smoothing_coefficient,
        "SoftKneeScale": settings.soft_knee_scale,
        "SmoothingLeakCoefficient": settings.smoothing_leak_coefficient,
    })
}

fn run() -> Result<(), String> {
    let options = options()?;
    let osu = Profile::from_toml_text(OSU_PROFILE, Path::new("bench osu! profile"))?;
    let relative = Profile::from_toml_text(RELATIVE_PROFILE, Path::new("bench relative profile"))?;
    let trace = trace::osu(options.reports, TRACE_SEED);
    let parsed = (0..trace.len())
        .filter(|&i| matches!(protocol::parse(trace.report(i)), Ok(Some(_))))
        .count();
    if parsed != trace.len() {
        return Err(format!(
            "{} of {} trace reports do not decode",
            trace.len() - parsed,
            trace.len()
        ));
    }
    if let Some(dir) = &options.export {
        return export(dir, &trace, &osu, &relative, options.rate_hz);
    }
    if let Some(dir) = &options.differential {
        return differential::export(dir, &trace, &osu, &relative, options.reports);
    }

    let mut plain = osu.clone();
    plain.radial_follow.clear();
    let desktop = desktop();
    let mapper = Some(desktop.mapper(&osu)?);
    eprintln!("calibrating the time-stamp counter");
    let tsc_hz = clock::tsc_hz();
    let thread_cycle_hz = clock::thread_cycle_hz();
    let mut bench = Bench {
        trace: &trace,
        options: &options,
        ns_per_tick: 1e9 / tsc_hz,
        ns_per_cycle: 1e9 / thread_cycle_hz,
        interval: Duration::from_secs_f64(1.0 / options.rate_hz),
        samples: Vec::with_capacity(trace.len()),
    };

    let mut cases = Vec::new();
    cases.push(bench.case(
        "empty",
        "the harness alone: timing, one boxed call per report",
        || {
            Ok(Box::new(|report: &[u8], _| {
                black_box(report);
            }))
        },
    )?);
    cases.push(
        bench.case("parse", "protocol::parse on each 192-byte report", || {
            Ok(Box::new(|report: &[u8], _| {
                let _ = black_box(protocol::parse(black_box(report)));
            }))
        })?,
    );
    cases.push(bench.case(
        "absolute",
        "decode, contact state, absolute mapping and the output decision; no filter; packets discarded",
        || pipeline(&plain, mapper, NoFilters, discard),
    )?);
    cases.push(bench.case(
        "absolute+radial_follow",
        "as absolute, with the built-in Radial Follow port (the osu! profile)",
        || pipeline(&osu, mapper, NoFilters, discard),
    )?);
    cases.push(bench.case(
        "relative",
        "decode, contact state and relative mapping at 10 counts/mm; packets discarded",
        || pipeline(&relative, None, NoFilters, discard),
    )?);
    if options.ema.exists() {
        let config = PluginConfig {
            path: options.ema.clone(),
            kind: PluginKind::Native,
            enabled: true,
            type_name: String::new(),
            settings_json: EMA_SETTINGS.into(),
        };
        cases.push(bench.case(
            "absolute+native_ema",
            "as absolute, with the sample native EMA DLL as a PreTransform filter",
            || pipeline(&plain, mapper, chain(config.clone())?, discard),
        )?);
    } else {
        eprintln!(
            "skipping absolute+native_ema: {} not found",
            options.ema.display()
        );
    }
    if let Some(path) = &options.radial_follow {
        let settings = radial_follow_settings(&osu.radial_follow[0]).to_string();
        let config = PluginConfig {
            path: path.clone(),
            kind: PluginKind::Dotnet,
            enabled: true,
            type_name: "RadialFollow.RadialFollowSmoothingTabletSpace".into(),
            settings_json: settings,
        };
        cases.push(bench.case(
            "absolute+managed_radial_follow",
            "as absolute, with the unchanged RadialFollow 0.3.0 DLL through the .NET bridge instead of the built-in port",
            || pipeline(&plain, mapper, chain(config.clone())?, discard),
        )?);
    }
    cases.push(session_case(&mut bench, &osu)?);

    let mut cursor = None;
    if options.send_input {
        cursor = Some(send_input_works()?);
        // The packets the osu! profile produces for the trace, without clicks.
        let mut packets = Vec::with_capacity(trace.len());
        let mut producer = ReportPipeline::new(&osu)?;
        for index in 0..trace.len() {
            if let Ok(Some(pen)) = protocol::parse(trace.report(index)) {
                let now = Instant::now() + bench.interval * index as u32;
                let _ = producer.process(pen, now, mapper, &mut NoFilters, |packet| {
                    packets.extend(moves_only(packet));
                    Ok(())
                });
            }
        }
        cases.push(bench.case(
            "sendinput",
            "one SendInput call per packet the osu! profile produces for the trace, buttons removed",
            || {
                let (packets, mut next) = (packets.clone(), 0);
                Ok(Box::new(move |_: &[u8], _| {
                    let _ = black_box(output::send_input(packets[next % packets.len()]));
                    next += 1;
                }))
            },
        )?);
        cases.push(bench.case(
            "sendinput_still",
            "SendInput to the same position every time: the call without cursor motion",
            || {
                let packet = packets[0];
                Ok(Box::new(move |_: &[u8], _| {
                    let _ = black_box(output::send_input(packet));
                }))
            },
        )?);
        cases.push(bench.case(
            "absolute+radial_follow+sendinput",
            "the osu! profile's whole report path with SendInput, buttons removed",
            || pipeline(&osu, mapper, NoFilters, send_moves),
        )?);
    }

    let replay = if options.replay_seconds > 0.0 {
        eprintln!("paced replay for {} s", options.replay_seconds);
        if options.send_input && cursor.is_none() {
            cursor = Some(send_input_works()?);
        }
        Some(
            replay::Replay {
                trace: &trace,
                profile: &osu,
                mapper,
                rate_hz: options.rate_hz,
                seconds: options.replay_seconds,
                send: options
                    .send_input
                    .then_some(send_moves as fn(MousePacket) -> io::Result<()>),
                ns_per_tick: bench.ns_per_tick,
            }
            .run()?,
        )
    } else {
        None
    };
    if let Some(point) = cursor {
        unsafe { SetCursorPos(point.x, point.y) };
    }

    let results = json!({
        "schema": "otd-bench/1",
        "harness": "rust",
        "driver": {
            "name": "opentabletdriver-rust",
            "version": env!("CARGO_PKG_VERSION"),
            "commit": std::env::var("OTD_BENCH_COMMIT").ok(),
        },
        "machine": {
            "cpu": clock::cpu_brand(),
            "logical_cpus": std::thread::available_parallelism().map(|n| n.get()).unwrap_or(0),
            "tsc_hz": tsc_hz.round(),
            "thread_cycle_hz": thread_cycle_hz.round(),
            "invariant_tsc": clock::invariant_tsc(),
        },
        "trace": {
            "generator": TRACE_NAME,
            "seed": TRACE_SEED,
            "reports": trace.len(),
            "report_bytes": trace::REPORT_BYTES,
            "fnv1a64": format!("{:016x}", trace.fnv1a64()),
            "rate_hz": options.rate_hz,
        },
        "method": {
            "rounds": options.rounds,
            "warmup_ms": options.warmup.as_millis() as u64,
            "timer": "rdtsc/rdtscp with lfence around each report, converted with the measured counter frequency",
            "allocations": "Rust allocations on the measuring thread during the timed pass",
        },
        "cases": cases.into_iter().flatten().collect::<Vec<_>>(),
        "replay": replay,
    });
    let text = serde_json::to_string_pretty(&results).map_err(|e| e.to_string())?;
    match &options.out {
        Some(path) => std::fs::write(path, text).map_err(|e| e.to_string()),
        None => {
            println!("{text}");
            Ok(())
        }
    }
}

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
