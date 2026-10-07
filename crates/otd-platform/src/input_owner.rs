//! One OS-code ownership domain for every native reader and original Desktop
//! input scope. Storage for native transitions is fixed; OS acceptance precedes
//! acknowledgement. Managed leases run on their separate cold service lane.
use std::{io,sync::{Mutex,OnceLock,atomic::{AtomicU64,Ordering}},time::{Duration,Instant},collections::BTreeMap};
use serde_json::Value;
use crate::managed_services::Operation;
#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub enum Code {Key(u16),Button(u8)}
impl Code {fn index(self)->io::Result<usize>{match self{Self::Key(code)if code<768=>Ok(code as usize),Self::Button(button)if button<5=>Ok(768+button as usize),_=>Err(io::Error::new(io::ErrorKind::InvalidInput,"Input code exceeds supported OS domain"))}}}
#[derive(Clone,Copy,PartialEq,Eq)]
enum Owner {Native(u64,u32),Managed(u64)}
#[derive(Clone,Copy)]
struct Hold {owner:Owner,index:usize}
type Writer=Box<dyn FnMut(Code,bool,Option<(f64,f64)>)->io::Result<()>+Send>;
struct State {holds:[Option<Hold>;4096],desired:[usize;773],emitted:[bool;773],positions:[Option<(f64,f64)>;773],writer:Option<Writer>,leases:BTreeMap<u64,Instant>}
impl State {
    fn hold(&mut self,owner:Owner,code:Code,held:bool,position:Option<(f64,f64)>)->io::Result<()>{
        let index=code.index()?;let existing=self.holds.iter().position(|hold|hold.is_some_and(|hold|hold.owner==owner&&hold.index==index));
        if held&&existing.is_none(){let slot=self.holds.iter_mut().find(|slot|slot.is_none()).ok_or_else(||io::Error::other("Shared input owner capacity reached"))?;*slot=Some(Hold{owner,index});self.desired[index]+=1;}
        else if !held{if let Some(slot)=existing{self.holds[slot]=None;self.desired[index]-=1;}}
        if self.emitted[index]!=(self.desired[index]!=0){self.positions[index]=position;}
        match self.flush(){
            // An unrelated pending failure must not hide this accepted prefix
            // from LocalActions, otherwise it cannot release what was sent.
            Err(_)if self.emitted[index]==(self.desired[index]!=0)=>Ok(()),
            Err(error)=>{if held&&existing.is_none(){if let Some(slot)=self.holds.iter().position(|hold|hold.is_some_and(|hold|hold.owner==owner&&hold.index==index)){self.holds[slot]=None;self.desired[index]-=1;}}
                Err(error)},Ok(())=>Ok(())
        }
    }
    fn flush(&mut self)->io::Result<()>{
        // Releases precede presses. Native chord ordering is already supplied
        // by LocalActions; a retry never acknowledges an unaccepted prefix.
        for pressed in [false,true]{for modifier in [pressed,!pressed]{for index in 0..773{let desired=self.desired[index]!=0;
            let is_modifier=if cfg!(target_os="linux"){[29,42,56,125,97,54,100,126].contains(&index)}else{[59,56,58,55,62,60,61,54].contains(&index)};
            if is_modifier==modifier&&desired==pressed&&self.emitted[index]!=desired{
                let code=if index<768{Code::Key(index as u16)}else{Code::Button((index-768) as u8)};
                (self.writer.as_mut().ok_or_else(||io::Error::other("Actual platform input writer is unavailable"))?)(code,pressed,self.positions[index])?;self.emitted[index]=pressed;self.positions[index]=None;
            }
        }}}Ok(())
    }
    fn release(&mut self,owner:Owner)->io::Result<()>{for hold in &mut self.holds{if hold.is_some_and(|hold|hold.owner==owner){let index=hold.take().unwrap().index;self.desired[index]-=1;}}self.flush()}
    fn release_native(&mut self,id:u64)->io::Result<()>{for hold in &mut self.holds{if hold.is_some_and(|hold|matches!(hold.owner,Owner::Native(owner,_)if owner==id)){let index=hold.take().unwrap().index;self.desired[index]-=1;}}self.flush()}
}
static STATE:OnceLock<Mutex<State>>=OnceLock::new();
fn state()->&'static Mutex<State>{STATE.get_or_init(||Mutex::new(State{holds:[None;4096],desired:[0;773],emitted:[false;773],positions:[None;773],writer:None,leases:BTreeMap::new()}))}
/// The factory is called once during cold resource setup. Keep the sole actual
/// OS writer alive through all readers and managed scope cleanup.
pub fn ensure(factory:impl FnOnce()->io::Result<Writer>)->io::Result<()>{let mut state=state().lock().map_err(|_|io::Error::other("Shared input ownership poisoned"))?;if state.writer.is_none(){state.writer=Some(factory()?);}Ok(())}
pub struct Native {id:u64}
impl Native {
    pub fn new()->io::Result<Self>{static NEXT:AtomicU64=AtomicU64::new(1);let id=NEXT.fetch_update(Ordering::Relaxed,Ordering::Relaxed,|id|id.checked_add(1)).map_err(|_|io::Error::other("Input owner IDs exhausted"))?;Ok(Self{id})}
    pub fn hold(&self,code:Code,held:bool)->io::Result<()>{self.hold_at(code,held,None)}
    pub fn hold_at(&self,code:Code,held:bool,position:Option<(f64,f64)>)->io::Result<()>{self.hold_action(0,code,held,position)}
    /// LocalActions merges bindings per portable action. Preserve that action's
    /// identity here: two distinct portable keys can alias one OS key code.
    pub fn hold_action(&self,action:u32,code:Code,held:bool,position:Option<(f64,f64)>)->io::Result<()>{state().lock().map_err(|_|io::Error::other("Shared input ownership poisoned"))?.hold(Owner::Native(self.id,action),code,held,position)}
    pub fn release(&self)->io::Result<()>{state().lock().map_err(|_|io::Error::other("Shared input ownership poisoned"))?.release_native(self.id)}
}
impl Drop for Native {fn drop(&mut self){if let Err(error)=self.release(){eprintln!("Shared native input cleanup failed: {error}");}}}
pub fn button_down(button:u8)->bool{state().lock().ok().is_some_and(|state|button<5&&state.emitted[768+button as usize])}
pub fn buttons()->u8{state().lock().map_or(0,|state|(0..5).fold(0,|mask,button|mask|((state.emitted[768+button] as u8)<<button)))}
pub fn key_mask(codes:[u16;8])->u8{state().lock().map_or(0,|state|codes.into_iter().enumerate().fold(0,|mask,(bit,code)|mask|((code<768&&state.emitted[code as usize]) as u8)<<bit))}
pub fn execute(operation:Operation,scope:u64,payload:&Value)->Result<Value,String>{
    if scope==0{return Err("Original input scope must be nonzero".into());}
    let mut state=state().lock().map_err(|_|"Shared input ownership poisoned")?;
    if operation==Operation::InputRelease{state.release(Owner::Managed(scope)).map_err(|error|error.to_string())?;state.leases.remove(&scope);return Ok(Value::Null);}
    if operation!=Operation::InputHold{return Err("Invalid original input operation".into());}
    let kind=payload["type"].as_str().ok_or("Original input type required")?;
    if kind=="renew"&&!state.leases.contains_key(&scope){return Err("Original input scope has expired".into());}
    if !state.leases.contains_key(&scope)&&state.leases.len()>=256{return Err("Original input scope capacity reached".into());}
    if kind!="renew"{
        let raw=payload["code"].as_u64().and_then(|code|u16::try_from(code).ok()).ok_or("Original input code required")?;
        let code=match kind{
            "key"=>{let platform=if cfg!(target_os="linux"){"linux"}else{"macos"};if payload["platform"]!=platform||(cfg!(target_os="linux")&&raw==0)||raw>=if cfg!(target_os="linux"){768}else{128}{return Err("Original keyboard platform/code is invalid".into());}Code::Key(raw)},
            "button"=>Code::Button(match raw{0=>return Ok(Value::Null),1=>0,2=>2,3=>1,4=>3,5=>4,_=>return Err("Unknown original mouse button".into())}),
            _=>return Err("Unknown original input type".into()),
        };
        let held=payload["held"].as_bool().ok_or("Original input held state required")?;
        // Keep a lease even when an OS send fails: expiry retries all pending
        // cleanup, and a later successful event cannot escape ownership.
        let lease=payload["lease_ms"].as_u64().unwrap_or(15000).clamp(1000,60000);
        state.leases.insert(scope,Instant::now()+Duration::from_millis(lease));
        let repeat=held&&matches!(code,Code::Key(_))&&state.holds.iter().any(|hold|hold.is_some_and(|hold|hold.owner==Owner::Managed(scope)&&hold.index==code.index().unwrap()));
        state.hold(Owner::Managed(scope),code,held,None).map_err(|error|error.to_string())?;
        if repeat{(state.writer.as_mut().ok_or("Actual platform input writer is unavailable")?)(code,true,None).map_err(|error|error.to_string())?;}
    }
    let lease=payload["lease_ms"].as_u64().unwrap_or(15000).clamp(1000,60000);state.leases.insert(scope,Instant::now()+Duration::from_millis(lease));Ok(Value::Null)
}
pub fn maintain(){if let Ok(mut state)=state().lock(){let now=Instant::now();let expired=state.leases.iter().filter_map(|(scope,expires)|(*expires<=now).then_some(*scope)).collect::<Vec<_>>();for scope in expired{match state.release(Owner::Managed(scope)){Ok(())=>{state.leases.remove(&scope);},Err(error)=>{state.leases.insert(scope,now+Duration::from_secs(1));eprintln!("Original input lease cleanup failed: {error}");}}}
    // A retired native reader has no lease or future report to trigger retry.
    // The independent lane remains the cleanup owner even with no scopes.
    let _=state.flush();
}}
pub fn shutdown(){if let Ok(mut state)=state().lock(){let scopes=state.leases.keys().copied().collect::<Vec<_>>();for scope in scopes{match state.release(Owner::Managed(scope)){Ok(())=>{state.leases.remove(&scope);},Err(error)=>eprintln!("Original input shutdown cleanup failed: {error}")}}
    for _ in 0..3{if state.flush().is_ok(){return;}std::thread::sleep(Duration::from_millis(50));}
    eprintln!("Shared input shutdown retains unacknowledged cleanup; actual output owner cannot be claimed clean");
}}
#[cfg(test)]
mod fixtures {
    use super::*;
    fn owner(writer:Writer)->State{State{holds:[None;4096],desired:[0;773],emitted:[false;773],positions:[None;773],writer:Some(writer),leases:BTreeMap::new()}}
    #[test]
    fn distinct_portable_aliases_and_managed_scope_require_the_last_release(){
        let events=std::sync::Arc::new(Mutex::new(Vec::new()));let captured=events.clone();
        let mut state=owner(Box::new(move|code,held,_|{captured.lock().unwrap().push((code,held));Ok(())}));
        state.hold(Owner::Native(1,0x53),Code::Key(47),true,None).unwrap();
        state.hold(Owner::Native(1,0x9c),Code::Key(47),true,None).unwrap();
        state.hold(Owner::Managed(2),Code::Key(47),true,None).unwrap();
        state.hold(Owner::Native(1,0x53),Code::Key(47),false,None).unwrap();
        state.release_native(1).unwrap();assert_eq!(*events.lock().unwrap(),vec![(Code::Key(47),true)]);
        state.release(Owner::Managed(2)).unwrap();assert_eq!(*events.lock().unwrap(),vec![(Code::Key(47),true),(Code::Key(47),false)]);
    }
    #[test]
    fn retired_native_pending_release_remains_retryable_without_any_lease(){
        let failures=std::sync::Arc::new(std::sync::atomic::AtomicU64::new(1));let output=failures.clone();
        let mut state=owner(Box::new(move|_,held,_|{if !held&&output.swap(0,Ordering::Relaxed)!=0{Err(io::Error::from(io::ErrorKind::WouldBlock))}else{Ok(())}}));
        state.hold(Owner::Native(1,4),Code::Key(30),true,None).unwrap();assert!(state.release_native(1).is_err());
        assert!(state.leases.is_empty());assert!(state.emitted[30]);assert_eq!(state.desired[30],0);
        state.flush().unwrap();assert!(!state.emitted[30]);
    }
}
