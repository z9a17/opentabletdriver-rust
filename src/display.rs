//! Reads the Windows desktop layout into the core's display types.

use std::mem::size_of;

use windows_sys::Win32::Foundation::{LPARAM, RECT};
use windows_sys::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, SM_CMONITORS, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
    SM_YVIRTUALSCREEN,
};

pub use otd_core::display::{DisplayFingerprint, DisplaySnapshot};

use crate::mapping::Rect;

/// Reads the virtual screen and monitor count; no enumeration or allocation.
pub fn read_fingerprint() -> DisplayFingerprint {
    DisplayFingerprint {
        virtual_screen: virtual_screen(),
        monitors: unsafe { GetSystemMetrics(SM_CMONITORS) },
    }
}

fn virtual_screen() -> Rect {
    let left = unsafe { GetSystemMetrics(SM_XVIRTUALSCREEN) };
    let top = unsafe { GetSystemMetrics(SM_YVIRTUALSCREEN) };
    Rect {
        left,
        top,
        right: left + unsafe { GetSystemMetrics(SM_CXVIRTUALSCREEN) },
        bottom: top + unsafe { GetSystemMetrics(SM_CYVIRTUALSCREEN) },
    }
}

unsafe extern "system" fn enumerate_monitor(
    monitor: HMONITOR,
    _dc: HDC,
    _rect: *mut RECT,
    context: LPARAM,
) -> i32 {
    let monitors = unsafe { &mut *(context as *mut Vec<Rect>) };
    let mut info = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        rcMonitor: RECT::default(),
        rcWork: RECT::default(),
        dwFlags: 0,
    };
    if unsafe { GetMonitorInfoW(monitor, &mut info) } != 0 {
        let r = info.rcMonitor;
        monitors.push(Rect {
            left: r.left,
            top: r.top,
            right: r.right,
            bottom: r.bottom,
        });
    }
    1
}

/// Enumerates every monitor, in the calling thread's DPI context.
pub fn read_snapshot() -> Result<DisplaySnapshot, String> {
    let virtual_screen = virtual_screen();
    if !virtual_screen.valid() {
        return Err("Windows did not report a valid desktop rectangle".into());
    }
    let mut monitors: Vec<Rect> = Vec::new();
    if unsafe {
        EnumDisplayMonitors(
            std::ptr::null_mut(),
            std::ptr::null(),
            Some(enumerate_monitor),
            (&mut monitors as *mut Vec<Rect>) as LPARAM,
        )
    } == 0
    {
        return Err(format!(
            "EnumDisplayMonitors failed: {}",
            std::io::Error::last_os_error()
        ));
    }
    monitors.sort_by_key(|r| (r.left, r.top, r.right, r.bottom));
    Ok(DisplaySnapshot {
        virtual_screen,
        monitors,
    })
}

/// The desktop as seen from the report thread, for the core session loop.
pub struct WindowsDisplays;

impl otd_core::session::Displays for WindowsDisplays {
    fn fingerprint(&mut self) -> DisplayFingerprint {
        read_fingerprint()
    }

    fn snapshot(&mut self) -> Result<DisplaySnapshot, String> {
        read_snapshot()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn fingerprint_matches_the_full_snapshot() {
        let snapshot = read_snapshot().unwrap();
        assert_eq!(read_fingerprint(), snapshot.fingerprint());
    }

    #[test]
    #[ignore = "manual timing of the report thread's display checks"]
    fn benchmark_display_checks() {
        const READS: u32 = 10_000;
        let start = Instant::now();
        for _ in 0..READS {
            std::hint::black_box(read_fingerprint());
        }
        let fingerprint = start.elapsed();
        let start = Instant::now();
        for _ in 0..READS {
            std::hint::black_box(read_snapshot().unwrap());
        }
        let snapshot = start.elapsed();
        println!(
            "fingerprint {:.2} us, full snapshot {:.2} us per read",
            fingerprint.as_secs_f64() * 1e6 / f64::from(READS),
            snapshot.as_secs_f64() * 1e6 / f64::from(READS)
        );
    }
}
