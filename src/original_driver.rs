//! Prevent two drivers from controlling the same devices. An existing original
//! driver must be stopped by its owner: executable paths alone cannot restore
//! command-line options, working directory or environment after termination.

use std::io;
use std::mem::size_of;

use windows_sys::Win32::Foundation::ERROR_NO_MORE_FILES;
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};

use crate::hid::OwnedHandle;

fn original_running() -> io::Result<bool> {
    let snapshot = OwnedHandle::new(unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) })?;
    let mut entry = PROCESSENTRY32W {
        dwSize: size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    if unsafe { Process32FirstW(snapshot.raw(), &mut entry) } == 0 {
        let error = io::Error::last_os_error();
        return if error.raw_os_error() == Some(ERROR_NO_MORE_FILES as i32) {
            Ok(false)
        } else {
            Err(error)
        };
    }
    loop {
        let length = entry.szExeFile.iter().position(|&unit| unit == 0)
            .unwrap_or(entry.szExeFile.len());
        let name = String::from_utf16_lossy(&entry.szExeFile[..length]);
        if name.eq_ignore_ascii_case("OpenTabletDriver.Daemon.exe")
            || name.eq_ignore_ascii_case("OpenTabletDriver.UX.Wpf.exe") {
            return Ok(true);
        }
        if unsafe { Process32NextW(snapshot.raw(), &mut entry) } == 0 {
            let error = io::Error::last_os_error();
            return if error.raw_os_error() == Some(ERROR_NO_MORE_FILES as i32) {
                Ok(false)
            } else {
                Err(error)
            };
        }
    }
}

pub fn ensure_stopped() -> io::Result<()> {
    if original_running()? {
        return Err(io::Error::other(
            "The original OpenTabletDriver is running. Stop it through its own panel before starting Rust output. It was left untouched because its complete launch settings cannot be safely restored after termination.",
        ));
    }
    Ok(())
}
