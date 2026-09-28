//! Versioned local daemon control, separate from the report-processing thread.
//!
//! One bounded JSON request and response per connection, framed with a little-
//! endian u32 byte length. Clients acknowledge a complete response with 0x06
//! before the server disconnects, avoiding discarded unread pipe data. Request
//! IDs correlate replies; they are not deduplication tokens. After a connection
//! failure a command may already have taken effect: query status before retrying.

mod pipe;

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

    fn validate(&self) -> Result<(), ControlError> {
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
            _ => None,
        };
        if profile.is_some_and(|profile| profile.len() > MAX_PROFILE_BYTES) {
            return Err(ControlError::new(
                ErrorCode::InvalidProfile,
                "profile exceeds 128 KiB",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "method", rename_all = "snake_case", deny_unknown_fields)]
pub enum Command {
    Status,
    /// None selects the daemon's default settings; Some is native profile TOML.
    /// The handler must validate and prepare it before starting a worker.
    Start {
        profile_toml: Option<String>,
    },
    Stop,
    Shutdown,
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
    Error {
        error: ControlError,
    },
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
    fn poll(&mut self) {}
    fn handle(&mut self, command: Command) -> Result<Reply, ControlError>;
}

/// Serve the current user's local endpoint until cancelled or shutdown accepted.
/// Returns an error if another server owns the endpoint. Does not launch a driver.
pub fn serve(handler: &mut impl ControlHandler, stop: &AtomicBool) -> io::Result<()> {
    pipe::serve(handler, stop)
}

/// Contact an existing daemon. Does not launch one, retry commands or inject input.
/// Timeout includes connection, framing, response and response acknowledgement.
pub fn request(request: &Request, timeout: Duration) -> io::Result<Response> {
    request
        .validate()
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error.message))?;
    pipe::request(request, timeout)
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
