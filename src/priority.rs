//! Scheduling for the thread that reads pen reports and injects input.
//!
//! OpenTabletDriver's daemon raises its process to the High priority class and
//! its device reader thread to AboveNormal, an effective priority of 14. This
//! driver decodes, filters, maps and calls `SendInput` on one thread, so only
//! that thread is raised: to time-critical, 15, the highest priority available
//! without real-time rights. It blocks on the HID read and works for
//! microseconds per report, so it cannot starve other threads. At normal
//! priority, busy threads of equal or higher priority on every CPU can delay
//! its wakeup by milliseconds (see `benchmark_reader_wake_latency_under_load`).
//!
//! Windows 11 can run background processes, such as the panel minimized to
//! the tray, under EcoQoS. The reader opts out while it holds this guard.

use std::ffi::c_void;
use std::io;
use std::mem::size_of;

use windows_sys::Win32::System::Threading::{
    GetCurrentThread, GetThreadPriority, SetThreadInformation, SetThreadPriority,
    THREAD_POWER_THROTTLING_CURRENT_VERSION, THREAD_POWER_THROTTLING_EXECUTION_SPEED,
    THREAD_POWER_THROTTLING_STATE, THREAD_PRIORITY_TIME_CRITICAL, ThreadPowerThrottling,
};

/// `GetThreadPriority` failure value (MAXLONG).
const THREAD_PRIORITY_ERROR_RETURN: i32 = i32::MAX;

/// Keeps the calling thread at reader priority until dropped.
pub struct ReaderPriority {
    previous: i32,
}

impl ReaderPriority {
    pub fn raise() -> Self {
        let thread = unsafe { GetCurrentThread() };
        let previous = unsafe { GetThreadPriority(thread) };
        if unsafe { SetThreadPriority(thread, THREAD_PRIORITY_TIME_CRITICAL) } == 0 {
            eprintln!(
                "could not raise the report thread's priority: {}",
                io::Error::last_os_error()
            );
        }
        // Unavailable before Windows 10 1709; the priority matters more.
        set_high_qos(true);
        Self { previous }
    }
}

impl Drop for ReaderPriority {
    fn drop(&mut self) {
        set_high_qos(false);
        if self.previous != THREAD_PRIORITY_ERROR_RETURN {
            unsafe { SetThreadPriority(GetCurrentThread(), self.previous) };
        }
    }
}

/// Opts the calling thread out of EcoQoS, or returns it to the system default.
fn set_high_qos(enabled: bool) -> bool {
    let state = THREAD_POWER_THROTTLING_STATE {
        Version: THREAD_POWER_THROTTLING_CURRENT_VERSION,
        // A controlled but clear execution-speed bit means "never throttle".
        ControlMask: if enabled {
            THREAD_POWER_THROTTLING_EXECUTION_SPEED
        } else {
            0
        },
        StateMask: 0,
    };
    unsafe {
        SetThreadInformation(
            GetCurrentThread(),
            ThreadPowerThrottling,
            (&state as *const THREAD_POWER_THROTTLING_STATE).cast::<c_void>(),
            size_of::<THREAD_POWER_THROTTLING_STATE>() as u32,
        ) != 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::System::Threading::THREAD_PRIORITY_NORMAL;

    #[test]
    fn guard_raises_the_thread_and_restores_it() {
        std::thread::spawn(|| {
            let current = || unsafe { GetThreadPriority(GetCurrentThread()) };
            assert_eq!(current(), THREAD_PRIORITY_NORMAL);
            {
                let _guard = ReaderPriority::raise();
                assert_eq!(current(), THREAD_PRIORITY_TIME_CRITICAL);
                assert!(set_high_qos(true), "EcoQoS opt-out is unavailable");
            }
            assert_eq!(current(), THREAD_PRIORITY_NORMAL);
        })
        .join()
        .unwrap();
    }

    /// Wakeup delay of a thread blocked on an event, like the reader waiting
    /// for a report, while busy threads occupy every logical CPU. Prints
    /// percentiles; keeps all CPUs busy for about ten seconds.
    #[test]
    #[ignore = "manual scheduling benchmark; saturates every CPU for about ten seconds"]
    fn benchmark_reader_wake_latency_under_load() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::time::{Duration, Instant};

        use crate::hid::Event;
        use windows_sys::Win32::System::Threading::{
            INFINITE, THREAD_PRIORITY_ABOVE_NORMAL, WaitForSingleObject,
        };

        const SAMPLES: usize = 500;
        let cpus = std::thread::available_parallelism().map_or(8, |n| n.get());
        println!("{cpus} logical CPUs, {SAMPLES} wakeups per row, signals 3 ms apart");
        for (load, load_priority) in [
            ("normal", THREAD_PRIORITY_NORMAL),
            ("above-normal", THREAD_PRIORITY_ABOVE_NORMAL),
        ] {
            for (waiter, raise) in [("normal", false), ("time-critical", true)] {
                let wake = Event::create(false).unwrap();
                let ack = Event::create(false).unwrap();
                let (wake_waiter, ack_waiter) =
                    (wake.duplicate().unwrap(), ack.duplicate().unwrap());
                let sent = Arc::new(std::sync::Mutex::new(Instant::now()));
                let sent_waiter = sent.clone();
                // The waiter and the signaler take their priorities before
                // the load starts, so neither is starved while starting.
                let waiter_thread = std::thread::spawn(move || {
                    let _priority = raise.then(ReaderPriority::raise);
                    ack_waiter.signal().unwrap();
                    let mut delays = Vec::with_capacity(SAMPLES);
                    for _ in 0..SAMPLES {
                        unsafe { WaitForSingleObject(wake_waiter.raw(), INFINITE) };
                        delays.push(sent_waiter.lock().unwrap().elapsed());
                        ack_waiter.signal().unwrap();
                    }
                    delays
                });
                unsafe { WaitForSingleObject(ack.raw(), INFINITE) };
                let _signaler = ReaderPriority::raise();
                let stop = Arc::new(AtomicBool::new(false));
                let spinners: Vec<_> = (0..cpus)
                    .map(|_| {
                        let stop = stop.clone();
                        std::thread::spawn(move || {
                            unsafe { SetThreadPriority(GetCurrentThread(), load_priority) };
                            let mut x = 1u64;
                            while !stop.load(Ordering::Relaxed) {
                                x = std::hint::black_box(x.wrapping_mul(6_364_136_223_846_793_005));
                            }
                        })
                    })
                    .collect();
                std::thread::sleep(Duration::from_millis(300));
                for _ in 0..SAMPLES {
                    std::thread::sleep(Duration::from_millis(3));
                    *sent.lock().unwrap() = Instant::now();
                    wake.signal().unwrap();
                    unsafe { WaitForSingleObject(ack.raw(), INFINITE) };
                }
                let mut delays = waiter_thread.join().unwrap();
                stop.store(true, Ordering::Relaxed);
                spinners.into_iter().for_each(|s| s.join().unwrap());
                delays.sort_unstable();
                let at =
                    |p: f64| delays[((delays.len() - 1) as f64 * p) as usize].as_secs_f64() * 1e6;
                let late = delays
                    .iter()
                    .filter(|d| **d > Duration::from_millis(1))
                    .count();
                println!(
                    "{cpus} {load} spinners, {waiter} reader: p50 {:.1} us, p99 {:.1} us, max {:.1} us, {late}/{SAMPLES} over 1 ms",
                    at(0.5),
                    at(0.99),
                    at(1.0)
                );
            }
        }
    }
}
