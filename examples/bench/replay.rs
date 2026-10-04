//! Paced replay. A device thread releases one report per interval by setting
//! an event, standing in for the HID class driver completing a read. A reader
//! thread at the driver's report priority waits on that event, decodes and
//! processes the report, and optionally calls `SendInput`, as a device
//! session does. Wake delay, pipeline work and output are timed separately,
//! and so is each report's whole time from the signal to pipeline return,
//! including the output call.
//! A high-resolution timer paces the device thread; how late it fires does
//! not count, because wake delay starts when the event is set.

use std::hint::black_box;
use std::ptr::{null, null_mut};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use windows_sys::Win32::System::Threading::{
    CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, CreateWaitableTimerExW, GetCurrentThread, INFINITE,
    SetThreadPriority, SetWaitableTimer, THREAD_PRIORITY_TIME_CRITICAL, TIMER_ALL_ACCESS,
    WaitForSingleObject,
};

use otd_core::config::Profile;
use otd_core::mapping::Mapper;
use otd_core::output::MousePacket;
use otd_core::pipeline::ReportPipeline;
use otd_core::plugins::Filters;
use otd_core::protocol;
use otd_core::test_alloc::Count;

use crate::clock;
use crate::hid::{Event, OwnedHandle};
use crate::priority::ReaderPriority;
use crate::stats::Distribution;
use crate::trace::Trace;

pub struct Replay<'a> {
    pub trace: &'a Trace,
    pub profile: &'a Profile,
    pub mapper: Option<Mapper>,
    pub rate_hz: f64,
    pub seconds: f64,
    /// Synthetic CPU contention, independent of real game/streaming processes.
    pub load_threads: usize,
    /// Sends real cursor moves; `None` discards packets.
    pub send: Option<fn(MousePacket) -> std::io::Result<()>>,
    pub ns_per_tick: f64,
}

struct CpuLoad {
    stop: Arc<AtomicBool>,
    workers: Vec<std::thread::JoinHandle<()>>,
}

impl CpuLoad {
    fn start(count: usize) -> Result<Self, String> {
        let mut load = Self {
            stop: Arc::new(AtomicBool::new(false)),
            workers: Vec::with_capacity(count),
        };
        for index in 0..count {
            let stop = load.stop.clone();
            load.workers.push(
                std::thread::Builder::new()
                    .name(format!("bench-cpu-load-{index}"))
                    .spawn(move || {
                        let mut value = 1u64;
                        while !stop.load(Ordering::Relaxed) {
                            value = black_box(value.wrapping_mul(6_364_136_223_846_793_005));
                        }
                    })
                    .map_err(|error| format!("Could not start benchmark load: {error}"))?,
            );
        }
        Ok(load)
    }
}

impl Drop for CpuLoad {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}

/// The device side: sets `event` once per interval, recording when.
fn device(
    event: Event,
    reports: u64,
    interval: Duration,
    signaled: &AtomicU64,
    sequence: &AtomicU64,
    canceled: &AtomicBool,
) {
    unsafe { SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_TIME_CRITICAL) };
    let timer = OwnedHandle::new(unsafe {
        CreateWaitableTimerExW(
            null(),
            null(),
            CREATE_WAITABLE_TIMER_HIGH_RESOLUTION,
            TIMER_ALL_ACCESS,
        )
    })
    .expect("high-resolution timer");
    let start = Instant::now() + Duration::from_millis(50);
    for index in 0..reports {
        if canceled.load(Ordering::Relaxed) {
            break;
        }
        let due = start + interval * index as u32;
        let now = Instant::now();
        if due > now {
            // Relative due time in 100 ns units.
            let wait = -(((due - now).as_nanos() / 100) as i64);
            unsafe {
                SetWaitableTimer(timer.raw(), &wait, 0, None, null_mut(), 0);
                WaitForSingleObject(timer.raw(), INFINITE);
            }
        }
        signaled.store(clock::start(), Ordering::Release);
        sequence.store(index + 1, Ordering::Release);
        event.signal().expect("signal report");
    }
}

impl Replay<'_> {
    pub fn run(&self, filters: &mut impl Filters) -> Result<Value, String> {
        let reports = (self.rate_hz * self.seconds).round() as u64;
        if reports < 200 {
            return Err("replay needs at least 200 reports".into());
        }
        let interval = Duration::from_secs_f64(1.0 / self.rate_hz);
        let mut pipeline = ReportPipeline::new(self.profile)?;
        let _load = CpuLoad::start(self.load_threads)?;
        let event = Event::create(false).map_err(|e| e.to_string())?;
        let device_event = event.duplicate().map_err(|e| e.to_string())?;
        let signaled = Arc::new(AtomicU64::new(0));
        let sequence = Arc::new(AtomicU64::new(0));
        let done = Arc::new(AtomicBool::new(false));
        let canceled = Arc::new(AtomicBool::new(false));
        let device = {
            let (signaled, sequence, done) = (signaled.clone(), sequence.clone(), done.clone());
            let canceled = canceled.clone();
            std::thread::spawn(move || {
                device(device_event, reports, interval, &signaled, &sequence, &canceled);
                done.store(true, Ordering::Release);
            })
        };

        let capacity = reports as usize;
        let (mut wake, mut work, mut output, mut driver) = (
            Vec::with_capacity(capacity),
            Vec::with_capacity(capacity),
            Vec::with_capacity(capacity),
            Vec::with_capacity(capacity),
        );
        let (mut last, mut coalesced, mut allocations) = (0u64, 0u64, None);
        let mut count = None;
        let outcome = (|| -> Result<(), String> {
            let _priority = ReaderPriority::raise();
            while last < reports {
                unsafe { WaitForSingleObject(event.raw(), 1_000) };
                let woke = clock::stop();
                let current = sequence.load(Ordering::Acquire);
                if current == last {
                    if done.load(Ordering::Acquire) || device.is_finished() {
                        break;
                    }
                    continue;
                }
                // An auto-reset event merges signals the reader was too late
                // to see separately; the HID class driver would queue them.
                coalesced += current - last - 1;
                last = current;
                let woke_after = woke.saturating_sub(signaled.load(Ordering::Acquire));
                wake.push(woke_after);
                // Count from the 100th report, after the pipeline has warmed up.
                if last >= 100 && count.is_none() && allocations.is_none() {
                    count = Some(Count::start());
                }
                let report = self.trace.report((last as usize - 1) % self.trace.len());
                let begin = clock::start();
                let mut sent = 0;
                if let Ok(Some(pen)) = protocol::parse(report) {
                    pipeline.process_with_raw(
                        pen,
                        report,
                        Instant::now(),
                        self.mapper,
                        filters,
                        |packet| {
                            let before = clock::start();
                            let result = match self.send {
                                Some(send) => send(packet),
                                None => {
                                    black_box(packet);
                                    Ok(())
                                }
                            };
                            sent += clock::stop() - before;
                            result
                        },
                    )
                    .map_err(|error| format!("Paced pipeline failed: {error}"))?;
                    if let Some(name) = filters.take_failure() {
                        return Err(format!("Paced replay disabled failing plugin: {name}"));
                    }
                }
                let total = clock::stop() - begin;
                work.push(total - sent);
                output.push(sent);
                driver.push(woke_after + total);
            }
            if let Some(count) = count.take() {
                allocations = Some(count.finish());
            }
            Ok(())
        })();
        canceled.store(true, Ordering::Relaxed);
        device.join().map_err(|_| "device thread panicked")?;
        outcome?;

        let ticks = self.ns_per_tick;
        Ok(json!({
            "rate_hz": self.rate_hz,
            "reports": reports,
            "processed": wake.len(),
            "coalesced": coalesced,
            "send_input": self.send.is_some(),
            "controlled_cpu_load_threads": self.load_threads,
            "model": "auto-reset event; missed signals coalesce rather than preserving a HID queue",
            "timing_endpoint": "pipeline_return_including_output",
            "allocations_note": "Rust heap only; managed heap allocations are not counted",
            "wake_ns": Distribution::of(&mut wake, ticks).json(),
            "pipeline_ns": Distribution::of(&mut work, ticks).json(),
            "output_ns": Distribution::of(&mut output, ticks).json(),
            "signal_to_output_ns": Distribution::of(&mut driver, ticks).json(),
            "allocations_after_warmup": allocations,
        }))
    }
}
