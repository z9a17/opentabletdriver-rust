//! Native Windows profile editor and plugin manager. Its message loop stays off
//! the driver thread; stop requests use a duplicated Windows event handle.
use crate::config::Profile;
use crate::hid::Event;
use crate::plugins::{PluginConfig, PluginKind};
use std::cell::RefCell;
use std::ffi::OsString;
use std::io::Write;
use std::mem::size_of;
use std::os::windows::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::mpsc::{self, Receiver};
use std::thread::JoinHandle;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, COLOR_BTNFACE, CreateFontW, DEFAULT_CHARSET,
    DEFAULT_PITCH, DeleteObject, HFONT, OUT_DEFAULT_PRECIS,
};
use windows_sys::Win32::Storage::FileSystem::{
    MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Controls::Dialogs::*;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::EnableWindow;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

const OPEN: usize = 10;
const SAVE: usize = 11;
const IMPORT: usize = 12;
const ABSOLUTE: usize = 13;
const RELATIVE: usize = 14;
const VALIDATE: usize = 15;
const ADD_NET: usize = 20;
const ADD_NATIVE: usize = 21;
const TOGGLE: usize = 22;
const REMOVE: usize = 23;
const APPLY: usize = 24;
const START: usize = 30;
const STOP: usize = 31;
const PLUGINS: usize = 40;
const EDITOR: usize = 41;

thread_local! { static UI: RefCell<Option<Ui>> = const { RefCell::new(None) }; }

fn w(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}
fn set_text(hwnd: HWND, text: &str) {
    unsafe { SetWindowTextW(hwnd, w(text).as_ptr()) };
}
fn text(hwnd: HWND) -> String {
    let count = unsafe { GetWindowTextLengthW(hwnd) }.max(0) as usize;
    let mut buffer = vec![0; count + 1];
    let read =
        unsafe { GetWindowTextW(hwnd, buffer.as_mut_ptr(), buffer.len() as i32) }.max(0) as usize;
    String::from_utf16_lossy(&buffer[..read])
}

struct Running {
    stop: Event,
    thread: JoinHandle<Result<(), String>>,
    messages: Receiver<String>,
}
struct Ui {
    window: HWND,
    path: HWND,
    editor: HWND,
    plugins: HWND,
    settings: HWND,
    status: HWND,
    log: HWND,
    start: HWND,
    stop: HWND,
    font: HFONT,
    code_font: HFONT,
    running: Option<Running>,
    closing: bool,
    dirty: bool,
    history: String,
}

impl Drop for Ui {
    fn drop(&mut self) {
        // Normal close waits asynchronously for this thread. This also covers
        // a message-loop failure without force-killing input or losing releases.
        if let Some(running) = self.running.take() {
            let _ = running.stop.signal();
            let _ = running.thread.join();
        }
        unsafe {
            DeleteObject(self.font);
            DeleteObject(self.code_font);
        }
    }
}

impl Ui {
    fn control(
        &self,
        class: &str,
        caption: &str,
        id: usize,
        style: u32,
        rect: [i32; 4],
    ) -> Result<HWND, String> {
        let hwnd = unsafe {
            CreateWindowExW(
                0,
                w(class).as_ptr(),
                w(caption).as_ptr(),
                WS_CHILD | WS_VISIBLE | style,
                rect[0],
                rect[1],
                rect[2],
                rect[3],
                self.window,
                id as _,
                GetModuleHandleW(ptr::null()),
                ptr::null(),
            )
        };
        if hwnd.is_null() {
            return Err(format!(
                "cannot create UI control: {}",
                std::io::Error::last_os_error()
            ));
        }
        unsafe {
            SendMessageW(hwnd, WM_SETFONT, self.font as usize, 1);
        }
        Ok(hwnd)
    }

    fn create(window: HWND) -> Result<Self, String> {
        let font = unsafe {
            CreateFontW(
                -16,
                0,
                0,
                0,
                400,
                0,
                0,
                0,
                DEFAULT_CHARSET.into(),
                OUT_DEFAULT_PRECIS.into(),
                CLIP_DEFAULT_PRECIS.into(),
                CLEARTYPE_QUALITY.into(),
                DEFAULT_PITCH.into(),
                w("Segoe UI").as_ptr(),
            )
        };
        let code_font = unsafe {
            CreateFontW(
                -15,
                0,
                0,
                0,
                400,
                0,
                0,
                0,
                DEFAULT_CHARSET.into(),
                OUT_DEFAULT_PRECIS.into(),
                CLIP_DEFAULT_PRECIS.into(),
                CLEARTYPE_QUALITY.into(),
                DEFAULT_PITCH.into(),
                w("Consolas").as_ptr(),
            )
        };
        let mut ui = Self {
            window,
            path: ptr::null_mut(),
            editor: ptr::null_mut(),
            plugins: ptr::null_mut(),
            settings: ptr::null_mut(),
            status: ptr::null_mut(),
            log: ptr::null_mut(),
            start: ptr::null_mut(),
            stop: ptr::null_mut(),
            font,
            code_font,
            running: None,
            closing: false,
            dirty: false,
            history: String::new(),
        };
        ui.control(
            "STATIC",
            "OpenTabletDriver Rust  /  PTH-660",
            0,
            0,
            [20, 14, 650, 26],
        )?;
        ui.control("STATIC", "Profile file", 0, 0, [20, 48, 80, 24])?;
        ui.path = ui.control(
            "EDIT",
            "",
            0,
            WS_BORDER | WS_TABSTOP | ES_AUTOHSCROLL as u32,
            [106, 45, 766, 28],
        )?;
        ui.control("BUTTON", "Open…", OPEN, WS_TABSTOP, [884, 45, 120, 28])?;
        for (caption, id, x, width) in [
            ("Save profile", SAVE, 20, 130),
            ("Import active OTD", IMPORT, 160, 165),
            ("Absolute template", ABSOLUTE, 335, 165),
            ("Relative template", RELATIVE, 510, 165),
            ("Validate", VALIDATE, 685, 115),
        ] {
            ui.control("BUTTON", caption, id, WS_TABSTOP, [x, 86, width, 32])?;
        }
        ui.control(
            "STATIC",
            "Profile settings (TOML)",
            0,
            0,
            [20, 132, 600, 24],
        )?;
        ui.control(
            "STATIC",
            "Plugins — only enable DLLs you trust",
            0,
            0,
            [642, 132, 362, 24],
        )?;
        ui.editor = ui.control(
            "EDIT",
            "",
            EDITOR,
            WS_BORDER
                | WS_TABSTOP
                | WS_VSCROLL
                | WS_HSCROLL
                | (ES_MULTILINE | ES_AUTOVSCROLL | ES_AUTOHSCROLL | ES_WANTRETURN) as u32,
            [20, 158, 602, 388],
        )?;
        unsafe {
            SendMessageW(ui.editor, WM_SETFONT, code_font as usize, 1);
            SendMessageW(ui.editor, 0x00c5, 262_144, 0);
        }
        ui.plugins = ui.control(
            "LISTBOX",
            "",
            PLUGINS,
            WS_BORDER | WS_TABSTOP | WS_VSCROLL | LBS_NOTIFY as u32,
            [642, 158, 362, 148],
        )?;
        ui.control(
            "BUTTON",
            "Add .NET DLL…",
            ADD_NET,
            WS_TABSTOP,
            [642, 316, 175, 30],
        )?;
        ui.control(
            "BUTTON",
            "Add native DLL…",
            ADD_NATIVE,
            WS_TABSTOP,
            [829, 316, 175, 30],
        )?;
        ui.control(
            "BUTTON",
            "Enable / disable",
            TOGGLE,
            WS_TABSTOP,
            [642, 354, 175, 30],
        )?;
        ui.control(
            "BUTTON",
            "Remove entry",
            REMOVE,
            WS_TABSTOP,
            [829, 354, 175, 30],
        )?;
        ui.control(
            "STATIC",
            "Selected plugin settings (JSON)",
            0,
            0,
            [642, 398, 362, 24],
        )?;
        ui.settings = ui.control(
            "EDIT",
            "{}",
            0,
            WS_BORDER
                | WS_TABSTOP
                | WS_VSCROLL
                | (ES_MULTILINE | ES_AUTOVSCROLL | ES_WANTRETURN) as u32,
            [642, 424, 362, 82],
        )?;
        unsafe {
            SendMessageW(ui.settings, WM_SETFONT, code_font as usize, 1);
            SendMessageW(ui.settings, 0x00c5, 65_536, 0);
        }
        ui.control(
            "BUTTON",
            "Apply plugin settings",
            APPLY,
            WS_TABSTOP,
            [642, 514, 362, 32],
        )?;
        ui.control(
            "STATIC",
            ".NET: synchronous tablet-coordinate filters. Changes take effect on the next Start.",
            0,
            0,
            [20, 558, 984, 24],
        )?;
        ui.start = ui.control(
            "BUTTON",
            "Start driver",
            START,
            WS_TABSTOP,
            [20, 594, 155, 36],
        )?;
        ui.stop = ui.control(
            "BUTTON",
            "Stop driver",
            STOP,
            WS_TABSTOP,
            [185, 594, 155, 36],
        )?;
        unsafe {
            EnableWindow(ui.stop, 0);
        }
        ui.status = ui.control(
            "STATIC",
            "Stopped — edit or import a profile, then Start.",
            0,
            0,
            [360, 601, 644, 28],
        )?;
        ui.log = ui.control(
            "EDIT",
            "",
            0,
            WS_BORDER | WS_VSCROLL | (ES_MULTILINE | ES_AUTOVSCROLL | ES_READONLY) as u32,
            [20, 644, 984, 84],
        )?;
        Ok(ui)
    }

    fn log(&mut self, message: &str) {
        set_text(self.status, message);
        self.history.push_str(message);
        self.history.push_str("\r\n");
        if self.history.len() > 16_384 {
            let boundary = self
                .history
                .char_indices()
                .find(|(at, _)| *at >= 8_192)
                .map(|(at, _)| at)
                .unwrap_or(0);
            self.history.drain(..boundary);
        }
        set_text(self.log, &self.history);
        unsafe {
            SendMessageW(self.log, 0x00b1, usize::MAX, -1);
            SendMessageW(self.log, 0x00b7, 0, 0);
        }
    }

    fn profile(&self) -> Result<Profile, String> {
        let profile = Profile::from_toml_text(&text(self.editor), Path::new(&text(self.path)))?;
        if profile.relative.is_none() {
            crate::display::DisplaySnapshot::read()?.mapper(&profile)?;
        }
        Ok(profile)
    }

    fn replace_profile(&mut self, profile: &Profile) -> Result<(), String> {
        set_text(self.editor, &profile.to_toml()?.replace('\n', "\r\n"));
        self.refresh_plugins(profile);
        Ok(())
    }

    fn refresh_plugins(&self, profile: &Profile) {
        unsafe {
            SendMessageW(self.plugins, LB_RESETCONTENT, 0, 0);
        }
        for plugin in &profile.plugins {
            let label = if plugin.kind == PluginKind::Dotnet {
                plugin.type_name.clone()
            } else {
                plugin
                    .path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned()
            };
            let label = format!("[{}] {}", if plugin.enabled { "on" } else { "off" }, label);
            unsafe {
                SendMessageW(self.plugins, LB_ADDSTRING, 0, w(&label).as_ptr() as isize);
            }
        }
        set_text(self.settings, "{}");
    }

    fn selected(&self) -> Result<usize, String> {
        let selected = unsafe { SendMessageW(self.plugins, LB_GETCURSEL, 0, 0) };
        if selected < 0 {
            Err("Select a plugin first.".into())
        } else {
            Ok(selected as usize)
        }
    }

    fn can_replace(&self) -> bool {
        !self.dirty
            || unsafe {
                MessageBoxW(
                    self.window,
                    w("Discard unsaved profile edits?").as_ptr(),
                    w("Unsaved profile").as_ptr(),
                    MB_YESNO | MB_ICONQUESTION,
                )
            } == IDYES
    }

    fn action(&mut self, id: usize) -> Result<(), String> {
        match id {
            OPEN => {
                if !self.can_replace() {
                    return Ok(());
                }
                if let Some(path) = file_dialog(self.window, false, false)? {
                    let profile = Profile::load(Some(&path))?;
                    set_text(self.path, &path.to_string_lossy());
                    self.replace_profile(&profile)?;
                    self.dirty = false;
                    self.log("Profile opened. Start applies its settings.");
                }
            }
            SAVE => {
                let profile = self.profile()?;
                let path = PathBuf::from(text(self.path));
                let path = if path.as_os_str().is_empty() {
                    file_dialog(self.window, true, false)?
                } else {
                    Some(path)
                };
                if let Some(path) = path {
                    save_profile(&path, &profile)?;
                    set_text(self.path, &path.to_string_lossy());
                    self.dirty = false;
                    self.log(
                        "Profile saved. Running output keeps its current settings until restarted.",
                    );
                }
            }
            IMPORT | ABSOLUTE | RELATIVE => {
                if !self.can_replace() {
                    return Ok(());
                }
                let profile = if id == IMPORT {
                    Profile::load(None)?
                } else if id == RELATIVE {
                    Profile::from_toml_text(
                        include_str!("../driver.relative.example.toml"),
                        Path::new(&text(self.path)),
                    )?
                } else {
                    Profile::default()
                };
                self.replace_profile(&profile)?;
                self.dirty = true;
                self.log(if id == IMPORT { "Imported mapping and built-in filters. Add unchanged .NET DLLs with Add .NET DLL; existing OTD files were not modified." } else { "Template ready. Edit settings and save or start." });
            }
            VALIDATE => {
                let profile = self.profile()?;
                self.refresh_plugins(&profile);
                self.log("Profile syntax and mapping are valid. DLL compatibility is checked when starting.");
            }
            ADD_NET | ADD_NATIVE => {
                let mut profile = self.profile()?;
                if let Some(path) = file_dialog(self.window, false, true)? {
                    if id == ADD_NET {
                        let entries = crate::dotnet::inspect(&path)?;
                        if entries.is_empty() {
                            return Err(
                                "This assembly exports no OpenTabletDriver position filters."
                                    .into(),
                            );
                        }
                        profile.plugins.extend(entries);
                    } else {
                        profile.plugins.push(PluginConfig {
                            path,
                            kind: PluginKind::Native,
                            enabled: false,
                            type_name: String::new(),
                            settings_json: "{}".into(),
                        });
                    }
                    if profile.plugins.len() > 32 {
                        return Err("At most 32 plugin entries are supported.".into());
                    }
                    self.replace_profile(&profile)?;
                    self.dirty = true;
                    self.log("Plugin entries added disabled. Select a filter, edit its settings, then enable it.");
                }
            }
            TOGGLE | REMOVE | APPLY => {
                let index = self.selected()?;
                let mut profile = self.profile()?;
                let plugin = profile
                    .plugins
                    .get_mut(index)
                    .ok_or("Plugin list changed; click Validate and select again.")?;
                if id == TOGGLE {
                    plugin.enabled = !plugin.enabled;
                }
                if id == APPLY {
                    plugin.settings_json = text(self.settings);
                    plugin.validate()?;
                }
                if id == REMOVE {
                    profile.plugins.remove(index);
                }
                self.replace_profile(&profile)?;
                self.dirty = true;
                self.log("Plugin profile updated. Restart the driver to apply changes.");
            }
            START => {
                if self.running.is_some() {
                    return Ok(());
                }
                let profile = self.profile()?;
                let stop = Event::create(true).map_err(|e| e.to_string())?;
                let worker_stop = stop.duplicate().map_err(|e| e.to_string())?;
                let (sender, messages) = mpsc::sync_channel(32);
                let thread = std::thread::Builder::new()
                    .name("tablet-driver".into())
                    .spawn(move || {
                        crate::drive(profile, &worker_stop, None, |message| {
                            let _ = sender.try_send(message.to_owned());
                        })
                    })
                    .map_err(|e| e.to_string())?;
                self.running = Some(Running {
                    stop,
                    thread,
                    messages,
                });
                unsafe {
                    SetTimer(self.window, 1, 200, None);
                    EnableWindow(self.start, 0);
                    EnableWindow(self.stop, 1);
                }
                self.log("Starting driver with the editor's settings…");
            }
            STOP => {
                if let Some(running) = &self.running {
                    running.stop.signal().map_err(|e| e.to_string())?;
                }
                self.log("Stopping; waiting for input cleanup and original driver restoration…");
            }
            _ => {}
        }
        Ok(())
    }

    fn poll(&mut self) {
        let mut messages = Vec::new();
        if let Some(running) = &self.running {
            messages.extend(running.messages.try_iter());
        }
        for message in messages {
            self.log(&message);
        }
        if self
            .running
            .as_ref()
            .is_some_and(|r| r.thread.is_finished())
        {
            let running = self.running.take().unwrap();
            unsafe {
                KillTimer(self.window, 1);
            }
            match running.thread.join() {
                Ok(Ok(())) => self.log("Driver stopped."),
                Ok(Err(error)) => self.log(&format!("Driver stopped: {error}")),
                Err(_) => self.log("Driver thread failed."),
            }
            unsafe {
                EnableWindow(self.start, 1);
                EnableWindow(self.stop, 0);
            }
        }
        if self.closing && self.running.is_none() {
            unsafe {
                DestroyWindow(self.window);
            }
        }
    }
}

fn file_dialog(window: HWND, save: bool, dll: bool) -> Result<Option<PathBuf>, String> {
    let mut buffer = vec![0u16; 32_768];
    let filter = if dll {
        w("Plugin DLL\0*.dll\0\0")
    } else {
        w("Rust profile\0*.toml\0All files\0*.*\0\0")
    };
    let extension = w(if dll { "dll" } else { "toml" });
    let mut dialog = OPENFILENAMEW {
        lStructSize: size_of::<OPENFILENAMEW>() as u32,
        hwndOwner: window,
        lpstrFilter: filter.as_ptr(),
        lpstrFile: buffer.as_mut_ptr(),
        nMaxFile: buffer.len() as u32,
        lpstrDefExt: extension.as_ptr(),
        Flags: OFN_NOCHANGEDIR
            | OFN_PATHMUSTEXIST
            | if save {
                OFN_OVERWRITEPROMPT
            } else {
                OFN_FILEMUSTEXIST
            },
        ..Default::default()
    };
    let success = unsafe {
        if save {
            GetSaveFileNameW(&mut dialog)
        } else {
            GetOpenFileNameW(&mut dialog)
        }
    };
    if success == 0 {
        let code = unsafe { CommDlgExtendedError() };
        return if code == 0 {
            Ok(None)
        } else {
            Err(format!("file dialog failed (0x{code:x})"))
        };
    }
    let length = buffer
        .iter()
        .position(|c| *c == 0)
        .ok_or("invalid file dialog path")?;
    Ok(Some(OsString::from_wide(&buffer[..length]).into()))
}

pub fn save_profile(path: &Path, profile: &Profile) -> Result<(), String> {
    let path = std::path::absolute(path).map_err(|e| e.to_string())?;
    let parent = path.parent().ok_or("profile path has no parent")?;
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_nanos();
    let temporary = parent.join(format!(".otd-profile-{}-{stamp}.tmp", std::process::id()));
    let contents = profile.to_toml()?;
    let outcome = (|| -> Result<(), String> {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|e| e.to_string())?;
        file.write_all(contents.as_bytes())
            .and_then(|_| file.sync_all())
            .map_err(|e| e.to_string())?;
        drop(file);
        let from = crate::plugins::wide(temporary.as_os_str())?;
        let to = crate::plugins::wide(path.as_os_str())?;
        if unsafe {
            MoveFileExW(
                from.as_ptr(),
                to.as_ptr(),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        } == 0
        {
            return Err(std::io::Error::last_os_error().to_string());
        }
        Ok(())
    })();
    if outcome.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    outcome
}

unsafe extern "system" fn window_proc(
    window: HWND,
    message: u32,
    wp: WPARAM,
    lp: LPARAM,
) -> LRESULT {
    if message == WM_DESTROY {
        unsafe {
            PostQuitMessage(0);
        }
        return 0;
    }
    UI.with(|slot| {
        // SetWindowText and native dialogs can synchronously reenter WndProc.
        // Never create aliased mutable references during such notifications.
        let Ok(mut slot) = slot.try_borrow_mut() else {
            return unsafe { DefWindowProcW(window, message, wp, lp) };
        };
        let Some(ui) = slot.as_mut() else {
            return unsafe { DefWindowProcW(window, message, wp, lp) };
        };
        match message {
            WM_COMMAND => {
                let id = wp & 0xffff;
                let notification = (wp >> 16) as u32;
                if id == EDITOR && notification == EN_CHANGE {
                    ui.dirty = true;
                } else if id == PLUGINS && notification == LBN_SELCHANGE {
                    let result = ui.selected().and_then(|index| {
                        ui.profile().and_then(|p| {
                            p.plugins
                                .get(index)
                                .cloned()
                                .ok_or("Plugin list changed".into())
                        })
                    });
                    match result {
                        Ok(plugin) => set_text(ui.settings, &plugin.settings_json),
                        Err(error) => ui.log(&error),
                    }
                } else if notification == BN_CLICKED
                    && let Err(error) = ui.action(id)
                {
                    ui.log(&error);
                }
                0
            }
            WM_TIMER => {
                ui.poll();
                0
            }
            WM_CLOSE => {
                if ui.closing {
                    return 0;
                }
                if !ui.can_replace() {
                    return 0;
                }
                ui.closing = true;
                if let Err(error) = ui.action(STOP) {
                    ui.log(&error);
                    ui.closing = false;
                }
                ui.poll();
                0
            }
            _ => unsafe { DefWindowProcW(window, message, wp, lp) },
        }
    })
}

pub fn run() -> Result<(), String> {
    let instance = unsafe { GetModuleHandleW(ptr::null()) };
    let class_name = w("OpenTabletDriverRustControlPanel");
    let class = WNDCLASSW {
        lpfnWndProc: Some(window_proc),
        hInstance: instance,
        lpszClassName: class_name.as_ptr(),
        hCursor: unsafe { LoadCursorW(ptr::null_mut(), IDC_ARROW) },
        hbrBackground: (COLOR_BTNFACE + 1) as _,
        ..Default::default()
    };
    if unsafe { RegisterClassW(&class) } == 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    let window = unsafe {
        CreateWindowExW(
            WS_EX_CONTROLPARENT,
            class_name.as_ptr(),
            w("OpenTabletDriver Rust — Control Panel").as_ptr(),
            WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            1040,
            784,
            ptr::null_mut(),
            ptr::null_mut(),
            instance,
            ptr::null(),
        )
    };
    if window.is_null() {
        return Err(std::io::Error::last_os_error().to_string());
    }
    let mut ui = Ui::create(window)?;
    let directory = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or(std::env::current_dir().map_err(|e| e.to_string())?)
        .join("OpenTabletDriverRust");
    let path = directory.join("driver.toml");
    set_text(ui.path, &path.to_string_lossy());
    let profile = if path.exists() {
        Profile::load(Some(&path))
    } else {
        Profile::load(None)
    };
    match profile {
        Ok(profile) => {
            ui.replace_profile(&profile)?;
            ui.log("Ready. Start uses the editor settings; Save writes your Rust profile.");
        }
        Err(error) => {
            ui.replace_profile(&Profile::default())?;
            ui.log(&format!("Could not load profile: {error}"));
        }
    }
    UI.with(|slot| *slot.borrow_mut() = Some(ui));
    unsafe {
        ShowWindow(window, SW_SHOW);
    }
    let mut message = MSG::default();
    loop {
        let result = unsafe { GetMessageW(&mut message, ptr::null_mut(), 0, 0) };
        if result <= 0 {
            UI.with(|slot| drop(slot.borrow_mut().take()));
            return if result < 0 {
                Err(std::io::Error::last_os_error().to_string())
            } else {
                Ok(())
            };
        }
        if unsafe { IsDialogMessageW(window, &message) } == 0 {
            unsafe {
                TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
    }
}
