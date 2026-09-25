//! Help > Check for updates, and the optional check when the panel opens.
//! Network and file work runs on background threads; results come back to
//! the panel as `WM_UPDATE` messages.
use super::commands::message_box;
use super::*;
use crate::update::{self, Release};
use std::sync::Mutex;

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
}

static EVENTS: Mutex<VecDeque<Event>> = Mutex::new(VecDeque::new());

fn send(window: isize, event: Event) {
    if let Ok(mut events) = EVENTS.lock() {
        events.push_back(event);
    }
    unsafe { PostMessageW(window as HWND, WM_UPDATE, 0, 0) };
}

/// Looks for a newer release. A manual check reports every outcome; the
/// check when the panel opens only mentions a newer release in the Console.
pub(super) fn check(window: HWND, manual: bool) {
    let target = window as isize;
    let _ = std::thread::Builder::new()
        .name("update-check".into())
        .spawn(move || {
            send(
                target,
                Event::Checked {
                    manual,
                    result: update::latest(),
                },
            )
        });
}

fn install(window: HWND, release: Release) {
    let target = window as isize;
    let _ = std::thread::Builder::new()
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
        });
}

/// Handles `WM_UPDATE` on the panel's thread.
pub(super) fn on_message(window: HWND) {
    let events: Vec<Event> = EVENTS
        .lock()
        .map(|mut events| events.drain(..).collect())
        .unwrap_or_default();
    for event in events {
        match event {
            Event::Checked { manual, result } => checked(window, manual, result),
            Event::Progress(line) => log(Level::Info, line),
            Event::Installed(Ok(tag)) => {
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
                message_box(
                    window,
                    &format!(
                        "The update could not be installed; the current version is unchanged.\n\n{error}"
                    ),
                    "Update failed",
                    MB_OK | MB_ICONERROR,
                );
            }
        }
    }
}

fn log(level: Level, message: String) {
    with_app(|app| app.log(level, "Updates", message));
}

fn checked(window: HWND, manual: bool, result: Result<Release, String>) {
    let current = env!("CARGO_PKG_VERSION");
    match result {
        Ok(release) if release.version > update::current_version() => {
            if !manual {
                log(
                    Level::Info,
                    format!(
                        "{} is available. Help > Check for updates installs it.",
                        release.tag
                    ),
                );
                return;
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
    let _ = crate::control::request(
        &crate::control::Request::new(1, crate::control::Command::Shutdown),
        Duration::from_secs(10),
    );
    let started = std::env::current_exe().and_then(|exe| {
        std::process::Command::new(exe)
            .arg("ui")
            .env(WAIT_PID, std::process::id().to_string())
            .spawn()
    });
    match started {
        Ok(_) => unsafe {
            PostMessageW(window, WM_CLOSE, 0, 0);
        },
        Err(error) => log(
            Level::Error,
            format!("Could not start the new panel: {error}"),
        ),
    }
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
