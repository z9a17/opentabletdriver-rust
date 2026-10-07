//! Real-time scheduling for the report loop while a tablet session drives
//! output. With a busy thread on every CPU, 44–65 % of reports reached the
//! output more than 1 ms late at normal priority, the slowest after 120–330 ms;
//! at SCHED_FIFO none did (`benchmark_report_loop_under_load`,
//! docs/REALTIME_SCHEDULING_2026-10-05.md). The loop blocks on hidraw between
//! reports and works for microseconds per report, and the kernel's real-time
//! throttling still reserves CPU time for other threads.
//!
//! Unprivileged users may use real-time priorities up to their
//! `RLIMIT_RTPRIO`, which `/etc/security/limits.d` can grant; `CAP_SYS_NICE`
//! lifts the limit. Without either the loop keeps normal scheduling.
//! `OTD_RUST_REALTIME=0` disables the change.

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};

/// Above every normal thread, below threaded interrupt handlers (50), which
/// can deliver the reports, and below audio: PipeWire's server and clients
/// request 88 and 83 by default.
const PRIORITY: libc::c_int = 40;
/// Children of the driver do not inherit the policy.
const SCHED_RESET_ON_FORK: libc::c_int = 0x4000_0000;

fn scheduling_parameters(priority: libc::c_int) -> libc::sched_param {
    // SAFETY: sched_param contains only integers and timespecs. musl adds
    // sporadic-scheduling fields that must stay zero for the policies we use.
    let mut param: libc::sched_param = unsafe { std::mem::zeroed() };
    param.sched_priority = priority;
    param
}

/// Keeps the calling thread at real-time priority until dropped.
pub struct RealtimePriority {
    previous: Option<(libc::c_int, libc::sched_param)>,
}

impl RealtimePriority {
    pub fn raise() -> Self {
        if !enabled() {
            return Self { previous: None };
        }
        // SAFETY: plain-data out-parameters for the calling thread (pid 0).
        let mut param = scheduling_parameters(0);
        let policy = unsafe { libc::sched_getscheduler(0) };
        if policy < 0 || unsafe { libc::sched_getparam(0, &mut param) } != 0 {
            return Self { previous: None };
        }
        let mut limit = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        // SAFETY: valid out-parameter.
        unsafe { libc::getrlimit(libc::RLIMIT_RTPRIO, &mut limit) };
        let allowed = libc::c_int::try_from(limit.rlim_cur).unwrap_or(libc::c_int::MAX);
        // A privileged process ignores the limit; others may use up to it.
        for priority in [PRIORITY, PRIORITY.min(allowed)] {
            if priority <= 0 {
                continue;
            }
            let wanted = scheduling_parameters(priority);
            // SAFETY: valid policy and parameter for the calling thread.
            if unsafe {
                libc::sched_setscheduler(0, libc::SCHED_FIFO | SCHED_RESET_ON_FORK, &wanted)
            } == 0
            {
                eprintln!("Report loop runs at real-time priority {priority} (SCHED_FIFO).");
                return Self {
                    previous: Some((policy, param)),
                };
            }
        }
        static HINTED: AtomicBool = AtomicBool::new(false);
        if !HINTED.swap(true, Ordering::Relaxed) {
            eprintln!(
                "Report loop runs at normal priority, so input can lag while every CPU is busy. \
                 To allow real-time priority, grant your user an rtprio limit of at least {PRIORITY} \
                 in /etc/security/limits.d (audio packages often grant one to a group such as \
                 audio, realtime or pipewire) and sign in again."
            );
        }
        Self { previous: None }
    }

    #[cfg(test)]
    fn is_realtime(&self) -> bool {
        self.previous.is_some()
    }
}

impl Drop for RealtimePriority {
    fn drop(&mut self) {
        if let Some((policy, param)) = self.previous {
            if let Err(error) = restore_policy(policy, |wanted| {
                // SAFETY: the saved priority and policy belong to this thread.
                if unsafe { libc::sched_setscheduler(0, wanted, &param) } == 0 {
                    Ok(())
                } else {
                    Err(io::Error::last_os_error())
                }
            }) {
                eprintln!("could not restore the report thread's scheduling: {error}");
            }
        }
    }
}

fn restore_policy(
    policy: libc::c_int,
    mut apply: impl FnMut(libc::c_int) -> io::Result<()>,
) -> io::Result<()> {
    match apply(policy) {
        // Without CAP_SYS_NICE, Linux refuses to clear reset-on-fork even
        // when lowering FIFO to SCHED_OTHER. Keep the flag to restore the
        // saved policy and priority; privileged callers restore both exactly.
        Err(error)
            if error.raw_os_error() == Some(libc::EPERM) && policy & SCHED_RESET_ON_FORK == 0 =>
        {
            apply(policy | SCHED_RESET_ON_FORK)
        }
        result => result,
    }
}

/// `OTD_RUST_REALTIME=0` keeps normal scheduling.
pub fn enabled() -> bool {
    std::env::var_os("OTD_RUST_REALTIME").is_none_or(|value| value != "0")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guard_restores_the_previous_policy() {
        std::thread::spawn(|| {
            let policy = || unsafe { libc::sched_getscheduler(0) };
            let priority = || {
                let mut param = scheduling_parameters(0);
                assert_eq!(unsafe { libc::sched_getparam(0, &mut param) }, 0);
                param.sched_priority
            };
            let before = policy();
            let before_priority = priority();
            {
                let guard = RealtimePriority::raise();
                // Containers and users without an rtprio limit fall back.
                if guard.is_realtime() {
                    assert_eq!(policy() & !SCHED_RESET_ON_FORK, libc::SCHED_FIFO);
                } else {
                    assert_eq!(policy(), before);
                }
            }
            assert_eq!(
                policy() & !SCHED_RESET_ON_FORK,
                before & !SCHED_RESET_ON_FORK
            );
            assert_eq!(priority(), before_priority);
        })
        .join()
        .unwrap();
    }

    #[test]
    fn unprivileged_restore_keeps_reset_on_fork() {
        let mut current = libc::SCHED_FIFO | SCHED_RESET_ON_FORK;
        restore_policy(libc::SCHED_OTHER, |wanted| {
            // Linux's permission rule also applies to non-real-time policies.
            if current & SCHED_RESET_ON_FORK != 0 && wanted & SCHED_RESET_ON_FORK == 0 {
                return Err(io::Error::from_raw_os_error(libc::EPERM));
            }
            current = wanted;
            Ok(())
        })
        .unwrap();
        assert_eq!(current, libc::SCHED_OTHER | SCHED_RESET_ON_FORK);
    }

    #[test]
    fn privileged_restore_uses_the_exact_previous_policy() {
        let mut policies = Vec::new();
        restore_policy(libc::SCHED_OTHER, |wanted| {
            policies.push(wanted);
            Ok(())
        })
        .unwrap();
        assert_eq!(policies, [libc::SCHED_OTHER]);
    }

    #[test]
    fn restore_reports_errors_without_unnecessary_retries() {
        for (policy, errno, expected_calls) in [
            (libc::SCHED_OTHER, libc::EPERM, 2),
            (libc::SCHED_OTHER | SCHED_RESET_ON_FORK, libc::EPERM, 1),
            (libc::SCHED_OTHER, libc::EINVAL, 1),
        ] {
            let mut calls = 0;
            let error = restore_policy(policy, |_| {
                calls += 1;
                Err(io::Error::from_raw_os_error(errno))
            })
            .unwrap_err();
            assert_eq!(error.raw_os_error(), Some(errno));
            assert_eq!(calls, expected_calls);
        }
    }

    /// Time from a report's arrival to its output through the driver's own
    /// hidraw reader and session loop, at normal and real-time priority, with
    /// and without a busy thread on every CPU. A FIFO stands in for the hidraw
    /// node: each 65-byte write is one report, read whole like a hidraw read,
    /// and ends with its send time. Takes about twenty seconds. The writer
    /// stands in for hardware that is always on time, so it needs real-time
    /// rights too: run as root or with an rtprio limit of 80.
    #[test]
    #[ignore = "manual scheduling benchmark; saturates every CPU for about ten seconds"]
    fn benchmark_report_loop_under_load() {
        use std::cell::{Cell, RefCell};
        use std::collections::BTreeMap;
        use std::io::Write;
        use std::rc::Rc;
        use std::sync::Arc;
        use std::time::{Duration, Instant};

        use otd_core::config::Profile;
        use otd_core::decoders::TabletDecoder;
        use otd_core::display::{DisplayFingerprint, DisplaySnapshot};
        use otd_core::endpoint_match::{Endpoint, Transport};
        use otd_core::mapping::Rect;
        use otd_core::plugins::NoFilters;
        use otd_core::session::{self, Displays, Mode, Read, ReportSource};

        use crate::linux::{Device, Hidraw};

        const REPORTS: u32 = 3000;
        const LENGTH: usize = 65;

        fn monotonic() -> u64 {
            let mut now = libc::timespec {
                tv_sec: 0,
                tv_nsec: 0,
            };
            unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut now) };
            now.tv_sec as u64 * 1_000_000_000 + now.tv_nsec as u64
        }

        struct Screen;
        impl Displays for Screen {
            fn fingerprint(&mut self) -> DisplayFingerprint {
                self.snapshot().unwrap().fingerprint()
            }
            fn snapshot(&mut self) -> Result<DisplaySnapshot, String> {
                let screen = Rect {
                    left: 0,
                    top: 0,
                    right: 2560,
                    bottom: 1440,
                };
                Ok(DisplaySnapshot {
                    virtual_screen: screen,
                    monitors: vec![screen],
                })
            }
        }

        /// Notes each report's send time for the output callback.
        struct Stamped<'a> {
            inner: Hidraw<'a>,
            sent: Rc<Cell<u64>>,
        }
        impl ReportSource for Stamped<'_> {
            fn label(&self) -> &str {
                self.inner.label()
            }
            fn now(&self) -> Instant {
                self.inner.now()
            }
            fn next(&mut self, timeout: Duration) -> std::io::Result<Read<'_>> {
                let read = self.inner.next(timeout)?;
                if let Read::Report { bytes, .. } = &read {
                    self.sent
                        .set(u64::from_ne_bytes(bytes[LENGTH - 8..].try_into().unwrap()));
                }
                Ok(read)
            }
        }

        let directory = std::env::temp_dir().join(format!("otd-rt-bench-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let fifo = directory.join("hidraw");
        let path = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
        let device = Device {
            endpoint: Endpoint {
                path: fifo.display().to_string(),
                physical_id: "benchmark".into(),
                transport: Transport::UsbHid,
                vendor_id: 0,
                product_id: 0,
                can_open: true,
                input_length: (LENGTH - 1) as u32,
                output_length: 0,
                feature_length: 0,
                strings: BTreeMap::new(),
                attributes: None,
            },
            node: fifo.clone(),
            usb: None,
            uses_report_ids: true,
            kernel_driver: None,
            string_errors: BTreeMap::new(),
        };
        let cpus = std::thread::available_parallelism().map_or(4, |n| n.get());
        println!("{cpus} logical CPUs, {REPORTS} reports per row, 1 ms apart");
        for load in [0, cpus] {
            for realtime in [false, true] {
                let stop: &'static AtomicBool = Box::leak(Box::new(AtomicBool::new(false)));
                let busy = Arc::new(AtomicBool::new(true));
                let spinners: Vec<_> = (0..load)
                    .map(|_| {
                        let busy = busy.clone();
                        std::thread::spawn(move || {
                            let mut x = 1u64;
                            while busy.load(Ordering::Relaxed) {
                                x = std::hint::black_box(x.wrapping_mul(6_364_136_223_846_793_005));
                            }
                        })
                    })
                    .collect();
                let device = &device;
                let (mut delays, applied) = std::thread::scope(|scope| {
                    let reader = scope.spawn(move || {
                        let priority = realtime.then(RealtimePriority::raise);
                        let applied = priority.as_ref().is_some_and(RealtimePriority::is_realtime);
                        let sent = Rc::new(Cell::new(0));
                        let delays = Rc::new(RefCell::new(Vec::with_capacity(REPORTS as usize)));
                        let mut source = Stamped {
                            inner: Hidraw::open(device, "benchmark".into(), stop).unwrap(),
                            sent: sent.clone(),
                        };
                        let recorded = delays.clone();
                        session::run_gated_with_devices(
                            &mut source,
                            &mut Screen,
                            &Profile::default(),
                            Mode::Driver,
                            &mut TabletDecoder::for_parser(
                                "OpenTabletDriver.Plugin.Tablet.TabletReportParser",
                                otd_core::spec::TabletSpec::PTH_660,
                            )
                            .unwrap(),
                            &mut NoFilters,
                            move |_| {
                                recorded
                                    .borrow_mut()
                                    .push((monotonic() - sent.get()) as f64 / 1000.0);
                                Ok(())
                            },
                            None,
                            None,
                            &|_| {},
                            || Ok(true),
                        )
                        .unwrap();
                        (delays.take(), applied)
                    });
                    let writer = scope.spawn(move || {
                        let param = scheduling_parameters(80);
                        unsafe { libc::sched_setscheduler(0, libc::SCHED_FIFO, &param) };
                        let mut fifo = std::fs::OpenOptions::new()
                            .write(true)
                            .open(&device.node)
                            .unwrap();
                        let mut next = Instant::now() + Duration::from_millis(300);
                        let mut report = [0u8; LENGTH];
                        report[0] = 1;
                        for index in 0..REPORTS {
                            next += Duration::from_millis(1);
                            while Instant::now() < next {
                                std::hint::spin_loop();
                            }
                            let x = 1000 + (index * 40 % 40_000) as u16;
                            report[2..4].copy_from_slice(&x.to_le_bytes());
                            report[4..6].copy_from_slice(&5000u16.to_le_bytes());
                            report[LENGTH - 8..].copy_from_slice(&monotonic().to_ne_bytes());
                            fifo.write_all(&report).unwrap();
                        }
                        std::thread::sleep(Duration::from_millis(50));
                        stop.store(true, Ordering::Release);
                    });
                    writer.join().unwrap();
                    reader.join().unwrap()
                });
                busy.store(false, Ordering::Relaxed);
                spinners.into_iter().for_each(|s| s.join().unwrap());
                let outputs = delays.len();
                delays.sort_by(f64::total_cmp);
                let at = |p: f64| delays[((delays.len() - 1) as f64 * p) as usize];
                let late = delays.iter().filter(|d| **d > 1000.0).count();
                println!(
                    "{load} busy threads, {} reader: p50 {:.1} us, p99 {:.1} us, p99.9 {:.1} us, max {:.1} us, {late}/{outputs} over 1 ms",
                    match (realtime, applied) {
                        (false, _) => "normal",
                        (true, true) => "SCHED_FIFO",
                        (true, false) => "normal (real-time denied)",
                    },
                    at(0.5),
                    at(0.99),
                    at(0.999),
                    at(1.0)
                );
            }
        }
        std::fs::remove_dir_all(&directory).unwrap();
    }
}
