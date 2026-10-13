//! Explicit cold I/O is admitted through the existing reader's mailbox. The
//! inactive successful native path performs no allocation or blocking lock.
use std::fs::File;
use std::io;
use std::os::fd::{AsRawFd,FromRawFd,RawFd};
use std::sync::{Arc,mpsc,atomic::{AtomicBool,Ordering}};
use std::time::{Duration,Instant};
use crate::managed_services::Operation;

pub struct OutputWake {read:File,write:File}
impl OutputWake {
    pub fn create(_manual:bool) -> io::Result<Self> {
        let mut fds=[-1;2];
        if unsafe{libc::pipe(fds.as_mut_ptr())}!=0{return Err(io::Error::last_os_error());}
        let value=Self{read:unsafe{File::from_raw_fd(fds[0])},write:unsafe{File::from_raw_fd(fds[1])}};
        for fd in fds {
            if unsafe{libc::fcntl(fd,libc::F_SETFD,libc::FD_CLOEXEC)}<0||unsafe{libc::fcntl(fd,libc::F_SETFL,libc::O_NONBLOCK)}<0{return Err(io::Error::last_os_error());}
        }Ok(value)
    }
    pub fn signal(&self) -> io::Result<()> {
        let result=unsafe{libc::write(self.write.as_raw_fd(),[1u8].as_ptr().cast(),1)};
        if result<0{let error=io::Error::last_os_error();if error.kind()!=io::ErrorKind::WouldBlock{return Err(error);}}Ok(())
    }
    pub fn raw(&self) -> RawFd {self.read.as_raw_fd()}
    pub fn drain(&self) {let mut bytes=[0u8;64];while unsafe{libc::read(self.read.as_raw_fd(),bytes.as_mut_ptr().cast(),bytes.len())}>0{}}
    pub fn take_signal(&self)->bool{let mut bytes=[0u8;64];let mut signaled=false;while unsafe{libc::read(self.read.as_raw_fd(),bytes.as_mut_ptr().cast(),bytes.len())}>0{signaled=true;}signaled}
}
#[derive(Clone,Copy)]
pub enum RequestKind {Report(Operation),String(u8)}
struct Call {kind:RequestKind,data:Vec<u8>,reply:mpsc::SyncSender<io::Result<Vec<u8>>>,deadline:Instant}
/// A cold admitted request. Keeping it alive retains its buffer and reply until
/// the existing reader finishes the native asynchronous operation.
pub struct ServiceCall {pub kind:RequestKind,pub data:Vec<u8>,reply:mpsc::SyncSender<io::Result<Vec<u8>>>,pub deadline:Instant}
impl ServiceCall {
    pub fn finish(self,result:io::Result<()>) {let _=self.reply.try_send(result.map(|_|self.data));}
}
pub struct SharedIo {queue:mpsc::SyncSender<Call>,pub wake:Arc<OutputWake>,closed:AtomicBool,
    pending:AtomicBool,output_length:u32,feature_length:u32}
pub struct ReaderServices {rx:mpsc::Receiver<Call>,io:Arc<SharedIo>}
impl SharedIo {
    pub fn new(output_length:u32,feature_length:u32) -> io::Result<(Arc<Self>,ReaderServices)> {
        let (queue,rx)=mpsc::sync_channel(16);let io=Arc::new(Self{queue,wake:Arc::new(OutputWake::create(true)?),closed:AtomicBool::new(false),pending:AtomicBool::new(false),output_length,feature_length});
        Ok((io.clone(),ReaderServices{rx,io}))
    }
    pub fn cancel_services(&self){self.closed.store(true,Ordering::Release);let _=self.wake.signal();}
    fn call(&self,kind:RequestKind,data:Vec<u8>) -> io::Result<Vec<u8>> {
        if self.closed.load(Ordering::Acquire){return Err(io::Error::new(io::ErrorKind::BrokenPipe,"Physical reader retired"));}
        let (reply,rx)=mpsc::sync_channel(1);
        self.queue.try_send(Call{kind,data,reply,deadline:Instant::now()+Duration::from_secs(5)}).map_err(|_|io::Error::new(io::ErrorKind::WouldBlock,"Physical reader service mailbox is full or closed"))?;
        self.pending.store(true,Ordering::Release);self.wake.signal()?;
        rx.recv_timeout(Duration::from_secs(5)).map_err(|_|io::Error::new(io::ErrorKind::TimedOut,"Physical reader I/O timed out; query owner before retrying a write"))?
    }
    pub fn string(&self,index:u8) -> io::Result<String> {
        String::from_utf8(self.call(RequestKind::String(index),Vec::new())?).map_err(|error|io::Error::new(io::ErrorKind::InvalidData,error))
    }
    pub fn service(&self,operation:Operation,data:&mut Vec<u8>) -> io::Result<()> {
        let length=if operation==Operation::WriteStream{self.output_length}else{self.feature_length} as usize;
        if length==0||length>65535||data.is_empty()||data.len()>length{return Err(io::Error::new(io::ErrorKind::InvalidInput,"Report exceeds endpoint capability"));}
        let requested=data.len();let mut bytes=data.clone();bytes.resize(length,0);
        let result=self.call(RequestKind::Report(operation),bytes)?;
        if operation==Operation::GetFeature{if result.len()<requested{return Err(io::Error::new(io::ErrorKind::UnexpectedEof,"Feature report returned too few bytes"));}data.copy_from_slice(&result[..requested]);}
        Ok(())
    }
}
impl ReaderServices {
    pub fn wake(&self)->&OutputWake {&self.io.wake}
    pub fn next(&mut self)->Option<ServiceCall> {
        next_service(&self.io,||self.rx.try_recv())
    }
    /// Only the existing reader invokes the handler; no HID handle crosses
    /// ownership. No queue or kernel call is made when no services are pending.
    pub fn drain(&mut self,mut handle:impl FnMut(RequestKind,&mut Vec<u8>)->io::Result<()>) {
        if !self.io.pending.swap(false,Ordering::AcqRel){return;}
        self.io.wake.drain();
        for _ in 0..16{match self.rx.try_recv(){Ok(mut call)=>{
            let result=if Instant::now()>=call.deadline{Err(io::Error::new(io::ErrorKind::TimedOut,"Reader service expired before execution"))}
                else if self.io.closed.load(Ordering::Acquire){Err(io::Error::new(io::ErrorKind::BrokenPipe,"Physical reader retired"))}
                else{handle(call.kind,&mut call.data).map(|_|call.data)};
            let _=call.reply.try_send(result);
        },Err(_)=>break}}
    }
}
fn next_service(io:&SharedIo,mut receive:impl FnMut()->Result<Call,mpsc::TryRecvError>)->Option<ServiceCall> {
    if !io.pending.load(Ordering::Acquire){return None;}
    loop {
        let call=match receive(){
            Ok(call)=>call,
            Err(_)=>{
                io.pending.store(false,Ordering::Release);io.wake.drain();
                // A sender can enqueue immediately before the flag is cleared.
                // Recheck the queue, then use the same admission checks as the
                // first receive before exposing any operation to the reader.
                match receive(){Ok(call)=>{io.pending.store(true,Ordering::Release);call},Err(_)=>return None}
            }
        };
        if Instant::now()>=call.deadline||io.closed.load(Ordering::Acquire){
            let _=call.reply.try_send(Err(io::Error::new(io::ErrorKind::TimedOut,"Reader service expired or reader retired")));
            continue;
        }
        return Some(ServiceCall{kind:call.kind,data:call.data,reply:call.reply,deadline:call.deadline});
    }
}
impl Drop for ReaderServices {fn drop(&mut self){self.io.cancel_services();while let Ok(call)=self.rx.try_recv(){let _=call.reply.try_send(Err(io::Error::new(io::ErrorKind::BrokenPipe,"Physical reader retired")));}}}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raced_recheck_never_admits_expired_or_retired_calls() {
        for retired in [false,true] {
            let(io,_services)=SharedIo::new(8,8).unwrap();
            io.pending.store(true,Ordering::Release);
            let(reply,rx)=mpsc::sync_channel(1);
            let deadline=if retired{Instant::now()+Duration::from_secs(5)}else{Instant::now()-Duration::from_secs(1)};
            let mut call=Some(Call{kind:RequestKind::Report(Operation::WriteStream),data:vec![0;8],reply,deadline});
            let mut reads=0;
            let admitted=next_service(&io,||{
                reads+=1;
                if reads==1{return Err(mpsc::TryRecvError::Empty);}
                if retired{io.closed.store(true,Ordering::Release);}
                call.take().ok_or(mpsc::TryRecvError::Empty)
            });
            assert!(admitted.is_none());
            assert_eq!(rx.try_recv().unwrap().unwrap_err().kind(),io::ErrorKind::TimedOut);
        }
    }

    #[test]
    fn valid_raced_recheck_preserves_buffer_and_reply() {
        let(io,_services)=SharedIo::new(8,8).unwrap();
        io.pending.store(true,Ordering::Release);
        let(reply,rx)=mpsc::sync_channel(1);
        let mut call=Some(Call{kind:RequestKind::Report(Operation::WriteStream),data:vec![1,2],reply,deadline:Instant::now()+Duration::from_secs(5)});
        let mut reads=0;
        let admitted=next_service(&io,||{
            reads+=1;
            if reads==1{Err(mpsc::TryRecvError::Empty)}else{call.take().ok_or(mpsc::TryRecvError::Empty)}
        }).unwrap();
        assert!(io.pending.load(Ordering::Acquire));
        admitted.finish(Ok(()));
        assert_eq!(rx.try_recv().unwrap().unwrap(),vec![1,2]);
    }
}
