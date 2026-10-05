//! Scheduling for the thread that reads pen reports and injects input.
//!
//! OpenTabletDriver's daemon raises its process to the High priority class and
//! its device reader thread to AboveNormal, an effective priority of 14. This
//! driver decodes, filters, maps and calls `SendInput` on one thread, so only
//! that thread is raised: to time-critical, 15, the highest priority available
//! without real-time rights. It blocks on the HID read and works for
//! microseconds per native report. Trusted plugins execute on this same thread
//! and can take longer; their work must not be assumed bounded. At normal
//! priority, busy threads of equal or higher priority on every CPU can delay
//! its wakeup by milliseconds (see `benchmark_reader_wake_latency_under_load`).
//!
//! Windows 11 can run background processes, such as the panel minimized to
//! the tray, under EcoQoS. The reader opts out while it holds this guard.
//! The Experimental tab can additionally register the reader with MMCSS's
//! Pro Audio task at session startup. That option is off by default.

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
    mmcss: Option<ProAudio>,
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
        Self {
            previous,
            mmcss: None,
        }
    }
    /// The optional task is registered on the actual report thread and kept
    /// for this session. Capture and benchmarks retain their explicit policy.
    pub fn for_driver(mmcss: bool, status: &impl Fn(&str)) -> Self {
        let mut priority = Self::raise();
        if mmcss {
            match ProAudio::register() {
                Ok(task) => {
                    priority.mmcss = Some(task);
                    status("Report thread uses experimental MMCSS Pro Audio scheduling.");
                }
                Err(error) => {
                    let message = format!("MMCSS Pro Audio scheduling could not be enabled; using time-critical scheduling: {error}");
                    eprintln!("{message}");
                    status(&message);
                }
            }
        }
        priority
    }
}

impl Drop for ReaderPriority {
    fn drop(&mut self) {
        // MMCSS restores its prior state before we restore the pre-session
        // thread priority. The HANDLE also keeps this guard !Send and !Sync.
        drop(self.mmcss.take());
        set_high_qos(false);
        if self.previous != THREAD_PRIORITY_ERROR_RETURN {
            unsafe { SetThreadPriority(GetCurrentThread(), self.previous) };
        }
    }
}

/// Registers the calling thread until dropped on the same thread.
/// https://learn.microsoft.com/windows/win32/api/avrt/nf-avrt-avrevertmmthreadcharacteristics
struct ProAudio(windows_sys::Win32::Foundation::HANDLE);

impl ProAudio {
    fn register() -> io::Result<Self> {
        use windows_sys::Win32::System::Threading::AvSetMmThreadCharacteristicsW;
        let mut task = 0;
        let handle = unsafe {
            AvSetMmThreadCharacteristicsW(windows_sys::core::w!("Pro Audio"), &mut task)
        };
        if handle.is_null() {
            Err(io::Error::last_os_error())
        } else {
            Ok(Self(handle))
        }
    }
}

impl Drop for ProAudio {
    fn drop(&mut self) {
        use windows_sys::Win32::System::Threading::AvRevertMmThreadCharacteristics;
        if unsafe { AvRevertMmThreadCharacteristics(self.0) } == 0 {
            eprintln!("could not leave MMCSS Pro Audio scheduling: {}", io::Error::last_os_error());
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

    #[test]
    fn mmcss_session_restores_the_calling_threads_priority() {
        std::thread::spawn(|| {
            let current = || unsafe { GetThreadPriority(GetCurrentThread()) };
            let before = current();
            let messages = std::cell::RefCell::new(Vec::new());
            let guard = ReaderPriority::for_driver(true, &|message| {
                messages.borrow_mut().push(message.to_owned());
            });
            let registered = guard.mmcss.is_some();
            assert_eq!(messages.borrow().len(), 1);
            if !registered {
                assert_eq!(current(), THREAD_PRIORITY_TIME_CRITICAL);
                assert!(messages.borrow()[0].contains("could not be enabled"));
            }
            drop(guard);
            assert_eq!(current(), before);
            println!("MMCSS registration succeeded: {registered}; previous priority restored");
        })
        .join()
        .unwrap();
    }

    /// Report initialization failure before waiting for benchmark signals.
    /// A disconnected coordinator also cancels a late initialization.
    fn initialize_benchmark_waiter<T>(
        initialize: impl FnOnce() -> io::Result<T>,
        ready: std::sync::mpsc::SyncSender<io::Result<()>>,
    ) -> Option<T> {
        match initialize() {
            Ok(state) => ready.send(Ok(())).ok().map(|()| state),
            Err(error) => {
                let _ = ready.send(Err(error));
                None
            }
        }
    }

    #[test]
    fn benchmark_startup_reports_registration_failure() {
        let (ready, started) = std::sync::mpsc::sync_channel(0);
        let waiter = std::thread::spawn(move || {
            initialize_benchmark_waiter::<()>(
                || Err(io::Error::other("MMCSS registration refused")),
                ready,
            )
        });
        let error = started
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("waiter must report a failed registration")
            .unwrap_err();
        assert_eq!(error.to_string(), "MMCSS registration refused");
        assert_eq!(waiter.join().unwrap(), None);
    }

    #[test]
    fn benchmark_startup_does_not_run_after_coordinator_exits() {
        let (ready, started) = std::sync::mpsc::sync_channel(0);
        drop(started);
        assert_eq!(initialize_benchmark_waiter(|| Ok(42), ready), None);
    }

    #[test]
    fn benchmark_startup_keeps_successful_initialization() {
        let (ready, started) = std::sync::mpsc::sync_channel(0);
        let waiter = std::thread::spawn(move || initialize_benchmark_waiter(|| Ok(42), ready));
        started
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("waiter must signal successful initialization")
            .unwrap();
        assert_eq!(waiter.join().unwrap(), Some(42));
    }

    /// Wakeup delay of a thread blocked on an event, like the reader waiting
    /// for a report, while busy threads occupy every logical CPU. Prints
    /// percentiles; keeps all CPUs busy for fifteen seconds or more. Behind
    /// time-critical load the desktop stops responding for several seconds.
    #[test]
    #[ignore = "manual scheduling benchmark; saturates every CPU for fifteen seconds or more"]
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
        let mmcss = ProAudio::register().is_ok();
        if !mmcss {
            println!("MMCSS is unavailable; its rows are skipped");
        }
        for (load, load_priority) in [
            ("normal", THREAD_PRIORITY_NORMAL),
            ("above-normal", THREAD_PRIORITY_ABOVE_NORMAL),
            ("time-critical", THREAD_PRIORITY_TIME_CRITICAL),
        ] {
            for (waiter, raise, task) in [
                ("normal", false, false),
                ("time-critical", true, false),
                ("MMCSS Pro Audio", true, true),
            ] {
                // Behind time-critical load a normal thread runs only when
                // Windows' starvation boost lifts it, every few seconds.
                if (task && !mmcss) || (!raise && load_priority == THREAD_PRIORITY_TIME_CRITICAL) {
                    continue;
                }
                let wake = Event::create(false).unwrap();
                let ack = Event::create(false).unwrap();
                let (wake_waiter, ack_waiter) =
                    (wake.duplicate().unwrap(), ack.duplicate().unwrap());
                let sent = Arc::new(std::sync::Mutex::new(Instant::now()));
                let sent_waiter = sent.clone();
                // The waiter and the signaler take their priorities before
                // the load starts, so neither is starved while starting.
                let (ready, started) = std::sync::mpsc::sync_channel(0);
                let waiter_thread = std::thread::spawn(move || {
                    let Some((_priority, _task)) = initialize_benchmark_waiter(
                        || {
                            let priority = raise.then(ReaderPriority::raise);
                            let registration = if task {
                                Some(ProAudio::register()?)
                            } else {
                                None
                            };
                            Ok((priority, registration))
                        },
                        ready,
                    ) else {
                        return Vec::new();
                    };
                    let mut delays = Vec::with_capacity(SAMPLES);
                    for _ in 0..SAMPLES {
                        unsafe { WaitForSingleObject(wake_waiter.raw(), INFINITE) };
                        delays.push(sent_waiter.lock().unwrap().elapsed());
                        ack_waiter.signal().unwrap();
                    }
                    delays
                });
                match started.recv_timeout(Duration::from_secs(10)) {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => {
                        waiter_thread.join().unwrap();
                        println!(
                            "{load} load, {waiter} reader: skipped; initialization failed: {error}"
                        );
                        continue;
                    }
                    Err(error) => panic!("{waiter} reader did not initialize: {error}"),
                }
                let _signaler = ReaderPriority::raise();
                // Keeps the signals 3 ms apart behind time-critical load. The
                // delay is measured from the signal, so this does not affect it.
                let _signaler_task =
                    (load_priority == THREAD_PRIORITY_TIME_CRITICAL).then(ProAudio::register);
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

    /// What the reader's priority costs a game: two busy threads stand in for
    /// a game such as osu! on an otherwise idle machine. A reader wakes from a
    /// high-resolution timer about 1000 times a second and works for 30 us,
    /// more than the driver's real per-report work. Prints how often the game
    /// threads paused and how often the reader ran on a CPU they were using.
    #[test]
    #[ignore = "manual scheduling benchmark; runs for about 40 seconds"]
    fn benchmark_reader_effect_on_game_threads() {
        use std::ptr::{null, null_mut};
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
        use std::time::{Duration, Instant};

        use crate::hid::OwnedHandle;
        use windows_sys::Win32::System::Threading::{
            CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, CreateWaitableTimerExW,
            GetCurrentProcessorNumber, SetWaitableTimer, TIMER_ALL_ACCESS, WaitForSingleObject,
        };

        const GAMES: usize = 2;
        const ROUNDS: usize = 6;
        const SECONDS: u64 = 2;

        fn busy(duration: Duration) {
            let start = Instant::now();
            while start.elapsed() < duration {
                std::hint::spin_loop();
            }
        }

        /// Returns pauses over 50 us, 200 us and 1 ms, the longest pause, and
        /// (reader wakeups on a game CPU, reader wakeups).
        fn run(raise: Option<bool>) -> ([u64; 3], Duration, (u32, u32)) {
            let stop = Arc::new(AtomicBool::new(false));
            let game_cpus: Arc<Vec<AtomicU32>> =
                Arc::new((0..GAMES).map(|_| AtomicU32::new(0)).collect());
            let reader = raise.map(|raise| {
                let (stop, game_cpus) = (stop.clone(), game_cpus.clone());
                std::thread::spawn(move || {
                    let _priority = raise.then(ReaderPriority::raise);
                    let timer = OwnedHandle::new(unsafe {
                        CreateWaitableTimerExW(
                            null(),
                            null(),
                            CREATE_WAITABLE_TIMER_HIGH_RESOLUTION,
                            TIMER_ALL_ACCESS,
                        )
                    })
                    .unwrap();
                    let due: i64 = -10_000; // 1 ms, relative, in 100 ns units
                    unsafe { SetWaitableTimer(timer.raw(), &due, 1, None, null_mut(), 0) };
                    let (mut shared, mut wakeups) = (0, 0);
                    while !stop.load(Ordering::Relaxed) {
                        unsafe { WaitForSingleObject(timer.raw(), 100) };
                        let cpu = unsafe { GetCurrentProcessorNumber() } + 1;
                        if game_cpus.iter().any(|c| c.load(Ordering::Relaxed) == cpu) {
                            shared += 1;
                        }
                        busy(Duration::from_micros(30));
                        wakeups += 1;
                    }
                    (shared, wakeups)
                })
            });
            let games: Vec<_> = (0..GAMES)
                .map(|index| {
                    let (stop, game_cpus) = (stop.clone(), game_cpus.clone());
                    std::thread::spawn(move || {
                        let (mut pauses, mut worst) = ([0u64; 3], Duration::ZERO);
                        let (mut last, mut x) = (Instant::now(), 1u64);
                        while !stop.load(Ordering::Relaxed) {
                            for _ in 0..200 {
                                x = std::hint::black_box(x.wrapping_mul(6_364_136_223_846_793_005));
                            }
                            let cpu = unsafe { GetCurrentProcessorNumber() } + 1;
                            game_cpus[index].store(cpu, Ordering::Relaxed);
                            let now = Instant::now();
                            let gap = now - last;
                            last = now;
                            worst = worst.max(gap);
                            for (limit, count) in [50, 200, 1000].into_iter().zip(&mut pauses) {
                                if gap > Duration::from_micros(limit) {
                                    *count += 1;
                                }
                            }
                        }
                        (pauses, worst)
                    })
                })
                .collect();
            std::thread::sleep(Duration::from_secs(SECONDS));
            stop.store(true, Ordering::Relaxed);
            let (mut pauses, mut worst) = ([0u64; 3], Duration::ZERO);
            for game in games {
                let (p, w) = game.join().unwrap();
                pauses.iter_mut().zip(p).for_each(|(total, n)| *total += n);
                worst = worst.max(w);
            }
            let shared = reader.map_or((0, 0), |r| r.join().unwrap());
            (pauses, worst, shared)
        }

        let configs = [
            ("no reader", None),
            ("normal-priority reader", Some(false)),
            ("time-critical reader", Some(true)),
        ];
        /// Pauses over each limit, worst pause per run, (shared, wakeups).
        type Totals = ([u64; 3], Vec<Duration>, (u32, u32));
        let mut totals: [Totals; 3] = std::array::from_fn(|_| ([0; 3], Vec::new(), (0, 0)));
        // Interleave so background activity affects every configuration alike.
        for _ in 0..ROUNDS {
            for (index, (_, raise)) in configs.iter().enumerate() {
                let (pauses, worst, shared) = run(*raise);
                let total = &mut totals[index];
                total.0.iter_mut().zip(pauses).for_each(|(t, n)| *t += n);
                total.1.push(worst);
                total.2 = (total.2.0 + shared.0, total.2.1 + shared.1);
            }
        }
        let per_second = (ROUNDS as u64 * SECONDS) as f64;
        for ((name, _), (pauses, mut worst, (shared, wakeups))) in configs.iter().zip(totals) {
            worst.sort_unstable();
            println!(
                "{GAMES} game threads, {name}: pauses per second over 50 us {:.1}, over 200 us {:.1}, over 1 ms {:.2}; median worst pause {:.0} us; reader on a game thread's CPU {shared} of {wakeups} wakeups",
                pauses[0] as f64 / per_second,
                pauses[1] as f64 / per_second,
                pauses[2] as f64 / per_second,
                worst[worst.len() / 2].as_secs_f64() * 1e6
            );
        }
    }
}
