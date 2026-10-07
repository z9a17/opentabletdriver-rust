//! One cold global tool owner, independent of the number of tablet readers.
//! Submission never initializes CLR. Wait for receipts outside daemon dispatch
//! so unchanged tool constructors can consult actual native services.
use std::sync::{Arc,Mutex,mpsc::{self,SyncSender,Receiver}};
use std::thread::JoinHandle;
use std::time::Duration;
use otd_core::plugins::{PluginConfig,PluginKind};

#[derive(Clone,Debug,Default)]
pub struct State {pub generation:u64,pub configured:usize,pub started:usize,pub failures:Vec<String>}
enum Work {Set{generation:u64,configs:Vec<PluginConfig>,reply:SyncSender<Result<State,String>>},Stop}
#[derive(Clone)]
pub struct Handle {tx:SyncSender<Work>,state:Arc<Mutex<State>>,next:Arc<Mutex<u64>>}
pub struct Receipt {rx:Receiver<Result<State,String>>}
impl Receipt {pub fn wait(self,timeout:Duration)->Result<State,String>{
    self.rx.recv_timeout(timeout).map_err(|_|"Global tools are still changing; query generation before retrying".to_owned())?
}}
impl Handle {
    pub fn pending(&self)->Result<bool,String>{let next=self.next.lock().map_err(|_|"Global tool generation poisoned")?;Ok(*next!=self.snapshot()?.generation)}
    pub fn snapshot(&self)->Result<State,String>{self.state.lock().map(|state|state.clone()).map_err(|_|"Global tool state poisoned".into())}
    /// The caller uses the observed committed generation. Reservations prevent
    /// a second client from overwriting a pending, not-yet-completed change.
    pub fn set(&self,expected:u64,configs:Vec<PluginConfig>)->Result<Receipt,String>{
        for config in &configs {config.validate()?;if config.kind!=PluginKind::DotnetTool{return Err("Global tool owner accepts unchanged tool configs only".into());}}
        let mut next=self.next.lock().map_err(|_|"Global tool generation poisoned")?;
        let state=self.snapshot()?;
        if state.generation!=expected||*next!=state.generation{return Err("Global tools changed or a replacement is pending; refresh before applying".into());}
        let generation=next.checked_add(1).ok_or("Global tool generation exhausted")?;
        let (reply,rx)=mpsc::sync_channel(1);
        self.tx.try_send(Work::Set{generation,configs,reply}).map_err(|_|"Global tool queue is full or stopped")?;
        *next=generation;Ok(Receipt{rx})
    }
    pub fn drain(&self,expected:u64)->Result<Receipt,String>{self.set(expected,Vec::new())}
}
pub struct Owner {pub handle:Handle,join:Option<JoinHandle<()>>}
impl Owner {
    pub fn start(log:impl Fn(&str)+Send+'static)->Result<Self,String>{
        let (tx,rx)=mpsc::sync_channel(8);let state=Arc::new(Mutex::new(State::default()));let next=Arc::new(Mutex::new(0));
        let worker_state=state.clone();
        let join=std::thread::Builder::new().name("global-tools".into()).spawn(move||{
            let mut tools=None;
            while let Ok(work)=rx.recv(){match work{
                Work::Stop=>break,
                Work::Set{generation,configs,reply}=>{
                    // Old tools release their timers/leases before constructors
                    // for this collection run. Upstream logs/skips a failed tool.
                    drop(tools.take());
                    if let Err(error)=crate::dotnet::drain_managed_retirements(Duration::from_secs(15)) {
                        if let Ok(mut state)=worker_state.lock(){*state=State{generation,configured:0,started:0,failures:vec![error.clone()]};}
                        let _=reply.try_send(Err(error));continue;
                    }
                    let configured=configs.iter().filter(|config|config.enabled).count();
                    let counts=std::cell::RefCell::new((0usize,Vec::new()));
                    let value=crate::plugins::Tools::start(&configs,|line|{
                        let mut counts=counts.borrow_mut();
                        if line.starts_with("Started tool "){counts.0+=1;}else if line.starts_with("Failed to start tool "){counts.1.push(line.to_owned());}log(line);
                    });
                    tools=Some(value);
                    if let Err(error)=crate::dotnet::drain_managed_retirements(Duration::from_secs(15)) {
                        let(started,mut failures)=counts.into_inner();failures.push(error.clone());
                        if let Ok(mut state)=worker_state.lock(){*state=State{generation,configured,started,failures};}
                        let _=reply.try_send(Err(error));continue;
                    }
                    let(started,failures)=counts.into_inner();
                    let snapshot=State{generation,configured,started,failures};
                    let result=worker_state.lock().map_err(|_|"Global tool state poisoned".to_owned()).map(|mut state|{*state=snapshot.clone();snapshot});
                    let _=reply.try_send(result);
                },
            }}drop(tools);
            if let Err(error)=crate::dotnet::drain_managed_retirements(Duration::from_secs(15)){log(&error);}
        }).map_err(|error|error.to_string())?;
        Ok(Self{handle:Handle{tx,state,next},join:Some(join)})
    }
}
impl Drop for Owner {fn drop(&mut self){
    // Owner teardown must happen outside a handler that tool disposal needs.
    let _=self.handle.tx.send(Work::Stop);if let Some(join)=self.join.take(){let _=join.join();}
}}
