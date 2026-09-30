//! Help > Check for updates, and the enabled-by-default startup check.
//! Network and file work runs on background threads; results come back to
//! the panel as `WM_UPDATE` messages.
use super::commands::message_box;
use super::*;
use crate::update::{self, Release};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::os::windows::process::CommandExt;

pub(super) const WM_UPDATE: u32 = WM_APP + 21;
/// Tells a restarted panel to wait for this process to exit first.
const WAIT_PID: &str = "OTD_RUST_WAIT_PID";

enum Event {
    Checked {
        manual: bool,
        result: Result<Release, String>,
    },
    Progress(String),
    Installed(Result<String, String>),
    Restarted(Result<(), String>),
}

static EVENTS: Mutex<VecDeque<Event>> = Mutex::new(VecDeque::new());
static RESTARTING: AtomicBool = AtomicBool::new(false);

fn send(window: isize, event: Event) {
    if let Ok(mut events) = EVENTS.lock() {
        events.push_back(event);
    }
    unsafe { PostMessageW(window as HWND, WM_UPDATE, 0, 0) };
}

/// Looks for a newer release and prompts when one is available. Manual
/// checks also report up-to-date and error outcomes in a dialog.
pub(super) fn check(window: HWND, manual: bool) {
    let target = window as isize;
    if let Err(error) = std::thread::Builder::new()
        .name("update-check".into())
        .spawn(move || {
            send(
                target,
                Event::Checked {
                    manual,
                    result: update::latest(),
                },
            )
        }) {
            send(target, Event::Checked { manual, result: Err(format!("Could not start the update check: {error}")) });
        }
}

fn install(window: HWND, release: Release) {
    let target = window as isize;
    if let Err(error) = std::thread::Builder::new()
        .name("update-install".into())
        .spawn(move || {
            let result = std::env::current_exe()
                .map_err(|error| error.to_string())
                .and_then(|exe| {
                    let folder = exe
                        .parent()
                        .ok_or("cannot find the install folder")?
                        .to_path_buf();
                    update::install(&release, &folder, &|line| {
                        send(target, Event::Progress(line.to_owned()))
                    })
                })
                .map(|()| release.tag.clone());
            send(target, Event::Installed(result));
        }) {
            send(target, Event::Installed(Err(format!("Could not start update installation: {error}"))));
        }
}

/// Handles `WM_UPDATE` on the panel's thread.
pub(super) fn on_message(window: HWND) {
    let events: Vec<Event> = EVENTS
        .lock()
        .map(|mut events| events.drain(..).collect())
        .unwrap_or_default();
    for event in events {
        match event {
            Event::Checked { manual, result } => {
                if !RESTARTING.load(Ordering::Acquire) { checked(window, manual, result); }
            }
            Event::Progress(line) => log(Level::Info, line),
            Event::Installed(Ok(tag)) => {
                if RESTARTING.load(Ordering::Acquire) {
                    log(Level::Info, format!("{tag} is installed; a restart is already in progress."));
                    continue;
                }
                let answer = message_box(
                    window,
                    &format!(
                        "{tag} is installed. Restart the driver and the panel now to use it?\n\nRestarting stops pen input for a moment."
                    ),
                    "Update installed",
                    MB_YESNO | MB_ICONQUESTION,
                );
                if answer == IDYES {
                    restart(window);
                }
            }
            Event::Installed(Err(error)) => {
                log(Level::Error, format!("Update failed: {error}"));
                if !RESTARTING.load(Ordering::Acquire) { message_box(
                    window,
                    &format!("The update did not complete.\n\n{error}"),
                    "Update failed",
                    MB_OK | MB_ICONERROR,
                ); }
            }
            Event::Restarted(result) => {
                match result {
                    Ok(()) => {
                        // Unsaved edits were resolved before shutdown and the
                        // editor stayed frozen; do not prompt again after spawn.
                        with_app(|app| app.update_close_approved = true);
                        unsafe { PostMessageW(window, WM_CLOSE, 0, 0); }
                    }
                    Err(error) => {
                        RESTARTING.store(false, Ordering::Release);
                        lock_for_restart(window, false);
                        log(Level::Error, format!("Restart did not complete: {error}"));
                    }
                }
            }
        }
    }
}

fn log(level: Level, message: String) {
    with_app(|app| app.log(level, "Updates", message));
}

fn checked(window: HWND, manual: bool, result: Result<Release, String>) {
    if with_app(|app| app.closing).unwrap_or(true) {
        return;
    }
    let current = env!("CARGO_PKG_VERSION");
    match result {
        Ok(release) if release.version > update::current_version() => {
            if !manual {
                // A launch minimized to the tray still needs a visible owner
                // for the update screen. Current/offline launches stay hidden.
                tray::show_panel(window);
            }
            let answer = message_box(
                window,
                &format!(
                    "{} is available; this is {current}.\n\nDownload and install it now? The driver keeps running until you restart.\n\n{}",
                    release.tag, release.page
                ),
                "Update available",
                MB_YESNO | MB_ICONQUESTION,
            );
            if answer == IDYES {
                log(Level::Info, format!("Installing {}...", release.tag));
                install(window, release);
            }
        }
        Ok(release) => {
            if manual {
                message_box(
                    window,
                    &format!(
                        "OpenTabletDriver Rust {current} is up to date (latest release {}).",
                        release.tag
                    ),
                    "No update",
                    MB_OK | MB_ICONINFORMATION,
                );
            }
        }
        Err(error) => {
            if manual {
                message_box(
                    window,
                    &format!("Could not check for updates.\n\n{error}"),
                    "Update check failed",
                    MB_OK | MB_ICONWARNING,
                );
            } else {
                log(
                    Level::Warning,
                    format!("Could not check for updates: {error}"),
                );
            }
        }
    }
}

/// Stops the running daemon, starts the new panel and closes this one. The
/// new panel starts the new driver if it is set to.
fn restart(window: HWND) {
    if RESTARTING.swap(true, Ordering::AcqRel) { return; }
    if with_app(|app| app.closing || app.control_busy).unwrap_or(true) {
        RESTARTING.store(false, Ordering::Release);
        log(Level::Warning, "Wait for the current daemon request before restarting for the update.".into());
        return;
    }
    if !confirm_discard(window) {
        RESTARTING.store(false, Ordering::Release);
        return;
    }
    lock_for_restart(window, true);
    log(Level::Info, "Waiting for the driver to stop before restarting...".into());
    let target = window as isize;
    if let Err(error) = std::thread::Builder::new().name("update-restart".into()).spawn(move || {
        send(target, Event::Restarted(restart_worker()));
    }) {
        RESTARTING.store(false, Ordering::Release);
        lock_for_restart(window, false);
        log(Level::Error, format!("Could not start restart work: {error}"));
    }
}

fn lock_for_restart(window: HWND, pending: bool) {
    with_app(|app| {
        app.update_restart_pending = pending;
        app.update_close_approved = false;
    });
    unsafe { EnableWindow(window, i32::from(!pending)); }
    plugin_manager::set_restart_pending(pending);
    if !pending {
        // Complete queued discovery/import/diagnostic work after a failed
        // restart; the owner can still inspect and save the unchanged edits.
        unsafe { PostMessageW(window, WM_BACKGROUND, 0, 0); }
    }
}

fn restart_worker() -> Result<(), String> {
    use crate::control::{Command, Reply, Request};
    use windows_sys::Win32::Foundation::WAIT_OBJECT_0;
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject, CREATE_NO_WINDOW};
    let status = crate::control::request(&Request::new(1, Command::Status), Duration::from_secs(5));
    match status {
        Ok(response) => {
            let Reply::Status { status } = response.reply else {
                return Err("The daemon did not return its process identity; the panel was kept open.".into());
            };
            let pid = status.instance.split_once('-').and_then(|(pid, _)| pid.parse::<u32>().ok())
                .ok_or("The daemon process identity is invalid; the panel was kept open.")?;
            let process = crate::hid::OwnedHandle::new(unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) })
                .map_err(|error| format!("Cannot watch the old daemon stop: {error}"))?;
            let response = crate::control::request(&Request::new(1, Command::Shutdown), Duration::from_secs(10))
                .map_err(|error| format!("The daemon shutdown was not acknowledged: {error}. Retry only after checking its state."))?;
            match response.reply {
                Reply::ShutdownAccepted => {}
                Reply::Error { error } => return Err(format!("The daemon refused shutdown: {}", error.message)),
                _ => return Err("The daemon returned an unexpected shutdown response.".into()),
            }
            if unsafe { WaitForSingleObject(process.raw(), 10_000) } != WAIT_OBJECT_0 {
                return Err("The old daemon has not exited; the new panel was not started. Check its state before retrying.".into());
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("Cannot determine whether the old daemon is running: {error}")),
    }
    std::env::current_exe().and_then(|exe| {
        std::process::Command::new(exe)
            .arg("ui")
            .env(WAIT_PID, std::process::id().to_string())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
    }).map(|_| ()).map_err(|error| format!("Could not start the new panel: {error}"))
}

/// In a panel started by `restart`, waits up to ten seconds for the previous
/// panel to exit, so it can take the single-panel lock.
pub(super) fn wait_for_previous_panel() {
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject,
    };
    let Some(pid) = std::env::var(WAIT_PID)
        .ok()
        .and_then(|pid| pid.parse::<u32>().ok())
    else {
        return;
    };
    // SAFETY: the handle is only waited on and closed.
    unsafe {
        let process = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
        if !process.is_null() {
            WaitForSingleObject(process, 10_000);
            windows_sys::Win32::Foundation::CloseHandle(process);
        }
    }
}
