//! Temporarily pause an already-running OpenTabletDriver during cursor output.
//! Discovery and process creation happen only at startup and shutdown.

use std::ffi::OsString;
use std::io;
use std::mem::size_of;
use std::os::windows::ffi::OsStringExt;
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;

use windows_sys::Win32::Foundation::{ERROR_NO_MORE_FILES, WAIT_OBJECT_0};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::Threading::{
    CREATE_NEW_PROCESS_GROUP, CREATE_NO_WINDOW, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    PROCESS_TERMINATE, QueryFullProcessImageNameW, TerminateProcess, WaitForSingleObject,
};

use crate::hid::OwnedHandle;

const DAEMON: &str = "OpenTabletDriver.Daemon.exe";
const UX: &str = "OpenTabletDriver.UX.Wpf.exe";
const PROCESS_SYNCHRONIZE: u32 = 0x0010_0000;

#[derive(Clone)]
struct RunningOriginal {
    name: &'static str,
    pid: u32,
}

fn running_originals() -> io::Result<Vec<RunningOriginal>> {
    let snapshot = OwnedHandle::new(unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) })?;
    let mut entry = PROCESSENTRY32W {
        dwSize: size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    if unsafe { Process32FirstW(snapshot.raw(), &mut entry) } == 0 {
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(ERROR_NO_MORE_FILES as i32) {
            return Ok(Vec::new());
        }
        return Err(error);
    }
    let mut found = Vec::new();
    loop {
        let length = entry
            .szExeFile
            .iter()
            .position(|&unit| unit == 0)
            .unwrap_or(entry.szExeFile.len());
        let name = String::from_utf16_lossy(&entry.szExeFile[..length]);
        let name = if name.eq_ignore_ascii_case(DAEMON) {
            Some(DAEMON)
        } else if name.eq_ignore_ascii_case(UX) {
            Some(UX)
        } else {
            None
        };
        if let Some(name) = name {
            found.push(RunningOriginal {
                name,
                pid: entry.th32ProcessID,
            });
        }
        if unsafe { Process32NextW(snapshot.raw(), &mut entry) } == 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(ERROR_NO_MORE_FILES as i32) {
                break;
            }
            return Err(error);
        }
    }
    found.sort_by_key(|original| (original.name != UX, original.pid));
    Ok(found)
}

fn executable_path(handle: &OwnedHandle) -> io::Result<PathBuf> {
    let mut buffer = vec![0u16; 32_768];
    let mut length = buffer.len() as u32;
    if unsafe { QueryFullProcessImageNameW(handle.raw(), 0, buffer.as_mut_ptr(), &mut length) } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(PathBuf::from(OsString::from_wide(
        &buffer[..length as usize],
    )))
}

#[derive(Default)]
pub struct OriginalDriverGuard {
    stopped: Vec<(&'static str, PathBuf)>,
}

impl OriginalDriverGuard {
    pub fn pause() -> io::Result<Self> {
        let mut guard = Self::default();
        for original in running_originals()? {
            let handle = OwnedHandle::new(unsafe {
                OpenProcess(
                    PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_TERMINATE | PROCESS_SYNCHRONIZE,
                    0,
                    original.pid,
                )
            })?;
            let path = executable_path(&handle)?;
            eprintln!("Pausing {} for Rust cursor output.", original.name);
            if unsafe { TerminateProcess(handle.raw(), 0) } == 0 {
                return Err(io::Error::last_os_error());
            }
            guard.stopped.push((original.name, path));
            if unsafe { WaitForSingleObject(handle.raw(), 2_000) } != WAIT_OBJECT_0 {
                return Err(io::Error::other(format!(
                    "{} did not stop within two seconds",
                    original.name
                )));
            }
        }
        if !running_originals()?.is_empty() {
            return Err(io::Error::other(
                "OpenTabletDriver restarted while it was being paused",
            ));
        }
        Ok(guard)
    }
}

impl Drop for OriginalDriverGuard {
    fn drop(&mut self) {
        self.stopped.sort_by_key(|(name, _)| *name != DAEMON);
        for (name, path) in &self.stopped {
            eprintln!("Restoring {name}.");
            if let Err(error) = Command::new(path)
                .creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW)
                .spawn()
            {
                eprintln!("Could not restore {name}: {error}");
            }
        }
    }
}
