//! Slow original service calls run outside native transaction dispatch. A
//! successful update reserves all owners until the actual response is flushed.
use std::{collections::{BTreeMap,VecDeque},sync::{Arc,Condvar,Mutex,atomic::{AtomicBool,AtomicU64,Ordering}},time::{Duration,Instant}};
use serde_json::{Value,json};
use crate::{daemon::{Command,Handle,Platform},control::WorkerIdentity};
#[derive(Default)]
struct Update {checked:Option<crate::update::Release>,busy:bool,token:Option<String>,staged:bool,plugin_calls:usize}
pub(crate) struct Services {platform:Arc<dyn Platform>,pub(crate) stopped:Arc<AtomicBool>,update:Mutex<Update>,plugins_done:Condvar,debug:Mutex<Option<Debug>>}
struct PluginCall<'a>(&'a Services);
impl Drop for PluginCall<'_>{fn drop(&mut self){if let Ok(mut update)=self.0.update.lock(){update.plugin_calls=update.plugin_calls.saturating_sub(1);self.0.plugins_done.notify_all();}}}
impl Services {
    pub(crate) fn new(platform:Arc<dyn Platform>,stopped:Arc<AtomicBool>)->Self{Self{platform,stopped,update:Mutex::new(Update::default()),plugins_done:Condvar::new(),debug:Mutex::new(None)}}
    pub(crate) fn invoke(&self,handle:&Handle,method:&str,params:&Value,expected:Option<WorkerIdentity>)->Result<Option<Value>,String>{
        let args=params.as_array().ok_or("Original method params must be a positional array")?;
        if matches!(method,"LoadPlugins"|"InstallPlugin"|"DownloadPlugin"|"UninstallPlugin"|"InstallPluginDirectory"|"UpdatePluginDirectory"|"UnloadPluginContext"|"RemovePluginAssembly"){
            // Admission is shared by native and managed clients. Reserve the
            // call without holding the mutex through CLR/plugin callbacks.
            let mut update=self.update.lock().map_err(|_|"Update service poisoned")?;
            if update.busy{return Err("Update reservation blocks plugin mutations".into());}
            update.plugin_calls=update.plugin_calls.checked_add(1).ok_or("Plugin admission count exhausted")?;
            drop(update);let _admission=PluginCall(self);
            return crate::plugin_manager::invoke(method,params,Some(&self.stopped));
        }
        match method{
            "CheckForUpdates"=>{if !args.is_empty(){return Err("CheckForUpdates takes no arguments".into());}
                let release=crate::update::latest_with_cancel(Some(&self.stopped))?;let available=release.version>crate::update::current_version();
                let value=if available{json!({"Version":format!("{}.{}.{}",release.version.0,release.version.1,release.version.2)})}else{Value::Null};
                let mut update=self.update.lock().map_err(|_|"Update service poisoned")?;
                if !update.busy{update.checked=available.then_some(release);}Ok(Some(value))},
            "InstallUpdate"=>{if !args.is_empty(){return Err("InstallUpdate takes no arguments".into());}self.install(handle,expected)?;Ok(Some(Value::Null))},
            "FinishUpdate"=>{if !args.is_empty(){return Err("FinishUpdate takes no arguments".into());}
                let token={let update=self.update.lock().map_err(|_|"Update service poisoned")?;if !update.staged{return Err("No successful staged update awaits response flush".into());}update.token.clone().ok_or("Missing staged reservation")?};
                handle.call(Command::FinishUpdate{token})?;Ok(Some(Value::Null))},
            "RequestDeviceString"=>{if args.len()!=3{return Err("RequestDeviceString requires vendorID, productID, index".into());}
                let vendor=args[0].as_u64().and_then(|v|u16::try_from(v).ok()).ok_or("vendorID must be 0..65535")?;
                let product=args[1].as_u64().and_then(|v|u16::try_from(v).ok()).ok_or("productID must be 0..65535")?;
                let index=args[2].as_u64().and_then(|v|u8::try_from(v).ok()).ok_or("index must be 0..255")?;
                Ok(Some(json!(self.platform.device_string(vendor,product,index)?)))},
            "SetTabletDebug"=>{if args.len()!=1{return Err("SetTabletDebug requires isEnabled".into());}let enabled=args[0].as_bool().ok_or("isEnabled must be bool")?;
                let mut debug=self.debug.lock().map_err(|_|"Native debug owner poisoned")?;
                if enabled&&debug.is_none(){*debug=Some(Debug::start()?);}else if !enabled{*debug=None;}Ok(Some(Value::Null))},
            // Native console extension: original RPC has its own per-client
            // DeviceReport event subscription in ManagedProviders.ConfigureDebug.
            "GetDebugReports"=>{if !args.is_empty(){return Err("GetDebugReports takes no arguments".into());}
                let debug=self.debug.lock().map_err(|_|"Native debug owner poisoned")?;
                let debug=debug.as_ref().ok_or("Tablet debug is not enabled")?;
                let mut reports=debug.reports.lock().map_err(|_|"Native debug reports poisoned")?;
                let mut values=Vec::new();let mut bytes=0;while let Some(report)=reports.front(){let size=serde_json::to_vec(report).map_err(|e|e.to_string())?.len();if bytes+size>200*1024{break;}bytes+=size;values.push(reports.pop_front().unwrap());}Ok(Some(json!(values)))},
            _=>Ok(None)
        }
    }
    fn install(&self,handle:&Handle,expected:Option<WorkerIdentity>)->Result<(),String>{
        let release={let mut update=self.update.lock().map_err(|_|"Update service poisoned")?;
            if update.busy{return Err("Another update owns staging; query native update state".into());}
            if update.checked.is_none(){return Err("No checked newer update; call CheckForUpdates first".into());}
            update.busy=true;let deadline=Instant::now()+Duration::from_secs(60);
            while update.plugin_calls!=0{
                if self.stopped.load(Ordering::Acquire)||Instant::now()>=deadline{update.busy=false;return Err("Update admission could not drain existing plugin mutations".into());}
                update=self.plugins_done.wait_timeout(update,Duration::from_millis(250)).map_err(|_|"Update service poisoned")?.0;
            }
            update.checked.take().unwrap()};
        let mut token=None;let mut cleanup_complete=false;
        let install=std::env::current_exe().map_err(|error|error.to_string()).and_then(|path|path.parent().map(std::path::Path::to_owned).ok_or("Install directory unavailable".into()));
        let result=(||{
            let (reserved,receipt)=handle.reserve_update(expected)?;token=Some(reserved.clone());
            if let Some(receipt)=receipt{receipt.wait(Duration::from_secs(60))?;}
            handle.call(Command::DrainUpdate{token:reserved.clone()})?;
            crate::dotnet::drain_managed_retirements(Duration::from_secs(15))?;
            cleanup_complete=true;handle.call(Command::ReadyUpdate{token:reserved})?;
            crate::update::install_with_cancel(&release,install.as_ref().map_err(|error|error.clone())?,&|line|eprintln!("{line}"),Some(&self.stopped))
        })();
        if result.is_ok(){let mut update=self.update.lock().map_err(|_|"Update service poisoned")?;update.token=token;update.staged=true;return Ok(());}
        let mut error=result.unwrap_err();
        if let Some(reserved)=&token{
            // An incomplete cleanup cannot be undone by reopening old outputs.
            // Journal recovery after partial replacement requires a fresh process.
            if cleanup_complete{match install.as_ref().map_err(|error|error.clone()).and_then(|path|crate::update::recover_for_daemon(path)){
                Ok(false)=>{let resumed=handle.call(Command::CancelUpdate{token:reserved.clone()}).and_then(|_|handle.call(Command::ResumeUpdate{token:reserved.clone()}));
                    if let Err(rollback)=resumed{error.push_str(&format!("; update rollback failed, reservation retained: {rollback}"));}else{token=None;}},
                Ok(true)=>error.push_str("; interrupted file replacement recovered; drained reservation retained, restart the process"),
                Err(recovery)=>error.push_str(&format!("; recovery failed, reservation retained: {recovery}"))
            }}else{error.push_str("; cleanup incomplete, drained reservation retained");}
        }
        let mut update=self.update.lock().map_err(|_|"Update service poisoned")?;update.busy=token.is_some();update.token=token;update.staged=false;Err(error)
    }
}
pub(crate) fn operating_system()->Result<Value,String>{
    let mut name:libc::utsname=unsafe{std::mem::zeroed()};if unsafe{libc::uname(&mut name)}!=0{return Err(std::io::Error::last_os_error().to_string());}
    let text=|value:&[libc::c_char]|unsafe{std::ffi::CStr::from_ptr(value.as_ptr())}.to_string_lossy().into_owned();
    if cfg!(target_os="linux"){
        let mut distro="Linux".to_owned();let mut version="Unknown".to_owned();let mut attributes=BTreeMap::new();
        if let Some(path)=["/etc/os-release","/usr/lib/os-release"].into_iter().find(|path|std::path::Path::new(path).is_file()){
            for line in std::fs::read_to_string(path).map_err(|e|e.to_string())?.lines(){if line.starts_with('#'){continue;}let parts=line.split('=').map(str::trim).collect::<Vec<_>>();if parts.len()!=2{continue;}let value=parts[1].trim_matches('"').to_owned();match parts[0]{"NAME"=>distro=value,"VERSION"=>version=value,"ID"|"ID_LIKE"|"PRETTY_NAME"|"VARIANT"|"VARIANT_ID"|"VERSION_ID"|"VERSION_CODENAME"|"BUILD_ID"|"SUPPORT_END"=>{attributes.insert(parts[0].to_owned(),value);},_=>{}}}
        }
        attributes.insert("KERNEL_VERSION".into(),text(&name.release));Ok(json!({"Name":distro,"Version":version,"Attributes":attributes}))
    }else{Ok(json!({"Name":"Unix","Version":text(&name.release),"Attributes":Value::Null}))}
}
struct Debug {stop:Arc<AtomicBool>,reports:Arc<Mutex<VecDeque<Value>>>,join:Option<std::thread::JoinHandle<()>>}
impl Debug {
    fn start()->Result<Self,String>{
        static NEXT:AtomicU64=AtomicU64::new(1u64<<63);let scope=NEXT.fetch_update(Ordering::Relaxed,Ordering::Relaxed,|n|n.checked_add(1)).map_err(|_|"Native debug scope exhausted")?;
        crate::shared_devices::execute(crate::managed_services::Operation::DeviceReports,scope,&json!({"enabled":true}))?;
        let stop=Arc::new(AtomicBool::new(false));let reports=Arc::new(Mutex::new(VecDeque::new()));let stopped=stop.clone();let output=reports.clone();
        let join=std::thread::Builder::new().name("native-original-debug".into()).spawn(move||{
            let mut cursor=0;let mut lost=0;let mut parsers=BTreeMap::<(u64,bool),(String,crate::dotnet::ManagedDebugDecoder)>::new();
            while !stopped.load(Ordering::Acquire){
                let result=(||{
                    let batch=crate::shared_devices::execute(crate::managed_services::Operation::DeviceReports,scope,&json!({"cursor":cursor,"limit":32}))?;
                    cursor=batch["next_sequence"].as_u64().ok_or("Debug cursor missing")?;
                    let next_lost=batch["lost_reports"].as_u64().ok_or("Debug loss count missing")?;
                    if lost!=next_lost{for (_,parser)in parsers.values_mut(){parser.reset()?;}eprintln!("Native debug lost {} reports; parser histories reset",next_lost.saturating_sub(lost));lost=next_lost;}
                    for event in batch["events"].as_array().ok_or("Debug event array missing")?{
                        let key=(event["reader_generation"].as_u64().ok_or("Debug reader epoch missing")?,event["auxiliary"].as_bool().ok_or("Debug endpoint role missing")?);
                        let name=event["parser"].as_str().ok_or("Debug parser missing")?;
                        if parsers.get(&key).is_none_or(|(saved,_)|saved!=name){if parsers.len()>=128{return Err("Native debug retained parser limit reached".into());}parsers.insert(key,(name.to_owned(),crate::dotnet::ManagedDebugDecoder::new(name)?));}
                        let raw=decode(event["raw"].as_str().ok_or("Debug raw bytes missing")?)?;let parser=&mut parsers.get_mut(&key).unwrap().1;
                        match parser.decode(&raw){Ok(Some(report))=>{
                            let report=json!({"Tablet":event["tablet"],"Path":report.path,"Data":report.data});let size=serde_json::to_vec(&report).map_err(|e|e.to_string())?.len();
                            if size<=128*1024{let mut out=output.lock().map_err(|_|"Debug output poisoned")?;while out.len()>=16{out.pop_front();}out.push_back(report);}
                        },Ok(None)=>{},Err(error)=>{parser.reset()?;eprintln!("Native debug parser failed and reset: {error}");}}
                    }
                    let active=crate::shared_devices::owned_metadata();parsers.retain(|(epoch,_),_|active.iter().any(|e|e["reader_generation"].as_u64()==Some(*epoch)));Ok::<(),String>(())
                })();
                if let Err(error)=result{eprintln!("Native tablet debug failed: {error}");break;}
                std::thread::sleep(Duration::from_millis(50));
            }
            let _=crate::shared_devices::execute(crate::managed_services::Operation::DeviceReports,scope,&json!({"enabled":false}));
        }).map_err(|error|{let _=crate::shared_devices::execute(crate::managed_services::Operation::DeviceReports,scope,&json!({"enabled":false}));error.to_string()})?;
        Ok(Self{stop,reports,join:Some(join)})
    }
}
impl Drop for Debug{fn drop(&mut self){self.stop.store(true,Ordering::Release);if let Some(join)=self.join.take(){let _=join.join();}}}
fn decode(text:&str)->Result<Vec<u8>,String>{if text.len()%2!=0||text.len()>131070{return Err("Invalid debug raw report length".into());}
    let nibble=|b:u8|match b{b'0'..=b'9'=>Some(b-b'0'),b'a'..=b'f'=>Some(b-b'a'+10),b'A'..=b'F'=>Some(b-b'A'+10),_=>None};
    text.as_bytes().chunks_exact(2).map(|b|Ok((nibble(b[0]).ok_or("Invalid debug hex")?<<4)|nibble(b[1]).ok_or("Invalid debug hex")?)).collect()
}
