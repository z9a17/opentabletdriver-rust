//! Managed endpoint leases share the native physical reader and its owned
//! handle. The report thread only copies into preallocated slots while a
//! subscriber exists; all JSON, waits and hardware service calls stay cold.
#[cfg(windows)]
mod io;
#[cfg(unix)]
use crate::shared_device_io as io;
pub use io::SharedIo;
use std::{cell::RefCell, collections::HashMap, sync::{Arc, Mutex, OnceLock, Weak,
    atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering}}, time::{Duration, Instant}};
use serde_json::{Value, json};
#[cfg(windows)]
use crate::hid::Event;
#[cfg(unix)]
use crate::shared_device_io::OutputWake as Event;

const MAX_STREAMS: usize = 64;
const MAX_RING_BYTES: usize = 2 * 1024 * 1024;
const LEASE: Duration = Duration::from_secs(15);
static NEXT: AtomicU64 = AtomicU64::new(1);
static EVENT_SEQUENCE: AtomicU64 = AtomicU64::new(1);
static REGISTRY: OnceLock<Mutex<Registry>> = OnceLock::new();
thread_local! { static SOURCE: RefCell<Option<Value>> = const { RefCell::new(None) }; }
fn registry() -> &'static Mutex<Registry> { REGISTRY.get_or_init(|| Mutex::new(Registry::default())) }
fn next() -> Result<u64,String> { NEXT.fetch_update(Ordering::Relaxed,Ordering::Relaxed,|n|n.checked_add(1)).map_err(|_| "Device lease identity exhausted".into()) }
pub fn source_session_json() -> Option<Value> { SOURCE.with(|source| source.borrow().clone()) }
pub fn live_source(value:&Value)->bool{
    let (Some(id),Some(generation),Some(epoch))=(value["id"].as_str(),value["device_generation"].as_u64(),value["reader_generation"].as_u64()) else{return false};
    registry().lock().ok().is_some_and(|state|state.readers.get(&epoch).is_some_and(|endpoints|endpoints.iter().filter_map(Weak::upgrade).any(|endpoint|
        endpoint.session_id==id&&endpoint.generation==generation&&endpoint.alive.load(Ordering::Acquire)&&endpoint.initialized.load(Ordering::Acquire)
            &&endpoint.output.started.load(Ordering::Acquire))))
}

/// One output authority for both collections of a physical tablet.
pub struct OutputGate {
    desired: AtomicBool, acknowledged: AtomicBool, started: AtomicBool,
    wake: Event, changed: std::sync::Condvar, gate: Mutex<()>,
}
// The event is a kernel synchronization object; state is atomic or locked.
unsafe impl Send for OutputGate {}
unsafe impl Sync for OutputGate {}
impl OutputGate {
    pub fn new() -> std::io::Result<Arc<Self>> { Ok(Arc::new(Self { desired: AtomicBool::new(true),
        acknowledged: AtomicBool::new(true), started: AtomicBool::new(false), wake: Event::create(true)?,
        changed: std::sync::Condvar::new(),gate:Mutex::new(()) })) }
    pub fn native_enabled(&self) -> bool { self.desired.load(Ordering::Acquire) }
    pub fn transition_pending(&self)->bool {self.native_enabled()!=self.acknowledged.load(Ordering::Acquire)}
    pub fn signal(&self)->std::io::Result<()> {self.wake.signal()}
    #[cfg(windows)]
    pub fn event(&self) -> windows_sys::Win32::Foundation::HANDLE { self.wake.raw() }
    #[cfg(unix)]
    pub fn reader_wake(&self) -> &Event { &self.wake }
    pub fn acknowledge(&self, enabled: bool) { if let Ok(_guard)=self.gate.lock() {
        self.acknowledged.store(enabled,Ordering::Release);self.changed.notify_all(); } }
    pub fn start(&self) { if let Ok(_guard)=self.gate.lock() { self.started.store(true,Ordering::Release); } }
    pub fn request(&self,native:bool) -> Result<(),String> {
        let mut guard=self.gate.lock().map_err(|_|"Output owner poisoned")?;
        self.desired.store(native,Ordering::Release);
        if !self.started.load(Ordering::Acquire) { self.acknowledged.store(native,Ordering::Release);return Ok(()); }
        self.wake.signal().map_err(|e|e.to_string())?;
        let until=Instant::now()+Duration::from_secs(3);
        while self.acknowledged.load(Ordering::Acquire)!=native {
            let left=until.saturating_duration_since(Instant::now());
            if left.is_zero() { return Err("Output ownership transfer did not finish; query owner before retry".into()); }
            let (next,_)=self.changed.wait_timeout(guard,left).map_err(|_|"Output owner poisoned")?;guard=next;
        } Ok(())
    }
}
struct Ring { bytes:Box<[u8]>,lengths:Box<[usize]>,sequences:Box<[u64]>,events:Box<[u64]>,write:usize }
pub struct Endpoint {
    pub io: Arc<SharedIo>, pub output: Arc<OutputGate>, pub epoch:u64, pub session_id:String, generation:u64,
    identifier:Value,configuration:Value,
    path:String,parser:String,auxiliary:bool,tablet:Mutex<Value>, report_length:usize,
    alive:AtomicBool,pub initialized:AtomicBool,subscribers:AtomicUsize,sequence:AtomicU64,ring:Mutex<Ring>,
}
pub struct Registration { pub endpoint:Arc<Endpoint> }
impl Registration {
    pub fn new(path:String,parser:String,auxiliary:bool,io:Arc<SharedIo>,output:Arc<OutputGate>,
        report_length:usize,tablet:Value,identifier:Value,configuration:Value,epoch:Option<u64>) -> Result<Self,String> {
        if !(1..=65535).contains(&report_length) { return Err("Invalid shared endpoint report length".into()); }
        let session_id=crate::device_sessions::debug_key().unwrap_or_else(||format!("foreground:{}",std::process::id()));
        let epoch=epoch.map_or_else(next,Ok)?;
        let capacity=(MAX_RING_BYTES/report_length).clamp(8,256);
        let generation=crate::device_sessions::source_generation();
        let endpoint=Arc::new(Endpoint { io,output,epoch,session_id:session_id.clone(),generation,identifier,configuration,path,parser,auxiliary,
            tablet:Mutex::new(tablet),report_length,alive:AtomicBool::new(true),initialized:AtomicBool::new(false),
            subscribers:AtomicUsize::new(0),sequence:AtomicU64::new(0),ring:Mutex::new(Ring {
            bytes:vec![0;capacity*report_length].into_boxed_slice(),lengths:vec![0;capacity].into_boxed_slice(),
            sequences:vec![0;capacity].into_boxed_slice(),events:vec![0;capacity].into_boxed_slice(),write:0 }) });
        registry().lock().map_err(|_|"Endpoint registry poisoned")?.readers.entry(epoch).or_default().push(Arc::downgrade(&endpoint));
        if !auxiliary { SOURCE.with(|source|*source.borrow_mut()=Some(json!({"id":session_id,"device_generation":generation,"reader_generation":epoch}))); }
        Ok(Self{endpoint})
    }
    pub fn publish(&self,bytes:&[u8]) {
        let endpoint=&self.endpoint;
        if endpoint.subscribers.load(Ordering::Relaxed)==0 || bytes.len()>endpoint.report_length { return; }
        let sequence=endpoint.sequence.fetch_add(1,Ordering::Relaxed)+1;
        let event=EVENT_SEQUENCE.fetch_add(1,Ordering::Relaxed);
        // A slow managed consumer never stalls input. Failed try_lock leaves a
        // sequence gap which the consumer must report, not silently conceal.
        let Ok(mut ring)=endpoint.ring.try_lock() else { return; };
        let index=ring.write;let offset=index*endpoint.report_length;
        ring.bytes[offset..offset+bytes.len()].copy_from_slice(bytes);
        ring.lengths[index]=bytes.len();ring.sequences[index]=sequence;ring.events[index]=event;
        ring.write=(index+1)%ring.lengths.len();
    }
    pub fn tablet(&self,tablet:Value) { if let Ok(mut target)=self.endpoint.tablet.lock(){*target=tablet;} }
}
impl Drop for Registration { fn drop(&mut self) {
    self.endpoint.alive.store(false,Ordering::Release);self.endpoint.io.cancel_services();
    if !self.endpoint.auxiliary { SOURCE.with(|source| {
        if source.borrow().as_ref().is_some_and(|value|value["reader_generation"].as_u64()==Some(self.endpoint.epoch)) { *source.borrow_mut()=None; }
    }); }
} }
struct Stream { endpoint:Arc<Endpoint>,scope:u64,expires:Instant,cursor:u64,lost:u64 }
impl Drop for Stream { fn drop(&mut self){self.endpoint.subscribers.fetch_sub(1,Ordering::Relaxed);} }
#[derive(Default)]
struct Registry { readers:HashMap<u64,Vec<Weak<Endpoint>>>, streams:HashMap<u64,Stream>,
    events:HashMap<u64,EventLease>, output_owners:HashMap<u64,u64> }
struct EventLease { streams:HashMap<(u64,bool),Stream>,expires:Instant,cursor:u64,lost:u64,
    last_request:Option<u64>,last_reply:Option<Value> }
fn prune(state:&mut Registry) {
    let now=Instant::now();state.streams.retain(|_,stream|now<stream.expires && stream.endpoint.alive.load(Ordering::Acquire));
    state.events.retain(|_,stream|now<stream.expires);
    state.readers.retain(|_,endpoints| { endpoints.retain(|endpoint|endpoint.upgrade().is_some_and(|e|e.alive.load(Ordering::Acquire)));!endpoints.is_empty() });
    state.output_owners.retain(|epoch,_|state.readers.contains_key(epoch));
}
fn readers(state:&Registry)->Vec<Arc<Endpoint>> { state.readers.values().flatten().filter_map(Weak::upgrade)
    .filter(|e|e.alive.load(Ordering::Acquire)).collect() }
pub fn owned_metadata()->Vec<Value> { registry().lock().ok().map(|mut state|{prune(&mut state); readers(&state).into_iter().map(|e|json!({
    "path":e.path,"DevicePath":e.path,"Identifier":e.identifier,"Configuration":e.configuration,
    "session_id":e.session_id,"device_generation":e.generation,"reader_generation":e.epoch,
    "initialized":e.initialized.load(Ordering::Acquire),"parser":e.parser,"auxiliary":e.auxiliary,
    "report_length":e.report_length,"tablet":e.tablet.lock().ok().map(|v|v.clone())})).collect()}).unwrap_or_default() }
pub fn device_string(path:&str,index:u8)->Option<Result<String,String>> {
    let endpoint=registry().lock().ok().and_then(|state|readers(&state).into_iter().find(|e|e.path.eq_ignore_ascii_case(path)))?;
    Some(endpoint.io.string(index).map_err(|error|error.to_string()))
}
fn number(value:&Value,key:&str)->Result<u64,String>{value[key].as_u64().ok_or_else(||format!("Missing device {key}"))}
fn stream<'a>(state:&'a mut Registry,scope:u64,payload:&Value)->Result<&'a mut Stream,String> {
    let id=number(payload,"stream")?;let stream=state.streams.get_mut(&id).ok_or("Shared endpoint lease is closed or expired")?;
    if stream.scope!=scope || !stream.endpoint.alive.load(Ordering::Acquire) { return Err("Shared endpoint owner changed".into()); }
    stream.expires=Instant::now()+LEASE;Ok(stream)
}
fn packets(stream:&mut Stream,after:u64,limit:usize)->Result<Vec<Value>,String> {
    let endpoint=&stream.endpoint;let ring=endpoint.ring.lock().map_err(|_|"Report ring poisoned")?;
    let after=after.max(stream.cursor);
    let mut slots:Vec<_>=ring.sequences.iter().copied().enumerate().filter(|(_,s)|*s>after).collect();
    slots.sort_unstable_by_key(|(_,s)|*s);let mut out=Vec::new();let mut expected=after.saturating_add(1);
    for (index,sequence) in slots.into_iter().take(limit) {
        stream.lost=stream.lost.saturating_add(sequence.saturating_sub(expected));expected=sequence.saturating_add(1);
        let offset=index*endpoint.report_length;let raw=&ring.bytes[offset..offset+ring.lengths[index]];
        out.push(json!({"sequence":sequence,"event_sequence":ring.events[index],"data":encode(raw)}));stream.cursor=sequence;
    }
    // A last dropped report is observable even when no later packet arrives.
    let received=endpoint.sequence.load(Ordering::Acquire);
    if out.len()<limit && received>stream.cursor {stream.lost=stream.lost.saturating_add(received-stream.cursor);stream.cursor=received;}
    Ok(out)
}
pub fn execute(operation:crate::managed_services::Operation,scope:u64,payload:&Value)->Result<Value,String>{
    use crate::managed_services::Operation;
    let mut state=registry().lock().map_err(|_|"Endpoint registry poisoned")?;prune(&mut state);
    match operation {
        Operation::OpenStream=>{
            if state.streams.len()>=MAX_STREAMS{return Err("Shared endpoint lease limit reached".into());}
            let path=payload["path"].as_str().ok_or("Endpoint path required")?;
            let epoch=payload.get("reader_generation").and_then(Value::as_u64)
                .or_else(||payload["source_session"]["reader_generation"].as_u64());
            let mut found:Vec<_>=readers(&state).into_iter().filter(|e|e.path.eq_ignore_ascii_case(path)&&epoch.is_none_or(|epoch|e.epoch==epoch)).collect();
            if let Some(id)=payload["source_session"]["id"].as_str() { found.retain(|e|e.session_id==id); }
            if found.len()!=1 {return Err("Shared endpoint needs one exact physical reader generation".into());}
            let endpoint=found.pop().ok_or("Endpoint has no owned physical reader")?;
            let cursor=endpoint.sequence.load(Ordering::Acquire);let id=next()?;
            endpoint.subscribers.fetch_add(1,Ordering::Relaxed);
            let result=json!({"stream":id,"session_id":endpoint.session_id,"device_generation":endpoint.generation,
                "reader_generation":endpoint.epoch,"initialized":endpoint.initialized.load(Ordering::Acquire),
                "report_length":endpoint.report_length,"last_sequence":cursor});
            state.streams.insert(id,Stream{endpoint,scope,expires:Instant::now()+LEASE,cursor,lost:0});Ok(result)
        },
        Operation::ReadStream=>{
            let stream=stream(&mut state,scope,payload)?;let after=payload["after_sequence"].as_u64().unwrap_or(stream.cursor);
            let limit=payload["limit"].as_u64().unwrap_or(1).clamp(1,32) as usize;
            let reports=packets(stream,after,limit)?;Ok(json!({"reports":reports,"next_sequence":stream.cursor,"lost_reports":stream.lost,"closed":false}))
        },
        Operation::CloseStream=>{
            let id=number(payload,"stream")?;
            if state.streams.get(&id).is_some_and(|s|s.scope!=scope){return Err("Shared endpoint scope differs".into());}
            state.streams.remove(&id);Ok(Value::Null)
        },
        Operation::WriteStream|Operation::GetFeature|Operation::SetFeature=>{
            let endpoint=Arc::clone(&stream(&mut state,scope,payload)?.endpoint);
            let mut data=decode(payload["data"].as_str().ok_or("Endpoint data required")?)?;
            // Do not hold registry/ring locks over a real OS operation.
            drop(state);if !endpoint.alive.load(Ordering::Acquire){return Err("Physical reader retired".into());}
            endpoint.io.service(operation,&mut data).map_err(|e|e.to_string())?;
            Ok(if operation==Operation::GetFeature {json!(encode(&data))}else{Value::Null})
        },
        Operation::OutputOwner=>{
            let id=payload["session_id"].as_str().ok_or("Output source session required")?;
            let epoch=payload["reader_generation"].as_u64().or_else(||payload["device_generation"].as_u64()).ok_or("Output reader generation required")?;
            let endpoint=readers(&state).into_iter().find(|e|e.session_id==id&&e.epoch==epoch&&!e.auxiliary).ok_or("Output physical source changed")?;
            let managed=payload["managed_output"].as_bool().ok_or("Output ownership flag required")?;
            match state.output_owners.get(&epoch).copied() {
                Some(owner) if owner!=scope=>return Err("This physical reader already has another managed output owner".into()),
                None if !managed=>return Ok(Value::Null),
                _=>{}
            }
            if managed {state.output_owners.insert(epoch,scope);}
            drop(state);endpoint.output.request(!managed)?;
            if !managed {if let Ok(mut state)=registry().lock() {if state.output_owners.get(&epoch)==Some(&scope){state.output_owners.remove(&epoch);}}}
            Ok(Value::Null)
        },
        Operation::DeviceReports=>events(&mut state,scope,payload),
        _=>Err("Not an endpoint service operation".into()),
    }
}
fn events(state:&mut Registry,scope:u64,payload:&Value)->Result<Value,String>{
    if payload["enabled"].as_bool()==Some(false){state.events.remove(&scope);return Ok(json!({"events":[],"closed":true}));}
    let arming=payload["enabled"].as_bool()==Some(true);
    if !state.events.contains_key(&scope) {
        if !arming {return Err("Managed report scope is not armed".into());}
        if state.events.len()>=MAX_STREAMS{return Err("Managed event scope limit reached".into());}
        state.events.insert(scope,EventLease{streams:HashMap::new(),expires:Instant::now()+LEASE,cursor:0,lost:0,last_request:None,last_reply:None});
    }
    let endpoints=readers(state);let lease=state.events.get_mut(&scope).unwrap();lease.expires=Instant::now()+LEASE;
    for endpoint in endpoints {
        let key=(endpoint.epoch,endpoint.auxiliary);
        lease.streams.entry(key).or_insert_with(||{endpoint.subscribers.fetch_add(1,Ordering::Relaxed);
            Stream{cursor:endpoint.sequence.load(Ordering::Acquire),endpoint,scope,expires:Instant::now()+LEASE,lost:0}});
    }
    if arming {return Ok(json!({"events":[],"next_sequence":lease.cursor,"lost_reports":lease.lost,"closed":false}));}
    let requested=payload["cursor"].as_u64().ok_or("Managed event cursor required")?;
    if requested!=lease.cursor {
        if lease.last_request==Some(requested) {return lease.last_reply.clone().ok_or("Managed event retry unavailable".into());}
        return Err("Managed report cursor is stale".into());
    }
    let limit=payload["limit"].as_u64().unwrap_or(32).clamp(1,32) as usize;
    let mut out=Vec::new();let mut available=Vec::new();
    // Peek every endpoint before committing cursors, then merge by native read
    // sequence. Frames which do not fit retain their cursor for the next poll.
    for (key,stream) in &mut lease.streams {
        let cursor=stream.cursor;let lost=stream.lost;let items=packets(stream,cursor,limit)?;
        let observed_lost=stream.lost;
        if items.is_empty() {lease.lost=lease.lost.saturating_add(observed_lost.saturating_sub(lost));}
        else {stream.cursor=cursor;stream.lost=lost;}
        for item in items {available.push((*key,item,observed_lost));}
    }
    available.sort_unstable_by_key(|(_,item,_)|item["event_sequence"].as_u64().unwrap_or(0));
    let mut bytes=0;
    for (key,item,lost) in available.into_iter().take(limit) {
        let stream=lease.streams.get_mut(&key).unwrap();let endpoint=&stream.endpoint;
        let next_sequence=lease.cursor.checked_add(1).ok_or("Managed event sequence exhausted")?;
        let event=json!({"sequence":next_sequence,"session_id":endpoint.session_id,
            "device_generation":endpoint.generation,"reader_generation":endpoint.epoch,"auxiliary":endpoint.auxiliary,
            "parser":endpoint.parser,"raw":item["data"],"tablet":endpoint.tablet.lock().map_err(|_|"Endpoint metadata poisoned")?.clone()});
        let size=serde_json::to_vec(&event).map_err(|e|e.to_string())?.len();
        if bytes+size>3*1024*1024{break;}bytes+=size;
        lease.lost=lease.lost.saturating_add(lost.saturating_sub(stream.lost));stream.lost=lost;
        stream.cursor=item["sequence"].as_u64().unwrap();lease.cursor=next_sequence;out.push(event);
    }
    lease.streams.retain(|_,stream|stream.endpoint.alive.load(Ordering::Acquire)
        || stream.cursor<stream.endpoint.sequence.load(Ordering::Acquire));
    let reply=json!({"events":out,"next_sequence":lease.cursor,"lost_reports":lease.lost,"closed":false});
    lease.last_request=Some(requested);lease.last_reply=Some(reply.clone());Ok(reply)
}
fn encode(data:&[u8])->String { const HEX:&[u8;16]=b"0123456789ABCDEF";let mut output=String::with_capacity(data.len()*2);
    for byte in data{output.push(HEX[(byte>>4)as usize]as char);output.push(HEX[(byte&15)as usize]as char);}output }
fn decode(data:&str)->Result<Vec<u8>,String>{
    if data.is_empty()||data.len()>131070||data.len()%2!=0{return Err("Invalid endpoint report hex length".into());}
    fn nibble(byte:u8)->Option<u8>{match byte{b'0'..=b'9'=>Some(byte-b'0'),b'a'..=b'f'=>Some(byte-b'a'+10),b'A'..=b'F'=>Some(byte-b'A'+10),_=>None}}
    data.as_bytes().chunks_exact(2).map(|b|Ok((nibble(b[0]).ok_or("Invalid endpoint hex")?<<4)|nibble(b[1]).ok_or("Invalid endpoint hex")?)).collect()
}
