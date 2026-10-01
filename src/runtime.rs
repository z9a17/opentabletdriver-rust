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
    pub fn spawn(profile: Profile) -> Result<Self, String> {
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

fn run(
    profile: Profile,
    interrupt: &Event,
    cancelled: &AtomicBool,
    commands: Receiver<Directive>,
    notices: SyncSender<Notice>,
    logs: SyncSender<String>,
) -> Result<(), String> {
    let companions = std::cell::RefCell::new(None::<crate::companions::Companions>);
    let outcome = (|| {
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
    profile.validate_runtime_tablet_in(database)?;
    profile.validate_filter_execution()?;
    let tablet_name = profile.tablet_name()?;
    if profile.plugins.iter().any(|plugin| plugin.enabled)
        && !crate::plugin_catalog::recover_installations()? {
        return Err("Plugins are being installed or recovered; retry starting the driver after that finishes.".into());
    }
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
    'connect: loop {
        if let Some(companions) = &mut *companions.borrow_mut() {
            companions.check_finished()?;
        }
        if cancelled.load(Ordering::Acquire) {
            return Ok(());
        }
        let devices = crate::hid::enumerate_with_database(database)
            .map_err(|error| format!("HID discovery failed: {error}"))?;
        let Some(selected) = crate::hid::select_device(
            &devices,
            database,
            profile.device_path.as_deref(),
            tablet_name.as_deref(),
        )?
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
                if let Some(companions) = &mut *companions.borrow_mut() {
                    companions.check_finished()?;
                }
                if cancelled.load(Ordering::Acquire) {
                    return Ok(());
                }
                // A quiesce while disconnected has no live report resources.
                match receive(&commands, cancelled).map_err(|error| error.to_string())? {
                    Directive::Quiesce => {
                        finish_companions(&companions)?;
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
        if let Some(companions) = &mut *companions.borrow_mut() {
            companions.reserve_primary(&selected.pen.path_text(), &selected.configuration.name)?;
        }
        // These stay on this thread and survive a quiesce/failed replacement.
        // Construction/reset executes trusted plugin code (including its
        // reset/range-loss callback), but has no live input or host output sink.
        let mut plugins = PluginChain::load_with_tablet(&profile.plugins, &selected.configuration)?;
        plugins.validate_output_mode(profile.relative.is_some())?;
        reset_plugins(&mut plugins)?;
        if cancelled.load(Ordering::Acquire) {
            return Ok(());
        }
        let mut prepared = Some(
            PreparedSession::new(&selected, &notification, interrupt)
                .map_err(|error| format!("HID preparation failed: {error}"))?,
        );
        notify(Notice::Prepared)?;
        loop {
            match receive(&commands, cancelled).map_err(|error| error.to_string())? {
                Directive::Stop => return Ok(()),
                Directive::Quiesce => {
                    prepared = None;
                    finish_companions(&companions)?;
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
            if let Some(companions) = &mut *companions.borrow_mut() {
                companions.check_finished()?;
            }
            if cancelled.load(Ordering::Acquire) {
                return Ok(());
            }
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
                        finish_companions(&companions)?;
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
            // Tools run while this worker owns the output, like the tablet.
            let tools = std::cell::RefCell::new(None);
            let result = source.run(&profile, &mut plugins, &log, || {
                if let Some(companions) = &mut *companions.borrow_mut() {
                    companions.check_finished().map_err(io::Error::other)?;
                }
                notify(Notice::ActivationReady).map_err(io::Error::other)?;
                match receive(&commands, cancelled)? {
                    Directive::Run if !cancelled.load(Ordering::Acquire) => {
                        running.set(true);
                        let companion_logs = logs.clone();
                        if companions.borrow().is_none() {
                            *companions.borrow_mut() = Some(
                            crate::companions::Companions::start(
                                profile.clone(),
                                database.clone(),
                                selected.pen.path_text(),
                                selected.configuration.name.clone(),
                                interrupt,
                                move |line| {
                                    let _ = companion_logs.try_send(line.to_owned());
                                    crate::control::wake();
                                },
                            )
                            .map_err(io::Error::other)?,
                            );
                        }
                        notify(Notice::Running).map_err(io::Error::other)?;
                        *tools.borrow_mut() =
                            Some(crate::plugins::Tools::start(&profile.plugins, |line| {
                                log(line)
                            }));
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
            drop(tools.take());
            if let Some(companions) = &mut *companions.borrow_mut() {
                companions.check_finished()?;
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
                finish_companions(&companions)?;
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
                if let Some(companions) = &mut *companions.borrow_mut() {
                    companions.check_finished()?;
                }
                match receive(&commands, cancelled).map_err(|error| error.to_string())? {
                    Directive::Stop => return Ok(()),
                    Directive::Quiesce => {
                        finish_companions(&companions)?;
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
    })();
    finish_companions(&companions).and(outcome)
}

fn finish_companions(companions: &std::cell::RefCell<Option<crate::companions::Companions>>) -> Result<(), String> {
    companions.take().map_or(Ok(()), |mut companions| companions.finish())
}
