//! Versioned local daemon control, separate from the report-processing thread.
//!
//! One bounded JSON request and response per connection, framed with a little-
//! endian u32 byte length. Clients acknowledge a complete response with 0x06
//! before the server disconnects, avoiding discarded unread pipe data. Request
//! IDs correlate replies; they are not deduplication tokens. After a connection
//! failure a command may already have taken effect: query status before retrying.

pub(crate) mod pipe;
mod log_time;

use serde::{Deserialize, Serialize};
use std::io;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

pub const PROTOCOL_VERSION: u32 = 2;
pub const MAX_FRAME_BYTES: usize = 256 * 1024;
pub const MAX_PROFILE_BYTES: usize = 128 * 1024;
pub const MAX_LOG_LINES: usize = 64;
// Even JSON's worst-case six-byte escaping keeps a full status under 256 KiB.
pub const MAX_LOG_LINE_BYTES: usize = 512;
/// Serialized message budget leaves room for the control response envelope.
pub const MAX_UPSTREAM_LOG_BYTES: usize = MAX_FRAME_BYTES - 8192;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub version: u32,
    /// Nonzero; zero is reserved for malformed requests without a usable ID.
    pub id: u64,
    pub command: Command,
}

impl Request {
    pub fn new(id: u64, command: Command) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            id,
            command,
        }
    }

    pub(crate) fn validate(&self) -> Result<(), ControlError> {
        if self.version != PROTOCOL_VERSION {
            return Err(ControlError::new(
                ErrorCode::UnsupportedVersion,
                format!(
                    "protocol version {} is unsupported; use {}",
                    self.version, PROTOCOL_VERSION
                ),
            ));
        }
        if self.id == 0 {
            return Err(ControlError::new(
                ErrorCode::InvalidRequest,
                "request ID must be nonzero",
            ));
        }
        let profile = match &self.command {
            Command::Start { profile_toml } => profile_toml.as_ref(),
            Command::StartIf { profile_toml, .. } | Command::Restart { profile_toml, .. } => {
                Some(profile_toml)
            }
            Command::ApplyDeviceProfile { profile_toml, .. }
            | Command::ApplyDeviceProfileWithInhibit { profile_toml, .. }
            | Command::ApplyOriginalDeviceProfile { profile_toml, .. }
            | Command::SaveDeviceProfile { profile_toml, .. } => Some(profile_toml),
            _ => None,
        };
        if profile.is_some_and(|profile| profile.len() > MAX_PROFILE_BYTES) {
            return Err(ControlError::new(
                ErrorCode::InvalidProfile,
                "profile exceeds 128 KiB",
            ));
        }
        match &self.command {
            Command::UpdateStatus { token } | Command::FinishUpdate { token, .. } => {
                if token.is_empty() || token.len() > 256 || token.chars().any(char::is_control) {
                    return Err(ControlError::new(ErrorCode::InvalidRequest, "invalid update reservation token"));
                }
            }
            Command::WriteMessage { message } => {
                if !log_time::valid(&message.time) {
                    return Err(ControlError::new(ErrorCode::InvalidRequest, "log Time must be a valid ISO or Microsoft JSON DateTime"));
                }
                if message.message.as_ref().is_some_and(|value| value.len() > MAX_LOG_LINE_BYTES)
                    || message.group.as_ref().is_some_and(|value| value.len() > 128)
                    || message.time.len() > 64 || message.stack_trace.as_ref().is_some_and(|value| value.len() > 4096)
                    || !(0..=4).contains(&message.level) {
                    return Err(ControlError::new(ErrorCode::InvalidRequest, "log message exceeds limits or has an unknown level"));
                }
            }
            Command::SelectDeviceSession { id, .. }
            | Command::GetDeviceProfile { id, .. }
            | Command::ApplyDeviceProfile { id, .. }
            | Command::ApplyDeviceProfileWithInhibit { id, .. }
            | Command::ApplyOriginalDeviceProfile { id, .. }
            | Command::SaveDeviceProfile { id, .. }
            | Command::StopDevice { id, .. }
            | Command::StartDevice { id, .. } => {
                if id.is_empty() || id.len() > 128 || id.chars().any(char::is_control) {
                    return Err(ControlError::new(ErrorCode::InvalidRequest, "invalid device session ID"));
                }
            }
            Command::SetIdleOriginalSettings { settings_json, .. } => {
                if settings_json.len() > MAX_PROFILE_BYTES {
                    return Err(ControlError::new(ErrorCode::InvalidRequest, "original settings collection exceeds 128 KiB"));
                }
            }
            Command::DebugCaptureStart { start_id, capacity_bytes, lease_ms, .. } => {
                if *start_id == 0 {
                    return Err(ControlError::new(ErrorCode::InvalidRequest, "start ID must be nonzero"));
                }
                otd_core::debug::validate_capture_start(*capacity_bytes as usize, *lease_ms)
                    .map_err(ControlError::from)?;
            }
            Command::DebugCaptureRead { capture, after_sequence, limit, acknowledge_through, lease_ms } => {
                capture.validate()?;
                otd_core::debug::validate_capture_read(*after_sequence, *limit as usize,
                    *acknowledge_through, *lease_ms).map_err(ControlError::from)?;
            }
            Command::DebugCaptureStop { capture } | Command::DebugCaptureRelease { capture } => {
                capture.validate()?;
            }
            _ => {}
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "method", rename_all = "snake_case", deny_unknown_fields)]
pub enum Command {
    Status,
    ListDeviceSessions,
    DetectDeviceSessions,
    BeginUpdate { expected: WorkerIdentity },
    UpdateStatus { token: String },
    FinishUpdate { token: String, success: bool },
    SelectDeviceSession { expected: WorkerIdentity, id: String },
    GetDeviceProfile { expected: WorkerIdentity, id: String, device_generation: u64 },
    ApplyDeviceProfile { expected: WorkerIdentity, id: String, device_generation: u64, profile_toml: String },
    ApplyDeviceProfileWithInhibit { expected: WorkerIdentity, id: String, device_generation: u64,
        profile_toml: String, binding_inhibit: u32 },
    ApplyOriginalDeviceProfile { expected: WorkerIdentity, id: String, device_generation: u64,
        profile_toml: String, binding_inhibit: Option<u32> },
    SaveDeviceProfile { expected: WorkerIdentity, id: String, device_generation: u64,
        expected_revision: Option<u64>, expected_digest: Option<String>, profile_toml: String },
    StopDevice { expected: WorkerIdentity, id: String, device_generation: u64 },
    StartDevice { expected: WorkerIdentity, id: String, device_generation: u64 },
    /// The supplied OTD log is validated at the RPC boundary. The daemon owner
    /// appends it to its actual recent log; no input thread handles RPC traffic.
    WriteMessage { message: UpstreamLogMessage },
    GetUpstreamLog,
    SetIdleOriginalSettings { expected: WorkerIdentity, expected_revision: u64, settings_json: String },
    SetExperimental {
        expected: WorkerIdentity,
        settings: crate::experimental::Settings,
    },
    /// None selects the daemon's default settings; Some is native profile TOML.
    /// The handler must validate and prepare it before starting a worker.
    Start {
        profile_toml: Option<String>,
    },
    Stop,
    Shutdown,
    /// Shut down only the daemon/worker generation the client just observed.
    ShutdownIf {
        expected: WorkerIdentity,
    },
    GetConfiguration {
        expected: WorkerIdentity,
    },
    StartIf {
        expected: WorkerIdentity,
        profile_toml: String,
    },
    StopIf {
        expected: WorkerIdentity,
    },
    /// Prepared first, then stop/start under one daemon-owned pending operation.
    Restart {
        expected: WorkerIdentity,
        profile_toml: String,
    },
    /// The latest tablet packet, decoded, for the tablet debugger. Each poll
    /// keeps the report thread's copy armed for about two seconds.
    Debug,
    /// Prepare the client writer first. Retry with the same nonzero start_id
    /// after a lost reply; this never rearms or extends the original lease.
    DebugCaptureStart {
        expected: WorkerIdentity,
        start_id: u64,
        capacity_bytes: u32,
        /// Renewable ownership lease, including frozen drains. The daemon
        /// stops an abandoned tap on its bounded control poll after expiry;
        /// this is not a physical packet timestamp cutoff.
        lease_ms: u32,
    },
    /// Acknowledge only previously durable packets. Repeating this read with
    /// the same cursor is safe; overflow is explicitly counted if the producer
    /// outruns the bounded ring before the retry or acknowledgment.
    DebugCaptureRead {
        capture: DebugCaptureToken,
        after_sequence: u64,
        limit: u32,
        acknowledge_through: u64,
        lease_ms: u32,
    },
    /// Freeze last_sequence. Drain until pending_reports is zero and the batch
    /// cursor reaches last_sequence, then release its bounded storage.
    DebugCaptureStop { capture: DebugCaptureToken },
    DebugCaptureRelease { capture: DebugCaptureToken },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DebugCaptureToken {
    pub instance: String,
    pub epoch: u64,
    pub session: u64,
}
impl DebugCaptureToken {
    fn validate(&self) -> Result<(), ControlError> {
        if self.instance.is_empty() || self.instance.len() > 512 ||
            self.epoch == 0 || self.epoch > u64::from(u32::MAX) || self.session == 0 {
            return Err(ControlError::new(ErrorCode::InvalidRequest, "invalid capture token"));
        }
        Ok(())
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DebugCaptureStopReason { Requested, LeaseExpired, SessionEnded, SequenceLimit }
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DebugCaptureStatus {
    pub token: DebugCaptureToken,
    pub tablet: String,
    pub parser: String,
    pub auxiliary_parser: Option<String>,
    pub report_length: u32,
    pub capacity_reports: u32,
    pub started_unix_ms: u64,
    pub active: bool,
    pub last_sequence: u64,
    pub resolved_reports: u64,
    pub pending_reports: u64,
    pub lost_tap: u64,
    pub overflow: u64,
    pub oversized: u64,
    pub acknowledged_sequence: u64,
    pub stop_reason: Option<DebugCaptureStopReason>,
    /// Remaining ownership lifetime, including while the tap is frozen.
    pub lease_remaining_ms: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DebugCapturePacket {
    pub sequence: u64,
    pub elapsed_us: u64,
    pub auxiliary: bool,
    pub raw_hex: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DebugCaptureBatch {
    pub capture: DebugCaptureStatus,
    pub packets: Vec<DebugCapturePacket>,
    pub next_sequence: u64,
}

/// What the tablet debugger shows: the tablet, its parser, a packet counter
/// for the report rate, the latest packet and its decoded values.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DebugReport {
    pub tablet: Option<String>,
    pub parser: Option<String>,
    pub sequence: u64,
    pub raw_hex: String,
    pub values: serde_json::Value,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerIdentity {
    pub instance: String,
    pub generation: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Response {
    pub version: u32,
    pub id: u64,
    pub reply: Reply,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case", deny_unknown_fields)]
pub enum Reply {
    UpdateAccepted { token: String },
    UpdateState { token: String, ready: bool, error: Option<String> },
    UpdateCancelled,
    ExperimentalSaved,
    MessageWritten,
    OriginalSettingsCommitted { revision: u64 },
    UpstreamLog { instance: String, sequence: u64, messages: Vec<UpstreamLogMessage> },
    DeviceSessions { sessions: Vec<crate::device_sessions::SessionSnapshot>, selected_id: Option<String> },
    DeviceSessionSelected { id: String },
    DeviceProfile { identity: WorkerIdentity, id: String, device_generation: u64, profile_toml: String },
    DeviceOperationAccepted { receipt: crate::device_sessions::SessionReceipt },
    DeviceProfileSaved { saved: crate::device_sessions::SavedDeviceProfile },
    Status {
        status: ControlStatus,
    },
    /// The handler defines when startup has actually been accepted.
    Started {
        generation: u64,
    },
    /// Must not be returned while worker/output cleanup is still outstanding.
    Stopped {
        generation: u64,
    },
    /// Accepted shutdown: transport sends this response before ending its loop.
    ShutdownAccepted,
    RestartAccepted {
        generation: u64,
    },
    Configuration {
        identity: WorkerIdentity,
        profile_toml: Option<String>,
    },
    Debug {
        report: DebugReport,
    },
    DebugCaptureStarted { capture: DebugCaptureStatus },
    DebugCaptureRead { batch: DebugCaptureBatch },
    DebugCaptureStopped { capture: DebugCaptureStatus },
    DebugCaptureReleased,
    Error {
        error: ControlError,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", deny_unknown_fields)]
pub struct UpstreamLogMessage {
    #[serde(default = "log_timestamp")]
    pub time: String,
    #[serde(default)]
    pub group: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub stack_trace: Option<String>,
    #[serde(default)]
    pub level: i32,
    #[serde(default)]
    pub notification: bool,
}

fn log_timestamp() -> String {
    use windows_sys::Win32::System::SystemInformation::GetSystemTime;
    let mut now = unsafe { std::mem::zeroed() };
    unsafe { GetSystemTime(&mut now) };
    format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z", now.wYear, now.wMonth,
        now.wDay, now.wHour, now.wMinute, now.wSecond, now.wMilliseconds)
}
impl UpstreamLogMessage {
    pub(crate) fn native(message: String) -> Self {
        Self { time: log_timestamp(), group: Some("RustDaemon".into()), message: Some(message),
            stack_trace: None, level: 1, notification: false }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControlStatus {
    pub instance: String,
    pub state: DriverState,
    pub generation: u64,
    pub profile: Option<String>,
    pub last_error: Option<String>,
    /// A snapshot of recent messages, not an unbounded subscription.
    pub logs: Vec<String>,
    /// Sequence number of the final log line, monotonically increasing per daemon.
    pub log_sequence: u64,
}

impl ControlStatus {
    pub fn identity(&self) -> WorkerIdentity {
        WorkerIdentity {
            instance: self.instance.clone(),
            generation: self.generation,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DriverState {
    Stopped,
    Starting,
    Running,
    Stopping,
    Failed,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControlError {
    pub code: ErrorCode,
    pub message: String,
}

impl ControlError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        let mut message = message.into();
        truncate(&mut message, MAX_LOG_LINE_BYTES);
        Self { code, message }
    }
}
impl From<otd_core::debug::CaptureError> for ControlError {
    fn from(error: otd_core::debug::CaptureError) -> Self {
        use otd_core::debug::CaptureError;
        match error {
            CaptureError::Invalid(message) => Self::new(ErrorCode::InvalidRequest, message),
            CaptureError::Busy => Self::new(ErrorCode::Busy, "capture is active, pending, or not released"),
            CaptureError::Conflict => Self::new(ErrorCode::Conflict, "capture token is no longer current"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    UnsupportedVersion,
    InvalidRequest,
    InvalidProfile,
    Busy,
    StartFailed,
    StopFailed,
    Internal,
    Conflict,
}

/// Handlers run on the control thread, never inside the report-processing loop.
/// Poll worker notifications without blocking. `handle` must also stay bounded:
/// schedule long-running work, then expose its state through `Status`.
pub trait ControlHandler {
    /// Called once after this process owns the endpoint, before dispatch.
    fn ready(&mut self) -> io::Result<()> { Ok(()) }
    fn poll(&mut self) {}
    fn handle(&mut self, command: Command) -> Result<Reply, ControlError>;
}

/// Serve the current user's local endpoint until cancelled or shutdown accepted.
/// Returns an error if another server owns the endpoint. Does not launch a driver.
pub fn serve(handler: &mut impl ControlHandler, stop: &AtomicBool) -> io::Result<()> {
    pipe::serve(handler, stop)
}

/// The auto-reset event that wakes the control thread to poll its handler.
/// Without it the control thread falls back to polling every 50 ms.
fn wake_event() -> Option<windows_sys::Win32::Foundation::HANDLE> {
    static WAKE: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    let event = *WAKE.get_or_init(|| unsafe {
        windows_sys::Win32::System::Threading::CreateEventW(
            std::ptr::null(),
            0,
            0,
            std::ptr::null(),
        ) as usize
    });
    (event != 0).then_some(event as windows_sys::Win32::Foundation::HANDLE)
}

/// Wakes the control thread, which otherwise sleeps until a client connects.
/// Call it after anything `ControlHandler::poll` must see: a worker notice or
/// log line, a worker's exit, a stop request.
pub fn wake() {
    if let Some(event) = wake_event() {
        unsafe { windows_sys::Win32::System::Threading::SetEvent(event) };
    }
}

/// Contact an existing daemon. Does not launch one, retry commands or inject input.
/// Timeout includes connection, framing, response and response acknowledgement.
pub fn request(request: &Request, timeout: Duration) -> io::Result<Response> {
    request
        .validate()
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error.message))?;
    pipe::request(request, timeout)
}
pub(crate) fn request_owned(request: &Request, timeout: Duration, expected_process: u32) -> io::Result<Response> {
    request.validate().map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error.message))?;
    pipe::request_owned(request, timeout, Some(expected_process))
}

/// Stable per-user discovery name; contains the caller's Windows token SID.
pub fn endpoint_name() -> io::Result<String> {
    pipe::endpoint_name()
}

/// Discovery only; never connects to or stops a protocol-v1 service.
pub fn legacy_service_present() -> io::Result<bool> {
    pipe::legacy_service_present()
}

fn dispatch(handler: &mut impl ControlHandler, bytes: &[u8]) -> Response {
    let value = match serde_json::from_slice::<serde_json::Value>(bytes) {
        Ok(value) => value,
        Err(error) => {
            return failure(
                0,
                ControlError::new(ErrorCode::InvalidRequest, error.to_string()),
            );
        }
    };
    let id = value
        .get("id")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    if value.get("version").and_then(serde_json::Value::as_u64) != Some(PROTOCOL_VERSION as u64) {
        return failure(
            id,
            ControlError::new(
                ErrorCode::UnsupportedVersion,
                format!("expected protocol version {PROTOCOL_VERSION}"),
            ),
        );
    }
    let request: Request = match serde_json::from_value(value) {
        Ok(request) => request,
        Err(error) => {
            return failure(
                id,
                ControlError::new(ErrorCode::InvalidRequest, error.to_string()),
            );
        }
    };
    if let Err(error) = request.validate() {
        return failure(id, error);
    }
    let mut reply = match handler.handle(request.command) {
        Ok(reply) => reply,
        Err(error) => Reply::Error { error },
    };
    match &mut reply {
        Reply::Status { status } => {
            if status.logs.len() > MAX_LOG_LINES {
                status.logs.drain(..status.logs.len() - MAX_LOG_LINES);
            }
            for line in &mut status.logs {
                truncate(line, MAX_LOG_LINE_BYTES);
            }
            if let Some(profile) = &mut status.profile {
                truncate(profile, MAX_LOG_LINE_BYTES);
            }
            if let Some(error) = &mut status.last_error {
                truncate(error, MAX_LOG_LINE_BYTES);
            }
        }
        Reply::Error { error } => truncate(&mut error.message, MAX_LOG_LINE_BYTES),
        _ => {}
    }
    Response {
        version: PROTOCOL_VERSION,
        id,
        reply,
    }
}

fn failure(id: u64, error: ControlError) -> Response {
    Response {
        version: PROTOCOL_VERSION,
        id,
        reply: Reply::Error { error },
    }
}

fn truncate(value: &mut String, max_bytes: usize) {
    if value.len() > max_bytes {
        let mut end = max_bytes;
        while !value.is_char_boundary(end) {
            end -= 1;
        }
        value.truncate(end);
    }
}

#[cfg(test)]
mod capture_tests {
    use super::*;
    fn token() -> DebugCaptureToken {
        DebugCaptureToken { instance: "offline-fixture".into(), epoch: 1, session: 7 }
    }
    #[test]
    fn capture_requests_reject_bad_limits_acknowledgments_and_tokens() {
        for (limit, after_sequence, acknowledge_through, lease_ms) in
            [(0, 0, 0, 1_000), (513, 0, 0, 1_000), (1, 0, 1, 1_000), (1, 0, 0, 999), (1, 0, 0, 30_001)] {
            let request = Request::new(1, Command::DebugCaptureRead {
                capture: token(), after_sequence, limit, acknowledge_through, lease_ms,
            });
            assert!(matches!(request.validate(), Err(ControlError { code: ErrorCode::InvalidRequest, .. })));
        }
        let mut bad_token = token();
        bad_token.epoch = 0;
        assert!(Request::new(1, Command::DebugCaptureStop { capture: bad_token }).validate().is_err());
        let start = Request::new(1, Command::DebugCaptureStart {
            expected: WorkerIdentity { instance: "offline-fixture".into(), generation: 1 },
            start_id: 0, capacity_bytes: 65_536, lease_ms: 1_000,
        });
        assert!(start.validate().is_err());
        assert!(serde_json::from_str::<Request>(r#"{"version":2,"id":1,"command":{"method":"debug_capture_stop","capture":{"instance":"fixture","epoch":1,"session":7},"ignored":true}}"#).is_err());
    }
    #[test]
    fn capture_read_wire_budget_includes_worst_case_escaped_metadata() {
        let capture = DebugCaptureStatus {
            token: DebugCaptureToken { instance: "\u{1}".repeat(512), epoch: u64::from(u32::MAX), session: u64::MAX },
            tablet: "\u{1}".repeat(512), parser: "\u{1}".repeat(512), auxiliary_parser: Some("\u{1}".repeat(512)),
            report_length: u16::MAX as u32, capacity_reports: u32::MAX,
            started_unix_ms: u64::MAX, active: false, last_sequence: u64::MAX,
            resolved_reports: u64::MAX, pending_reports: u64::MAX, lost_tap: u64::MAX,
            overflow: u64::MAX, oversized: u64::MAX, acknowledged_sequence: u64::MAX,
            stop_reason: Some(DebugCaptureStopReason::SequenceLimit), lease_remaining_ms: u64::MAX,
        };
        // Max-limit small packets use nearly all of the core's packet budget.
        let count = otd_core::debug::MAX_CAPTURE_LIMIT;
        let hex_length = (otd_core::debug::CAPTURE_PACKET_JSON_BUDGET / count - 160) & !1;
        let packets = (0..count).map(|_| DebugCapturePacket {
            sequence: u64::MAX, elapsed_us: u64::MAX, auxiliary: false, raw_hex: "a".repeat(hex_length),
        }).collect();
        let response = Response { version: PROTOCOL_VERSION, id: u64::MAX,
            reply: Reply::DebugCaptureRead { batch: DebugCaptureBatch { capture: capture.clone(), packets, next_sequence: u64::MAX } } };
        assert!(serde_json::to_vec(&response).unwrap().len() <= MAX_FRAME_BYTES);
        let response = Response { version: PROTOCOL_VERSION, id: u64::MAX,
            reply: Reply::DebugCaptureRead { batch: DebugCaptureBatch { capture,
                packets: vec![DebugCapturePacket { sequence: u64::MAX, elapsed_us: u64::MAX, auxiliary: false,
                    raw_hex: "ab".repeat(otd_core::debug::MAX_BYTES) }], next_sequence: u64::MAX } } };
        assert!(serde_json::to_vec(&response).unwrap().len() <= MAX_FRAME_BYTES);
    }
}
