//! Opt-in upstream Windows RPC. Native v2 framing and daemon ownership remain
//! separate. Never start the original daemon, a tablet worker, or .NET for RPC.
mod protocol;
mod service;
mod settings;
mod apply;

use std::io;
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use crate::control::pipe::CompatPipe;

pub const DEFAULT_PIPE: &str = "OpenTabletDriverRust.Compat";
const MAX_CLIENTS: u32 = 4;
const IDLE_BUDGET: Duration = Duration::from_secs(120);
const FRAME_BUDGET: Duration = Duration::from_secs(5);

pub struct Listener { stop: Arc<AtomicBool>, workers: Vec<JoinHandle<()>> }
impl Listener {
    pub fn start(name: &str) -> io::Result<Self> {
        let pipes = CompatPipe::instances(name, MAX_CLIENTS)?;
        let shared = Arc::new(service::Shared::default());
        let mut listener = Self { stop: Arc::new(AtomicBool::new(false)), workers: Vec::new() };
        for (index, pipe) in pipes.into_iter().enumerate() {
            let stop = Arc::clone(&listener.stop);
            let shared = Arc::clone(&shared);
            listener.workers.push(std::thread::Builder::new().name(format!("upstream-rpc-{index}"))
                .spawn(move || {
                    while !stop.load(Ordering::Acquire) {
                        if pipe.connect(&stop).is_err() {
                            pipe.disconnect();
                            if !stop.load(Ordering::Acquire) { std::thread::sleep(Duration::from_millis(25)); }
                            continue;
                        }
                        let mut service = service::Connection::new(Arc::clone(&shared), Arc::clone(&stop));
                        let close_after_reply = serve_connection(&pipe, &stop, &mut service).unwrap_or(false);
                        if close_after_reply { break; }
                        // Do not FlushFileBuffers: a stalled client could block
                        // daemon cleanup. All pending IO has completed/drained.
                        pipe.disconnect();
                    }
                })?);
        }
        Ok(listener)
    }
}
impl Drop for Listener {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        for worker in self.workers.drain(..) { let _ = worker.join(); }
    }
}
fn write(pipe: &CompatPipe, stop: &AtomicBool, value: &serde_json::Value) -> io::Result<()> {
    pipe.write(&protocol::encode(value)?, Instant::now() + FRAME_BUDGET, stop)
}
fn serve_connection(pipe: &CompatPipe, stop: &AtomicBool, service: &mut service::Connection) -> io::Result<bool> {
    while !stop.load(Ordering::Acquire) {
        let mut failure = None;
        let body = read_message(|buffer, deadline| pipe.read(buffer, deadline, stop, &mut || {
            if failure.is_none() {
                for event in service.events() {
                    if let Err(error) = write(pipe, stop, &event) {
                        failure = Some(error);
                        pipe.cancel_pending();
                        break;
                    }
                }
            }
        }))?;
        if let Some(error) = failure { return Err(error); }
        if let Some(response) = protocol::response(&body, service) {
            pipe.write(&protocol::encode_response(&response)?, Instant::now() + FRAME_BUDGET, stop)?;
        }
        if service.after_reply() {
            // CloseHandle preserves normal pipe EOF semantics; explicitly
            // disconnecting here would discard an unread final update reply.
            return Ok(true);
        }
        // Busy clients must receive events too, rather than depending on IO wait.
        for event in service.events() { write(pipe, stop, &event)?; }
    }
    Ok(false)
}

/// Exact reads preserve coalesced frames and fragmented headers without an
/// unbounded receive buffer. Idle budget starts per message; after the first
/// byte, the complete header/body share a five-second budget.
fn read_message(mut read: impl FnMut(&mut [u8], Instant) -> io::Result<usize>) -> io::Result<Vec<u8>> {
    let mut header = Vec::with_capacity(128);
    let mut deadline = Instant::now() + IDLE_BUDGET;
    loop {
        let mut byte = [0];
        if read(&mut byte, deadline)? != 1 { return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "RPC connection closed")); }
        if header.is_empty() { deadline = Instant::now() + FRAME_BUDGET; }
        header.push(byte[0]);
        if header.len() > protocol::MAX_HEADER { return Err(io::Error::new(io::ErrorKind::InvalidData, "RPC headers exceed 4 KiB")); }
        if header.ends_with(b"\r\n\r\n") { break; }
    }
    let size = protocol::body_length(&header)?;
    let mut body = vec![0; size];
    let mut filled = 0;
    while filled < size {
        let bytes = read(&mut body[filled..], deadline)?;
        if bytes == 0 || bytes > size - filled { return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "RPC body truncated")); }
        filled += bytes;
    }
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    #[test]
    fn persistent_coalesced_and_fragmented_streamjsonrpc_frames() {
        let value = serde_json::json!({"jsonrpc":"2.0","method":"GetTablets","id":1});
        let encoded = protocol::encode(&value).unwrap();
        let mut stream = std::io::Cursor::new([encoded.clone(),encoded].concat());
        for _ in 0..2 {
            let body = read_message(|buffer, _| { let size = buffer.len().min(3); stream.read(&mut buffer[..size]) }).unwrap();
            assert_eq!(serde_json::from_slice::<serde_json::Value>(&body).unwrap(), value);
        }
        assert!(read_message(|buffer, _| stream.read(buffer)).is_err());
    }
    #[test]
    fn truncated_body_and_oversized_headers_never_dispatch() {
        for bytes in [b"Content-Length: 4\r\n\r\n{}".to_vec(), vec![b'x';protocol::MAX_HEADER + 1]] {
            let mut stream = std::io::Cursor::new(bytes);
            assert!(read_message(|buffer, _| stream.read(buffer)).is_err());
        }
        assert_eq!(service::METHODS.len(), 19);
    }
}
