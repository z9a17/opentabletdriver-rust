//! Several tablets at once (D05, first slice). The primary tablet keeps the
//! existing worker with its prepare/activate protocol; every other connected
//! tablet with a supported parser gets a companion session on its own thread
//! while its driver generation runs, including primary reconnects. A companion
//! uses the profile when the profile
//! names no tablet, otherwise that tablet's OpenTabletDriver profile, or the
//! full area of the tablet when there is none. OpenTabletDriver runs every
//! detected tablet the same way, each with its own profile.

use std::collections::BTreeMap;
use std::sync::{Arc, mpsc};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;

use crate::config::Profile;
use crate::hid::{self, Event, Notification};
use crate::plugins::PluginChain;
use crate::session::{self, Mode};
use otd_core::tablets::Database;

/// One active generation. The primary path is reserved before this is created.
pub struct Companions {
    stop: Arc<AtomicBool>,
    wake: Event,
    thread: Option<JoinHandle<Result<(), String>>>,
    reservations: mpsc::Sender<(String, String, mpsc::SyncSender<Result<(), String>>)>,
    primary: String,
    completed: Arc<AtomicBool>,
}

impl Companions {
    pub fn start(
        profile: Profile,
        database: Database,
        primary: String,
        primary_name: String,
        primary_interrupt: &Event,
        log: impl Fn(&str) + Send + Sync + 'static,
    ) -> Result<Self, String> {
        let stop = Arc::new(AtomicBool::new(false));
        let wake = Event::create(true).map_err(|error| error.to_string())?;
        let thread_wake = wake.duplicate().map_err(|error| error.to_string())?;
        let interrupt = primary_interrupt
            .duplicate()
            .map_err(|error| error.to_string())?;
        let thread_stop = Arc::clone(&stop);
        let log = Arc::new(log);
        let completed = Arc::new(AtomicBool::new(false));
        let thread_completed = Arc::clone(&completed);
        let (reservations, requests) = mpsc::channel();
        let reserved = primary.clone();
        let thread = std::thread::Builder::new()
            .name("tablet-companions".into())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    supervise(profile, database, reserved, primary_name, thread_stop, thread_wake, requests, log)
                })).unwrap_or_else(|_| Err("companion supervisor panicked".into()));
                thread_completed.store(true, Ordering::Release);
                // Optional discovery can end harmlessly. Only a failure that
                // threatens cleanup interrupts the primary report worker.
                if result.is_err() { let _ = interrupt.signal(); }
                result
            })
            .map_err(|error| error.to_string())?;
        Ok(Self {
            stop,
            wake,
            thread: Some(thread),
            reservations,
            primary,
            completed,
        })
    }

    /// Drain a discovered endpoint before the primary opens it. The previous
    /// primary configuration remains reserved while its path is absent.
    pub fn reserve_primary(&mut self, path: &str, name: &str) -> Result<(), String> {
        self.check_finished()?;
        if self.thread.is_none() || self.primary.eq_ignore_ascii_case(path) { return Ok(()); }
        let (reply, response) = mpsc::sync_channel(1);
        if self.reservations.send((path.to_owned(), name.to_owned(), reply)).is_err() {
            return self.finish();
        }
        self.wake.signal().map_err(|error| error.to_string())?;
        match response.recv() {
            Ok(result) => result?,
            // Discovery may have stopped normally after the check above.
            // Joining still preserves any cleanup error from that exit.
            Err(_) => return self.finish(),
        }
        self.primary = path.to_owned();
        Ok(())
    }

    pub fn check_finished(&mut self) -> Result<(), String> {
        if self.completed.load(Ordering::Acquire) {
            return self.finish();
        }
        Ok(())
    }

    /// Returns only after every companion has stopped and reported cleanup.
    pub fn finish(&mut self) -> Result<(), String> {
        if self.thread.is_none() { return Ok(()); }
        self.stop.store(true, Ordering::Release);
        let signalled = self.wake.signal().map_err(|error| error.to_string());
        let joined = self.thread.take().map_or(Ok(()), |thread| {
            thread
                .join()
                .unwrap_or_else(|_| Err("companion supervisor panicked".into()))
        });
        joined.and(signalled)
    }
}
impl Drop for Companions {
    fn drop(&mut self) {
        if let Err(error) = self.finish() {
            eprintln!("Companion cleanup failed: {error}");
        }
    }
}

struct Running {
    stop: Event,
    thread: JoinHandle<std::io::Result<()>>,
    /// Set when the session returns, before the thread has fully exited.
    done: Arc<AtomicBool>,
}

/// How long discovery sleeps without a device change or a companion ending.
/// The rescan after it repairs a missed notification.
const RESCAN: std::time::Duration = std::time::Duration::from_secs(60);

fn supervise(
    profile: Profile,
    database: Database,
    mut primary: String,
    mut primary_name: String,
    stop: Arc<AtomicBool>,
    wake: Event,
    requests: mpsc::Receiver<(String, String, mpsc::SyncSender<Result<(), String>>)>,
    log: Arc<dyn Fn(&str) + Send + Sync>,
) -> Result<(), String> {
    let notification = match Notification::register() {
        Ok(notification) => notification,
        Err(error) => {
            log(&format!("Companion tablets are unavailable: {error}"));
            return Ok(());
        }
    };
    let named = profile.tablet_name().ok().flatten();
    let mut running: BTreeMap<String, Running> = BTreeMap::new();
    let mut known = Vec::new();
    let mut rescan = true;
    let mut scanned = std::time::Instant::now();
    let mut rejected = std::collections::BTreeSet::new();
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        while !stop.load(Ordering::Acquire) {
            // Reset before draining requests so a later reservation cannot lose
            // its wake between queue consumption and the next wait.
            if unsafe { windows_sys::Win32::System::Threading::ResetEvent(wake.raw()) } == 0 {
                return Err(std::io::Error::last_os_error().to_string());
            }
            // A stop signalled between the loop test and ResetEvent must not
            // lose its wake and wait for the discovery timeout.
            if stop.load(Ordering::Acquire) { break; }
            for (path, name, reply) in requests.try_iter() {
                primary = path;
                primary_name = name;
                let key = running.keys().find(|path| path.eq_ignore_ascii_case(&primary)).cloned();
                let result = if let Some(key) = key {
                    let mut reserved = BTreeMap::new();
                    if let Some(session) = running.remove(&key) { reserved.insert(key, session); }
                    stop_all(&mut reserved)
                } else { Ok(()) };
                let _ = reply.send(result.clone());
                result?;
            }
            let finished: Vec<_> = running
                .iter()
                .filter(|(_, session)| {
                    session.done.load(Ordering::Acquire) || session.thread.is_finished()
                })
                .map(|(path, _)| path.clone())
                .collect();
            for path in finished {
                if let Some(session) = running.remove(&path) {
                    match session.thread.join() {
                        Ok(Err(error)) if otd_core::session::is_cleanup_failure(&error) => {
                            return Err(error.to_string());
                        }
                        Ok(Err(error)) => log(&format!("Companion disconnected: {error}")),
                        Err(_) => return Err("companion report worker panicked".into()),
                        Ok(Ok(())) => {}
                    }
                }
            }
            // PnP changes drive discovery. A slow fallback repairs missed notifications.
            if rescan || scanned.elapsed() >= RESCAN {
                known = candidates(&database);
                scanned = std::time::Instant::now();
                rejected.clear();
            }
            // Reserve the configuration across changed-path reconnects. While
            // the primary is absent this also defers new identical-model
            // companions; already-running companions retain their ownership.
            let primary_present = known.iter().any(|(path, _)| path.eq_ignore_ascii_case(&primary));
            for (path, name) in &known {
                if stop.load(Ordering::Acquire) {
                    break;
                }
                if running.contains_key(path) || rejected.contains(path) || path.eq_ignore_ascii_case(&primary)
                    || (!primary_present && name == &primary_name) {
                    continue;
                }
                let profile = match companion_profile(&profile, named.as_deref(), name, import_otd) {
                    Ok(profile) => profile,
                    Err(error) => {
                        log(&format!("{name} was not started: {error}"));
                        rejected.insert(path.clone());
                        continue;
                    }
                };
                match spawn(path.clone(), name.clone(), profile, database.clone(), &wake) {
                    Ok(session) => {
                        log(&format!("Also running {name} (a second tablet)."));
                        running.insert(path.clone(), session);
                    }
                    Err(error) => log(&format!("Could not start {name}: {error}")),
                }
            }
            let until_rescan = RESCAN.saturating_sub(scanned.elapsed());
            rescan = match session::companion_wake(&notification, &wake, until_rescan) {
                Ok(changed) => changed,
                Err(error) => {
                    log(&format!("Companion discovery stopped: {error}"));
                    break;
                }
            };
        }
        Ok(())
    })).unwrap_or_else(|_| Err("companion supervision panicked".into()));
    let cleanup = stop_all(&mut running);
    cleanup.and(outcome)
}

fn stop_all(running: &mut BTreeMap<String, Running>) -> Result<(), String> {
    let sessions = std::mem::take(running);
    let mut errors = Vec::new();
    // Signal everyone before waiting for any one session.
    for session in sessions.values() {
        if let Err(error) = session.stop.signal() {
            errors.push(error.to_string());
        }
    }
    for (_, session) in sessions {
        match session.thread.join() {
            Ok(Err(error)) if otd_core::session::is_cleanup_failure(&error) => {
                errors.push(error.to_string())
            }
            Ok(Err(error)) => eprintln!("Companion disconnected during shutdown: {error}"),
            Err(_) => errors.push("companion worker panicked".into()),
            Ok(Ok(())) => {}
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}
/// Every connected pen interface with a supported configuration, by path.
fn candidates(database: &Database) -> Vec<(String, String)> {
    let Ok(devices) = hid::enumerate_with_database(database) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for device in &devices {
        let path = device.path_text();
        if let Ok(Some(selected)) = hid::select_device(&devices, database, Some(&path), None) {
            found.push((path, selected.configuration.name.clone()));
        }
    }
    found
}

/// The profile a companion tablet runs with: the shared profile when it
/// names no tablet, else `import`'s profile for the tablet, else the
/// tablet's full area.
fn companion_profile(
    profile: &Profile,
    named: Option<&str>,
    tablet: &str,
    import: impl FnOnce(&str) -> Result<Option<Profile>, String>,
) -> Result<Profile, String> {
    if named.is_none() {
        return Ok(profile.clone());
    }
    match import(tablet)? {
        Some(imported) if imported.tablet_name()?.as_deref() == Some(tablet) => Ok(imported),
        Some(_) => Err("saved profile belongs to another tablet".into()),
        None => Ok(Profile {
            target_tablet: Some(tablet.to_owned()),
            tablet: otd_core::config::runtime_tablet(tablet)?,
            source: format!("full-area defaults for {tablet}"),
            ..Profile::default()
        }),
    }
}

/// OpenTabletDriver's profile for a tablet, from its settings file.
fn import_otd(tablet: &str) -> Result<Option<Profile>, String> {
    Profile::load_otd_tablet(tablet)
}

/// Starts a companion session that wakes discovery when it ends.
fn spawn(
    path: String,
    name: String,
    profile: Profile,
    database: Database,
    wake: &Event,
) -> Result<Running, String> {
    let stop = Event::create(true).map_err(|error| error.to_string())?;
    let thread_stop = stop.duplicate().map_err(|error| error.to_string())?;
    let thread_wake = wake.duplicate().map_err(|error| error.to_string())?;
    let done = Arc::new(AtomicBool::new(false));
    let thread_done = Arc::clone(&done);
    let thread = std::thread::Builder::new()
        .name(format!("tablet-{name}"))
        .spawn(move || {
            let result = run_companion(&path, &name, &profile, &database, &thread_stop);
            thread_done.store(true, Ordering::Release);
            let _ = thread_wake.signal();
            result
        })
        .map_err(|error| error.to_string())?;
    Ok(Running { stop, thread, done })
}

fn run_companion(
    path: &str,
    name: &str,
    profile: &Profile,
    database: &Database,
    stop: &Event,
) -> std::io::Result<()> {
    let notification = Notification::register()?;
    let devices = hid::enumerate_with_database(database)?;
    let selected = hid::select_device(&devices, database, Some(path), Some(name))
        .map_err(std::io::Error::other)?
        .ok_or_else(|| std::io::Error::other("the tablet is no longer connected"))?;
    // A companion's own OpenTabletDriver profile has not been through this.
    let mut profile = profile.clone();
    crate::plugin_catalog::use_native_ports(&mut profile, |_| {});
    let profile = &profile;
    let mut plugins = PluginChain::load_with_tablet(&profile.plugins, &selected.configuration)
        .map_err(std::io::Error::other)?;
    plugins
        .validate_output_mode(profile.relative.is_some())
        .map_err(std::io::Error::other)?;
    session::run(
        &selected,
        profile,
        &notification,
        stop,
        Mode::Driver,
        &mut plugins,
        &|_| {},
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shutdown_signals_every_companion_and_preserves_cleanup_errors() {
        use windows_sys::Win32::{
            Foundation::WAIT_OBJECT_0, System::Threading::WaitForMultipleObjects,
        };
        let a = Event::create(true).unwrap();
        let b = Event::create(true).unwrap();
        let mut running = BTreeMap::new();
        let completed = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        for (name, stop, failure) in [
            ("a", a.duplicate().unwrap(), true),
            ("b", b.duplicate().unwrap(), false),
        ] {
            let first = a.duplicate().unwrap();
            let second = b.duplicate().unwrap();
            let completed = completed.clone();
            let thread = std::thread::spawn(move || {
                let handles = [first.raw(), second.raw()];
                assert_eq!(
                    unsafe { WaitForMultipleObjects(2, handles.as_ptr(), 1, 2000) },
                    WAIT_OBJECT_0
                );
                completed.fetch_add(1, Ordering::SeqCst);
                if failure {
                    Err(otd_core::session::cleanup_failure(std::io::Error::other(
                        "unreleased contact",
                    )))
                } else {
                    Ok(())
                }
            });
            running.insert(
                name.into(),
                Running {
                    stop,
                    thread,
                    done: Arc::default(),
                },
            );
        }
        let error = stop_all(&mut running).unwrap_err();
        assert!(error.contains("unreleased contact"), "{error}");
        assert_eq!(completed.load(Ordering::SeqCst), 2);
        assert!(running.is_empty());
    }

    #[test]
    fn companions_use_the_shared_profile_or_their_own() {
        let shared = Profile {
            rotation: 90,
            ..Profile::default()
        };
        let same = companion_profile(&shared, None, "Wacom CTL-4100", |_| unreachable!()).unwrap();
        assert_eq!(same.rotation, 90);
        let named = Profile {
            target_tablet: Some("Wacom PTH-660".into()),
            ..Profile::default()
        };
        let own = companion_profile(&named, Some("Wacom PTH-660"), "Wacom CTL-4100", |_| Ok(None)).unwrap();
        assert_eq!(own.target_tablet.as_deref(), Some("Wacom CTL-4100"));
        assert_eq!(
            own.tablet,
            otd_core::config::spec_for_tablet("Wacom CTL-4100").unwrap()
        );
        assert_eq!(own.rotation, 0);
        // An import for another tablet is not used.
        let wrong = companion_profile(&named, Some("Wacom PTH-660"), "Wacom CTL-4100", |_| {
            Ok(Some(named.clone()))
        });
        assert!(wrong.is_err());
        let disabled = companion_profile(&named, Some("Wacom PTH-660"), "Wacom CTL-4100", |_| {
            Err("output mode is disabled".into())
        });
        assert_eq!(disabled.unwrap_err(), "output mode is disabled");
    }
}
