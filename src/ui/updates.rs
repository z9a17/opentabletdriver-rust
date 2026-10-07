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
pub(super) const WM_OFFER_UPDATE: u32 = WM_APP + 23;
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

#[derive(Default)]
pub(super) struct UpdateState {
    phase: UpdatePhase,
    close_requested: bool,
    installed: Option<String>,
    restart_deferred: bool,
    deferred: Option<Release>,
}

#[derive(Default)]
enum UpdatePhase {
    #[default]
    Idle,
    Checking { manual: bool },
    Prompting,
    Installing,
}

impl UpdateState {
    fn begin_check(&mut self, manual: bool) -> bool {
        match &mut self.phase {
            UpdatePhase::Idle => {
                self.phase = UpdatePhase::Checking { manual };
                true
            }
            UpdatePhase::Checking { manual: requested } => {
                *requested |= manual;
                false
            }
            UpdatePhase::Prompting | UpdatePhase::Installing => false,
        }
    }

    pub(super) fn busy(&self) -> bool {
        !matches!(self.phase, UpdatePhase::Idle)
    }

    fn finish_check(&mut self, manual: bool) -> bool {
        match std::mem::take(&mut self.phase) {
            UpdatePhase::Checking { manual: requested } => manual || requested,
            _ => manual,
        }
    }

    fn begin_install(&mut self) -> bool {
        if self.busy() { return false; }
        self.phase = UpdatePhase::Installing;
        self.deferred = None;
        true
    }

    fn finish_install(&mut self, installed: Option<String>) {
        self.phase = UpdatePhase::Idle;
        if let Some(tag) = installed { self.installed = Some(tag); }
    }

    pub(super) fn blocking(&self) -> bool {
        matches!(self.phase, UpdatePhase::Installing | UpdatePhase::Prompting)
    }

    fn defer_close(&mut self) -> bool {
        if self.blocking() {
            self.close_requested = true;
            true
        } else {
            false
        }
    }
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
    let start = with_app(|app| {
        if app.closing || app.update_restart_pending {
            return false;
        }
        app.updates.begin_check(manual)
    }).unwrap_or(false);
    if !start { return; }
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
    let start = with_app(|app| {
        if app.closing || app.update_restart_pending {
            return false;
        }
        app.updates.begin_install()
    }).unwrap_or(false);
    if !start { return; }
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

/// Whether the user is looking at the panel or one of its windows.
fn in_front(window: HWND) -> bool {
    unsafe {
        let foreground = GetForegroundWindow();
        IsWindowVisible(window) != 0
            && IsIconic(window) == 0
            && !foreground.is_null()
            && (foreground == window || GetAncestor(foreground, GA_ROOTOWNER) == window)
    }
}

/// Offers a deferred update; called when the panel becomes active.
pub(super) fn offer_deferred(window: HWND) {
    if !in_front(window) { return; }
    let installed = with_app(|app| {
        if app.closing || app.update_restart_pending || app.updates.busy() {
            return None;
        }
        if std::mem::take(&mut app.updates.restart_deferred) {
            app.updates.installed.clone()
        } else {
            None
        }
    }).flatten();
    if let Some(tag) = installed {
        offer_restart(window, &tag);
        return;
    }
    let release = with_app(|app| {
        if app.closing || app.update_restart_pending || app.updates.busy() {
            return None;
        }
        app.updates.deferred.take()
    }).flatten();
    if let Some(release) = release {
        checked(window, false, Ok(release));
    }
}

/// A normal close waits for file replacement and rollback to finish. The
/// completion posts WM_CLOSE again, leaving daemon cleanup to its usual path.
pub(super) fn defer_close() -> bool {
    with_app(|app| {
        if !app.updates.defer_close() { return false; }
        app.show_status("Waiting for the update before closing…".into(), Level::Info, false);
        true
    }).unwrap_or(false)
}

fn resume_close(window: HWND) -> bool {
    let requested = with_app(|app| std::mem::take(&mut app.updates.close_requested)).unwrap_or(false);
    if requested {
        unsafe { PostMessageW(window, WM_CLOSE, 0, 0); }
    }
    requested
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
                let manual = with_app(|app| app.updates.finish_check(manual)).unwrap_or(manual);
                if !RESTARTING.load(Ordering::Acquire) { checked(window, manual, result); }
            }
            Event::Progress(line) => log(Level::Info, line),
            Event::Installed(Ok(tag)) => {
                with_app(|app| {
                    app.updates.finish_install(Some(tag.clone()));
                });
                if resume_close(window) { continue; }
                if RESTARTING.load(Ordering::Acquire) {
                    log(Level::Info, format!("{tag} is installed; a restart is already in progress."));
                    continue;
                }
                if in_front(window) {
                    offer_restart(window, &tag);
                } else {
                    let message = format!("{tag} is installed. Open the panel to restart it.");
                    log(Level::Info, message.clone());
                    if with_app(|app| app.in_tray).unwrap_or(false) {
                        tray::balloon(window, "Update installed", &message);
                    }
                    with_app(|app| app.updates.restart_deferred = true);
                }
            }
            Event::Installed(Err(error)) => {
                with_app(|app| app.updates.finish_install(None));
                log(Level::Error, format!("Update failed: {error}"));
                if resume_close(window) { continue; }
                if !RESTARTING.load(Ordering::Acquire) { prompt(
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
    if with_app(|app| app.closing || app.update_restart_pending
        || app.updates.busy()).unwrap_or(true) {
        return;
    }
    let current = env!("CARGO_PKG_VERSION");
    match result {
        Ok(release) if release.version > update::current_version() => {
            if !manual && !in_front(window) {
                let message = format!(
                    "{} is available. Open the panel to install it.",
                    release.tag
                );
                // Announce each release once, not every time it is deferred.
                let announce = with_app(|app| app.updates.deferred.as_ref()
                    .is_none_or(|known| known.tag != release.tag)).unwrap_or(false);
                if announce {
                    log(Level::Info, message.clone());
                    if with_app(|app| app.in_tray).unwrap_or(false) {
                        tray::balloon(window, "Update available", &message);
                    }
                }
                with_app(|app| app.updates.deferred = Some(release));
                return;
            }
            with_app(|app| app.updates.deferred = None);
            if with_app(|app| app.updates.installed.as_deref() == Some(release.tag.as_str())).unwrap_or(false) {
                offer_restart(window, &release.tag);
                return;
            }
            let answer = prompt(
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
            with_app(|app| app.updates.deferred = None);
            if manual {
                prompt(
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
                prompt(
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

fn offer_restart(window: HWND, tag: &str) {
    with_app(|app| app.updates.restart_deferred = false);
    let answer = prompt(
        window,
        &format!("{tag} is installed. Restart the driver and the panel now to use it?\n\nRestarting stops pen input for a moment."),
        "Update installed",
        MB_YESNO | MB_ICONQUESTION,
    );
    if answer == IDYES { restart(window); }
}

fn prompt(window: HWND, text: &str, caption: &str, flags: MESSAGEBOX_STYLE) -> MESSAGEBOX_RESULT {
    with_app(|app| app.updates.phase = UpdatePhase::Prompting);
    let answer = message_box(window, text, caption, flags);
    with_app(|app| app.updates.phase = UpdatePhase::Idle);
    if resume_close(window) { IDNO } else { answer }
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
    debugger::close();
    if debugger::is_open() {
        log(Level::Info, "Finishing debugger recording before restarting...".into());
        unsafe { SetTimer(window, 0xD07, 33, None); }
        return;
    }
    resume_restart(window);
}

pub(super) fn resume_restart(window: HWND) {
    if !RESTARTING.load(Ordering::Acquire) { return; }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manual_update_check_shares_the_startup_request_and_receives_its_result() {
        let mut updates = UpdateState::default();
        assert!(updates.begin_check(false));
        assert!(!updates.begin_check(true));
        assert!(!updates.begin_check(false));
        assert!(updates.finish_check(false));
        assert!(updates.begin_check(false));
        assert!(!updates.finish_check(false));
    }

    #[test]
    fn update_installation_blocks_duplicate_work_and_preserves_a_pending_close() {
        let mut updates = UpdateState::default();
        assert!(updates.begin_install());
        assert!(!updates.begin_install());
        assert!(!updates.begin_check(true));
        assert!(updates.defer_close());
        updates.finish_install(Some("v0.15.4".into()));
        assert!(!updates.busy());
        assert_eq!(updates.installed.as_deref(), Some("v0.15.4"));
        assert!(std::mem::take(&mut updates.close_requested));
        assert!(!updates.close_requested);
    }

    #[test]
    fn failed_update_installation_allows_retry_after_a_pending_close() {
        let mut updates = UpdateState::default();
        assert!(updates.begin_install());
        assert!(updates.defer_close());
        updates.finish_install(None);
        assert!(updates.close_requested);
        assert!(updates.installed.is_none());
        assert!(updates.begin_check(true));
        assert!(!updates.defer_close());
        assert!(updates.finish_check(false));
    }
}
