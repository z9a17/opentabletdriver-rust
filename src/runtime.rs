//! Windows PTH-660 worker preparation and activation. Plugins are constructed,
//! consumed and disposed on this worker thread; only control messages cross it.
use std::cell::Cell;
use std::io;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver, SyncSender},
};
use std::thread::JoinHandle;
use windows_sys::Win32::System::Threading::ResetEvent;

use crate::config::Profile;
use crate::hid::{Event, Notification, OwnedHandle};
use crate::plugins::PluginChain;
use crate::session::PreparedSession;

/// Held by the daemon across candidate preparation, quiesce and rollback.
/// Foreground run/capture retain their existing local ownership guard.
pub struct Ownership {
    _instance: OwnedHandle,
}
impl Ownership {
    pub fn acquire() -> Result<Self, String> {
        let instance = crate::single_instance()?;
        crate::original_driver::ensure_stopped()
            .map_err(|error| format!("driver coexistence check failed: {error}"))?;
        Ok(Self {
            _instance: instance,
        })
    }
}

#[derive(Clone, Copy, Debug)]
pub enum Directive {
    Activate,
    Run,
    Quiesce,
    Stop,
}
#[derive(Debug)]
pub enum Notice {
    PresetRequested(crate::binding_presets::Request),
    PreparedProfile(Box<Profile>),
    Prepared,
    ActivationReady,
    Running,
    Quiesced,
    Waiting,
    ActivationFailed(String),
}

pub struct Worker {
    interrupt: Event,
    cancelled: Arc<AtomicBool>,
    commands: SyncSender<Directive>,
    pub notices: Receiver<Notice>,
    pub logs: Receiver<String>,
    pub thread: JoinHandle<Result<(), String>>,
    done: Arc<AtomicBool>,
}

/// Marks the worker finished and wakes the control thread, last thing on the
/// worker thread, even if it unwinds. `JoinHandle::is_finished` turns true
/// only later, so a poll woken by this must not rely on it.
struct Done(Arc<AtomicBool>);
impl Drop for Done {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
        crate::control::wake();
    }
}
impl Worker {
    #[cfg(test)]
    pub(crate) fn fixture(script: impl FnOnce(Receiver<Directive>, SyncSender<Notice>, Arc<AtomicBool>) -> Result<(), String> + Send + 'static) -> Self {
        let interrupt = Event::create(true).unwrap();
        let cancelled = Arc::new(AtomicBool::new(false));
        let thread_cancelled = Arc::clone(&cancelled);
        let (commands, receiver) = mpsc::sync_channel(4);
        let (notifier, notices) = mpsc::sync_channel(8);
        let (_logger, logs) = mpsc::sync_channel(1);
        let done = Arc::new(AtomicBool::new(false));
        let thread_done = Done(Arc::clone(&done));
        let thread = std::thread::spawn(move || { let _done = thread_done; script(receiver, notifier, thread_cancelled) });
        Self { interrupt, cancelled, commands, notices, logs, thread, done }
    }
    pub fn spawn(profile: Profile, devices: crate::device_sessions::Handle, prefer_saved: bool) -> Result<Self, String> {
        Self::spawn_bound(profile, devices, None, prefer_saved)
    }
    pub(crate) fn spawn_device(profile: Profile, devices: crate::device_sessions::Handle, id: String) -> Result<Self, String> {
        Self::spawn_bound(profile, devices, Some(id), false)
    }
    fn spawn_bound(profile: Profile, devices: crate::device_sessions::Handle, id: Option<String>, prefer_saved: bool) -> Result<Self, String> {
        let interrupt = Event::create(true).map_err(|error| error.to_string())?;
        let thread_interrupt = interrupt.duplicate().map_err(|error| error.to_string())?;
        let cancelled = Arc::new(AtomicBool::new(false));
        let thread_cancelled = Arc::clone(&cancelled);
        let (commands, receiver) = mpsc::sync_channel(4);
        let (notifier, notices) = mpsc::sync_channel(8);
        let (logger, logs) = mpsc::sync_channel(crate::control::MAX_LOG_LINES);
        let done = Arc::new(AtomicBool::new(false));
        let thread_done = Done(Arc::clone(&done));
        let thread = std::thread::Builder::new()
            .name("tablet-driver".into())
            .spawn(move || {
                let _done = thread_done;
                run(
                    profile,
                    &thread_interrupt,
                    &thread_cancelled,
                    receiver,
                    notifier,
                    logger,
                    devices,
                    id,
                    prefer_saved,
                )
            })
            .map_err(|error| error.to_string())?;
        Ok(Self {
            interrupt,
            cancelled,
            commands,
            notices,
            logs,
            thread,
            done,
        })
    }
    pub fn command(&self, command: Directive) -> Result<(), String> {
        self.commands
            .try_send(command)
            .map_err(|error| format!("worker command failed: {error}"))?;
        if matches!(command, Directive::Quiesce) {
            self.interrupt.signal().map_err(|error| error.to_string())?;
        }
        Ok(())
    }
    pub fn stop(&self) -> Result<(), String> {
        self.cancelled.store(true, Ordering::Release);
        // Cancellation must wake either a command gate or a pending HID read.
        let _ = self.commands.try_send(Directive::Stop);
        self.interrupt.signal().map_err(|error| error.to_string())
    }
    /// The worker has returned; `join` then waits only for its thread exit.
    pub fn finished(&self) -> bool {
        self.done.load(Ordering::Acquire) || self.thread.is_finished()
    }
    pub fn join(self) -> Result<(), String> {
        self.thread
            .join()
            .unwrap_or_else(|_| Err("driver worker panicked".into()))
    }
}

fn receive(commands: &Receiver<Directive>, cancelled: &AtomicBool) -> io::Result<Directive> {
    if cancelled.load(Ordering::Acquire) {
        return Ok(Directive::Stop);
    }
    commands
        .recv()
        .map_err(|_| io::Error::other("daemon control owner disconnected"))
}

fn reset_plugins(plugins: &mut PluginChain) -> Result<(), String> {
    // Native reset / managed range-loss notification is the available plugin
    // contract. Keeping the instance does not guarantee its private state is
    // cleared; a third-party implementation may ignore that notification.
    plugins.reset();
    if let Some(name) = plugins.take_failure() {
        return Err(format!("Plugin failed during reset notification: {name}"));
    }
    Ok(())
}

/// Execution-only substitutions must never become the settings published to
/// clients or compared with a saved physical profile. Clone before porting.
fn execution_profile(authored: &Profile, port: impl FnOnce(&mut Profile)) -> Profile {
    let mut execution = authored.clone();
    port(&mut execution);
    execution
}

fn run(
    profile: Profile,
    interrupt: &Event,
    cancelled: &AtomicBool,
    commands: Receiver<Directive>,
    notices: SyncSender<Notice>,
    logs: SyncSender<String>,
    device_sessions: crate::device_sessions::Handle,
    bound_id: Option<String>,
    prefer_saved: bool,
) -> Result<(), String> {
    let log = |line: &str| {
        let _ = logs.try_send(line.to_owned());
        crate::control::wake();
    };
    let notify = |notice| {
        let sent = notices
            .send(notice)
            .map_err(|_| "daemon control owner disconnected".to_owned());
        crate::control::wake();
        sent
    };
    let configured_tablets = crate::check_tablet_configurations()?;
    let database = configured_tablets.as_ref();
    crate::plugins::prepare_parser_registry(&profile, database)?;
    profile.validate_runtime_tablet_in_with_parser_support(database, &crate::dotnet::installed_report_parser)?;
    profile.validate_filter_execution()?;
    let tablet_name = profile.tablet_name()?;
    if profile.plugin_configs().any(|plugin| plugin.enabled)
        && !crate::plugin_catalog::recover_installations()? {
        return Err("Plugins are being installed or recovered; retry starting the driver after that finishes.".into());
    }
    let mut authored_profile = profile;
    let binding_inhibit = authored_profile.binding_inhibit.take();
    let mut profile = execution_profile(&authored_profile, |execution| {
        execution.binding_inhibit = binding_inhibit;
        crate::plugin_catalog::use_native_ports(execution, log);
    });
    // Exercise deterministic pipeline construction before old output pauses.
    // A fresh output/relative pipeline is used on activation and rollback.
    if tablet_name.is_some() {
        let _ = otd_core::pipeline::ReportPipeline::new(&profile)?;
        if profile.relative.is_none() {
            crate::display::read_snapshot()?.mapper(&profile)?;
        }
    }
    let notification = Notification::register().map_err(|error| error.to_string())?;
    let mut waiting = false;
    let mut profile_loaded = false;
    'connect: loop {
        if cancelled.load(Ordering::Acquire) {
            return Ok(());
        }
        let devices = crate::hid::enumerate_with_database(database)
            .map_err(|error| format!("HID discovery failed: {error}"))?;
        let stable_id = bound_id.clone().or(device_sessions.primary_id()?);
        let choice = if let Some(id) = &stable_id {
            device_sessions.validate_profile(id, &profile)?;
            device_sessions.find(id, &devices, database)?
        } else {
            crate::hid::select_device_for_start(&devices, database, profile.device_path.as_deref(), tablet_name.as_deref())?
        };
        let Some(selected) = choice
        else {
            if !waiting {
                notify(Notice::Waiting)?;
                log(&format!(
                    "Waiting for {}",
                    tablet_name.as_deref().unwrap_or("a supported tablet")
                ));
                waiting = true;
            }
            if !crate::session::wait_for_retry(&notification, interrupt)
                .map_err(|error| error.to_string())?
            {
                if cancelled.load(Ordering::Acquire) {
                    return Ok(());
                }
                // A quiesce while disconnected has no live report resources.
                match receive(&commands, cancelled).map_err(|error| error.to_string())? {
                    Directive::Quiesce => {
                        notify(Notice::Quiesced)?;
                        match receive(&commands, cancelled).map_err(|error| error.to_string())? {
                            Directive::Stop => return Ok(()),
                            Directive::Activate => {}
                            _ => return Err("unexpected disconnected worker command".into()),
                        }
                    }
                    Directive::Stop => return Ok(()),
                    _ => return Err("unexpected waiting worker command".into()),
                }
                if unsafe { ResetEvent(interrupt.raw()) } == 0 {
                    return Err(io::Error::last_os_error().to_string());
                }
            }
            continue;
        };
        waiting = false;
        let id = if let Some(id) = &bound_id {
            device_sessions.discover(&selected)?;
            id.clone()
        } else { device_sessions.reserve_primary(&selected)? };
        crate::device_sessions::set_debug_key(&id);
        let source_generation = device_sessions.snapshot()?.sessions.into_iter().find(|session| session.id == id)
            .map(|session| session.pending_generation.unwrap_or(session.device_generation)).unwrap_or(1);
        crate::device_sessions::set_source_generation(source_generation);
        let mut reload_profile = !profile_loaded;
        if !profile_loaded {
            if prefer_saved { authored_profile.use_settings_collection = true; }
            // A physical file returned by this lookup clears the transient
            // marker through native TOML parsing; no second file-read inference.
            authored_profile = device_sessions.effective_profile(&id, &authored_profile, prefer_saved)?;
        } else if authored_profile.use_settings_collection {
            // Cold physical reconnect only. Inner quiesce/rollback activation
            // keeps its owned graph and profile; reports never call this path.
            if let Some(saved) = device_sessions.saved_profile(&id)? {
                authored_profile = saved;
                authored_profile.use_settings_collection = false;
                reload_profile = true;
            } else if let Some(mut retained) = crate::upstream_rpc::profile_for_tablet(&selected.configuration)? {
                // Read one owned document snapshot on every physical reconnect;
                // separately sampled revision/profile reads can miss a publish.
                retained.device_path.clone_from(&authored_profile.device_path);
                reload_profile = retained.to_toml()? != authored_profile.to_toml()?;
                authored_profile = retained;
            }
        }
        if reload_profile {
            if authored_profile.plugin_configs().any(|plugin| plugin.enabled) && !crate::plugin_catalog::recover_installations()? {
                return Err("Plugins are being installed or recovered; retry starting the driver after that finishes.".into());
            }
            profile = execution_profile(&authored_profile, |execution| {
                execution.binding_inhibit = binding_inhibit;
                crate::plugin_catalog::use_native_ports(execution, log);
            });
            crate::plugins::prepare_parser_registry(&profile, database)?;
            profile.validate_runtime_tablet_in_with_parser_support(database, &crate::dotnet::installed_report_parser)?;
            profile.validate_filter_execution()?;
            let _ = otd_core::pipeline::ReportPipeline::new(&profile.for_tablet(selected.spec)?)?;
            if profile.relative.is_none() { crate::display::read_snapshot()?.mapper(&profile.for_tablet(selected.spec)?)?; }
            profile_loaded = true;
        }
        if cancelled.load(Ordering::Acquire) { return Ok(()); }
        let source = PreparedSession::new(&selected, &notification, interrupt)
            .map_err(|error| format!("HID preparation failed: {error}"))?;
        // Metadata names the endpoints actually opened, not configuration
        // alternatives. Preparation still issues no input read or init write.
        let mut plugins = PluginChain::load_for_profile_with_identifiers(
            &profile, &selected.configuration, &source.identifiers())?;
        plugins.validate_output_mode(profile.relative.is_some())?;
        reset_plugins(&mut plugins)?;
        if cancelled.load(Ordering::Acquire) { return Ok(()); }
        let mut prepared = Some(source);
        notify(Notice::PreparedProfile(Box::new(authored_profile.clone())))?;
        notify(Notice::Prepared)?;
        loop {
            match receive(&commands, cancelled).map_err(|error| error.to_string())? {
                Directive::Stop => return Ok(()),
                Directive::Quiesce => {
                    if let Some(mut source)=prepared.take(){source.retire().map_err(|error|format!("prepared device retirement failed: {error}"))?;}
                    reset_plugins(&mut plugins)?;
                    notify(Notice::Quiesced)?;
                    continue;
                }
                Directive::Activate => {}
                Directive::Run => return Err("worker received Run before activation".into()),
            }
            if unsafe { ResetEvent(interrupt.raw()) } == 0 {
                return Err(io::Error::last_os_error().to_string());
            }
            if cancelled.load(Ordering::Acquire) {
                return Ok(());
            }
            let source_generation = device_sessions.snapshot()?.sessions.into_iter().find(|session| session.id == id)
                .map(|session| session.pending_generation.unwrap_or(session.device_generation)).unwrap_or(1);
            crate::device_sessions::set_source_generation(source_generation);
            let source = prepared.take().map_or_else(
                || PreparedSession::new(&selected, &notification, interrupt),
                Ok,
            );
            let mut source = match source {
                Ok(source) => source,
                Err(error) => {
                    notify(Notice::ActivationFailed(error.to_string()))?;
                    continue;
                }
            };
            if let Err(error) = source.activate() {
                drop(source);
                if cancelled.load(Ordering::Acquire) {
                    return Ok(());
                }
                // A reconnect activation can overlap the daemon's quiesce.
                // Initialization observes the same interrupt as the reader;
                // acknowledge that command after closing the prepared handle.
                match commands.try_recv() {
                    Ok(Directive::Quiesce) => {
                        reset_plugins(&mut plugins)?;
                        notify(Notice::Quiesced)?;
                        continue;
                    }
                    Ok(Directive::Stop) => return Ok(()),
                    Ok(_) => return Err("unexpected command after initialization failed".into()),
                    Err(mpsc::TryRecvError::Disconnected) => {
                        return Err("daemon control owner disconnected".into());
                    }
                    Err(mpsc::TryRecvError::Empty) => {}
                }
                notify(Notice::ActivationFailed(format!(
                    "Device initialization failed; some hardware writes may already have completed: {error}"
                )))?;
                continue;
            }
            let running = Cell::new(false);
            let quiesced = Cell::new(false);
            let opened_epoch = Cell::new(None);
            let preset_notices = notices.clone();
            let mut last_preset_press = std::time::Instant::now();
            let preset: crate::binding_presets::Callback = Box::new(move |owner, name| {
                let now = std::time::Instant::now();
                let elapsed = now.saturating_duration_since(last_preset_press);
                last_preset_press = now;
                if elapsed <= std::time::Duration::from_millis(50) { return Ok(()); }
                preset_notices.try_send(Notice::PresetRequested(crate::binding_presets::Request::new(owner, name)))
                    .map_err(|_| io::Error::from(io::ErrorKind::WouldBlock))?;
                crate::control::wake();
                Ok(())
            });
            let result = source.run(&profile, &mut plugins, &log, Some(preset), |identifiers| {
                notify(Notice::ActivationReady).map_err(io::Error::other)?;
                match receive(&commands, cancelled)? {
                    Directive::Run if !cancelled.load(Ordering::Acquire) => {
                        let epoch = device_sessions.activated_with_identifiers(&id, &selected, identifiers);
                        if epoch == 0 { return Err(io::Error::other("Cannot register this activation's actual opened endpoints.")); }
                        opened_epoch.set(Some(epoch));
                        running.set(true);
                        notify(Notice::Running).map_err(io::Error::other)?;
                        // Pinned DriverDaemon owns one Settings.Tools collection,
                        // separate from tablet profiles. Peer workers run their
                        // filters/output/bindings, never another global tool set.
                        if bound_id.is_none() {
                            if let Err(error)=crate::tool_host::apply_profile(&profile.plugins) {log(&format!("Global tool submission failed: {error}"));}
                        }
                        Ok(true)
                    }
                    Directive::Stop | Directive::Run => Ok(false),
                    Directive::Quiesce => {
                        quiesced.set(true);
                        Ok(false)
                    }
                    _ => Err(io::Error::other("unexpected activation gate command")),
                }
            });
            if let Some(epoch) = opened_epoch.get() {
                device_sessions.clear_opened_identifiers(&id, epoch);
            }
            // source has drained its read and core output cleanup has completed.
            if let Err(error) = &result
                && otd_core::session::is_cleanup_failure(error)
            {
                return Err(format!("worker output cleanup failed: {error}"));
            }
            if cancelled.load(Ordering::Acquire) {
                return Ok(());
            }
            let requested = if quiesced.get() {
                Some(Directive::Quiesce)
            } else {
                commands.try_recv().ok()
            };
            if matches!(requested, Some(Directive::Stop)) || cancelled.load(Ordering::Acquire) {
                return Ok(());
            }
            if matches!(requested, Some(Directive::Quiesce)) {
                reset_plugins(&mut plugins)?;
                notify(Notice::Quiesced)?;
                continue;
            }
            if requested.is_some() {
                return Err("unexpected command after device session ended".into());
            }
            if !running.get() {
                let error = result.err().map_or_else(
                    || "activation gate ended without Run".into(),
                    |error| error.to_string(),
                );
                notify(Notice::ActivationFailed(error))?;
                continue;
            }
            if let Err(error) = result {
                log(&format!("Device session ended: {error}"));
            }
            // A physical reconnect constructs a fresh graph, as foreground run
            // does. A transactional quiesce above retains the existing graph.
            notify(Notice::Waiting)?;
            if !crate::session::wait_for_retry(&notification, interrupt)
                .map_err(|error| error.to_string())?
            {
                match receive(&commands, cancelled).map_err(|error| error.to_string())? {
                    Directive::Stop => return Ok(()),
                    Directive::Quiesce => {
                        reset_plugins(&mut plugins)?;
                        notify(Notice::Quiesced)?;
                        // Retain this graph while the daemon attempts apply.
                        // The next Activate reopens the old endpoint/config.
                        continue;
                    }
                    _ => return Err("unexpected reconnect command".into()),
                }
            }
            continue 'connect;
        }
    }
}

#[cfg(test)]
mod authored_profile_tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn native_filter_execution_preserves_saved_settings_and_reconnect_publication() {
        let path = Path::new("E:/AgentWork/tmp/authored-profile-fixture.toml");
        let source = format!(
            "[[plugins]]\npath='radial.dll'\nkind='dotnet'\nenabled=true\ntype_name='{}'\nsettings_json='{{\"InnerRadius\":0.302,\"OuterRadius\":0.7039,\"SmoothingCoefficient\":0.302,\"SmoothingLeakCoefficient\":0.201,\"SoftKneeScale\":0.603}}'\n",
            otd_core::radial_follow::FILTER_PATH,
        );
        let authored = Profile::from_toml_text(&source, path).unwrap();
        let saved = authored.to_toml().unwrap();
        // The real port transformation runs with an injected verification
        // predicate; the fixture loads no DLL and never opens a device.
        let execution = execution_profile(&authored, |execution| {
            assert_eq!(execution.use_native_ports(|dll| dll.ends_with("radial.dll")), 1);
        });
        assert_eq!(execution.radial_follow.len(), 1);
        assert!(!execution.plugins[0].enabled);
        assert_ne!(execution.to_toml().unwrap(), saved);
        assert!(authored.plugins[0].enabled);
        assert!(authored.radial_follow.is_empty());
        let persisted = Profile::from_toml_text(&saved, path).unwrap();
        for published in [&authored, &persisted] {
            // PreparedProfile is what both primary and peer transactions
            // commit, including physical reconnect after saved-file reload.
            let Notice::PreparedProfile(profile) =
                Notice::PreparedProfile(Box::new(published.clone())) else { unreachable!() };
            assert_eq!(profile.to_toml().unwrap(), persisted.to_toml().unwrap(),
                "raw saved-profile semantic comparison must remain equal");
            assert!(profile.plugins[0].enabled);
            let reopened = execution_profile(&profile, |execution| {
                assert_eq!(execution.use_native_ports(|_| true), 1);
            });
            assert_eq!(serde_json::to_value(&reopened.radial_follow).unwrap(),
                serde_json::to_value(&execution.radial_follow).unwrap());
            assert_eq!(profile.to_toml().unwrap(), saved);
        }
    }
}
