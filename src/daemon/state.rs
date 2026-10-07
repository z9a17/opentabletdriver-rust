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
    fn foreign_capture_commands_and_stopped_start_do_not_change_driver_lifecycle() {
        let mut daemon = Daemon::new(Arc::new(AtomicBool::new(false)));
        let capture = control::DebugCaptureToken { instance: "other-daemon".into(), epoch: 1, session: 1 };
        for command in [
            Command::DebugCaptureRead { capture: capture.clone(), after_sequence: 0, limit: 1,
                acknowledge_through: 0, lease_ms: 1_000 },
            Command::DebugCaptureStop { capture: capture.clone() },
            Command::DebugCaptureRelease { capture },
        ] {
            assert!(matches!(daemon.handle(command), Err(ControlError { code: ErrorCode::Conflict, .. })));
        }
        let expected = daemon.identity();
        let start = Command::DebugCaptureStart { expected, start_id: 1, capacity_bytes: 65_536, lease_ms: 1_000 };
        assert!(matches!(daemon.handle(start), Err(ControlError { code: ErrorCode::Busy, .. })));
        assert_eq!(daemon.state, DriverState::Stopped);
        assert!(!daemon.stopping);
        assert!(daemon.worker.is_none());
    }
    #[test]
    fn stale_shutdown_does_not_stop_a_replacement_daemon() {
        let mut daemon = Daemon::new(Arc::new(AtomicBool::new(false)));
        let error = daemon.handle(Command::ShutdownIf {
            expected: WorkerIdentity { instance: "previous-daemon".into(), generation: 0 },
        }).unwrap_err();
        assert!(matches!(error.code, ErrorCode::Conflict));
        assert!(!daemon.stopping);
        assert_eq!(daemon.state, DriverState::Stopped);
        assert_eq!(daemon.generation, 0);
    }
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
    applied_profile: Profile,
    generation: u64,
    initial: bool,
    device_start: bool,
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
    devices: Option<crate::companions::Supervisor>,
    primary_stopped: bool,
    primary_stop_generation: Option<u64>,
    stopping: bool,
    cleanup_error: Option<String>,
    state: DriverState,
    generation: u64,
    profile: Option<String>,
    configuration: Option<String>,
    last_error: Option<String>,
    logs: VecDeque<String>,
    log_sequence: u64,
    upstream_logs: VecDeque<(control::UpstreamLogMessage, usize)>,
    upstream_log_bytes: usize,
    update_reservation: Option<String>,
    next_update: u64,
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
            devices: None,
            primary_stopped: false,
            primary_stop_generation: None,
            stopping: false,
            cleanup_error: None,
            state: DriverState::Stopped,
            generation: 0,
            profile: None,
            configuration: None,
            last_error: None,
            logs: VecDeque::new(),
            log_sequence: 0,
            upstream_logs: VecDeque::new(),
            upstream_log_bytes: 0,
            update_reservation: None,
            next_update: 0,
        }
    }
    fn log(&mut self, mut line: String) {
        if line.len() > control::MAX_LOG_LINE_BYTES {
            let mut end = control::MAX_LOG_LINE_BYTES;
            while !line.is_char_boundary(end) { end -= 1; }
            line.truncate(end);
        }
        let message = control::UpstreamLogMessage::native(line.clone());
        self.append_log(line, message);
    }
    fn append_log(&mut self, mut line: String, message: control::UpstreamLogMessage) {
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
        let size = serde_json::to_vec(&message).map_or(control::MAX_UPSTREAM_LOG_BYTES,
            |encoded| encoded.len() + 1);
        while self.upstream_logs.len() >= control::MAX_LOG_LINES
            || self.upstream_log_bytes.saturating_add(size) > control::MAX_UPSTREAM_LOG_BYTES {
            let Some((_, removed)) = self.upstream_logs.pop_front() else { break; };
            self.upstream_log_bytes = self.upstream_log_bytes.saturating_sub(removed);
        }
        self.upstream_log_bytes += size;
        self.upstream_logs.push_back((message, size));
    }
    pub(super) fn scheduling_warning(&mut self, error: String) {
        self.log(format!("Experimental driver CPU affinity was not applied: {error}"));
    }
    pub(crate) fn device_sessions(&self) -> Option<crate::device_sessions::Handle> {
        self.devices.as_ref().map(crate::companions::Supervisor::handle)
    }
    fn primary_device_guard(&self, id: &str, generation: u64) -> Result<crate::device_sessions::SessionSnapshot, ControlError> {
        if self.stopping || self.cancelled.load(Ordering::Acquire) || self.pending.is_some() || self.retiring.is_some() {
            return Err(ControlError::new(ErrorCode::Busy, "primary device is transitioning"));
        }
        let handle = self.device_sessions().ok_or_else(|| ControlError::new(ErrorCode::Busy, "device supervisor is unavailable"))?;
        let snapshot = handle.snapshot().map_err(|error| ControlError::new(ErrorCode::Internal, error))?.sessions.into_iter()
            .find(|session| session.id == id && session.primary)
            .ok_or_else(|| ControlError::new(ErrorCode::Conflict, "requested session is not the primary device"))?;
        if snapshot.device_generation != generation || snapshot.pending_generation.is_some() {
            return Err(ControlError::new(ErrorCode::Conflict, "device generation changed; refresh device sessions"));
        }
        Ok(snapshot)
    }
    pub(crate) fn primary_stop(&mut self, id: &str, generation: u64) -> Result<crate::device_sessions::SessionReceipt, ControlError> {
        self.primary_device_guard(id, generation)?;
        let next = generation.checked_add(1).ok_or_else(|| ControlError::new(ErrorCode::Internal, "device generation exhausted"))?;
        let worker = self.worker.take().ok_or_else(|| ControlError::new(ErrorCode::Busy, "primary device is already stopped"))?;
        let signalled = worker.stop();
        self.retiring = Some(worker);
        self.primary_stop_generation = Some(next);
        self.device_sessions().unwrap().primary_pending(next, crate::device_sessions::SessionState::Stopping);
        signalled.map_err(|error| ControlError::new(ErrorCode::StopFailed, error))?;
        Ok(crate::device_sessions::SessionReceipt { id: id.to_owned(), device_generation: generation,
            target_generation: next, accepted_pending: true })
    }
    pub(crate) fn primary_start(&mut self, id: &str, generation: u64) -> Result<crate::device_sessions::SessionReceipt, ControlError> {
        self.primary_device_guard(id, generation)?;
        if !self.primary_stopped || self.worker.is_some() || self.ownership.is_none() {
            return Err(ControlError::new(ErrorCode::Busy, "primary device is already active or ownership is unavailable"));
        }
        let next = generation.checked_add(1).ok_or_else(|| ControlError::new(ErrorCode::Internal, "device generation exhausted"))?;
        let text = self.device_sessions().unwrap().profile(id, generation).map_err(|error| ControlError::new(ErrorCode::InvalidProfile, error))?;
        let (profile, text) = Self::prepare(Some(text))?;
        self.begin(profile, text, true)?;
        self.device_sessions().unwrap().primary_pending(next, crate::device_sessions::SessionState::Preparing);
        Ok(crate::device_sessions::SessionReceipt { id: id.to_owned(), device_generation: generation,
            target_generation: next, accepted_pending: true })
    }
    pub(crate) fn primary_apply(&mut self, id: &str, generation: u64, profile_toml: String) -> Result<crate::device_sessions::SessionReceipt, ControlError> {
        self.primary_device_guard(id, generation)?;
        let next = generation.checked_add(1).ok_or_else(|| ControlError::new(ErrorCode::Internal, "device generation exhausted"))?;
        let (profile, text) = Self::prepare(Some(profile_toml))?;
        self.device_sessions().unwrap().validate_profile(id, &profile).map_err(|error| ControlError::new(ErrorCode::InvalidProfile, error))?;
        if self.worker.is_none() && self.primary_stopped {
            let handle = self.device_sessions().unwrap();
            self.profile = Some(profile.source.clone());
            self.configuration = Some(text);
            handle.commit(id, next, profile);
            handle.primary_stopped(next, None);
            return Ok(crate::device_sessions::SessionReceipt { id: id.to_owned(), device_generation: generation,
                target_generation: next, accepted_pending: false });
        }
        if self.worker.is_none() { return Err(ControlError::new(ErrorCode::Busy, "primary device is unavailable")); }
        self.begin(profile, text, false)?;
        self.device_sessions().unwrap().primary_pending(next, crate::device_sessions::SessionState::Running);
        Ok(crate::device_sessions::SessionReceipt { id: id.to_owned(), device_generation: generation,
            target_generation: next, accepted_pending: true })
    }
    fn device_lifecycle(&mut self, id: &str, generation: u64, start: bool)
        -> Result<crate::device_sessions::SessionReceipt, ControlError> {
        let handle = self.device_sessions().ok_or_else(|| ControlError::new(ErrorCode::Busy,
            "device supervisor is unavailable"))?;
        let primary = handle.snapshot().map_err(|error| ControlError::new(ErrorCode::Internal, error))?
            .sessions.iter().any(|session| session.id == id && session.primary);
        if primary {
            if start { self.primary_start(id, generation) } else { self.primary_stop(id, generation) }
        } else {
            (if start { handle.start_device(id, generation) } else { handle.stop_device(id, generation) })
                .map_err(|error| ControlError::new(ErrorCode::Conflict, error))
        }
    }
    fn update_ready(&self) -> bool {
        !self.stopping && self.worker.is_none() && self.pending.is_none()
            && self.retiring.is_none() && self.devices.is_none() && self.ownership.is_none()
    }
    fn check_update_token(&self, token: &str) -> Result<(), ControlError> {
        if self.update_reservation.as_deref() != Some(token) {
            return Err(ControlError::new(ErrorCode::Conflict, "update reservation belongs to another request or daemon"));
        }
        Ok(())
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
    fn check_capture_instance(&self, capture: &control::DebugCaptureToken) -> Result<(), ControlError> {
        if capture.instance != self.instance {
            return Err(ControlError::new(ErrorCode::Conflict, "capture belongs to a different daemon instance"));
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
            None => crate::load_runtime_profile(None, None),
        }
        .and_then(|profile| {
            crate::plugins::validate_runtime_profile(&profile)?;
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
        if initial && (self.worker.is_some() || (self.ownership.is_some() && !self.primary_stopped)) {
            return Err(ControlError::new(
                ErrorCode::Busy,
                "driver is active or cleanup is incomplete",
            ));
        }
        let device_start = initial && self.ownership.is_some() && self.primary_stopped;
        let generation = self.next_generation()?;
        let name = profile.source.clone();
        if self.devices.is_none() {
            let database = crate::config::configured_tablets()
                .map_err(|error| ControlError::new(ErrorCode::StartFailed, error))?;
            crate::plugins::prepare_connected_parsers(database.as_ref())
                .map_err(|error| ControlError::new(ErrorCode::StartFailed, error))?;
            self.devices = Some(crate::companions::Supervisor::prepare(profile.clone())
                .map_err(|error| ControlError::new(ErrorCode::StartFailed, error))?);
        }
        let device_handle = self.devices.as_ref().unwrap().handle();
        let worker = match Worker::spawn(profile.clone(), device_handle, initial && !device_start) {
            Ok(worker) => worker,
            Err(error) => {
                if initial && !device_start {
                    if let Some(mut devices) = self.devices.take() { let _ = devices.finish(); }
                }
                return Err(ControlError::new(ErrorCode::StartFailed, error));
            }
        };
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
        if let Some(devices) = &self.devices {
            if let Ok(list) = devices.handle().snapshot() {
                if let Some(primary) = list.sessions.into_iter().find(|session| session.primary) {
                    if let Some(next) = primary.device_generation.checked_add(1) {
                        devices.handle().primary_pending(next, if initial { crate::device_sessions::SessionState::Preparing } else { primary.state });
                    }
                }
            }
        }
        self.pending = Some(Pending {
            worker: Some(worker),
            expected: self.identity(),
            profile: name,
            text,
            applied_profile: profile,
            generation,
            initial,
            device_start,
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
        if let Some(devices) = &self.devices { if let Err(error) = devices.stop() { errors.push(error); } }
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
            Notice::PreparedProfile(_) => {}
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
                    if let Some(devices) = &self.devices { devices.handle().primary_state(crate::device_sessions::SessionState::Running, None); }
                }
                if self
                    .pending
                    .as_ref()
                    .is_some_and(|pending| pending.phase == Phase::ResumeSent)
                {
                    if let Some(devices) = &self.devices {
                        let handle = devices.handle();
                        if let Ok(Some(id)) = handle.primary_id() { handle.reject(&id, self.last_error.clone().unwrap_or_else(|| "replacement failed".into())); }
                    }
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
                if let Some(devices) = &self.devices { devices.handle().primary_state(crate::device_sessions::SessionState::Waiting, None); }
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
            Notice::PreparedProfile(profile) if phase == Phase::Preparing => {
                match profile.to_toml() {
                    Ok(text) if text.len() <= control::MAX_PROFILE_BYTES && serde_json::to_vec(&text).is_ok_and(|encoded| encoded.len() <= control::MAX_FRAME_BYTES - 1024) => {
                        let pending = self.pending.as_mut().unwrap();
                        pending.profile = profile.source.clone();
                        pending.text = text;
                        pending.applied_profile = *profile;
                    }
                    Ok(_) => self.reject_candidate("effective physical profile exceeds control frame limits".into(), false),
                    Err(error) => self.reject_candidate(error, false),
                }
            }
            Notice::Prepared if phase == Phase::Preparing => {
                if pending.expected != self.identity() {
                    self.reject_candidate(
                        "Apply cancelled because its expected driver generation changed.".into(),
                        false,
                    );
                    return;
                }
                if initial {
                    if self.ownership.is_none() {
                        match Ownership::acquire() {
                            Ok(ownership) => self.ownership = Some(ownership),
                            Err(error) => {
                                self.reject_candidate(error, false);
                                return;
                            }
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
                if let Some(devices) = &self.devices {
                    if let Err(error) = devices.handle().primary_commit(pending.applied_profile.clone()) {
                        self.fail_stop(error); return;
                    }
                }
                self.last_error = None;
            }
            Notice::Running if phase == Phase::CommitSent => {
                let mut pending = self.pending.take().unwrap();
                let previous = self.worker.take();
                self.worker = pending.worker.take();
                self.state = DriverState::Running;
                self.primary_stopped = false;
                if let Some(devices) = &self.devices {
                    devices.handle().primary_state(crate::device_sessions::SessionState::Running, None);
                    if let Ok(Some(id)) = devices.handle().primary_id() {
                        if devices.handle().snapshot().is_ok_and(|list| list.selected_id.as_ref() == Some(&id)) { let _ = devices.handle().select(&id); }
                    }
                    devices.enable();
                }
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
            Notice::Waiting => {
                if initial && self.ownership.is_none() {
                    match Ownership::acquire() {
                        Ok(ownership) => {
                            self.ownership = Some(ownership);
                            if let Some(devices) = &self.devices { devices.enable(); }
                        }
                        Err(error) => { self.reject_candidate(error, false); return; }
                    }
                }
                self.log("Replacement is waiting for its tablet; current generation remains unchanged.".into());
            }
            _ => self.fail_stop("Unexpected candidate lifecycle transition.".into()),
        }
    }
    fn reap(&mut self) {
        if self.devices.as_ref().is_some_and(crate::companions::Supervisor::finished) {
            let result = self.devices.as_mut().unwrap().finish();
            self.devices = None;
            if let Err(error) = result {
                if self.stopping { self.remember_cleanup_error(error); } else { self.fail_stop(error); }
            } else if !self.stopping { self.fail_stop("device supervisor ended unexpectedly".into()); }
        }
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
        if self.retiring.as_ref().is_some_and(Worker::finished) {
            let result = self.retiring.take().unwrap().join();
            if let Some(generation) = self.primary_stop_generation.take() {
                if self.stopping {
                    if let Err(error) = result { self.remember_cleanup_error(error); }
                } else {
                    self.primary_stopped = true;
                    self.state = DriverState::Running;
                    if let Some(devices) = &self.devices { devices.handle().primary_stopped(generation, result.as_ref().err().cloned()); }
                    if let Err(error) = result { self.last_error = Some(error.clone()); self.log(error); }
                    self.log("Primary tablet stopped; independently owned tablet sessions remain enabled.".into());
                }
            } else if let Err(error) = result {
                self.fail_stop(format!("Previous graph retirement failed: {error}"));
            }
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
            let device_start = pending.device_start;
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
                if let Some(devices) = &self.devices {
                    let handle = devices.handle();
                    if let Ok(Some(id)) = handle.primary_id() { handle.reject(&id, error.clone()); }
                    if !initial { handle.primary_state(crate::device_sessions::SessionState::Running, Some(error.clone())); }
                }
                if initial && !device_start {
                    self.fail_stop(error);
                } else if device_start {
                    self.primary_stopped = true;
                    self.state = DriverState::Running;
                    if let Some(devices) = &self.devices {
                        let handle = devices.handle();
                        if let Ok(Some(id)) = handle.primary_id() { handle.reject(&id, error.clone()); }
                        handle.primary_state(crate::device_sessions::SessionState::Failed, Some(error));
                    }
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
            && self.devices.is_none()
            && self.worker.is_none()
            && self.retiring.is_none()
            && self
                .pending
                .as_ref()
                .is_none_or(|pending| pending.worker.is_none())
        {
            self.pending = None;
            self.primary_stop_generation = None;
            self.primary_stopped = false;
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
        if let Some(mut devices) = self.devices.take() {
            if let Err(error) = devices.finish() { self.remember_cleanup_error(error); }
        }
        // The mutex remains owned through all worker joins above.
        self.ownership = None;
        self.cleanup_error.clone().map_or(Ok(()), Err)
    }
}

#[cfg(test)]
mod upstream_log_tests {
    use super::*;
    #[test]
    fn retained_messages_preserve_original_fields_and_fit_the_control_frame() {
        let mut daemon = Daemon::new(Arc::new(AtomicBool::new(false)));
        let original: control::UpstreamLogMessage = serde_json::from_value(serde_json::json!({
            "Time":"2026-10-07T09:08:07.654+02:00", "Group":null,
            "Message":"\u{0001}".repeat(control::MAX_LOG_LINE_BYTES),
            "StackTrace":"\u{0001}".repeat(4096), "Level":4,"Notification":true
        })).unwrap();
        for _ in 0..control::MAX_LOG_LINES {
            let request = control::Request::new(1, Command::WriteMessage { message: original.clone() });
            request.validate().unwrap();
            daemon.handle(request.command).unwrap();
        }
        match daemon.handle(Command::GetUpstreamLog).unwrap() {
            Reply::UpstreamLog { sequence, messages, .. } => {
                assert_eq!(sequence, control::MAX_LOG_LINES as u64);
                assert!(messages.len() < control::MAX_LOG_LINES);
                for message in messages { assert_eq!(serde_json::to_value(message).unwrap(),
                    serde_json::to_value(&original).unwrap()); }
            }
            _ => panic!("wrong retained log response"),
        }
        let response = control::Response { version: control::PROTOCOL_VERSION, id: 2,
            reply: daemon.handle(Command::GetUpstreamLog).unwrap() };
        assert!(serde_json::to_vec(&response).unwrap().len() < control::MAX_FRAME_BYTES);
    }
    #[test]
    fn native_messages_keep_creation_time_across_snapshots() {
        let mut daemon = Daemon::new(Arc::new(AtomicBool::new(false)));
        daemon.log("ready".into());
        let first = serde_json::to_value(daemon.handle(Command::GetUpstreamLog).unwrap()).unwrap();
        let second = serde_json::to_value(daemon.handle(Command::GetUpstreamLog).unwrap()).unwrap();
        assert_eq!(first, second);
    }
}

impl ControlHandler for Daemon {
    fn poll(&mut self) {
        otd_core::debug::capture_poll();
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
            Command::StopIf { expected } | Command::ShutdownIf { expected } => {
                self.check_identity(expected)?;
                self.stop_all()
                    .map_err(|error| ControlError::new(ErrorCode::StopFailed, error))?;
            }
            _ => {}
        }
        self.poll();
        if self.update_reservation.is_some() && matches!(&command,
            Command::Start { .. } | Command::StartIf { .. } | Command::Restart { .. }
            | Command::ApplyDeviceProfile { .. } | Command::SaveDeviceProfile { .. } | Command::StartDevice { .. }
            | Command::SelectDeviceSession { .. } | Command::DetectDeviceSessions) {
            return Err(ControlError::new(ErrorCode::Busy, "an update owns the stopped driver; wait for completion or cancel its reservation"));
        }
        match command {
            Command::Status => Ok(self.status()),
            Command::BeginUpdate { expected } => {
                self.check_identity(&expected)?;
                if self.update_reservation.is_some() { return Err(ControlError::new(ErrorCode::Busy, "another update is already reserved")); }
                self.next_update = self.next_update.checked_add(1)
                    .ok_or_else(|| ControlError::new(ErrorCode::Internal, "update reservation sequence exhausted"))?;
                let token = format!("{}:update:{}", self.instance, self.next_update);
                self.update_reservation = Some(token.clone());
                if let Err(error) = self.stop_all() {
                    self.update_reservation = None;
                    return Err(ControlError::new(ErrorCode::StopFailed, error));
                }
                self.log("Update reserved; all tablet sessions and global tools are draining before installation.".into());
                Ok(Reply::UpdateAccepted { token })
            }
            Command::UpdateStatus { token } => {
                self.check_update_token(&token)?;
                Ok(Reply::UpdateState { token, ready: self.update_ready() && self.cleanup_error.is_none(),
                    error: self.cleanup_error.clone() })
            }
            Command::FinishUpdate { token, success } => {
                self.check_update_token(&token)?;
                if success {
                    if !self.update_ready() || self.cleanup_error.is_some() {
                        return Err(ControlError::new(ErrorCode::Busy, "update cannot exit before successful device cleanup"));
                    }
                    self.log("Update staged and response delivered; shutting down the reserved daemon.".into());
                    Ok(Reply::ShutdownAccepted)
                } else {
                    self.update_reservation = None;
                    self.log("Update cancelled. Tablet input remains stopped; Start driver explicitly resumes it.".into());
                    Ok(Reply::UpdateCancelled)
                }
            }
            Command::DetectDeviceSessions => {
                let handle = self.device_sessions().ok_or_else(|| ControlError::new(ErrorCode::Busy,
                    "no active device supervisor; Start driver enables discovery"))?;
                let list = handle.refresh().map_err(|error| ControlError::new(ErrorCode::Internal, error))?;
                Ok(Reply::DeviceSessions { sessions: list.sessions, selected_id: list.selected_id })
            }
            Command::ListDeviceSessions => {
                let list = self.device_sessions().map(|handle| handle.snapshot()).transpose()
                    .map_err(|error| ControlError::new(ErrorCode::Internal, error))?;
                Ok(Reply::DeviceSessions { sessions: list.as_ref().map_or_else(Vec::new, |list| list.sessions.clone()),
                    selected_id: list.and_then(|list| list.selected_id) })
            }
            Command::SelectDeviceSession { expected, id } => {
                self.check_identity(&expected)?;
                let handle = self.device_sessions().ok_or_else(|| ControlError::new(ErrorCode::Busy,
                    "device supervisor is unavailable"))?;
                handle.select(&id).map_err(|error| ControlError::new(ErrorCode::Busy, error))?;
                Ok(Reply::DeviceSessionSelected { id })
            }
            Command::GetDeviceProfile { expected, id, device_generation } => {
                self.check_identity(&expected)?;
                let handle = self.device_sessions().ok_or_else(|| ControlError::new(ErrorCode::Busy,
                    "device supervisor is unavailable"))?;
                let profile_toml = handle.profile(&id, device_generation)
                    .map_err(|error| ControlError::new(ErrorCode::Conflict, error))?;
                Ok(Reply::DeviceProfile { identity: self.identity(), id, device_generation, profile_toml })
            }
            Command::ApplyDeviceProfile { expected, id, device_generation, profile_toml } => {
                self.check_identity(&expected)?;
                let handle = self.device_sessions().ok_or_else(|| ControlError::new(ErrorCode::Busy,
                    "device supervisor is unavailable"))?;
                let primary = handle.snapshot().map_err(|error| ControlError::new(ErrorCode::Internal, error))?
                    .sessions.iter().any(|session| session.id == id && session.primary);
                let receipt = if primary { self.primary_apply(&id, device_generation, profile_toml)? }
                    else {
                        let (profile, _) = Self::prepare(Some(profile_toml))?;
                        handle.apply(&id, device_generation, profile)
                            .map_err(|error| ControlError::new(ErrorCode::Conflict, error))?
                    };
                Ok(Reply::DeviceOperationAccepted { receipt })
            }
            Command::StopDevice { expected, id, device_generation } => {
                self.check_identity(&expected)?;
                Ok(Reply::DeviceOperationAccepted { receipt: self.device_lifecycle(&id, device_generation, false)? })
            }
            Command::SaveDeviceProfile { expected, id, device_generation, expected_revision, expected_digest, profile_toml } => {
                self.check_identity(&expected)?;
                let handle = self.device_sessions().ok_or_else(|| ControlError::new(ErrorCode::Busy, "device supervisor is unavailable"))?;
                let path = otd_core::storage::data_directory().map_err(|error| ControlError::new(ErrorCode::Internal, error))?.join("driver.toml");
                let profile = Profile::from_toml_text(&profile_toml, &path).map_err(|error| ControlError::new(ErrorCode::InvalidProfile, error))?;
                let saved = handle.save_profile(&id, device_generation, expected_revision, expected_digest.as_deref(), &profile)
                    .map_err(|error| ControlError::new(ErrorCode::Conflict, error))?;
                Ok(Reply::DeviceProfileSaved { saved })
            }
            Command::StartDevice { expected, id, device_generation } => {
                self.check_identity(&expected)?;
                Ok(Reply::DeviceOperationAccepted { receipt: self.device_lifecycle(&id, device_generation, true)? })
            }
            Command::WriteMessage { message } => {
                let line = format!("[{}] {}", message.group.as_deref().unwrap_or(""),
                    message.message.as_deref().unwrap_or(""));
                self.append_log(line, message);
                Ok(Reply::MessageWritten)
            }
            Command::GetUpstreamLog => Ok(Reply::UpstreamLog { instance: self.instance.clone(),
                sequence: self.log_sequence, messages: self.upstream_logs.iter()
                    .map(|(message, _)| message.clone()).collect() }),
            Command::SetExperimental { expected, settings } => {
                self.check_identity(&expected)?;
                let path = crate::experimental::path()
                    .map_err(|error| ControlError::new(ErrorCode::InvalidRequest, error))?;
                crate::experimental::save_driver_at(&path, &settings)
                    .map_err(|error| ControlError::new(ErrorCode::InvalidRequest, error))?;
                self.log(format!("Experimental settings saved. GUI CPUs: {}; driver CPUs: {}; MMCSS Pro Audio: {} for the next tablet session.",
                    crate::experimental::format_cpus(&settings.ui_cpus),
                    crate::experimental::format_cpus(&settings.driver_cpus),
                    if settings.mmcss { "enabled" } else { "disabled" }));
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
            Command::Shutdown | Command::ShutdownIf { .. } => Ok(Reply::ShutdownAccepted),
            Command::Debug => Ok(Reply::Debug {
                report: crate::decode_cli::debug_report(),
            }),
            Command::DebugCaptureStart { expected, start_id, capacity_bytes, lease_ms } => {
                self.check_identity(&expected)?;
                if self.state != DriverState::Running || self.pending.is_some() || self.retiring.is_some() {
                    return Err(ControlError::new(ErrorCode::Busy, "no stable running tablet session to capture"));
                }
                let capture = otd_core::debug::capture_start(start_id, capacity_bytes as usize, lease_ms)
                    .map_err(ControlError::from)?;
                Ok(Reply::DebugCaptureStarted {
                    capture: crate::decode_cli::debug_capture_status(&self.instance, capture),
                })
            }
            Command::DebugCaptureRead { capture, after_sequence, limit, acknowledge_through, lease_ms } => {
                self.check_capture_instance(&capture)?;
                let batch = otd_core::debug::capture_read(capture.epoch, capture.session,
                    after_sequence, limit as usize, acknowledge_through, lease_ms)
                    .map_err(ControlError::from)?;
                Ok(Reply::DebugCaptureRead {
                    batch: crate::decode_cli::debug_capture_batch(&self.instance, batch),
                })
            }
            Command::DebugCaptureStop { capture } => {
                self.check_capture_instance(&capture)?;
                let capture = otd_core::debug::capture_stop(capture.epoch, capture.session)
                    .map_err(ControlError::from)?;
                Ok(Reply::DebugCaptureStopped {
                    capture: crate::decode_cli::debug_capture_status(&self.instance, capture),
                })
            }
            Command::DebugCaptureRelease { capture } => {
                self.check_capture_instance(&capture)?;
                otd_core::debug::capture_release(capture.epoch, capture.session)
                    .map_err(ControlError::from)?;
                Ok(Reply::DebugCaptureReleased)
            }
        }
    }
}
