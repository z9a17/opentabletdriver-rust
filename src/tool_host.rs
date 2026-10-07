//! Global tool construction and disposal stay outside native control dispatch.
use std::sync::{Mutex,OnceLock,mpsc::{self,SyncSender}};
use std::thread::JoinHandle;
use std::time::Duration;
use otd_core::plugins::{PluginConfig,PluginKind};
use serde_json::Value;
enum Task {Configs(Vec<PluginConfig>,Option<SyncSender<Result<(),String>>>),Document(Value,Option<SyncSender<Result<(),String>>>),Suspend(std::sync::Arc<Mutex<Option<Result<(),String>>>>),Resume(Value),ResumeConfigs(Vec<PluginConfig>),Stop}
pub struct Completion {result:std::sync::Arc<Mutex<Option<Result<(),String>>>>}
impl Completion {pub fn result(&self)->Option<Result<(),String>> {match self.result.lock(){Ok(result)=>result.clone(),Err(_)=>Some(Err("Global tool completion poisoned".into()))}}}
static CURRENT:OnceLock<Mutex<Option<SyncSender<Task>>>>=OnceLock::new();
pub struct Owner {tx:SyncSender<Task>,join:Option<JoinHandle<()>>}
fn configs(document:&Value)->Result<Vec<PluginConfig>,String> {
    let stores=document["Tools"].as_array().ok_or("Original Tools must be an array")?;
    if !stores.iter().any(|store|store["Enable"].as_bool()==Some(true)) {return Ok(Vec::new());}
    crate::plugins::load_parser_registry()?;
    let registry=crate::dotnet::registry_snapshot().ok_or("Original tool registry unavailable")?;
    stores.iter().filter(|store|store["Enable"].as_bool()==Some(true)).map(|store|{
        let path=store["Path"].as_str().ok_or("Original tool path required")?;
        let mut entries=registry.plugins.iter().filter(|entry|entry.config.kind==PluginKind::DotnetTool && entry.config.type_name==path);
        let mut config=entries.next().ok_or_else(||format!("Original tool '{path}' is not installed"))?.config.clone();
        if entries.next().is_some(){return Err(format!("Original tool '{path}' is ambiguous"));}
        config.enabled=true;config.settings_json=crate::config::Profile::managed_store_settings(store)?;config.validate()?;Ok(config)
    }).collect()
}
impl Owner {
    pub fn start()->Result<Self,String> {
        let (tx,rx)=mpsc::sync_channel(8);
        let join=std::thread::Builder::new().name("tool-configuration".into()).spawn(move||{
            let owner=match crate::global_tools::Owner::start(|line|{
                eprintln!("{line}");
                let _=crate::daemon::call(crate::control::Command::WriteMessage {message:crate::control::UpstreamLogMessage::native(line.into())});
            }) {Ok(owner)=>owner,Err(error)=>{eprintln!("Global tool owner: {error}");return;}};
            let mut current:Option<Vec<u8>>=None;
            let mut suspended=false;
            while let Ok(task)=rx.recv() {
                let mut completion=None;
                let (desired,reply)=match task {
                    Task::Stop=>break,
                    Task::Suspend(result)=>{suspended=true;completion=Some(result);(Ok(Vec::new()),None)},
                    Task::Resume(document)=>{suspended=false;(configs(&document),None)},
                    Task::ResumeConfigs(configs)=>{suspended=false;(Ok(configs),None)},
                    Task::Configs(configs,reply)=>(if suspended{Err("Global tools are reserved for update".into())}else{Ok(configs)},reply),
                    Task::Document(document,reply)=>(if suspended{Err("Global tools are reserved for update".into())}else{configs(&document)},reply)
                };
                let result=desired.and_then(|configs|{
                    let encoded=serde_json::to_vec(&configs).map_err(|error|error.to_string())?;
                    if current.as_ref()==Some(&encoded){return Ok(());}
                    let state=owner.handle.snapshot()?;
                    owner.handle.set(state.generation,configs)?.wait(Duration::from_secs(30))?;
                    current=Some(encoded);Ok(())
                });
                if let Err(error)=&result {eprintln!("Global tools: {error}");}
                if let Some(completion)=completion {if let Ok(mut state)=completion.lock(){*state=Some(result.clone());}crate::control::wake();}
                if let Some(reply)=reply {let _=reply.try_send(result);}
            }
            drop(owner);
        }).map_err(|error|error.to_string())?;
        *CURRENT.get_or_init(||Mutex::new(None)).lock().map_err(|_|"Global tool dispatcher poisoned")?=Some(tx.clone());
        Ok(Self{tx,join:Some(join)})
    }
}
fn submit(task:Task)->Result<(),String> {
    let tx=CURRENT.get_or_init(||Mutex::new(None)).lock().map_err(|_|"Global tool dispatcher poisoned")?.clone().ok_or("Global tool owner unavailable")?;
    tx.try_send(task).map_err(|_|"Global tool dispatcher full or stopped".into())
}
pub fn apply_profile(configs:&[PluginConfig])->Result<(),String>{submit(Task::Configs(configs.iter().filter(|config|config.kind==PluginKind::DotnetTool).cloned().collect(),None))}
pub fn apply_document(document:&Value)->Result<(),String> {
    let (reply,rx)=mpsc::sync_channel(1);submit(Task::Document(document.clone(),Some(reply)))?;
    rx.recv_timeout(Duration::from_secs(35)).map_err(|_|"Global tool application timed out; settings may already be committed; query state before retrying".to_owned())?
}
pub fn initialize_document(document:Value)->Result<(),String>{submit(Task::Document(document,None))}
pub fn suspend()->Result<Completion,String>{let result=std::sync::Arc::new(Mutex::new(None));
    if CURRENT.get_or_init(||Mutex::new(None)).lock().map_err(|_|"Global tool dispatcher poisoned")?.is_none(){
        *result.lock().map_err(|_|"Global tool completion poisoned")?=Some(Ok(()));
    }else{submit(Task::Suspend(result.clone()))?;}Ok(Completion{result})}
pub fn resume(document:Value)->Result<(),String>{if CURRENT.get_or_init(||Mutex::new(None)).lock().map_err(|_|"Global tool dispatcher poisoned")?.is_none(){return Ok(());}submit(Task::Resume(document))}
pub fn resume_profile(configs:&[PluginConfig])->Result<(),String>{if CURRENT.get_or_init(||Mutex::new(None)).lock().map_err(|_|"Global tool dispatcher poisoned")?.is_none(){return Ok(());}submit(Task::ResumeConfigs(configs.iter().filter(|config|config.kind==PluginKind::DotnetTool).cloned().collect()))}
pub fn drain()->Result<(),String>{let (reply,rx)=mpsc::sync_channel(1);submit(Task::Configs(Vec::new(),Some(reply)))?;
    rx.recv_timeout(Duration::from_secs(35)).map_err(|_|"Global tool drain is incomplete".to_owned())?}
impl Drop for Owner {fn drop(&mut self){
    if let Ok(mut current)=CURRENT.get_or_init(||Mutex::new(None)).lock(){*current=None;}
    let _=self.tx.send(Task::Stop);if let Some(join)=self.join.take(){let _=join.join();}
}}
