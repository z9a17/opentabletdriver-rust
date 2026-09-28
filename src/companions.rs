//! Several tablets at once (D05, first slice). The primary tablet keeps the
//! existing worker with its prepare/activate protocol; every other connected
//! tablet with a supported parser gets a companion session on its own thread
//! while the primary runs. A companion uses the profile when the profile
//! names no tablet, otherwise that tablet's OpenTabletDriver profile, or the
//! full area of the tablet when there is none. OpenTabletDriver runs every
//! detected tablet the same way, each with its own profile.

use std::collections::BTreeMap;
use std::sync::Arc;
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
}

impl Companions {
    pub fn start(
        profile: Profile,
        database: Database,
        primary: String,
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
        let thread = std::thread::Builder::new()
            .name("tablet-companions".into())
            .spawn(move || {
                let result = supervise(profile, database, primary, thread_stop, thread_wake, log);
                if result.is_err() {
                    let _ = interrupt.signal();
                }
                result
            })
            .map_err(|error| error.to_string())?;
        Ok(Self {
            stop,
            wake,
            thread: Some(thread),
        })
    }

    /// Returns only after every companion has stopped and reported cleanup.
    pub fn finish(&mut self) -> Result<(), String> {
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
}

fn supervise(
    profile: Profile,
    database: Database,
    primary: String,
    stop: Arc<AtomicBool>,
    wake: Event,
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
    let outcome = (|| {
        while !stop.load(Ordering::Acquire) {
            let finished: Vec<_> = running
                .iter()
                .filter(|(_, session)| session.thread.is_finished())
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
            if rescan || scanned.elapsed() >= std::time::Duration::from_secs(60) {
                known = candidates(&database);
                scanned = std::time::Instant::now();
            }
            for (path, name) in &known {
                if stop.load(Ordering::Acquire) {
                    break;
                }
                if running.contains_key(path) || path.eq_ignore_ascii_case(&primary) {
                    continue;
                }
                let profile = companion_profile(&profile, named.as_deref(), name, import_otd);
                match spawn(path.clone(), name.clone(), profile, database.clone()) {
                    Ok(session) => {
                        log(&format!("Also running {name} (a second tablet)."));
                        running.insert(path.clone(), session);
                    }
                    Err(error) => log(&format!("Could not start {name}: {error}")),
                }
            }
            rescan = match session::companion_wake(&notification, &wake) {
                Ok(changed) => changed,
                Err(error) => {
                    log(&format!("Companion discovery stopped: {error}"));
                    break;
                }
            };
        }
        Ok(())
    })();
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
    import: impl FnOnce(&str) -> Option<Profile>,
) -> Profile {
    if named.is_none() {
        return profile.clone();
    }
    import(tablet)
        .filter(|imported| imported.tablet_name().ok().flatten().as_deref() == Some(tablet))
        .unwrap_or_else(|| Profile {
            target_tablet: Some(tablet.to_owned()),
            tablet: otd_core::config::spec_for_tablet(tablet),
            source: format!("full-area defaults for {tablet}"),
            ..Profile::default()
        })
}

/// OpenTabletDriver's profile for a tablet, from its settings file.
fn import_otd(tablet: &str) -> Option<Profile> {
    Profile::load_connected(None, &[tablet.to_owned()]).ok()
}

fn spawn(
    path: String,
    name: String,
    profile: Profile,
    database: Database,
) -> Result<Running, String> {
    let stop = Event::create(true).map_err(|error| error.to_string())?;
    let thread_stop = stop.duplicate().map_err(|error| error.to_string())?;
    let thread = std::thread::Builder::new()
        .name(format!("tablet-{name}"))
        .spawn(move || run_companion(&path, &name, &profile, &database, &thread_stop))
        .map_err(|error| error.to_string())?;
    Ok(Running { stop, thread })
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
            running.insert(name.into(), Running { stop, thread });
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
        let same = companion_profile(&shared, None, "Wacom CTL-4100", |_| unreachable!());
        assert_eq!(same.rotation, 90);
        let named = Profile {
            target_tablet: Some("Wacom PTH-660".into()),
            ..Profile::default()
        };
        let own = companion_profile(&named, Some("Wacom PTH-660"), "Wacom CTL-4100", |_| None);
        assert_eq!(own.target_tablet.as_deref(), Some("Wacom CTL-4100"));
        assert_eq!(
            own.tablet,
            otd_core::config::spec_for_tablet("Wacom CTL-4100")
        );
        assert_eq!(own.rotation, 0);
        // An import for another tablet is not used.
        let wrong = companion_profile(&named, Some("Wacom PTH-660"), "Wacom CTL-4100", |_| {
            Some(named.clone())
        });
        assert_eq!(wrong.target_tablet.as_deref(), Some("Wacom CTL-4100"));
    }
}
