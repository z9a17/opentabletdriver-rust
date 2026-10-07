//! Current-user Windows named pipes with cancellable overlapped operations.
//! SECURITY_IDENTIFICATION prevents a contacted server from impersonating the
//! client; checking the server process token additionally rejects other users'
//! lookalike endpoints. The server ACL grants only its current user and SYSTEM.
//!
//! API contracts:
//! https://learn.microsoft.com/en-us/windows/win32/ipc/named-pipe-security-and-access-rights
//! https://learn.microsoft.com/en-us/windows/win32/api/namedpipeapi/nf-namedpipeapi-disconnectnamedpipe
//! https://learn.microsoft.com/en-us/windows/win32/api/ioapiset/nf-ioapiset-cancelioex

use super::{ControlHandler, MAX_FRAME_BYTES, PROTOCOL_VERSION, Reply, Request, Response};
use std::ffi::c_void;
use std::io;
use std::mem::{size_of, zeroed};
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_BROKEN_PIPE, ERROR_FILE_NOT_FOUND, ERROR_IO_PENDING, ERROR_NO_DATA,
    ERROR_PIPE_BUSY, ERROR_PIPE_CONNECTED, ERROR_PIPE_NOT_CONNECTED, HANDLE, INVALID_HANDLE_VALUE,
    LocalFree, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows_sys::Win32::Security::{
    GetTokenInformation, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER, TokenUser,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED, OPEN_EXISTING,
    PIPE_ACCESS_DUPLEX, ReadFile, SECURITY_IDENTIFICATION, SECURITY_SQOS_PRESENT, WriteFile,
};
use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, GetNamedPipeServerProcessId,
    PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT, WaitNamedPipeW,
};
use windows_sys::Win32::System::Threading::{
    CreateEventW, GetCurrentProcess, INFINITE, OpenProcess, OpenProcessToken,
    PROCESS_QUERY_LIMITED_INFORMATION, WaitForMultipleObjects,
};

const POLL_MS: u32 = 50;
const CLIENT_BUDGET: Duration = Duration::from_secs(5);
const ACK: u8 = 0x06;
// Use explicit read/write rights, not GENERIC_WRITE (which also grants the
// FILE_CREATE_PIPE_INSTANCE bit for named pipes). FILE_GENERIC_READ/WRITE
// without FILE_APPEND_DATA: 0x0012019f & !0x4.
const CLIENT_ACCESS: u32 = 0x0012_019b;

struct Handle(HANDLE);
impl Handle {
    fn new(raw: HANDLE) -> io::Result<Self> {
        if raw.is_null() || raw == INVALID_HANDLE_VALUE {
            Err(io::Error::last_os_error())
        } else {
            Ok(Self(raw))
        }
    }
}
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}

struct LocalAllocation(*mut c_void);
impl Drop for LocalAllocation {
    fn drop(&mut self) {
        unsafe {
            LocalFree(self.0);
        }
    }
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}

fn process_sid(process: HANDLE) -> io::Result<String> {
    let mut raw_token = null_mut();
    if unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut raw_token) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let token = Handle::new(raw_token)?;
    let mut needed = 0;
    unsafe {
        GetTokenInformation(token.0, TokenUser, null_mut(), 0, &mut needed);
    }
    if needed == 0 || needed > 65536 {
        return Err(io::Error::other("invalid token-user size"));
    }
    // usize storage satisfies TOKEN_USER alignment; the buffer also contains SID data.
    let mut storage = vec![0usize; (needed as usize).div_ceil(size_of::<usize>())];
    if unsafe {
        GetTokenInformation(
            token.0,
            TokenUser,
            storage.as_mut_ptr().cast(),
            needed,
            &mut needed,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let user = unsafe { &*storage.as_ptr().cast::<TOKEN_USER>() };
    let mut sid_string = null_mut();
    if unsafe { ConvertSidToStringSidW(user.User.Sid, &mut sid_string) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let allocation = LocalAllocation(sid_string.cast());
    let mut length = 0;
    // Windows owns the NUL-terminated SID string allocated by this API.
    while unsafe { *sid_string.add(length) } != 0 {
        length += 1;
    }
    let sid = String::from_utf16(unsafe { std::slice::from_raw_parts(sid_string, length) })
        .map_err(|_| io::Error::other("token SID is not valid UTF-16"))?;
    drop(allocation);
    Ok(sid)
}

fn current_sid() -> io::Result<String> {
    process_sid(unsafe { GetCurrentProcess() })
}
fn name_for_sid(sid: &str) -> String {
    versioned_name(sid, PROTOCOL_VERSION)
}
fn versioned_name(sid: &str, version: u32) -> String {
    let name = format!(r"\\.\pipe\OpenTabletDriverRust.Control.v{version}.{sid}");
    #[cfg(test)]
    let name = format!("{name}.test-{}", std::process::id());
    name
}
pub(super) fn endpoint_name() -> io::Result<String> {
    current_sid().map(|sid| name_for_sid(&sid))
}

pub(super) fn legacy_service_present() -> io::Result<bool> {
    let name = wide(&versioned_name(&current_sid()?, 1));
    if unsafe { WaitNamedPipeW(name.as_ptr(), 1) } != 0 {
        return Ok(true);
    }
    let error = io::Error::last_os_error();
    Ok(error.raw_os_error() != Some(ERROR_FILE_NOT_FOUND as i32))
}

fn create_server(sid: &str) -> io::Result<Handle> {
    create_named_server(sid, &name_for_sid(sid), true, 1)
}

fn create_named_server(sid: &str, endpoint: &str, first: bool, instances: u32) -> io::Result<Handle> {
    // Protected DACL: current user and SYSTEM only. The user receives full access
    // so the server can create its endpoint; clients request the narrower mask.
    let sddl = wide(&format!("D:P(A;;GA;;;SY)(A;;GA;;;{sid})"));
    let mut descriptor = null_mut();
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut descriptor,
            null_mut(),
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let descriptor = LocalAllocation(descriptor);
    let security = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor.0,
        bInheritHandle: 0,
    };
    let name = wide(endpoint);
    let handle = unsafe {
        CreateNamedPipeW(
            name.as_ptr(),
            PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED | if first { FILE_FLAG_FIRST_PIPE_INSTANCE } else { 0 },
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            instances,
            65536,
            65536,
            1000,
            &security,
        )
    };
    Handle::new(handle).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("cannot own daemon endpoint (another daemon may be running): {error}"),
        )
    })
}

/// Own the OVERLAPPED and event until completion, including error/panic cleanup.
/// On cancellation, CancelIoEx is followed by completion draining before the
/// caller's read/write buffer can be freed. This occurs only on the control
/// thread; report processing does not wait on pipe I/O.
struct Operation {
    pipe: HANDLE,
    event: Handle,
    overlapped: Box<OVERLAPPED>,
    pending: bool,
    wake_control: bool,
}
impl Operation {
    fn new(pipe: HANDLE) -> io::Result<Self> {
        let event = Handle::new(unsafe { CreateEventW(null(), 1, 0, null()) })?;
        let mut overlapped: Box<OVERLAPPED> = Box::new(unsafe { zeroed() });
        overlapped.hEvent = event.0;
        Ok(Self {
            pipe,
            event,
            overlapped,
            pending: false,
            wake_control: true,
        })
    }

    fn wait(
        &mut self,
        deadline: Option<Instant>,
        stop: &AtomicBool,
        tick: &mut impl FnMut(),
    ) -> io::Result<u32> {
        // Sleep until the operation completes or the handler has work; a
        // timer here would wake an idle daemon many times a second.
        let wake = self.wake_control.then(super::wake_event).flatten();
        let handles = [self.event.0, wake.unwrap_or(null_mut())];
        let count = if wake.is_some() { 2 } else { 1 };
        loop {
            if stop.load(Ordering::Acquire) {
                return Err(io::Error::new(
                    io::ErrorKind::Interrupted,
                    "control server cancelled",
                ));
            }
            let wait_ms = match deadline {
                Some(deadline) => {
                    let left = deadline.saturating_duration_since(Instant::now());
                    if left.is_zero() {
                        return Err(io::Error::new(
                            io::ErrorKind::TimedOut,
                            "control request timed out",
                        ));
                    }
                    left.as_millis().clamp(1, u128::from(INFINITE - 1)) as u32
                }
                None => INFINITE,
            };
            let wait_ms = if wake.is_some() {
                wait_ms
            } else {
                wait_ms.min(POLL_MS)
            };
            match unsafe { WaitForMultipleObjects(count, handles.as_ptr(), 0, wait_ms) } {
                WAIT_OBJECT_0 => {
                    let mut bytes = 0;
                    let ok =
                        unsafe { GetOverlappedResult(self.pipe, &*self.overlapped, &mut bytes, 0) };
                    self.pending = false;
                    return if ok != 0 {
                        Ok(bytes)
                    } else {
                        Err(io::Error::last_os_error())
                    };
                }
                // The wake event, or a deadline check.
                result if result == WAIT_OBJECT_0 + 1 || result == WAIT_TIMEOUT => tick(),
                _ => return Err(io::Error::last_os_error()),
            }
        }
    }
}
impl Drop for Operation {
    fn drop(&mut self) {
        if self.pending {
            unsafe {
                CancelIoEx(self.pipe, &*self.overlapped);
                let mut bytes = 0;
                GetOverlappedResult(self.pipe, &*self.overlapped, &mut bytes, 1);
            }
        }
    }
}

fn connect_server(pipe: HANDLE, stop: &AtomicBool, tick: &mut impl FnMut()) -> io::Result<()> {
    let mut op = Operation::new(pipe)?;
    if unsafe { ConnectNamedPipe(pipe, &mut *op.overlapped) } != 0 {
        return Ok(());
    }
    let error = io::Error::last_os_error();
    match error.raw_os_error().map(|code| code as u32) {
        Some(ERROR_PIPE_CONNECTED) => Ok(()),
        Some(ERROR_IO_PENDING) => {
            op.pending = true;
            op.wait(None, stop, tick).map(|_| ())
        }
        _ => Err(error),
    }
}

enum Buffer<'a> {
    Read(&'a mut [u8]),
    Write(&'a [u8]),
}
/// Exclusive per-worker compatibility connection. Reuses native cancellation
/// draining and ACL construction, but never consumes the daemon owner's wake.
pub(crate) struct CompatPipe { pipe: Handle }
// Each instance is moved once to its dedicated worker and never shared.
unsafe impl Send for CompatPipe {}
impl CompatPipe {
    /// A persistent same-user client used by the original-console workflow.
    /// The caller captures the native daemon PID before connecting, so a
    /// replacement or another application cannot receive its mutations.
    pub(crate) fn open_client(name: &str, deadline: Instant, expected_process: Option<u32>) -> io::Result<Self> {
        if name.is_empty() || name.len() > 200 || name.bytes().any(|byte| byte < 32 || matches!(byte, b'\\' | b'/' | b':')) {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid compatibility pipe name"));
        }
        let sid = current_sid()?;
        let endpoint = format!(r"\\.\pipe\{name}");
        Ok(Self { pipe: connect_named_client(&sid, &endpoint, deadline, expected_process)? })
    }
    pub(crate) fn instances(name: &str, count: u32) -> io::Result<Vec<Self>> {
        if name.is_empty() || name.len() > 200 || name.bytes().any(|byte| byte < 32 || matches!(byte, b'\\' | b'/' | b':'))
            || !(1..=4).contains(&count) {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid compatibility pipe name or client limit"));
        }
        let sid = current_sid()?;
        let endpoint = format!(r"\\.\pipe\{name}");
        let mut pipes = Vec::new();
        // Keep the first instance alive while creating all others. First-instance
        // ownership failure leaves an existing original or Rust daemon untouched.
        for index in 0..count {
            pipes.push(Self { pipe: create_named_server(&sid, &endpoint, index == 0, count)? });
        }
        Ok(pipes)
    }
    /// Read-only readiness ownership check. Closing this temporary connection
    /// dispatches no RPC request and cannot start a tablet worker.
    pub(crate) fn server_process_id(endpoint: &str) -> io::Result<u32> {
        let name = wide(endpoint);
        let pipe = Handle::new(unsafe { CreateFileW(name.as_ptr(), 0, 0, null(), OPEN_EXISTING,
            SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION, null_mut()) })?;
        let mut pid = 0;
        if unsafe { GetNamedPipeServerProcessId(pipe.0, &mut pid) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(pid)
    }
    pub(crate) fn connect(&self, stop: &AtomicBool) -> io::Result<()> {
        let mut op = Operation::new(self.pipe.0)?;
        op.wake_control = false;
        if unsafe { ConnectNamedPipe(self.pipe.0, &mut *op.overlapped) } == 0 {
            let error = io::Error::last_os_error();
            match error.raw_os_error().map(|value| value as u32) {
                Some(ERROR_PIPE_CONNECTED) => {},
                Some(ERROR_IO_PENDING) => { op.pending = true; op.wait(None, stop, &mut || {})?; },
                _ => return Err(error),
            }
        }
        Ok(())
    }
    pub(crate) fn disconnect(&self) {
        unsafe { DisconnectNamedPipe(self.pipe.0); }
    }
    pub(crate) fn cancel_pending(&self) {
        // The owning worker drains its OVERLAPPED before freeing the buffer.
        unsafe { CancelIoEx(self.pipe.0, null()); }
    }
    pub(crate) fn read(&self, buffer: &mut [u8], deadline: Instant, stop: &AtomicBool,
        tick: &mut impl FnMut()) -> io::Result<usize> {
        transfer_with_wake(self.pipe.0, Buffer::Read(buffer), deadline, stop, tick, false)
    }
    pub(crate) fn write(&self, mut buffer: &[u8], deadline: Instant, stop: &AtomicBool) -> io::Result<()> {
        while !buffer.is_empty() {
            let bytes = transfer_with_wake(self.pipe.0, Buffer::Write(buffer), deadline, stop, &mut || {}, false)?;
            if bytes == 0 { return Err(io::Error::new(io::ErrorKind::WriteZero, "compatibility pipe closed")); }
            buffer = &buffer[bytes..];
        }
        Ok(())
    }
}

fn transfer_with_wake(
    pipe: HANDLE,
    buffer: Buffer<'_>,
    deadline: Instant,
    stop: &AtomicBool,
    tick: &mut impl FnMut(),
    wake_control: bool,
) -> io::Result<usize> {
    if stop.load(Ordering::Acquire) {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "control server cancelled",
        ));
    }
    if Instant::now() >= deadline {
        return Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "control request timed out",
        ));
    }
    let mut op = Operation::new(pipe)?;
    op.wake_control = wake_control;
    let mut bytes = 0;
    let ok = unsafe {
        match buffer {
            Buffer::Read(buffer) => ReadFile(
                pipe,
                buffer.as_mut_ptr(),
                buffer.len() as u32,
                &mut bytes,
                &mut *op.overlapped,
            ),
            Buffer::Write(buffer) => WriteFile(
                pipe,
                buffer.as_ptr(),
                buffer.len() as u32,
                &mut bytes,
                &mut *op.overlapped,
            ),
        }
    };
    if ok != 0 {
        return Ok(bytes as usize);
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() != Some(ERROR_IO_PENDING as i32) {
        return Err(error);
    }
    op.pending = true;
    op.wait(Some(deadline), stop, tick)
        .map(|bytes| bytes as usize)
}

fn read_exact(
    pipe: HANDLE,
    buffer: &mut [u8],
    deadline: Instant,
    stop: &AtomicBool,
    tick: &mut impl FnMut(),
) -> io::Result<()> {
    read_exact_with_wake(pipe, buffer, deadline, stop, tick, true)
}
fn read_exact_with_wake(pipe: HANDLE, mut buffer: &mut [u8], deadline: Instant,
    stop: &AtomicBool, tick: &mut impl FnMut(), wake_control: bool) -> io::Result<()> {
    while !buffer.is_empty() {
        let bytes = transfer_with_wake(pipe, Buffer::Read(buffer), deadline, stop, tick, wake_control)?;
        if bytes == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "daemon pipe closed",
            ));
        }
        buffer = &mut buffer[bytes..];
    }
    Ok(())
}
fn write_all_with_wake(pipe: HANDLE, mut buffer: &[u8], deadline: Instant,
    stop: &AtomicBool, tick: &mut impl FnMut(), wake_control: bool) -> io::Result<()> {
    while !buffer.is_empty() {
        let bytes = transfer_with_wake(pipe, Buffer::Write(buffer), deadline, stop, tick, wake_control)?;
        if bytes == 0 {
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "daemon pipe accepted no data",
            ));
        }
        buffer = &buffer[bytes..];
    }
    Ok(())
}
fn read_frame(
    pipe: HANDLE,
    deadline: Instant,
    stop: &AtomicBool,
    tick: &mut impl FnMut(),
) -> io::Result<Vec<u8>> {
    read_frame_with_wake(pipe, deadline, stop, tick, true)
}
fn read_frame_with_wake(pipe: HANDLE, deadline: Instant, stop: &AtomicBool,
    tick: &mut impl FnMut(), wake_control: bool) -> io::Result<Vec<u8>> {
    let mut prefix = [0u8; 4];
    read_exact_with_wake(pipe, &mut prefix, deadline, stop, tick, wake_control)?;
    let size = u32::from_le_bytes(prefix) as usize;
    if size == 0 || size > MAX_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "control frame must be 1..=256 KiB",
        ));
    }
    let mut frame = vec![0; size];
    read_exact_with_wake(pipe, &mut frame, deadline, stop, tick, wake_control)?;
    Ok(frame)
}
fn write_frame(
    pipe: HANDLE,
    frame: &[u8],
    deadline: Instant,
    stop: &AtomicBool,
    tick: &mut impl FnMut(),
) -> io::Result<()> {
    write_frame_with_wake(pipe, frame, deadline, stop, tick, true)
}
fn write_frame_with_wake(pipe: HANDLE, frame: &[u8], deadline: Instant,
    stop: &AtomicBool, tick: &mut impl FnMut(), wake_control: bool) -> io::Result<()> {
    if frame.is_empty() || frame.len() > MAX_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "control frame must be 1..=256 KiB",
        ));
    }
    write_all_with_wake(
        pipe,
        &(frame.len() as u32).to_le_bytes(),
        deadline,
        stop,
        tick,
        wake_control,
    )?;
    write_all_with_wake(pipe, frame, deadline, stop, tick, wake_control)
}

pub(super) fn serve(handler: &mut impl ControlHandler, stop: &AtomicBool) -> io::Result<()> {
    let pipe = create_server(&current_sid()?)?;
    while !stop.load(Ordering::Acquire) {
        handler.poll();
        if let Err(error) = connect_server(pipe.0, stop, &mut || handler.poll()) {
            if stop.load(Ordering::Acquire) {
                break;
            }
            if matches!(
                error.raw_os_error().map(|code| code as u32),
                Some(ERROR_NO_DATA | ERROR_BROKEN_PIPE | ERROR_PIPE_NOT_CONNECTED)
            ) {
                // A client can open and disappear before ConnectNamedPipe runs.
                unsafe {
                    DisconnectNamedPipe(pipe.0);
                }
                continue;
            }
            return Err(error);
        }
        let deadline = Instant::now() + CLIENT_BUDGET;
        let mut shutdown = false;
        // A malformed/slow/disconnected client must not terminate the daemon.
        let transaction = (|| -> io::Result<()> {
            let frame = read_frame(pipe.0, deadline, stop, &mut || handler.poll())?;
            // Dispatch a complete Stop/Shutdown before another lifecycle poll
            // can activate a pending restart that the command would cancel.
            let response = super::dispatch(handler, &frame);
            shutdown = matches!(response.reply, Reply::ShutdownAccepted);
            let bytes = serde_json::to_vec(&response).map_err(io::Error::other)?;
            write_frame(pipe.0, &bytes, deadline, stop, &mut || handler.poll())?;
            let mut ack = [0u8; 1];
            read_exact(pipe.0, &mut ack, deadline, stop, &mut || handler.poll())?;
            if ack[0] != ACK {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "invalid response acknowledgement",
                ));
            }
            Ok(())
        })();
        // Do not FlushFileBuffers: it can block forever on a stalled client.
        unsafe {
            DisconnectNamedPipe(pipe.0);
        }
        if shutdown || stop.load(Ordering::Acquire) {
            break;
        }
        // The transaction outcome belongs to this connection. Worker lifecycle
        // failures are represented in protocol replies/status by the handler.
        let _ = transaction;
    }
    Ok(())
}

fn connect_client(sid: &str, deadline: Instant, expected_process: Option<u32>) -> io::Result<Handle> {
    connect_named_client(sid, &name_for_sid(sid), deadline, expected_process)
}

fn connect_named_client(sid: &str, endpoint: &str, deadline: Instant, expected_process: Option<u32>) -> io::Result<Handle> {
    let name = wide(endpoint);
    loop {
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "daemon connection timed out; request was not sent",
            ));
        }
        let raw = unsafe {
            CreateFileW(
                name.as_ptr(),
                CLIENT_ACCESS,
                0,
                null(),
                OPEN_EXISTING,
                FILE_FLAG_OVERLAPPED | SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION,
                null_mut(),
            )
        };
        if raw != INVALID_HANDLE_VALUE {
            let pipe = Handle::new(raw)?;
            let mut pid = 0;
            if unsafe { GetNamedPipeServerProcessId(pipe.0, &mut pid) } == 0 {
                return Err(io::Error::last_os_error());
            }
            if expected_process.is_some_and(|expected| expected != pid) {
                return Err(io::Error::new(io::ErrorKind::PermissionDenied,
                    "native endpoint is not owned by this compatibility daemon; request was not sent"));
            }
            let process =
                Handle::new(unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) })?;
            if process_sid(process.0)? != sid {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "daemon endpoint belongs to another user",
                ));
            }
            return Ok(pipe);
        }
        let error = io::Error::last_os_error();
        match error.raw_os_error().map(|code| code as u32) {
            Some(ERROR_FILE_NOT_FOUND) => {
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    "no control daemon is running for this user",
                ));
            }
            Some(ERROR_PIPE_BUSY) => {
                let left = deadline.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "daemon is busy; request was not sent",
                    ));
                }
                unsafe {
                    WaitNamedPipeW(
                        name.as_ptr(),
                        left.as_millis().clamp(1, POLL_MS as u128) as u32,
                    );
                }
            }
            _ => return Err(error),
        }
    }
}

pub(super) fn request(request: &Request, timeout: Duration) -> io::Result<Response> {
    request_owned(request, timeout, None)
}
pub(super) fn request_owned(request: &Request, timeout: Duration, expected_process: Option<u32>) -> io::Result<Response> {
    if timeout.is_zero() || timeout > Duration::from_secs(60) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "control timeout must be greater than zero and at most 60 seconds",
        ));
    }
    let deadline = Instant::now() + timeout;
    let sid = current_sid()?;
    let bytes = serde_json::to_vec(request).map_err(io::Error::other)?;
    if bytes.len() > MAX_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "serialized request exceeds 256 KiB",
        ));
    }
    let pipe = connect_client(&sid, deadline, expected_process)?;
    let stop = AtomicBool::new(false);
    // A client in this daemon process must not steal the control owner's wake.
    write_frame_with_wake(pipe.0, &bytes, deadline, &stop, &mut || {}, false)?;
    let frame = read_frame_with_wake(pipe.0, deadline, &stop, &mut || {}, false)?;
    let response: Response = serde_json::from_slice(&frame)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    if response.version != PROTOCOL_VERSION || response.id != request.id {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "daemon response version or request ID does not match",
        ));
    }
    write_all_with_wake(pipe.0, &[ACK], deadline, &stop, &mut || {}, false)?;
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::AtomicUsize;

    /// An idle server sleeps until it is woken: no timer polls its handler.
    #[test]
    fn idle_wait_polls_only_when_woken() {
        let stop = Arc::new(AtomicBool::new(false));
        let ticks = Arc::new(AtomicUsize::new(0));
        let waiter = {
            let (stop, ticks) = (Arc::clone(&stop), Arc::clone(&ticks));
            std::thread::spawn(move || {
                // An operation on no pipe whose event never completes.
                let mut operation = Operation::new(null_mut()).unwrap();
                operation.wait(None, &stop, &mut || {
                    ticks.fetch_add(1, Ordering::SeqCst);
                })
            })
        };
        std::thread::sleep(Duration::from_millis(500));
        assert_eq!(ticks.load(Ordering::SeqCst), 0, "polled without a wake");
        crate::control::wake();
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(ticks.load(Ordering::SeqCst), 1);
        stop.store(true, Ordering::Release);
        crate::control::wake();
        let error = waiter.join().unwrap().unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Interrupted);
    }
}
