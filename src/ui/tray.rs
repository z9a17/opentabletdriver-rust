//! Notification-area icon, as in OpenTabletDriver's UX: it stays while the
//! panel runs, a click brings the panel back, and its menu offers Show Window
//! and Close. Minimizing the panel hides it here.
use windows_sys::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NIM_SETVERSION,
    NIN_SELECT, NINF_KEY, NOTIFYICON_VERSION_4, NOTIFYICONDATAW, Shell_NotifyIconW,
};

use super::*;

const ICON_ID: u32 = 1;
const NIN_KEYSELECT: u32 = NIN_SELECT | NINF_KEY;

fn data(window: HWND, tip: &str) -> NOTIFYICONDATAW {
    let mut data = NOTIFYICONDATAW {
        cbSize: size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: window,
        uID: ICON_ID,
        uFlags: NIF_TIP | NIF_SHOWTIP,
        ..Default::default()
    };
    let units: Vec<u16> = tip.encode_utf16().take(data.szTip.len() - 1).collect();
    data.szTip[..units.len()].copy_from_slice(&units);
    data
}

/// Adds the icon and returns whether the notification area has it.
pub(super) fn add(window: HWND, icon: HICON, tip: &str) -> bool {
    let mut data = data(window, tip);
    data.uFlags |= NIF_MESSAGE | NIF_ICON;
    data.uCallbackMessage = WM_TRAY;
    data.hIcon = icon;
    unsafe {
        if Shell_NotifyIconW(NIM_ADD, &data) == 0 {
            // Explorer can repeat TaskbarCreated while the icon still exists.
            return Shell_NotifyIconW(NIM_MODIFY, &data) != 0;
        }
        data.Anonymous.uVersion = NOTIFYICON_VERSION_4;
        Shell_NotifyIconW(NIM_SETVERSION, &data);
    }
    true
}

pub(super) fn set_tip(window: HWND, tip: &str) {
    unsafe { Shell_NotifyIconW(NIM_MODIFY, &data(window, tip)) };
}

pub(super) fn remove(window: HWND) {
    unsafe { Shell_NotifyIconW(NIM_DELETE, &data(window, "")) };
}

/// Brings the panel back from the tray or from behind other windows.
pub(super) fn show_panel(window: HWND) {
    unsafe {
        ShowWindow(
            window,
            if IsIconic(window) != 0 {
                SW_RESTORE
            } else {
                SW_SHOW
            },
        );
        // An open dialog comes forward instead of the panel it blocks.
        SetForegroundWindow(GetLastActivePopup(window));
    }
}

/// Handles `WM_TRAY`, the icon's callback message.
pub(super) fn notify(window: HWND, wp: WPARAM, lp: LPARAM) {
    match (lp & 0xFFFF) as u32 {
        NIN_SELECT | NIN_KEYSELECT | WM_LBUTTONDBLCLK => show_panel(window),
        WM_CONTEXTMENU => menu(window, point_from(wp as LPARAM)),
        _ => {}
    }
}

fn menu(window: HWND, (x, y): (i32, i32)) {
    if unsafe { IsWindowEnabled(window) } == 0 {
        show_panel(window);
        return;
    }
    let Some((running, busy)) = with_app(|app| (app.running.is_some(), app.control_busy)) else {
        return;
    };
    let menu = unsafe { CreatePopupMenu() };
    append(menu, MF_STRING, CMD_SHOW, "Show Window");
    append(
        menu,
        if busy { MF_GRAYED } else { MF_STRING },
        CMD_START_STOP,
        if running {
            "Stop driver"
        } else {
            "Start driver"
        },
    );
    unsafe { AppendMenuW(menu, MF_SEPARATOR, 0, ptr::null()) };
    append(menu, MF_STRING, CMD_QUIT, "Close");
    let align = if unsafe { GetSystemMetrics(SM_MENUDROPALIGNMENT) } != 0 {
        TPM_RIGHTALIGN
    } else {
        TPM_LEFTALIGN
    };
    let command = unsafe {
        SetMenuDefaultItem(menu, u32::from(CMD_SHOW), 0);
        // The menu only closes on an outside click if its owner is in front.
        SetForegroundWindow(window);
        let command = TrackPopupMenuEx(
            menu,
            align | TPM_BOTTOMALIGN | TPM_RIGHTBUTTON | TPM_RETURNCMD,
            x,
            y,
            window,
            ptr::null(),
        );
        DestroyMenu(menu);
        PostMessageW(window, WM_NULL, 0, 0);
        command
    };
    if command != 0 {
        on_command(window, command as u16, 0, ptr::null_mut());
    }
}
