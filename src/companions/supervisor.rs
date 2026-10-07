//! Daemon-lived device supervision. Each tablet owns a worker and transaction;
//! the primary's replacement never destroys its peers.
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, atomic::{AtomicBool, Ordering}, mpsc};
use std::thread::JoinHandle;
use std::time::Duration;
use crate::config::Profile;
use crate::device_sessions::{DeviceKey, Handle, Request, SessionState};
use crate::hid::{self, Event, Notification};
use crate::runtime::{Directive, Notice, Worker};

pub struct Supervisor {
    handle: Handle,
    stop: Arc<AtomicBool>,
    enabled: Arc<AtomicBool>,
    wake: Event,
    thread: Option<JoinHandle<Result<(), String>>>,
    done: Arc<AtomicBool>,
}
impl Supervisor {
    pub fn prepare(profile: Profile) -> Result<Self, String> {
        let wake = Event::create(true).map_err(|error| error.to_string())?;
        let (handle, requests) = Handle::channel(wake.duplicate().map_err(|error| error.to_string())?);
        let thread_wake = wake.duplicate().map_err(|error| error.to_string())?;
        let stop = Arc::new(AtomicBool::new(false));
        let enabled = Arc::new(AtomicBool::new(false));
        let done = Arc::new(AtomicBool::new(false));
        let thread_handle = handle.clone();
        let thread_stop = Arc::clone(&stop);
        let thread_enabled = Arc::clone(&enabled);
        let thread_done = Arc::clone(&done);
        let thread = std::thread::Builder::new().name("tablet-supervisor".into()).spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(||
                supervise(profile, thread_handle, requests, thread_wake, thread_stop, thread_enabled)
            )).unwrap_or_else(|_| Err("device supervisor panicked".into()));
            thread_done.store(true, Ordering::Release);
            crate::control::wake();
            result
        }).map_err(|error| error.to_string())?;
        Ok(Self { handle, stop, enabled, wake, thread: Some(thread), done })
    }
    pub fn handle(&self) -> Handle { self.handle.clone() }
    /// Only the daemon, after acquiring the driver ownership mutex, enables
    /// input. Preparing a primary or listing devices cannot start companions.
    pub fn enable(&self) { self.enabled.store(true, Ordering::Release); let _ = self.wake.signal(); }
    pub fn stop(&self) -> Result<(), String> {
        self.stop.store(true, Ordering::Release);
        if let Ok(mut registry) = self.handle.registry.lock() { registry.shutting_down = true; }
        self.wake.signal().map_err(|error| error.to_string())
    }
    pub fn finished(&self) -> bool { self.done.load(Ordering::Acquire) || self.thread.as_ref().is_none_or(JoinHandle::is_finished) }
    pub fn finish(&mut self) -> Result<(), String> {
        let signalled = self.stop();
        let joined = self.thread.take().map_or(Ok(()), |thread| thread.join().unwrap_or_else(|_| Err("device supervisor panicked".into())));
        joined.and(signalled)
    }
}
impl Drop for Supervisor { fn drop(&mut self) { if let Err(error) = self.finish() { eprintln!("Device supervisor cleanup failed: {error}"); } } }

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase { Preparing, Quiescing, Activating, Abort, Rollback, Resuming, Stopping }
struct Transaction {
    active: Option<Worker>,
    candidate: Option<Worker>,
    retiring: Option<Worker>,
    phase: Option<Phase>,
    profile: Profile,
    pending_profile: Option<Profile>,
    generation: u64,
    error: Option<String>,
}
impl Transaction {
    fn new(profile: Profile) -> Self {
        Self { active: None, candidate: None, retiring: None, phase: None, profile,
            pending_profile: None, generation: 0, error: None }
    }
    fn apply(&mut self, id: &str, handle: &Handle, profile: Profile, generation: u64) -> Result<(), String> {
        if self.phase.is_some() || self.retiring.is_some() { return Err("device resources are still transitioning".into()); }
        let worker = Worker::spawn_device(profile.clone(), handle.clone(), id.to_owned())?;
        self.candidate = Some(worker);
        self.pending_profile = Some(profile);
        self.generation = generation;
        self.phase = Some(Phase::Preparing);
        self.error = None;
        if self.active.is_none() { handle.state(id, SessionState::Preparing, None); }
        Ok(())
    }
    fn signal_all(&self) -> Result<(), String> {
        let mut errors = Vec::new();
        for worker in [self.active.as_ref(), self.candidate.as_ref(), self.retiring.as_ref()].into_iter().flatten() {
            if let Err(error) = worker.stop() { errors.push(error); }
        }
        if errors.is_empty() { Ok(()) } else { Err(errors.join("; ")) }
    }
    fn fail_candidate(&mut self, id: &str, handle: &Handle, error: String) -> Result<(), String> {
        let rollback = matches!(self.phase, Some(Phase::Quiescing | Phase::Activating));
        self.error = Some(error.clone());
        self.phase = Some(if rollback { Phase::Rollback } else { Phase::Abort });
        if rollback { handle.state(id, SessionState::Starting, Some(error)); }
        else if self.active.is_some() { handle.error(id, error); }
        else { handle.state(id, SessionState::Failed, Some(error)); }
        self.candidate.as_ref().map_or(Ok(()), Worker::stop)
    }
    fn poll(&mut self, id: &str, handle: &Handle) -> Result<(), String> {
        // Process the retained worker first: candidate activation requires its
        // Quiesced acknowledgment, never just a signalled interrupt.
        let notices: Vec<_> = self.active.as_ref().map(|worker| worker.notices.try_iter().collect()).unwrap_or_default();
        for notice in notices {
            let suppress = matches!(self.phase, Some(Phase::Quiescing | Phase::Activating | Phase::Rollback | Phase::Stopping));
            match notice {
                Notice::Quiesced if self.phase == Some(Phase::Quiescing) => {
                    self.phase = Some(Phase::Activating);
                    handle.state(id, SessionState::Starting, None);
                    self.candidate.as_ref().ok_or("device candidate disappeared")?.command(Directive::Activate)?;
                }
                Notice::Prepared if !suppress => self.active.as_ref().unwrap().command(Directive::Activate)?,
                Notice::ActivationReady if !suppress => self.active.as_ref().unwrap().command(Directive::Run)?,
                Notice::Running if !suppress => {
                    handle.state(id, SessionState::Running, self.error.clone());
                    if self.phase == Some(Phase::Resuming) {
                        if let Some(error) = &self.error { handle.reject(id, error.clone()); }
                        self.phase = None;
                    }
                }
                Notice::Waiting if !suppress => handle.state(id, SessionState::Waiting, self.error.clone()),
                Notice::ActivationFailed(error) if !suppress => return Err(format!("device activation failed: {error}")),
                _ => {}
            }
        }
        let notices: Vec<_> = self.candidate.as_ref().map(|worker| worker.notices.try_iter().collect()).unwrap_or_default();
        for notice in notices {
            if matches!(self.phase, Some(Phase::Abort | Phase::Rollback | Phase::Stopping)) { continue; }
            if self.candidate.is_none() {
                match notice {
                    Notice::Prepared => self.active.as_ref().unwrap().command(Directive::Activate)?,
                    Notice::ActivationReady => self.active.as_ref().unwrap().command(Directive::Run)?,
                    Notice::Running => handle.state(id, SessionState::Running, None),
                    Notice::Waiting => handle.state(id, SessionState::Waiting, None),
                    Notice::ActivationFailed(error) => return Err(format!("device reconnect activation failed: {error}")),
                    Notice::Quiesced => return Err("unexpected quiesce after device commit".into()),
                }
                continue;
            }
            match notice {
                Notice::Prepared if self.phase == Some(Phase::Preparing) => {
                    if let Some(active) = &self.active {
                        self.phase = Some(Phase::Quiescing);
                        handle.state(id, SessionState::Stopping, None);
                        active.command(Directive::Quiesce)?;
                    } else {
                        self.phase = Some(Phase::Activating);
                        handle.state(id, SessionState::Starting, None);
                        self.candidate.as_ref().unwrap().command(Directive::Activate)?;
                    }
                }
                Notice::ActivationReady if self.phase == Some(Phase::Activating) => {
                    self.candidate.as_ref().unwrap().command(Directive::Run)?;
                    self.profile = self.pending_profile.take().ok_or("device pending profile disappeared")?;
                    handle.commit(id, self.generation, self.profile.clone());
                    self.retiring = self.active.take();
                    if let Some(retiring) = &self.retiring { retiring.stop()?; }
                    self.active = self.candidate.take();
                    self.phase = None;
                }
                Notice::Waiting if self.phase == Some(Phase::Preparing) && self.active.is_none() => handle.state(id, SessionState::Waiting, None),
                Notice::ActivationFailed(error) => self.fail_candidate(id, handle, error)?,
                _ => {}
            }
        }
        for worker in [self.active.as_ref(), self.candidate.as_ref(), self.retiring.as_ref()].into_iter().flatten() {
            for line in worker.logs.try_iter() { eprintln!("Tablet {id}: {line}"); }
        }
        if self.retiring.as_ref().is_some_and(Worker::finished) { self.retiring.take().unwrap().join()?; }
        if self.candidate.as_ref().is_some_and(Worker::finished) {
            let result = self.candidate.take().unwrap().join();
            if self.phase == Some(Phase::Stopping) { result?; }
            else {
                if let Err(error) = result { self.error = Some(error); }
                let error = self.error.clone().unwrap_or_else(|| "device candidate ended before commit".into());
                self.pending_profile = None;
                if matches!(self.phase, Some(Phase::Rollback | Phase::Quiescing | Phase::Activating)) && self.active.is_some() {
                    self.phase = Some(Phase::Resuming);
                    handle.state(id, SessionState::Starting, Some(error));
                    self.active.as_ref().unwrap().command(Directive::Activate)?;
                } else {
                    handle.reject(id, error.clone());
                    self.phase = None;
                    if self.active.is_none() { return Err(error); }
                    handle.error(id, error);
                }
            }
        }
        if self.active.as_ref().is_some_and(Worker::finished) {
            let result = self.active.take().unwrap().join();
            if self.phase != Some(Phase::Stopping) { return Err(result.err().unwrap_or_else(|| "device worker stopped unexpectedly".into())); }
            result?;
        }
        if self.phase == Some(Phase::Stopping) && self.active.is_none() && self.candidate.is_none() && self.retiring.is_none() {
            self.phase = None;
            if let Ok(mut registry) = handle.registry.lock() {
                if let Some(entry) = registry.entries.get_mut(id) {
                    entry.enabled = false;
                    entry.snapshot.device_generation = self.generation;
                    entry.snapshot.pending_generation = None;
                    entry.snapshot.state = SessionState::Stopped;
                }
            }
        }
        Ok(())
    }
    fn join_all(&mut self) -> Result<(), String> {
        let mut errors = Vec::new();
        for worker in [self.active.take(), self.candidate.take(), self.retiring.take()].into_iter().flatten() {
            if let Err(error) = worker.join() { errors.push(error); }
        }
        if errors.is_empty() { Ok(()) } else { Err(errors.join("; ")) }
    }
}

fn supervise(profile: Profile, handle: Handle, requests: mpsc::Receiver<Request>, wake: Event,
    stop: Arc<AtomicBool>, enabled: Arc<AtomicBool>) -> Result<(), String> {
    let configured = crate::check_tablet_configurations()?;
    let database = configured.as_ref();
    let notification = Notification::register().map_err(|error| error.to_string())?;
    let mut sessions = BTreeMap::<String, Transaction>::new();
    let mut rescan = true;
    let mut scanned = std::time::Instant::now();
    let mut was_enabled = false;
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        while !stop.load(Ordering::Acquire) {
            if unsafe { windows_sys::Win32::System::Threading::ResetEvent(wake.raw()) } == 0 { return Err(std::io::Error::last_os_error().to_string()); }
            if stop.load(Ordering::Acquire) { break; }
            let now_enabled = enabled.load(Ordering::Acquire);
            if now_enabled && !was_enabled { rescan = true; }
            was_enabled = now_enabled;
            if rescan || scanned.elapsed() >= super::RESCAN {
                let devices = hid::enumerate_with_database(database).map_err(|error| error.to_string())?;
                if handle.primary_id()?.is_none() {
                    if let Ok(Some(primary)) = hid::select_device(&devices, database, profile.device_path.as_deref(), profile.tablet_name()?.as_deref()) {
                        handle.reserve_primary(&primary)?;
                    }
                }
                let mut keys = BTreeSet::new();
                let mut connected = BTreeSet::new();
                for device in &devices {
                    if let Ok(Some(selected)) = hid::select_device(&devices, database, Some(&device.path_text()), None) {
                        if !keys.insert(DeviceKey::of(&selected)) { continue; }
                        let id = match handle.discover(&selected) { Ok(id) => id, Err(error) => { eprintln!("Tablet discovery: {error}"); continue; } };
                        connected.insert(id.clone());
                        let primary = handle.primary_id()?.as_ref() == Some(&id);
                        if primary || sessions.contains_key(&id) || !enabled.load(Ordering::Acquire) { continue; }
                        let (should_start, stored) = {
                            let registry = handle.registry.lock().map_err(|_| "device registry poisoned")?;
                            let entry = &registry.entries[&id];
                            (entry.enabled, entry.profile.clone())
                        };
                        if !should_start { continue; }
                        let own_profile = stored.map_or_else(|| super::companion_profile(&profile, profile.tablet_name()?.as_deref(), &selected.configuration.name, super::import_otd), Ok);
                        match own_profile {
                            Ok(own_profile) => {
                                let generation = { let mut registry = handle.registry.lock().map_err(|_| "device registry poisoned")?;
                                    let entry = registry.entries.get_mut(&id).unwrap();
                                    let next = entry.snapshot.device_generation.checked_add(1).ok_or("device generation exhausted")?;
                                    entry.snapshot.pending_generation = Some(next); next };
                                let mut transaction = Transaction::new(own_profile.clone());
                                if let Err(error) = transaction.apply(&id, &handle, own_profile, generation) { handle.reject(&id, error.clone()); handle.state(&id, SessionState::Failed, Some(error)); }
                                sessions.insert(id, transaction);
                            }
                            Err(error) => { handle.state(&id, SessionState::Failed, Some(error)); }
                        }
                    }
                }
                if let Ok(mut registry) = handle.registry.lock() {
                    for (id, entry) in &mut registry.entries { entry.snapshot.connected = connected.contains(id); }
                }
                scanned = std::time::Instant::now();
                rescan = false;
            }
            for request in requests.try_iter() {
                let (id, generation) = match &request { Request::Apply { id, generation, .. } | Request::Stop { id, generation } | Request::Start { id, generation } => (id.clone(), *generation) };
                let result = if let Some(transaction) = sessions.get_mut(&id) {
                    match request {
                        Request::Apply { profile, .. } => {
                            let stopped = handle.registry.lock().map_err(|_| "device registry poisoned")?.entries.get(&id).is_some_and(|entry| !entry.enabled);
                            if stopped && transaction.active.is_none() && transaction.phase.is_none() {
                                transaction.profile = profile.clone();
                                handle.commit(&id, generation, profile);
                                if let Ok(mut registry) = handle.registry.lock() { if let Some(entry) = registry.entries.get_mut(&id) { entry.enabled = false; entry.snapshot.state = SessionState::Stopped; } }
                                Ok(())
                            } else { transaction.apply(&id, &handle, profile, generation) }
                        },
                        Request::Stop { .. } => { transaction.generation = generation; transaction.phase = Some(Phase::Stopping); handle.state(&id, SessionState::Stopping, None); transaction.signal_all() },
                        Request::Start { .. } => {
                            if transaction.active.is_some() || transaction.phase.is_some() { Err("device session is already active".into()) }
                            else { transaction.apply(&id, &handle, transaction.profile.clone(), generation) }
                        }
                    }
                } else { Err("device session has no prepared runtime profile".into()) };
                if let Err(error) = result { handle.reject(&id, error); }
            }
            for (id, transaction) in &mut sessions {
                if let Err(error) = transaction.poll(id, &handle) {
                    let signalled = transaction.signal_all();
                    let joined = transaction.join_all();
                    let error = [Some(error), signalled.err(), joined.err()].into_iter().flatten().collect::<Vec<_>>().join("; ");
                    handle.reject(id, error.clone());
                    handle.state(id, SessionState::Failed, Some(error));
                    if let Ok(mut registry) = handle.registry.lock() { if let Some(entry) = registry.entries.get_mut(id) { entry.enabled = false; } }
                    transaction.phase = None;
                }
            }
            // Control transactions progress even without reports or PnP. This
            // bounded wait has no work on the successful HID report path.
            rescan |= crate::session::companion_wake(&notification, &wake, Duration::from_millis(20)).map_err(|error| error.to_string())?;
        }
        Ok(())
    })).unwrap_or_else(|_| Err("device supervision panicked".into()));
    let mut errors = Vec::new();
    // Signal every device before joining any, including replacement candidates.
    for transaction in sessions.values() { if let Err(error) = transaction.signal_all() { errors.push(error); } }
    for (id, transaction) in &mut sessions {
        if let Err(error) = transaction.join_all() { handle.state(id, SessionState::Failed, Some(error.clone())); errors.push(error); }
        else { handle.state(id, SessionState::Stopped, None); }
    }
    if let Err(error) = outcome { errors.push(error); }
    if errors.is_empty() { Ok(()) } else { Err(errors.join("; ")) }
}
impl Drop for Transaction {
    fn drop(&mut self) {
        // A supervisor/fixture unwind must never detach live report workers.
        let signal = self.signal_all();
        let join = self.join_all();
        if let Err(error) = join.and(signal) { eprintln!("Device transaction cleanup failed: {error}"); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    type Observed = Arc<Mutex<Vec<(String, Directive)>>>;
    fn scripted(name: &str, prepared: bool, fail: bool, observed: &Observed) -> Worker {
        let name = name.to_owned();
        let observed = Arc::clone(observed);
        Worker::fixture(move |commands, notices, cancelled| {
            if prepared { notices.send(Notice::Prepared).unwrap(); }
            loop {
                if cancelled.load(Ordering::Acquire) { return Ok(()); }
                let command = match commands.recv_timeout(Duration::from_millis(20)) {
                    Ok(command) => command,
                    Err(mpsc::RecvTimeoutError::Timeout) => continue,
                    Err(_) => return Ok(()),
                };
                observed.lock().unwrap().push((name.clone(), command));
                let notice = match command {
                    Directive::Quiesce => Notice::Quiesced,
                    Directive::Activate if fail => Notice::ActivationFailed("fixture initialization rejected".into()),
                    Directive::Activate => Notice::ActivationReady,
                    Directive::Run => Notice::Running,
                    Directive::Stop => return Ok(()),
                };
                notices.send(notice).unwrap();
            }
        })
    }
    fn wait_for(transaction: &mut Transaction, id: &str, handle: &Handle, ready: impl Fn(&Transaction) -> bool) {
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while !ready(transaction) {
            assert!(std::time::Instant::now() < deadline, "fixture transaction stalled");
            transaction.poll(id, handle).unwrap();
            std::thread::yield_now();
        }
    }
    #[test]
    fn replacement_and_stop_affect_only_the_requested_device_and_drain_before_activation() {
        let (handle, _requests) = Handle::channel(Event::create(true).unwrap());
        let a = crate::device_sessions::tests::candidate("a", "parent-a");
        let b = crate::device_sessions::tests::candidate("b", "parent-b");
        let id_a = handle.discover(&crate::device_sessions::tests::selected(&a)).unwrap();
        let id_b = handle.discover(&crate::device_sessions::tests::selected(&b)).unwrap();
        let old = Profile::default();
        let peer_profile = Profile { rotation: 180, ..Profile::default() };
        handle.commit(&id_a, 1, old.clone());
        handle.commit(&id_b, 1, peer_profile.clone());
        let observed: Observed = Arc::default();
        let mut a = Transaction::new(old);
        a.active = Some(scripted("old", false, false, &observed));
        a.candidate = Some(scripted("candidate", true, false, &observed));
        a.pending_profile = Some(Profile { rotation: 90, ..Profile::default() });
        a.generation = 2;
        a.phase = Some(Phase::Preparing);
        let mut b = Transaction::new(peer_profile);
        b.active = Some(scripted("peer", false, false, &observed));
        wait_for(&mut a, &id_a, &handle, |transaction| transaction.phase.is_none() && transaction.retiring.is_none());
        assert_eq!(a.profile.rotation, 90);
        assert_eq!(b.profile.rotation, 180);
        assert!(!b.active.as_ref().unwrap().finished());
        let sequence = observed.lock().unwrap();
        let quiesce = sequence.iter().position(|(name, command)| name == "old" && matches!(command, Directive::Quiesce)).unwrap();
        let activate = sequence.iter().position(|(name, command)| name == "candidate" && matches!(command, Directive::Activate)).unwrap();
        assert!(quiesce < activate);
        assert!(sequence.iter().all(|(name, _)| name != "peer"));
        drop(sequence);
        a.phase = Some(Phase::Stopping);
        a.generation = 3;
        a.signal_all().unwrap();
        wait_for(&mut a, &id_a, &handle, |transaction| transaction.phase.is_none());
        assert_eq!(handle.snapshot().unwrap().sessions.iter().find(|entry| entry.id == id_a).unwrap().state, SessionState::Stopped);
        assert!(!b.active.as_ref().unwrap().finished());
        b.signal_all().unwrap();
        b.join_all().unwrap();
    }
    #[test]
    fn failed_candidate_resumes_retained_profile_and_preserves_its_generation() {
        let (handle, _requests) = Handle::channel(Event::create(true).unwrap());
        let candidate = crate::device_sessions::tests::candidate("a", "parent-a");
        let id = handle.discover(&crate::device_sessions::tests::selected(&candidate)).unwrap();
        handle.commit(&id, 4, Profile { rotation: 180, ..Profile::default() });
        let observed: Observed = Arc::default();
        let mut transaction = Transaction::new(Profile { rotation: 180, ..Profile::default() });
        transaction.active = Some(scripted("retained", false, false, &observed));
        transaction.candidate = Some(scripted("failed", true, true, &observed));
        transaction.pending_profile = Some(Profile { rotation: 90, ..Profile::default() });
        transaction.generation = 5;
        transaction.phase = Some(Phase::Preparing);
        wait_for(&mut transaction, &id, &handle, |transaction| transaction.phase.is_none());
        assert_eq!(transaction.profile.rotation, 180);
        let entry = handle.snapshot().unwrap().sessions.into_iter().find(|entry| entry.id == id).unwrap();
        assert_eq!(entry.device_generation, 4);
        assert_eq!(entry.state, SessionState::Running);
        assert!(entry.last_error.unwrap().contains("fixture initialization rejected"));
        let observed = observed.lock().unwrap();
        assert!(observed.iter().any(|(name, command)| name == "retained" && matches!(command, Directive::Run)));
        drop(observed);
        transaction.signal_all().unwrap();
        transaction.join_all().unwrap();
    }
}
