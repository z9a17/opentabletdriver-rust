//! Several tablets at once (D05, first slice). The primary tablet keeps the
//! existing worker with its prepare/activate protocol; every other connected
//! tablet with a supported parser gets a companion session on its own thread
//! while the primary runs. A companion uses the profile when the profile
//! names no tablet, otherwise that tablet's OpenTabletDriver profile, or the
//! full area of the tablet when there is none. OpenTabletDriver runs every
//! detected tablet the same way, each with its own profile.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use crate::config::Profile;
use crate::hid::{self, Event, Notification};
use crate::plugins::PluginChain;
use crate::session::{self, Mode};
use otd_core::tablets::Database;

/// Runs companion sessions until dropped.
pub struct Companions {
    stop: Arc<AtomicBool>,
    wake: Event,
    thread: Option<JoinHandle<()>>,
}

/// The primary session's device path, which companions leave alone.
pub type Primary = Arc<Mutex<Option<String>>>;

impl Companions {
    /// `active` gates the companions: they run only while it is true.
    pub fn start(
        profile: Profile,
        database: Database,
        primary: Primary,
        active: Arc<AtomicBool>,
        log: impl Fn(&str) + Send + Sync + 'static,
    ) -> Result<Self, String> {
        let stop = Arc::new(AtomicBool::new(false));
        let wake = Event::create(true).map_err(|error| error.to_string())?;
        let thread_wake = wake.duplicate().map_err(|error| error.to_string())?;
        let thread_stop = Arc::clone(&stop);
        let log = Arc::new(log);
        let thread = std::thread::Builder::new()
            .name("tablet-companions".into())
            .spawn(move || {
                supervise(
                    profile,
                    database,
                    primary,
                    active,
                    thread_stop,
                    thread_wake,
                    log,
                )
            })
            .map_err(|error| error.to_string())?;
        Ok(Self {
            stop,
            wake,
            thread: Some(thread),
        })
    }
}

impl Drop for Companions {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let _ = self.wake.signal();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

struct Running {
    stop: Event,
    thread: JoinHandle<()>,
}

fn supervise(
    profile: Profile,
    database: Database,
    primary: Primary,
    active: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    wake: Event,
    log: Arc<dyn Fn(&str) + Send + Sync>,
) {
    let Ok(notification) = Notification::register() else {
        log("Companion tablets are unavailable: device notifications failed.");
        return;
    };
    let named = profile.tablet_name().ok().flatten();
    let mut running: BTreeMap<String, Running> = BTreeMap::new();
    while !stop.load(Ordering::Acquire) {
        running.retain(|_, session| !session.thread.is_finished());
        if active.load(Ordering::Acquire) {
            let primary_path = primary.lock().ok().and_then(|path| path.clone());
            for (path, name) in candidates(&database) {
                if running.contains_key(&path)
                    || primary_path
                        .as_deref()
                        .is_some_and(|p| p.eq_ignore_ascii_case(&path))
                {
                    continue;
                }
                let profile = companion_profile(&profile, named.as_deref(), &name, import_otd);
                match spawn(
                    path.clone(),
                    name.clone(),
                    profile,
                    database.clone(),
                    Arc::clone(&log),
                ) {
                    Ok(session) => {
                        log(&format!("Also running {name} (a second tablet)."));
                        running.insert(path, session);
                    }
                    Err(error) => log(&format!("Could not start {name}: {error}")),
                }
            }
        } else if !running.is_empty() {
            stop_all(&mut running);
        }
        // Wake on device changes, a stop, or every two seconds.
        let _ = session::wait_for_retry(&notification, &wake);
    }
    stop_all(&mut running);
}

fn stop_all(running: &mut BTreeMap<String, Running>) {
    for (_, session) in std::mem::take(running) {
        let _ = session.stop.signal();
        let _ = session.thread.join();
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
    log: Arc<dyn Fn(&str) + Send + Sync>,
) -> Result<Running, String> {
    let stop = Event::create(true).map_err(|error| error.to_string())?;
    let thread_stop = stop.duplicate().map_err(|error| error.to_string())?;
    let thread = std::thread::Builder::new()
        .name(format!("tablet-{name}"))
        .spawn(move || {
            if let Err(error) = run_companion(&path, &name, &profile, &database, &thread_stop) {
                log(&format!("{name} stopped: {error}"));
            }
        })
        .map_err(|error| error.to_string())?;
    Ok(Running { stop, thread })
}

fn run_companion(
    path: &str,
    name: &str,
    profile: &Profile,
    database: &Database,
    stop: &Event,
) -> Result<(), String> {
    let notification = Notification::register().map_err(|error| error.to_string())?;
    let devices = hid::enumerate_with_database(database).map_err(|error| error.to_string())?;
    let selected = hid::select_device(&devices, database, Some(path), Some(name))?
        .ok_or("the tablet is no longer connected")?;
    let mut plugins = PluginChain::load_with_tablet(&profile.plugins, &selected.configuration)?;
    plugins.validate_output_mode(profile.relative.is_some())?;
    session::run(
        &selected,
        profile,
        &notification,
        stop,
        Mode::Driver,
        &mut plugins,
        &|_| {},
    )
    .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

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
