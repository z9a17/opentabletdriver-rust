//! Sole physical read owner for an actual original managed custom hub stream.
//! Each bounded ring is preallocated. CLR reads run only on this dedicated
//! worker; native source polling never invokes managed code or allocates.
use std::{io, sync::{Arc,Weak,OnceLock,Mutex,Condvar,atomic::{AtomicBool,AtomicU64,AtomicUsize,Ordering}},
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
    ready:Box<[Instant]>,head:usize, count:usize, error:Option<String>, done:bool }
struct Shared { ring:Mutex<Ring>, attempted:AtomicU64, lost:AtomicU64,
    stopped:AtomicBool, wake:Mutex<Arc<dyn Fn()+Send+Sync>> }
struct Pump {handle:Arc<Handle>,shared:Arc<Shared>,thread:Mutex<Option<JoinHandle<()>>>,report_length:usize,closing:AtomicBool,leases:AtomicUsize}
static POOL:OnceLock<Mutex<std::collections::HashMap<u64,Weak<Pump>>>>=OnceLock::new();
fn pool()->&'static Mutex<std::collections::HashMap<u64,Weak<Pump>>>{POOL.get_or_init(||Mutex::new(std::collections::HashMap::new()))}
pub struct Reader {pump:Arc<Pump>,sequence:u64,closed:bool,wake:Arc<dyn Fn()+Send+Sync>}
impl Reader {
    pub fn open(endpoint:u64,report_length:usize,wake:Arc<dyn Fn()+Send+Sync>)->io::Result<Self> {
        if endpoint==0 || !(1..=65535).contains(&report_length){return Err(io::Error::new(io::ErrorKind::InvalidInput,"Invalid custom managed endpoint/report size"));}
        let mut owners=pool().lock().map_err(|_|io::Error::other("Managed stream owner pool poisoned"))?;
        owners.retain(|_,owner|owner.strong_count()>0);
        if let Some(owner)=owners.get(&endpoint).and_then(Weak::upgrade).filter(|owner|!owner.closing.load(Ordering::Acquire)) {
            if owner.report_length!=report_length{return Err(io::Error::new(io::ErrorKind::InvalidData,"Custom endpoint input length changed while physically open"));}
            owner.leases.fetch_add(1,Ordering::Relaxed);
            return Ok(Self{pump:owner,sequence:0,closed:false,wake});
        }
        let token=managed::open(endpoint).map_err(io::Error::other)?;
        let handle=Arc::new(Handle{token,endpoint,closed:AtomicBool::new(false),completion:Mutex::new(None),completed:Condvar::new()});
        let capacity=(RING_BYTES/report_length).clamp(8,256);
        let shared=Arc::new(Shared{ring:Mutex::new(Ring {bytes:vec![0;capacity*report_length].into_boxed_slice(),
            lengths:vec![0;capacity].into_boxed_slice(),sequences:vec![0;capacity].into_boxed_slice(),ready:vec![Instant::now();capacity].into_boxed_slice(),head:0,count:0,error:None,done:false}),
            attempted:AtomicU64::new(0),lost:AtomicU64::new(0),stopped:AtomicBool::new(false),wake:Mutex::new(wake.clone())});
        let owner=handle.clone();let target=shared.clone();
        let started=std::thread::Builder::new().name("OTD original managed device reader".into()).spawn(move||{
            let mut buffer=vec![0u8;report_length];
            while !target.stopped.load(Ordering::Acquire) {
                let result=managed::read(owner.token,&mut buffer);
                let ready=Instant::now();
                if target.stopped.load(Ordering::Acquire){break;}
                match result {
                    Ok(length)=>{
                        if let Ok(mut ring)=target.ring.lock() {
                            let sequence=target.attempted.fetch_add(1,Ordering::Relaxed).saturating_add(1);
                            if ring.count==ring.lengths.len(){target.lost.fetch_add(1,Ordering::Relaxed);}
                            else {let index=(ring.head+ring.count)%ring.lengths.len();let offset=index*report_length;
                                ring.bytes[offset..offset+length].copy_from_slice(&buffer[..length]);ring.lengths[index]=length;
                                ring.sequences[index]=sequence;ring.ready[index]=ready;ring.count+=1;}
                        } else {target.lost.fetch_add(1,Ordering::Relaxed);}
                    },
                    Err(error)=>{if let Ok(mut ring)=target.ring.lock(){ring.error=Some(error);}break;}
                }
                if let Ok(wake)=target.wake.lock().map(|wake|wake.clone()){wake();}
            }
            if let Ok(mut ring)=target.ring.lock(){ring.done=true;}if let Ok(wake)=target.wake.lock().map(|wake|wake.clone()){wake();}
        });
        let thread=match started {Ok(thread)=>thread,Err(error)=>{handle.start_close();return Err(error);}};
        let pump=Arc::new(Pump{handle,shared,thread:Mutex::new(Some(thread)),report_length,closing:AtomicBool::new(false),leases:AtomicUsize::new(1)});
        owners.insert(endpoint,Arc::downgrade(&pump));
        Ok(Self{pump,sequence:0,closed:false,wake})
    }
    pub fn handle(&self)->Arc<Handle>{self.pump.handle.clone()}
    pub fn lost_reports(&self)->u64{self.pump.shared.lost.load(Ordering::Acquire)}
    pub fn poll(&mut self,output:&mut[u8])->io::Result<Option<usize>> {
        self.poll_report(output).map(|report|report.map(|(length,_)|length))
    }
    /// Completion time belongs to the sole original Read, before native tee
    /// queuing or a slower consumer. It is retained in preallocated slots.
    pub fn poll_report(&mut self,output:&mut[u8])->io::Result<Option<(usize,Instant)>> {
        if self.closed{return Err(io::Error::new(io::ErrorKind::BrokenPipe,"Managed reader lease retired"));}
        if self.lost_reports()!=0{return Err(io::Error::new(io::ErrorKind::InvalidData,format!("Managed physical report ring lost {} reports; stream continuity is unavailable",self.lost_reports())));}
        let mut ring=self.pump.shared.ring.lock().map_err(|_|io::Error::other("Managed input ring poisoned"))?;
        if ring.count>0 {
            let index=ring.head;let length=ring.lengths[index];
            if length>output.len(){return Err(io::Error::new(io::ErrorKind::InvalidInput,"Native custom report buffer is too small"));}
            if ring.sequences[index]!=self.sequence.saturating_add(1){return Err(io::Error::new(io::ErrorKind::InvalidData,"Managed physical report sequence gap"));}
            let offset=index*self.pump.report_length;output[..length].copy_from_slice(&ring.bytes[offset..offset+length]);
            let ready=ring.ready[index];self.sequence=ring.sequences[index];ring.head=(index+1)%ring.lengths.len();ring.count-=1;return Ok(Some((length,ready)));
        }
        if let Some(error)=&ring.error{return Err(io::Error::new(io::ErrorKind::BrokenPipe,error.clone()));}
        if ring.done{return Err(io::Error::new(io::ErrorKind::BrokenPipe,"Managed physical reader closed"));}
        Ok(None)
    }
    /// Called only after the prior source quiesces: this is the FIFO consumer
    /// handoff. Old queued reports/loss belong to the retired source boundary.
    pub fn flush(&mut self)->io::Result<()> {
        let mut ring=self.pump.shared.ring.lock().map_err(|_|io::Error::other("Managed input ring poisoned"))?;
        if self.closed || self.pump.closing.load(Ordering::Acquire){return Err(io::Error::new(io::ErrorKind::BrokenPipe,"Managed reader lease retired"));}
        ring.head=0;ring.count=0;self.sequence=self.pump.shared.attempted.load(Ordering::Acquire);
        self.pump.shared.lost.store(0,Ordering::Release);
        *self.pump.shared.wake.lock().map_err(|_|io::Error::other("Managed wake poisoned"))?=self.wake.clone();Ok(())
    }
    pub fn close(&mut self,timeout:Duration)->io::Result<()> {
        if self.closed{return Ok(());}self.closed=true;
        let owners=pool().lock().map_err(|_|io::Error::other("Managed stream owner pool poisoned"))?;
        if self.pump.leases.fetch_sub(1,Ordering::AcqRel)>1 {return Ok(());}
        self.pump.closing.store(true,Ordering::Release);
        drop(owners);
        let deadline=Instant::now()+timeout;
        self.pump.shared.stopped.store(true,Ordering::Release);self.pump.handle.start_close();if let Ok(wake)=self.pump.shared.wake.lock().map(|wake|wake.clone()){wake();}
        let result=self.pump.handle.wait_close(deadline);
        let mut worker=self.pump.thread.lock().map_err(|_|io::Error::other("Managed reader join poisoned"))?;
        while worker.as_ref().is_some_and(|thread|!thread.is_finished()) && Instant::now()<deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        if worker.as_ref().is_some_and(JoinHandle::is_finished) {
            if worker.take().expect("finished worker").join().is_err(){return Err(io::Error::other("Managed physical reader panicked"));}
        } else if worker.is_some(){return Err(io::Error::new(io::ErrorKind::TimedOut,"Original managed Read did not cancel; reader retirement remains incomplete"));}
        result
    }
}
impl Drop for Reader {fn drop(&mut self){if let Err(error)=self.close(Duration::from_secs(3)){eprintln!("Managed device retirement failed: {error}");}}}
