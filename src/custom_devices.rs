//! Sole physical read owner for an actual original managed custom hub stream.
//! Each bounded ring is preallocated. CLR reads run only on this dedicated
//! worker; native source polling never invokes managed code or allocates.
use std::{io, sync::{Arc,Mutex,Condvar,atomic::{AtomicBool,AtomicU64,Ordering}},
    thread::JoinHandle,time::{Duration,Instant}};
use crate::dotnet::custom_devices as managed;
const RING_BYTES:usize=2*1024*1024;

pub struct Handle {
    token:u64, endpoint:u64, closed:AtomicBool,
    completion:Mutex<Option<Result<(),String>>>, completed:Condvar,
}
impl Handle {
    fn check(&self)->io::Result<()> {if self.closed.load(Ordering::Acquire){Err(io::Error::new(io::ErrorKind::BrokenPipe,"Managed physical reader retired"))}else{Ok(())}}
    pub fn write(&self,data:&[u8])->io::Result<()> {self.check()?;managed::write(self.token,data).map_err(io::Error::other)}
    pub fn get_feature(&self,data:&mut[u8])->io::Result<()> {self.check()?;managed::get_feature(self.token,data).map_err(io::Error::other)}
    pub fn set_feature(&self,data:&[u8])->io::Result<()> {self.check()?;managed::set_feature(self.token,data).map_err(io::Error::other)}
    pub fn string(&self,index:u8)->io::Result<String> {self.check()?;managed::device_string(self.endpoint,index).map_err(io::Error::other)}
    fn start_close(self:&Arc<Self>) {
        if self.closed.swap(true,Ordering::AcqRel){return;}
        let owner=self.clone();
        // Arbitrary third-party Dispose may hang. It never runs on native
        // control/report threads; an independent close unblocks ordinary Read.
        let started=std::thread::Builder::new().name("OTD managed stream close".into()).spawn(move||{
            let result=std::panic::catch_unwind(||managed::close(owner.token))
                .unwrap_or_else(|_|Err("Managed stream close panicked".into()));
            if let Ok(mut completion)=owner.completion.lock(){*completion=Some(result);owner.completed.notify_all();}
        });
        if let Err(error)=started {if let Ok(mut completion)=self.completion.lock(){*completion=Some(Err(error.to_string()));self.completed.notify_all();}}
    }
    fn wait_close(&self,deadline:Instant)->io::Result<()> {
        let mut completion=self.completion.lock().map_err(|_|io::Error::other("Managed close poisoned"))?;
        loop {
            if let Some(result)=completion.as_ref(){return result.clone().map_err(io::Error::other);}
            let left=deadline.saturating_duration_since(Instant::now());
            if left.is_zero(){return Err(io::Error::new(io::ErrorKind::TimedOut,"Original managed Dispose did not finish; physical stream retirement remains incomplete"));}
            completion=self.completed.wait_timeout(completion,left).map_err(|_|io::Error::other("Managed close poisoned"))?.0;
        }
    }
}
struct Ring { bytes:Box<[u8]>, lengths:Box<[usize]>, sequences:Box<[u64]>,
    head:usize, count:usize, error:Option<String>, done:bool }
struct Shared { ring:Mutex<Ring>, attempted:AtomicU64, lost:AtomicU64,
    stopped:AtomicBool, wake:Arc<dyn Fn()+Send+Sync> }
pub struct Reader { handle:Arc<Handle>,shared:Arc<Shared>,thread:Option<JoinHandle<()>>,report_length:usize,sequence:u64 }
impl Reader {
    pub fn open(endpoint:u64,report_length:usize,wake:Arc<dyn Fn()+Send+Sync>)->io::Result<Self> {
        if endpoint==0 || !(1..=65535).contains(&report_length){return Err(io::Error::new(io::ErrorKind::InvalidInput,"Invalid custom managed endpoint/report size"));}
        let token=managed::open(endpoint).map_err(io::Error::other)?;
        let handle=Arc::new(Handle{token,endpoint,closed:AtomicBool::new(false),completion:Mutex::new(None),completed:Condvar::new()});
        let capacity=(RING_BYTES/report_length).clamp(8,256);
        let shared=Arc::new(Shared{ring:Mutex::new(Ring {bytes:vec![0;capacity*report_length].into_boxed_slice(),
            lengths:vec![0;capacity].into_boxed_slice(),sequences:vec![0;capacity].into_boxed_slice(),head:0,count:0,error:None,done:false}),
            attempted:AtomicU64::new(0),lost:AtomicU64::new(0),stopped:AtomicBool::new(false),wake});
        let owner=handle.clone();let target=shared.clone();
        let started=std::thread::Builder::new().name("OTD original managed device reader".into()).spawn(move||{
            let mut buffer=vec![0u8;report_length];
            while !target.stopped.load(Ordering::Acquire) {
                let result=managed::read(owner.token,&mut buffer);
                if target.stopped.load(Ordering::Acquire){break;}
                match result {
                    Ok(length)=>{
                        let sequence=target.attempted.fetch_add(1,Ordering::Relaxed).saturating_add(1);
                        if let Ok(mut ring)=target.ring.try_lock() {
                            if ring.count==ring.lengths.len(){target.lost.fetch_add(1,Ordering::Relaxed);}
                            else {let index=(ring.head+ring.count)%ring.lengths.len();let offset=index*report_length;
                                ring.bytes[offset..offset+length].copy_from_slice(&buffer[..length]);ring.lengths[index]=length;
                                ring.sequences[index]=sequence;ring.count+=1;}
                        } else {target.lost.fetch_add(1,Ordering::Relaxed);}
                    },
                    Err(error)=>{if let Ok(mut ring)=target.ring.lock(){ring.error=Some(error);}break;}
                }
                (target.wake)();
            }
            if let Ok(mut ring)=target.ring.lock(){ring.done=true;}(target.wake)();
        });
        let thread=match started {Ok(thread)=>thread,Err(error)=>{handle.start_close();return Err(error);}};
        Ok(Self{handle,shared,thread:Some(thread),report_length,sequence:0})
    }
    pub fn handle(&self)->Arc<Handle>{self.handle.clone()}
    pub fn lost_reports(&self)->u64{self.shared.lost.load(Ordering::Acquire)}
    pub fn poll(&mut self,output:&mut[u8])->io::Result<Option<usize>> {
        if self.lost_reports()!=0{return Err(io::Error::new(io::ErrorKind::InvalidData,format!("Managed physical report ring lost {} reports; stream continuity is unavailable",self.lost_reports())));}
        let mut ring=self.shared.ring.lock().map_err(|_|io::Error::other("Managed input ring poisoned"))?;
        if ring.count>0 {
            let index=ring.head;let length=ring.lengths[index];
            if length>output.len(){return Err(io::Error::new(io::ErrorKind::InvalidInput,"Native custom report buffer is too small"));}
            if ring.sequences[index]!=self.sequence.saturating_add(1){return Err(io::Error::new(io::ErrorKind::InvalidData,"Managed physical report sequence gap"));}
            let offset=index*self.report_length;output[..length].copy_from_slice(&ring.bytes[offset..offset+length]);
            self.sequence=ring.sequences[index];ring.head=(index+1)%ring.lengths.len();ring.count-=1;return Ok(Some(length));
        }
        if let Some(error)=&ring.error{return Err(io::Error::new(io::ErrorKind::BrokenPipe,error.clone()));}
        if ring.done{return Err(io::Error::new(io::ErrorKind::BrokenPipe,"Managed physical reader closed"));}
        Ok(None)
    }
    pub fn close(&mut self,timeout:Duration)->io::Result<()> {
        let deadline=Instant::now()+timeout;
        self.shared.stopped.store(true,Ordering::Release);self.handle.start_close();(self.shared.wake)();
        let result=self.handle.wait_close(deadline);
        while self.thread.as_ref().is_some_and(|thread|!thread.is_finished()) && Instant::now()<deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        if self.thread.as_ref().is_some_and(JoinHandle::is_finished) {
            if self.thread.take().expect("finished worker").join().is_err(){return Err(io::Error::other("Managed physical reader panicked"));}
        } else if self.thread.is_some(){return Err(io::Error::new(io::ErrorKind::TimedOut,"Original managed Read did not cancel; reader retirement remains incomplete"));}
        result
    }
}
impl Drop for Reader {fn drop(&mut self){if let Err(error)=self.close(Duration::from_secs(3)){eprintln!("Managed device retirement failed: {error}");}}}
