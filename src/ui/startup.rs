//! "Start with Windows": a value under the current user's Run key that opens
//! the panel in the tray at sign-in. Only this application's value is read,
//! written or removed.
use super::*;
use windows_sys::Win32::System::Registry::{
    HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ, RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW,
};

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE: &str = "OpenTabletDriverRust";

/// The command the Run value holds: the panel, started in the tray.
fn command() -> Result<String, String> {
    let exe = std::env::current_exe().map_err(|error| error.to_string())?;
    let panel = exe.with_file_name("opentabletdriver-rust-ui.exe");
    let panel = if panel.is_file() { panel } else { exe };
    Ok(format!("\"{}\" ui --tray", panel.display()))
}

/// The registered command, if any.
fn registered() -> Option<String> {
    let key = wide(RUN_KEY);
    let value = wide(VALUE);
    let mut buffer = [0u16; 1024];
    let mut bytes = (buffer.len() * 2) as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_SZ,
            ptr::null_mut(),
            buffer.as_mut_ptr().cast(),
            &mut bytes,
        )
    };
    (status == 0).then(|| {
        let length = (bytes as usize / 2).saturating_sub(1);
        String::from_utf16_lossy(&buffer[..length])
    })
}

/// Whether the panel starts with Windows from this install.
pub(super) fn enabled() -> bool {
    registered().is_some_and(|value| command().is_ok_and(|command| command == value))
}

pub(super) fn set(enabled: bool) -> Result<(), String> {
    let key = wide(RUN_KEY);
    let value = wide(VALUE);
    let status = if enabled {
        let data = wide(&command()?);
        unsafe {
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                value.as_ptr(),
                REG_SZ,
                data.as_ptr().cast(),
                (data.len() * 2) as u32,
            )
        }
    } else {
        match unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), value.as_ptr()) } {
            // Already absent.
            2 => 0,
            status => status,
        }
    };
    if status == 0 {
        Ok(())
    } else {
        Err(std::io::Error::from_raw_os_error(status as i32).to_string())
    }
}
