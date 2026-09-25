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
    CreateEventW, GetCurrentProcess, OpenProcess, OpenProcessToken,
    PROCESS_QUERY_LIMITED_INFORMATION, WaitForSingleObject,
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
    format!(r"\\.\pipe\OpenTabletDriverRust.Control.v1.{sid}")
}
pub(super) fn endpoint_name() -> io::Result<String> {
    current_sid().map(|sid| name_for_sid(&sid))
}

fn create_server(sid: &str) -> io::Result<Handle> {
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
    let name = wide(&name_for_sid(sid));
    let handle = unsafe {
        CreateNamedPipeW(
            name.as_ptr(),
            PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED | FILE_FLAG_FIRST_PIPE_INSTANCE,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            1,
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
        })
    }

    fn wait(
        &mut self,
        deadline: Option<Instant>,
        stop: &AtomicBool,
        tick: &mut impl FnMut(),
    ) -> io::Result<u32> {
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
                    left.as_millis().clamp(1, POLL_MS as u128) as u32
                }
                None => POLL_MS,
            };
            match unsafe { WaitForSingleObject(self.event.0, wait_ms) } {
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
                WAIT_TIMEOUT => tick(),
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
fn transfer(
    pipe: HANDLE,
    buffer: Buffer<'_>,
    deadline: Instant,
    stop: &AtomicBool,
    tick: &mut impl FnMut(),
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
    mut buffer: &mut [u8],
    deadline: Instant,
    stop: &AtomicBool,
    tick: &mut impl FnMut(),
) -> io::Result<()> {
    while !buffer.is_empty() {
        let bytes = transfer(pipe, Buffer::Read(buffer), deadline, stop, tick)?;
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
fn write_all(
    pipe: HANDLE,
    mut buffer: &[u8],
    deadline: Instant,
    stop: &AtomicBool,
    tick: &mut impl FnMut(),
) -> io::Result<()> {
    while !buffer.is_empty() {
        let bytes = transfer(pipe, Buffer::Write(buffer), deadline, stop, tick)?;
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
    let mut prefix = [0u8; 4];
    read_exact(pipe, &mut prefix, deadline, stop, tick)?;
    let size = u32::from_le_bytes(prefix) as usize;
    if size == 0 || size > MAX_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "control frame must be 1..=256 KiB",
        ));
    }
    let mut frame = vec![0; size];
    read_exact(pipe, &mut frame, deadline, stop, tick)?;
    Ok(frame)
}
fn write_frame(
    pipe: HANDLE,
    frame: &[u8],
    deadline: Instant,
    stop: &AtomicBool,
    tick: &mut impl FnMut(),
) -> io::Result<()> {
    if frame.is_empty() || frame.len() > MAX_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "control frame must be 1..=256 KiB",
        ));
    }
    write_all(
        pipe,
        &(frame.len() as u32).to_le_bytes(),
        deadline,
        stop,
        tick,
    )?;
    write_all(pipe, frame, deadline, stop, tick)
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
            handler.poll();
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

fn connect_client(sid: &str, deadline: Instant) -> io::Result<Handle> {
    let name = wide(&name_for_sid(sid));
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
    let pipe = connect_client(&sid, deadline)?;
    let stop = AtomicBool::new(false);
    write_frame(pipe.0, &bytes, deadline, &stop, &mut || {})?;
    let frame = read_frame(pipe.0, deadline, &stop, &mut || {})?;
    let response: Response = serde_json::from_slice(&frame)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    if response.version != PROTOCOL_VERSION || response.id != request.id {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "daemon response version or request ID does not match",
        ));
    }
    write_all(pipe.0, &[ACK], deadline, &stop, &mut || {})?;
    Ok(response)
}
