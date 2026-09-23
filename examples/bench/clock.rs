//! Cycle-accurate timestamps. Per-report work takes tens of nanoseconds, below
//! the 100 ns resolution of `Instant` on Windows, so cases are timed with the
//! time-stamp counter and converted with a frequency measured against
//! `Instant`. `lfence` keeps the reads from moving across the timed work.

use std::arch::x86_64::{__cpuid, __rdtscp, _mm_lfence, _rdtsc};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::System::Threading::GetCurrentThread;

// In windows-sys this needs a feature the driver does not use.
#[link(name = "kernel32")]
unsafe extern "system" {
    fn QueryThreadCycleTime(thread: HANDLE, cycles: *mut u64) -> i32;
}

#[inline(always)]
pub fn start() -> u64 {
    unsafe {
        _mm_lfence();
        let stamp = _rdtsc();
        _mm_lfence();
        stamp
    }
}

#[inline(always)]
pub fn stop() -> u64 {
    unsafe {
        let mut processor = 0;
        let stamp = __rdtscp(&mut processor);
        _mm_lfence();
        stamp
    }
}

/// Whether the counter ticks at a constant rate in every power state.
pub fn invariant_tsc() -> bool {
    __cpuid(0x8000_0000).eax >= 0x8000_0007 && __cpuid(0x8000_0007).edx & (1 << 8) != 0
}

pub fn cpu_brand() -> String {
    let mut bytes = Vec::with_capacity(48);
    for leaf in 0x8000_0002..=0x8000_0004u32 {
        let registers = __cpuid(leaf);
        for value in [registers.eax, registers.ebx, registers.ecx, registers.edx] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
    }
    String::from_utf8_lossy(&bytes)
        .trim_matches(char::from(0))
        .trim()
        .to_owned()
}

/// Counter ticks per second: the median of five 100 ms comparisons.
pub fn tsc_hz() -> f64 {
    let mut rates: Vec<f64> = (0..5)
        .map(|_| {
            let (instant, ticks) = (Instant::now(), start());
            std::thread::sleep(Duration::from_millis(100));
            let (elapsed, ticks) = (instant.elapsed(), stop() - ticks);
            ticks as f64 / elapsed.as_secs_f64()
        })
        .collect();
    rates.sort_by(f64::total_cmp);
    rates[2]
}

/// The rate of `thread_cycles` while the thread runs: the median of five
/// 50 ms busy loops. It can differ from the counter frequency, for example
/// when it counts core cycles at boost clock.
pub fn thread_cycle_hz() -> f64 {
    let mut rates: Vec<f64> = (0..5)
        .map(|_| {
            let (instant, cycles) = (Instant::now(), thread_cycles());
            while instant.elapsed() < Duration::from_millis(50) {
                std::hint::spin_loop();
            }
            let (elapsed, cycles) = (instant.elapsed(), thread_cycles() - cycles);
            cycles as f64 / elapsed.as_secs_f64()
        })
        .collect();
    rates.sort_by(f64::total_cmp);
    rates[2]
}

/// Processor cycles charged to the calling thread, which excludes time it
/// spent preempted.
pub fn thread_cycles() -> u64 {
    let mut cycles = 0;
    unsafe { QueryThreadCycleTime(GetCurrentThread(), &mut cycles) };
    cycles
}
