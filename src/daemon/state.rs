//! One control-thread transaction at a time. Preparing preserves the published
//! generation and running worker. Quiesced acknowledges reader teardown,
//! output release and plugin reset notification; only then may the candidate
//! initialize hardware. Ready
//! acknowledges initialization and session construction, and Run commits the
//! generation before another control request can observe it.
//!
//! Before Run, failed activation drains the candidate and resumes the retained
//! old worker. After Run, failures stop the committed generation; they never
//! replay the old one. Stop/Shutdown cancel every role before handling queued
//! readiness. Ownership outlives all worker joins and output cleanup.
//! This is a synchronous PTH-660 transaction, not rollback of hardware writes
//! or arbitrary side effects of trusted plugin constructors/disposal.
use std::collections::VecDeque;
use std::path::Path;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::config::Profile;
use crate::control::{
    self, Command, ControlError, ControlHandler, ControlStatus, DriverState, ErrorCode, Reply,
    WorkerIdentity,
};
use crate::runtime::{Directive, Notice, Ownership, Worker};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Preparing,
    Quiescing,
    Activating,
    CommitSent,
    AbortKeepOld,
    RollbackDrain,
    Resuming,
    ResumeSent,
}

#[cfg(test)]
mod scheduling_tests {
    use super::*;
    #[test]
    fn stale_affinity_requests_do_not_change_driver_state_or_scheduling() {
        let mut daemon = Daemon::new(Arc::new(AtomicBool::new(false)));
        let before = crate::experimental::masks().unwrap().0;
        let error = daemon.handle(Command::SetExperimental {
            expected: WorkerIdentity { instance: "different-daemon".into(), generation: 0 },
            settings: crate::experimental::Settings::default(),
        }).unwrap_err();
        assert!(matches!(error.code, ErrorCode::Conflict));
        assert_eq!(crate::experimental::masks().unwrap().0, before);
        assert_eq!(daemon.state, DriverState::Stopped);
        assert_eq!(daemon.generation,0);
        assert!(daemon.worker.is_none());
    }
}
struct Pending {
    worker: Option<Worker>,
    expected: WorkerIdentity,
    profile: String,
    text: String,
    generation: u64,
    initial: bool,
    phase: Phase,
    error: Option<String>,
}

pub(super) struct Daemon {
    cancelled: Arc<AtomicBool>,
    instance: String,
    worker: Option<Worker>,
    pending: Option<Pending>,
    retiring: Option<Worker>,
    ownership: Option<Ownership>,
    stopping: bool,
    cleanup_error: Option<String>,
    state: DriverState,
    generation: u64,
    profile: Option<String>,
    configuration: Option<String>,
    last_error: Option<String>,
    logs: VecDeque<String>,
    log_sequence: u64,
}

impl Daemon {
    pub(super) fn new(cancelled: Arc<AtomicBool>) -> Self {
        Self {
            cancelled,
            instance: format!(
                "{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()
            ),
            worker: None,
            pending: None,
            retiring: None,
            ownership: None,
            stopping: false,
            cleanup_error: None,
            state: DriverState::Stopped,
            generation: 0,
            profile: None,
            configuration: None,
            last_error: None,
            logs: VecDeque::new(),
            log_sequence: 0,
        }
    }
    fn log(&mut self, mut line: String) {
        self.log_sequence = self.log_sequence.saturating_add(1);
        if line.len() > control::MAX_LOG_LINE_BYTES {
            let mut end = control::MAX_LOG_LINE_BYTES;
            while !line.is_char_boundary(end) {
                end -= 1;
            }
            line.truncate(end);
        }
        if self.logs.len() == control::MAX_LOG_LINES {
            self.logs.pop_front();
        }
        self.logs.push_back(line);
    }
    pub(super) fn scheduling_warning(&mut self, error: String) {
        self.log(format!("Experimental driver CPU affinity was not applied: {error}"));
    }
    fn identity(&self) -> WorkerIdentity {
        WorkerIdentity {
            instance: self.instance.clone(),
            generation: self.generation,
        }
    }
    fn check_identity(&self, expected: &WorkerIdentity) -> Result<(), ControlError> {
        if *expected != self.identity() {
            return Err(ControlError::new(
                ErrorCode::Conflict,
                "The daemon or driver generation changed. Refresh status before retrying.",
            ));
        }
        Ok(())
    }
    fn status(&self) -> Reply {
        Reply::Status {
            status: ControlStatus {
                instance: self.instance.clone(),
                state: self.state,
                generation: self.generation,
                profile: self.profile.clone(),
                last_error: self.last_error.clone(),
                logs: self.logs.iter().cloned().collect(),
                log_sequence: self.log_sequence,
            },
        }
    }
    fn prepare(text: Option<String>) -> Result<(Profile, String), ControlError> {
        let profile = match text {
            Some(text) => Profile::from_toml_text(&text, Path::new("daemon-request.toml")),
            None => crate::load_profile(None, None),
        }
        .and_then(|profile| {
            profile.validate_runtime_tablet()?;
            profile.validate_filter_execution()?;
            if profile.tablet_name()?.is_some() && profile.relative.is_none() {
                crate::display::read_snapshot()?.mapper(&profile)?;
            }
            Ok(profile)
        })
        .map_err(|error| ControlError::new(ErrorCode::InvalidProfile, error))?;
        let text = profile
            .to_toml()
            .map_err(|error| ControlError::new(ErrorCode::InvalidProfile, error))?;
        let encoded = serde_json::to_vec(&text)
            .map_err(|error| ControlError::new(ErrorCode::Internal, error.to_string()))?;
        if text.len() > control::MAX_PROFILE_BYTES
            || encoded.len() > control::MAX_FRAME_BYTES - 1024
        {
            return Err(ControlError::new(
                ErrorCode::InvalidProfile,
                "Profile exceeds daemon control limits; use foreground run for this profile.",
            ));
        }
        Ok((profile, text))
    }
    fn next_generation(&self) -> Result<u64, ControlError> {
        self.generation
            .checked_add(1)
            .ok_or_else(|| ControlError::new(ErrorCode::Internal, "generation counter exhausted"))
    }
    fn begin(
        &mut self,
        profile: Profile,
        text: String,
        initial: bool,
    ) -> Result<Reply, ControlError> {
        if self.cancelled.load(Ordering::Acquire)
            || self.stopping
            || self.pending.is_some()
            || self.retiring.is_some()
        {
            return Err(ControlError::new(
                ErrorCode::Busy,
                "driver is transitioning or daemon is shutting down",
            ));
        }
        if initial && (self.worker.is_some() || self.ownership.is_some()) {
            return Err(ControlError::new(
                ErrorCode::Busy,
                "driver is active or cleanup is incomplete",
            ));
        }
        let generation = self.next_generation()?;
        let name = profile.source.clone();
        let worker = Worker::spawn(profile)
            .map_err(|error| ControlError::new(ErrorCode::StartFailed, error))?;
        if initial {
            // A start reserves its identity immediately so StopIf can cancel
            // preparation using the generation returned by Started.
            self.generation = generation;
            self.profile = Some(name.clone());
            self.configuration = Some(text.clone());
            self.state = DriverState::Starting;
            self.last_error = None;
            self.cleanup_error = None;
            self.logs.clear();
        }
        self.pending = Some(Pending {
            worker: Some(worker),
            expected: self.identity(),
            profile: name,
            text,
            generation,
            initial,
            phase: Phase::Preparing,
            error: None,
        });
        self.log(
            if initial {
                "Start accepted; preparing worker resources."
            } else {
                "Apply accepted; current output continues while replacement resources are prepared."
            }
            .into(),
        );
        Ok(if initial {
            Reply::Started { generation }
        } else {
            Reply::RestartAccepted {
                generation: self.generation,
            }
        })
    }
    fn remember_cleanup_error(&mut self, error: String) {
        let combined = self
            .cleanup_error
            .take()
            .map_or_else(|| error.clone(), |previous| format!("{previous}; {error}"));
        self.last_error = Some(combined.clone());
        self.cleanup_error = Some(combined);
        self.log(error);
    }
    fn stop_all(&mut self) -> Result<(), String> {
        self.stopping = true;
        self.state = DriverState::Stopping;
        let mut errors = Vec::new();
        for worker in [
            self.worker.as_ref(),
            self.pending
                .as_ref()
                .and_then(|pending| pending.worker.as_ref()),
            self.retiring.as_ref(),
        ]
        .into_iter()
        .flatten()
        {
            if let Err(error) = worker.stop() {
                errors.push(error);
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("; "))
        }
    }
    fn fail_stop(&mut self, error: String) {
        self.remember_cleanup_error(error);
        if let Err(error) = self.stop_all() {
            self.remember_cleanup_error(error);
        }
    }
    fn send_active(&mut self, directive: Directive) -> bool {
        let result = self
            .worker
            .as_ref()
            .ok_or_else(|| "active worker disappeared".to_owned())
            .and_then(|worker| worker.command(directive));
        if let Err(error) = result {
            self.fail_stop(error);
            false
        } else {
            true
        }
    }
    fn send_candidate(&mut self, directive: Directive) -> bool {
        let result = self
            .pending
            .as_ref()
            .and_then(|pending| pending.worker.as_ref())
            .ok_or_else(|| "candidate worker disappeared".to_owned())
            .and_then(|worker| worker.command(directive));
        if let Err(error) = result {
            self.fail_stop(error);
            false
        } else {
            true
        }
    }
    fn reject_candidate(&mut self, error: String, rollback: bool) {
        let Some(pending) = self.pending.as_mut() else {
            return;
        };
        pending.error = Some(error.clone());
        pending.phase = if rollback {
            Phase::RollbackDrain
        } else {
            Phase::AbortKeepOld
        };
        let stop = pending.worker.as_ref().map_or(Ok(()), Worker::stop);
        self.last_error = Some(error.clone());
        self.log(error);
        if let Err(error) = stop {
            self.fail_stop(error);
        }
    }
    fn active_notice(&mut self, notice: Notice) {
        if self.stopping {
            return;
        }
        let phase = self.pending.as_ref().map(|pending| pending.phase);
        match notice {
            Notice::Prepared => {
                // A disconnect can race the queued quiesce. Do not enqueue a
                // second activation behind that command and accidentally resume
                // the old worker while the candidate is being initialized.
                if !matches!(
                    phase,
                    Some(
                        Phase::Quiescing
                            | Phase::Activating
                            | Phase::CommitSent
                            | Phase::RollbackDrain
                    )
                ) {
                    self.send_active(Directive::Activate);
                }
            }
            Notice::ActivationReady => {
                if !matches!(
                    phase,
                    Some(
                        Phase::Quiescing
                            | Phase::Activating
                            | Phase::CommitSent
                            | Phase::RollbackDrain
                    )
                ) && self.send_active(Directive::Run)
                    && let Some(pending) = &mut self.pending
                    && pending.phase == Phase::Resuming
                {
                    pending.phase = Phase::ResumeSent;
                }
            }
            Notice::Running => {
                if !matches!(
                    phase,
                    Some(
                        Phase::Quiescing
                            | Phase::Activating
                            | Phase::CommitSent
                            | Phase::RollbackDrain
                    )
                ) {
                    self.state = DriverState::Running;
                }
                if self
                    .pending
                    .as_ref()
                    .is_some_and(|pending| pending.phase == Phase::ResumeSent)
                {
                    self.pending = None;
                    self.log("Replacement failed; previous profile resumed with fresh output/relative state. Retained plugins received reset/range-loss notification; their private state depends on their implementation. A physical disconnect recreates instances. Hardware initialization was reapplied, not undone.".into());
                }
            }
            Notice::Quiesced => {
                if self
                    .pending
                    .as_ref()
                    .is_some_and(|pending| pending.phase == Phase::Quiescing)
                {
                    self.pending.as_mut().unwrap().phase = Phase::Activating;
                    self.state = DriverState::Starting;
                    self.send_candidate(Directive::Activate);
                }
            }
            Notice::Waiting => {
                self.state = DriverState::Starting;
                if self
                    .pending
                    .as_ref()
                    .is_some_and(|pending| pending.phase == Phase::Preparing)
                {
                    self.reject_candidate("Apply cancelled because the current device session changed during preparation.".into(), false);
                }
            }
            Notice::ActivationFailed(error) => self.fail_stop(format!(
                "Current profile activation/rollback failed: {error}"
            )),
        }
    }
    fn candidate_notice(&mut self, notice: Notice) {
        if self.stopping {
            return;
        }
        let Some(pending) = &self.pending else {
            return;
        };
        let phase = pending.phase;
        let initial = pending.initial;
        if matches!(
            phase,
            Phase::AbortKeepOld | Phase::RollbackDrain | Phase::Resuming | Phase::ResumeSent
        ) {
            return;
        }
        match notice {
            Notice::Prepared if phase == Phase::Preparing => {
                if pending.expected != self.identity() {
                    self.reject_candidate(
                        "Apply cancelled because its expected driver generation changed.".into(),
                        false,
                    );
                    return;
                }
                if initial {
                    match Ownership::acquire() {
                        Ok(ownership) => self.ownership = Some(ownership),
                        Err(error) => {
                            self.reject_candidate(error, false);
                            return;
                        }
                    }
                    self.pending.as_mut().unwrap().phase = Phase::Activating;
                    self.send_candidate(Directive::Activate);
                } else if self.state == DriverState::Running
                    && self
                        .worker
                        .as_ref()
                        .is_some_and(|worker| !worker.finished())
                {
                    self.pending.as_mut().unwrap().phase = Phase::Quiescing;
                    self.state = DriverState::Stopping;
                    self.log(
                        "Replacement prepared; draining current reader and releasing its output."
                            .into(),
                    );
                    self.send_active(Directive::Quiesce);
                } else {
                    self.reject_candidate(
                        "Apply cancelled because the current worker is no longer ready.".into(),
                        false,
                    );
                }
            }
            Notice::ActivationReady if phase == Phase::Activating => {
                if pending.expected != self.identity() {
                    self.reject_candidate(
                        "Apply generation changed before activation.".into(),
                        !initial,
                    );
                    return;
                }
                // Run is the commit boundary. The worker has not issued a pen
                // read or dispatched live input before receiving this command.
                if !self.send_candidate(Directive::Run) {
                    return;
                }
                let pending = self.pending.as_mut().unwrap();
                pending.phase = Phase::CommitSent;
                self.generation = pending.generation;
                self.profile = Some(pending.profile.clone());
                self.configuration = Some(pending.text.clone());
                self.last_error = None;
            }
            Notice::Running if phase == Phase::CommitSent => {
                let mut pending = self.pending.take().unwrap();
                let previous = self.worker.take();
                self.worker = pending.worker.take();
                self.state = DriverState::Running;
                if let Some(previous) = previous {
                    let stop = previous.stop();
                    self.retiring = Some(previous);
                    if let Err(error) = stop {
                        self.fail_stop(error);
                        return;
                    }
                }
                self.log("Driver generation activated; replacement starts with fresh filter/output/relative state.".into());
            }
            Notice::ActivationFailed(error) if phase == Phase::Activating => {
                self.reject_candidate(error, !initial)
            }
            Notice::Waiting => self.log(
                "Replacement is waiting for its tablet; current generation remains unchanged."
                    .into(),
            ),
            _ => self.fail_stop("Unexpected candidate lifecycle transition.".into()),
        }
    }
    fn reap(&mut self) {
        if self.worker.as_ref().is_some_and(Worker::finished) {
            let result = self.worker.take().unwrap().join();
            if self.stopping {
                if let Err(error) = result {
                    self.remember_cleanup_error(error);
                }
            } else {
                self.fail_stop(
                    result
                        .err()
                        .unwrap_or_else(|| "active worker stopped unexpectedly".into()),
                );
            }
        }
        if self.retiring.as_ref().is_some_and(Worker::finished)
            && let Err(error) = self.retiring.take().unwrap().join()
        {
            self.fail_stop(format!("Previous graph retirement failed: {error}"));
        }
        if self
            .pending
            .as_ref()
            .and_then(|pending| pending.worker.as_ref())
            .is_some_and(Worker::finished)
        {
            let pending = self.pending.as_mut().unwrap();
            let result = pending.worker.take().unwrap().join();
            let phase = pending.phase;
            let initial = pending.initial;
            let error = pending.error.clone();
            if self.stopping {
                if let Err(error) = result {
                    self.remember_cleanup_error(error);
                }
                self.pending = None;
            } else if phase == Phase::Preparing || phase == Phase::AbortKeepOld {
                self.pending = None;
                let error = match (error, result.err()) {
                    (Some(error), Some(retirement)) => {
                        format!("{error}; candidate retirement also failed: {retirement}")
                    }
                    (Some(error), None) | (None, Some(error)) => error,
                    (None, None) => "candidate stopped during preparation".into(),
                };
                self.last_error = Some(error.clone());
                self.log(format!("Replacement preparation rejected: {error}"));
                if initial {
                    self.fail_stop(error);
                }
                // A failed preparation has never owned output. Keep the old
                // thread and current generation completely intact.
            } else if phase == Phase::RollbackDrain && result.is_ok() {
                self.pending.as_mut().unwrap().phase = Phase::Resuming;
                self.state = DriverState::Starting;
                self.send_active(Directive::Activate);
            } else {
                self.fail_stop(
                    result
                        .err()
                        .unwrap_or_else(|| "candidate ended before completing activation".into()),
                );
                self.pending = None;
            }
        }
        if self.stopping
            && self.worker.is_none()
            && self.retiring.is_none()
            && self
                .pending
                .as_ref()
                .is_none_or(|pending| pending.worker.is_none())
        {
            self.pending = None;
            // Every worker has joined before releasing the driver mutex.
            self.ownership = None;
            self.stopping = false;
            self.state = if self.cleanup_error.is_some() {
                DriverState::Failed
            } else {
                DriverState::Stopped
            };
            if self.state == DriverState::Stopped {
                self.log("Driver stopped; output cleanup completed.".into());
            }
        }
    }
    pub(super) fn cleanup(&mut self) -> Result<(), String> {
        if let Err(error) = self.stop_all() {
            self.remember_cleanup_error(error);
        }
        // Critical notices cannot accumulate unboundedly: each worker waits for
        // a daemon command at every transition. Stop also wakes those gates.
        for worker in [
            self.worker.take(),
            self.pending
                .as_mut()
                .and_then(|pending| pending.worker.take()),
            self.retiring.take(),
        ]
        .into_iter()
        .flatten()
        {
            if let Err(error) = worker.join() {
                self.remember_cleanup_error(error);
            }
        }
        self.pending = None;
        // The mutex remains owned through all worker joins above.
        self.ownership = None;
        self.cleanup_error.clone().map_or(Ok(()), Err)
    }
}

impl ControlHandler for Daemon {
    fn poll(&mut self) {
        if self.cancelled.load(Ordering::Acquire)
            && !self.stopping
            && let Err(error) = self.stop_all()
        {
            self.remember_cleanup_error(error);
        }
        let mut logs = Vec::new();
        for worker in [
            self.worker.as_ref(),
            self.pending
                .as_ref()
                .and_then(|pending| pending.worker.as_ref()),
            self.retiring.as_ref(),
        ]
        .into_iter()
        .flatten()
        {
            logs.extend(worker.logs.try_iter());
        }
        for line in logs {
            self.log(line);
        }
        let notices: Vec<_> = self
            .worker
            .as_ref()
            .map(|worker| worker.notices.try_iter().collect())
            .unwrap_or_default();
        for notice in notices {
            self.active_notice(notice);
        }
        let notices: Vec<_> = self
            .pending
            .as_ref()
            .and_then(|pending| pending.worker.as_ref())
            .map(|worker| worker.notices.try_iter().collect())
            .unwrap_or_default();
        for notice in notices {
            // Running promotes the candidate. Further notices already queued
            // by that same worker (including a fast disconnect/reconnect) now
            // belong to the active role and must not be discarded.
            if self
                .pending
                .as_ref()
                .and_then(|pending| pending.worker.as_ref())
                .is_some()
            {
                self.candidate_notice(notice);
            } else {
                self.active_notice(notice);
            }
        }
        self.reap();
    }
    fn handle(&mut self, command: Command) -> Result<Reply, ControlError> {
        // Cancel before polling so an already queued Prepared/Ready notice
        // cannot activate a replacement in response to a Stop request.
        match &command {
            Command::Stop | Command::Shutdown => self
                .stop_all()
                .map_err(|error| ControlError::new(ErrorCode::StopFailed, error))?,
            Command::StopIf { expected } => {
                self.check_identity(expected)?;
                self.stop_all()
                    .map_err(|error| ControlError::new(ErrorCode::StopFailed, error))?;
            }
            _ => {}
        }
        self.poll();
        match command {
            Command::Status => Ok(self.status()),
            Command::SetExperimental { expected, settings } => {
                self.check_identity(&expected)?;
                let path = crate::experimental::path()
                    .map_err(|error| ControlError::new(ErrorCode::InvalidRequest, error))?;
                crate::experimental::save_driver_at(&path, &settings)
                    .map_err(|error| ControlError::new(ErrorCode::InvalidRequest, error))?;
                self.log(format!("Experimental CPU affinity saved. GUI: {}; driver: {}.",
                    crate::experimental::format_cpus(&settings.ui_cpus),
                    crate::experimental::format_cpus(&settings.driver_cpus)));
                Ok(Reply::ExperimentalSaved)
            }
            Command::GetConfiguration { expected } => {
                self.check_identity(&expected)?;
                Ok(Reply::Configuration {
                    identity: self.identity(),
                    profile_toml: self.configuration.clone(),
                })
            }
            Command::Start { profile_toml } => {
                let (profile, text) = Self::prepare(profile_toml)?;
                self.begin(profile, text, true)
            }
            Command::StartIf {
                expected,
                profile_toml,
            } => {
                self.check_identity(&expected)?;
                let (profile, text) = Self::prepare(Some(profile_toml))?;
                self.begin(profile, text, true)
            }
            Command::Restart {
                expected,
                profile_toml,
            } => {
                self.check_identity(&expected)?;
                if self.worker.is_none() || self.state != DriverState::Running {
                    return Err(ControlError::new(
                        ErrorCode::Busy,
                        "driver is not ready for apply",
                    ));
                }
                let (profile, text) = Self::prepare(Some(profile_toml))?;
                self.begin(profile, text, false)
            }
            Command::Stop | Command::StopIf { .. } => {
                if self.state == DriverState::Stopped {
                    Ok(Reply::Stopped {
                        generation: self.generation,
                    })
                } else {
                    Ok(self.status())
                }
            }
            Command::Shutdown => Ok(Reply::ShutdownAccepted),
            Command::Debug => Ok(Reply::Debug {
                report: crate::decode_cli::debug_report(),
            }),
        }
    }
}
