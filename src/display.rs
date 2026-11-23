//! Reads the Windows desktop layout into the core's display types.

use std::mem::size_of;
use std::ptr;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, GetSystemMetrics, MSG,
    RegisterClassW, SM_CMONITORS, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
    SM_YVIRTUALSCREEN, WM_DISPLAYCHANGE, WNDCLASSW,
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

static DISPLAY_EPOCH: AtomicU64 = AtomicU64::new(1);
static DISPLAY_WATCHER_ACTIVE: AtomicBool = AtomicBool::new(false);
static DISPLAY_WATCHER: OnceLock<bool> = OnceLock::new();

unsafe extern "system" fn display_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_DISPLAYCHANGE {
        DISPLAY_EPOCH.fetch_add(1, Ordering::Release);
    }
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// Starts a demoted cold background thread with a hidden top-level window
/// (not `HWND_MESSAGE`, which does not receive broadcast `WM_DISPLAYCHANGE`)
/// that bumps `DISPLAY_EPOCH` whenever the Windows display topology changes.
fn ensure_display_watcher() {
    let _ = DISPLAY_WATCHER.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::sync_channel::<bool>(1);
        let spawned = std::thread::Builder::new()
            .name("display-change-watcher".into())
            .spawn(move || {
                crate::priority::demote_background_thread();
                let hinstance = unsafe { GetModuleHandleW(ptr::null()) };
                let class_name = windows_sys::core::w!("OtdRustDisplayWatcher");
                let wc = WNDCLASSW {
                    lpfnWndProc: Some(display_wndproc),
                    hInstance: hinstance,
                    lpszClassName: class_name,
                    ..unsafe { std::mem::zeroed() }
                };
                unsafe { RegisterClassW(&wc) };
                // Hidden top-level window receives broadcast `WM_DISPLAYCHANGE`.
                let hwnd = unsafe {
                    CreateWindowExW(
                        0,
                        class_name,
                        ptr::null(),
                        0,
                        0,
                        0,
                        0,
                        0,
                        ptr::null_mut(),
                        ptr::null_mut(),
                        hinstance,
                        ptr::null(),
                    )
                };
                if hwnd.is_null() {
                    let _ = tx.send(false);
                    return;
                }
                DISPLAY_WATCHER_ACTIVE.store(true, Ordering::Release);
                let _ = tx.send(true);
                let mut msg: MSG = unsafe { std::mem::zeroed() };
                while unsafe { GetMessageW(&mut msg, ptr::null_mut(), 0, 0) } > 0 {
                    unsafe { DispatchMessageW(&msg) };
                }
                DISPLAY_WATCHER_ACTIVE.store(false, Ordering::Release);
            });
        if spawned.is_err() {
            return false;
        }
        rx.recv_timeout(Duration::from_millis(500)).unwrap_or(false)
    });
}

/// The desktop as seen from the report thread, for the core session loop.
/// Caches the last display fingerprint and snapshot keyed by the cold
/// `WM_DISPLAYCHANGE` epoch so steady-state checks perform zero win32k calls.
#[derive(Default)]
pub struct WindowsDisplays {
    fingerprint_epoch: u64,
    cached_fingerprint: Option<DisplayFingerprint>,
    snapshot_epoch: u64,
    cached_snapshot: Option<DisplaySnapshot>,
}

impl WindowsDisplays {
    pub fn new() -> Self {
        ensure_display_watcher();
        Self::default()
    }
}

impl otd_core::session::Displays for WindowsDisplays {
    fn fingerprint(&mut self) -> DisplayFingerprint {
        if DISPLAY_WATCHER_ACTIVE.load(Ordering::Acquire) {
            let epoch = DISPLAY_EPOCH.load(Ordering::Acquire);
            if epoch == self.fingerprint_epoch
                && let Some(cached) = self.cached_fingerprint
            {
                return cached;
            }
            let fingerprint = read_fingerprint();
            self.fingerprint_epoch = epoch;
            self.cached_fingerprint = Some(fingerprint);
            return fingerprint;
        }
        read_fingerprint()
    }

    fn topology_changed(&mut self) -> bool {
        if DISPLAY_WATCHER_ACTIVE.load(Ordering::Acquire) {
            return DISPLAY_EPOCH.load(Ordering::Acquire) != self.snapshot_epoch;
        }
        false
    }

    fn snapshot(&mut self) -> Result<DisplaySnapshot, String> {
        if DISPLAY_WATCHER_ACTIVE.load(Ordering::Acquire) {
            let epoch = DISPLAY_EPOCH.load(Ordering::Acquire);
            if epoch == self.snapshot_epoch
                && let Some(cached) = &self.cached_snapshot
            {
                return Ok(cached.clone());
            }
            let snapshot = read_snapshot()?;
            self.snapshot_epoch = epoch;
            self.cached_snapshot = Some(snapshot.clone());
            return Ok(snapshot);
        }
        read_snapshot()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn windows_displays_caches_by_epoch_and_detects_topology_bump() {
        use otd_core::session::Displays;
        let mut displays = WindowsDisplays::new();
        let first_fp = displays.fingerprint();
        let first_snap = displays.snapshot().unwrap();
        assert_eq!(first_fp, first_snap.fingerprint());
        assert!(!displays.topology_changed());
        assert_eq!(displays.fingerprint(), first_fp);
        DISPLAY_EPOCH.fetch_add(1, Ordering::Release);
        if DISPLAY_WATCHER_ACTIVE.load(Ordering::Acquire) {
            assert!(displays.topology_changed());
        }
        let refreshed = displays.snapshot().unwrap();
        assert_eq!(refreshed.fingerprint(), displays.fingerprint());
        assert!(!displays.topology_changed());
    }

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
