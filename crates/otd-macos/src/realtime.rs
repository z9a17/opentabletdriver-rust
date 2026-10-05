//! Mach time-constraint scheduling for the report loop while a session drives
//! output, the policy Core Audio threads use: the thread declares that it
//! needs at most 1 ms of CPU time within 2 ms of waking, and the kernel runs
//! it ahead of ordinary threads. The values follow OpenTabletDriver's device
//! reader in [PR #5071](https://github.com/OpenTabletDriver/OpenTabletDriver/pull/5071),
//! which measured fewer late reports under CPU load. The loop blocks in the
//! run loop between reports. The kernel returns a thread that keeps overrunning
//! its budget to normal scheduling. `OTD_RUST_REALTIME=0` disables the change.

/// Keeps the calling thread under the time-constraint policy until dropped.
pub struct TimeConstraint {
    thread: libc::mach_port_t,
}

#[repr(C)]
struct Timebase {
    numer: u32,
    denom: u32,
}

unsafe extern "C" {
    fn mach_timebase_info(info: *mut Timebase) -> libc::kern_return_t;
}

impl TimeConstraint {
    pub fn raise() -> Option<Self> {
        if std::env::var_os("OTD_RUST_REALTIME").is_some_and(|value| value == "0") {
            return None;
        }
        let mut timebase = Timebase { numer: 0, denom: 0 };
        // SAFETY: valid out-parameter.
        if unsafe { mach_timebase_info(&mut timebase) } != 0 || timebase.numer == 0 {
            return None;
        }
        // Mach absolute time units: nanoseconds × denom / numer.
        let ticks =
            |nanos: u64| (nanos * u64::from(timebase.denom) / u64::from(timebase.numer)) as u32;
        let mut policy = libc::thread_time_constraint_policy {
            period: 0,
            computation: ticks(1_000_000),
            constraint: ticks(2_000_000),
            preemptible: 1,
        };
        // SAFETY: the port of the calling thread; pthread_mach_thread_np adds no reference.
        let thread = unsafe { libc::pthread_mach_thread_np(libc::pthread_self()) };
        // SAFETY: the policy matches the flavor and count.
        let result = unsafe {
            libc::thread_policy_set(
                thread,
                libc::THREAD_TIME_CONSTRAINT_POLICY as libc::thread_policy_flavor_t,
                (&mut policy as *mut libc::thread_time_constraint_policy).cast(),
                libc::THREAD_TIME_CONSTRAINT_POLICY_COUNT,
            )
        };
        if result != 0 {
            eprintln!(
                "Report loop runs at normal priority: the time-constraint policy failed ({result})."
            );
            return None;
        }
        eprintln!("Report loop runs under the real-time time-constraint policy.");
        Some(Self { thread })
    }
}

impl Drop for TimeConstraint {
    fn drop(&mut self) {
        let mut standard: libc::integer_t = 0;
        // SAFETY: the standard policy carries no data.
        unsafe {
            libc::thread_policy_set(
                self.thread,
                libc::THREAD_STANDARD_POLICY as libc::thread_policy_flavor_t,
                &mut standard,
                libc::THREAD_STANDARD_POLICY_COUNT as libc::mach_msg_type_number_t,
            )
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reads whether the calling thread uses the time-constraint policy.
    fn constrained() -> bool {
        let mut policy = libc::thread_time_constraint_policy {
            period: 0,
            computation: 0,
            constraint: 0,
            preemptible: 0,
        };
        let mut count = libc::THREAD_TIME_CONSTRAINT_POLICY_COUNT;
        let mut default: libc::boolean_t = 0;
        // SAFETY: valid out-parameters for the calling thread's port.
        let result = unsafe {
            libc::thread_policy_get(
                libc::pthread_mach_thread_np(libc::pthread_self()),
                libc::THREAD_TIME_CONSTRAINT_POLICY as libc::thread_policy_flavor_t,
                (&mut policy as *mut libc::thread_time_constraint_policy).cast(),
                &mut count,
                &mut default,
            )
        };
        result == 0 && default == 0
    }

    #[test]
    fn guard_sets_the_policy_and_restores_it() {
        std::thread::spawn(|| {
            assert!(!constrained());
            {
                let guard = TimeConstraint::raise();
                assert_eq!(guard.is_some(), constrained());
            }
            assert!(!constrained());
        })
        .join()
        .unwrap();
    }
}
