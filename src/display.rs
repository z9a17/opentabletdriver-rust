use std::mem::size_of;

use windows_sys::Win32::Foundation::{LPARAM, RECT};
use windows_sys::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
};

use crate::config::Profile;
use crate::mapping::{Mapper, Rect};

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
        let left = unsafe { GetSystemMetrics(SM_XVIRTUALSCREEN) };
        let top = unsafe { GetSystemMetrics(SM_YVIRTUALSCREEN) };
        let width = unsafe { GetSystemMetrics(SM_CXVIRTUALSCREEN) };
        let height = unsafe { GetSystemMetrics(SM_CYVIRTUALSCREEN) };
        let virtual_screen = Rect {
            left,
            top,
            right: left + width,
            bottom: top + height,
        };
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
