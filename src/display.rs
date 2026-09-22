use std::mem::size_of;

use windows_sys::Win32::Foundation::{LPARAM, RECT};
use windows_sys::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, SM_CMONITORS, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
    SM_YVIRTUALSCREEN,
};

use crate::config::Profile;
use crate::mapping::{Mapper, Rect};

/// The virtual screen and the monitor count: enough to notice a resolution or
/// monitor change from the report thread without enumerating monitors or
/// allocating.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DisplayFingerprint {
    virtual_screen: Rect,
    monitors: i32,
}

impl DisplayFingerprint {
    pub fn read() -> Self {
        Self {
            virtual_screen: virtual_screen(),
            monitors: unsafe { GetSystemMetrics(SM_CMONITORS) },
        }
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DisplaySnapshot {
    pub virtual_screen: Rect,
    pub monitors: Vec<Rect>,
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

impl DisplaySnapshot {
    pub fn read() -> Result<Self, String> {
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
        Ok(Self {
            virtual_screen,
            monitors,
        })
    }

    pub fn mapper(&self, profile: &Profile) -> Result<Mapper, String> {
        if let Some(settings) = profile.otd_mapping {
            return Mapper::from_otd(settings, self.virtual_screen)
                .ok_or_else(|| "invalid OpenTabletDriver absolute-area mapping".into());
        }
        let dest = if let Some(index) = profile.monitor {
            *self
                .monitors
                .get(index)
                .ok_or_else(|| format!("monitor {index} is not present"))?
        } else {
            self.virtual_screen
        };
        Mapper::new(profile.crop, profile.rotation, dest, self.virtual_screen)
            .ok_or_else(|| "invalid tablet-to-display mapping".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn fingerprint_matches_the_full_snapshot() {
        let snapshot = DisplaySnapshot::read().unwrap();
        let fingerprint = DisplayFingerprint::read();
        assert_eq!(fingerprint.virtual_screen, snapshot.virtual_screen);
        assert_eq!(fingerprint.monitors as usize, snapshot.monitors.len());
    }

    #[test]
    #[ignore = "manual timing of the report thread's display checks"]
    fn benchmark_display_checks() {
        const READS: u32 = 10_000;
        let start = Instant::now();
        for _ in 0..READS {
            std::hint::black_box(DisplayFingerprint::read());
        }
        let fingerprint = start.elapsed();
        let start = Instant::now();
        for _ in 0..READS {
            std::hint::black_box(DisplaySnapshot::read().unwrap());
        }
        let snapshot = start.elapsed();
        println!(
            "fingerprint {:.2} us, full snapshot {:.2} us per read",
            fingerprint.as_secs_f64() * 1e6 / f64::from(READS),
            snapshot.as_secs_f64() * 1e6 / f64::from(READS)
        );
    }
}
