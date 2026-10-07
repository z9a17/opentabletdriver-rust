//! Native Unix multi-device owner. Control and discovery are cold paths;
//! every HID/OS handle is constructed, used and retired on its reader thread.
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::{Arc, mpsc::{self, Receiver, SyncSender}, atomic::{AtomicBool, Ordering}};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use serde::{Serialize, Deserialize};
use serde_json::{Value, json};
use otd_core::{config::Profile, endpoint_match::Endpoint, mapping::Rect, tablets::{DeviceIdentifier, TabletConfiguration}};
use crate::{control::WorkerIdentity, device_sessions::SessionState};

const MAX_DEVICES: usize = 64;
const TRANSACTION_WAIT: Duration = Duration::from_secs(20);

#[derive(Clone, Debug)]
pub struct Device {
    pub endpoint: Endpoint,
    pub configuration: TabletConfiguration,
    pub identifier: DeviceIdentifier,
    pub auxiliary: Option<(Endpoint, DeviceIdentifier)>,
    pub custom_endpoint:Option<u64>,pub auxiliary_custom_endpoint:Option<u64>,
}
impl Device {
    fn key(&self) -> String { format!("{}\0{}",self.endpoint.physical_id,self.configuration.name) }
    pub fn tablet_reference(&self) -> Value {
        let mut identifiers = vec![json!(self.identifier)];
        if let Some((_,identifier)) = &self.auxiliary { identifiers.push(json!(identifier)); }
        json!({"Properties":self.configuration,"Identifiers":identifiers})
    }
}
pub trait Platform: Send + Sync + 'static {
    fn default_profile(&self,_tablet:&str)->Result<Option<Profile>,String>{Ok(None)}
    fn startup_tools(&self)->Option<Vec<otd_core::plugins::PluginConfig>>{None}
    fn prepare_start(&self) -> Result<(),String> { Ok(()) }
    /// Must only discover here. No output, initialization or CLR construction.
    fn discover(&self) -> Result<Vec<Device>,String>;
    fn screen(&self) -> Result<Rect,String>;
    /// Prepare graph/resources, call context.activate BEFORE opening HID, then
    /// call context.running AFTER real input/output initialization succeeds.
    /// All exit paths must dispose output and report cleanup errors accurately.
    fn run(&self, device: Device, profile: Profile, context: WorkerContext) -> Result<(),String>;
    fn inventory(&self) -> Result<Value,String>;
    fn device_string(&self,vendor:u16,product:u16,index:u8)->Result<String,String>;
    /// Executed on the independent I/O lane. Implementations must route through
    /// the actual reader's bounded mailbox, never open a second input stream.
    fn service_io(&self, request: crate::managed_services::Request) -> Result<Value,String>;
}

enum Event { Prepared, Running, Ended(Result<(),String>) }
pub fn discover(database:&otd_core::tablets::Database,endpoints:&[Endpoint]) -> Result<Vec<Device>,String> {
    use otd_core::tablets::{Role,ParserSupport,parser_support};
    let mut devices=Vec::new();let mut seen=BTreeSet::new();
    for endpoint in endpoints {
        let Some(found)=database.find(endpoint.vendor_id,endpoint.product_id).find(|found|found.role==Role::Digitizer
            && otd_core::endpoint_match::matches(endpoint,found).is_ok()
            && (parser_support(found.identifier.parser())!=ParserSupport::Missing||crate::dotnet::installed_report_parser(found.identifier.parser()))) else{continue;};
        let key=(endpoint.physical_id.clone(),found.configuration.name.clone());
        if !seen.insert(key){continue;}
        otd_core::spec::TabletSpec::from_configuration(found.configuration)?;
        let auxiliary=endpoints.iter().filter(|aux|aux.path!=endpoint.path&&aux.physical_id==endpoint.physical_id)
            .find_map(|aux|database.find(aux.vendor_id,aux.product_id).find(|candidate|candidate.role==Role::Auxiliary
                && candidate.configuration.name==found.configuration.name&&otd_core::endpoint_match::matches(aux,candidate).is_ok())
                .map(|candidate|(aux.clone(),candidate.identifier.clone())));
        devices.push(Device{endpoint:endpoint.clone(),configuration:found.configuration.clone(),identifier:found.identifier.clone(),auxiliary,custom_endpoint:None,auxiliary_custom_endpoint:None});
    }
    Ok(devices)
}
pub fn prepare_connected(database:&otd_core::tablets::Database,endpoints:&[Endpoint]) -> Result<(),String> {
    let missing=endpoints.iter().any(|endpoint|database.find(endpoint.vendor_id,endpoint.product_id).any(|found|
        found.role==otd_core::tablets::Role::Digitizer&&otd_core::endpoint_match::matches(endpoint,&found).is_ok()
        &&otd_core::tablets::parser_support(found.identifier.parser())==otd_core::tablets::ParserSupport::Missing
        &&!crate::dotnet::installed_report_parser(found.identifier.parser())));
    if missing{crate::plugins::load_parser_registry()?;}Ok(())
}
pub fn inventory(endpoints:&[Endpoint]) -> Value {
    json!(endpoints.iter().map(|endpoint|json!({"DevicePath":endpoint.path,"VendorID":endpoint.vendor_id,"ProductID":endpoint.product_id,
        "CanOpen":endpoint.can_open,"InputReportLength":endpoint.input_length,"OutputReportLength":endpoint.output_length,
        "FeatureReportLength":endpoint.feature_length,"Manufacturer":Value::Null,"ProductName":Value::Null,
        "SerialNumber":Value::Null,"DeviceAttributes":endpoint.attributes})).collect::<Vec<_>>())
}
pub struct WorkerContext {
    pub id: String,
    pub generation: u64,
    pub reader_generation: u64,
    pub stop: Arc<AtomicBool>,
    events: SyncSender<Event>,
    activation: Receiver<()>,
    bindings:SyncSender<(u64,crate::binding_presets::Request)>,
}
impl WorkerContext {
    pub fn actions(&self,sink:Box<dyn otd_core::output::buttons::ActionSink>,inhibit:Option<u32>)->Box<dyn otd_core::output::buttons::ActionSink>{
        let tx=self.bindings.clone();let generation=self.reader_generation;
        crate::binding_presets::wrap(sink,Box::new(move|owner,name|tx.try_send((generation,crate::binding_presets::Request::new(owner,name)))
            .map_err(|_|std::io::Error::new(std::io::ErrorKind::WouldBlock,"Deferred preset queue is full or owner retired"))),inhibit)
    }
    pub fn activate(&self) -> Result<(),String> {
        self.events.send(Event::Prepared).map_err(|_| "Device transaction ended before preparation".to_owned())?;
        loop {
            if self.stop.load(Ordering::Acquire) { return Err("Candidate device activation cancelled".into()); }
            match self.activation.recv_timeout(Duration::from_millis(100)) {
                Ok(()) => return Ok(()),
                Err(mpsc::RecvTimeoutError::Timeout) => {},
                Err(_) => return Err("Candidate device activation owner disappeared".into()),
            }
        }
    }
    pub fn running(&self) -> Result<(),String> {
        self.events.send(Event::Running).map_err(|_| "Device transaction ended before activation".into())
    }
}
/// Running is committed after core construction has resolved every binding,
/// mapper and output. The callback itself is allocation-free and runs once.
pub struct LifecycleSource<S>{source:S,ready:Option<SyncSender<Event>>,started:AtomicBool}
impl<S> LifecycleSource<S>{pub fn new(source:S,context:Option<&WorkerContext>)->Self{Self{source,ready:context.map(|context|context.events.clone()),started:AtomicBool::new(false)}}}
impl<S> LifecycleSource<S>{pub fn source_mut(&mut self)->&mut S{&mut self.source}}
impl<S:otd_core::session::ReportSource> otd_core::session::ReportSource for LifecycleSource<S>{
    fn shared_output(&self)->bool{self.source.shared_output()}
    fn label(&self)->&str{self.source.label()}
    fn now(&self)->Instant{self.source.now()}
    fn next(&mut self,timeout:Duration)->std::io::Result<otd_core::session::Read<'_>>{self.source.next(timeout)}
    fn native_output_enabled(&self)->bool{self.source.native_output_enabled()}
    fn output_started(&self){self.source.output_started();if !self.started.swap(true,Ordering::AcqRel){if let Some(ready)=&self.ready{let _=ready.try_send(Event::Running);}}}
    fn output_acknowledged(&self,enabled:bool){self.source.output_acknowledged(enabled)}
}
struct Worker {
    stop: Arc<AtomicBool>, join: Option<JoinHandle<()>>, events: Receiver<Event>, activate: SyncSender<()>,
}
impl Worker {
    fn prepare(platform:Arc<dyn Platform>,device:Device,profile:Profile,id:String,generation:u64,reader_generation:u64,bindings:SyncSender<(u64,crate::binding_presets::Request)>) -> Result<Self,String> {
        let stop = Arc::new(AtomicBool::new(false));
        let (tx,events) = mpsc::sync_channel(4);
        let (activate,rx) = mpsc::sync_channel(1);
        let context = WorkerContext { id,generation,reader_generation,stop:stop.clone(),events:tx.clone(),activation:rx,bindings };
        let join = std::thread::Builder::new().name(format!("tablet-{}",context.id)).spawn(move || {
            let _source = crate::device_sessions::source_scope(&context.id,context.generation,context.reader_generation);
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| platform.run(device,profile,context)))
                .unwrap_or_else(|_| Err("Device worker panicked; output cleanup may be incomplete".into()));
            let _ = tx.send(Event::Ended(result));
        }).map_err(|error| error.to_string())?;
        let mut worker = Self { stop,join:Some(join),events,activate };
        if let Err(error) = worker.wait(false) { let _ = worker.retire(); return Err(error); }
        Ok(worker)
    }
    fn wait(&mut self,running:bool) -> Result<(),String> {
        match self.events.recv_timeout(TRANSACTION_WAIT) {
            Ok(Event::Prepared) if !running => Ok(()),
            Ok(Event::Running) if running => Ok(()),
            Ok(Event::Ended(Err(error))) => Err(error),
            Ok(Event::Ended(Ok(()))) => Err("Device disconnected before activation completed".into()),
            Ok(_) => Err("Unexpected device activation state".into()),
            Err(_) => Err("Device activation timed out; candidate is being cancelled".into()),
        }
    }
    fn start(&mut self) -> Result<(),String> { self.activate.send(()).map_err(|_| "Candidate worker ended")?; self.wait(true) }
    fn retire(&mut self) -> Result<(),String> {
        self.stop.store(true,Ordering::Release);
        // Reader timeout is bounded; joining is mandatory before replacement.
        // A stuck foreign plugin cannot safely be abandoned/replaced in-process.
        if let Some(join) = self.join.take() { join.join().map_err(|_| "Device worker panicked during cleanup")?; }
        let mut result = Ok(());
        while let Ok(event) = self.events.try_recv() { if let Event::Ended(value) = event { result = value; } }
        result
    }
}
impl Drop for Worker { fn drop(&mut self) { let _ = self.retire(); } }

#[derive(Clone,Debug,Serialize,Deserialize)]
pub struct Session {
    pub id:String, pub device_generation:u64, pub reader_generation:u64,
    pub tablet:String, pub connected:bool, pub state:SessionState,
    pub properties:TabletConfiguration, pub identifiers:Vec<DeviceIdentifier>,
    pub last_error:Option<String>,
}
struct Slot { device:Device, session:Session, profile:Profile, worker:Option<Worker>, explicitly_stopped:bool }
fn tool_configs(settings:&Value)->Result<Vec<otd_core::plugins::PluginConfig>,String>{
    let tools=settings["Tools"].as_array().cloned().unwrap_or_default();
    if !tools.iter().any(|store|store["Enable"]==true){return Ok(Vec::new());}
    let registry=match crate::dotnet::registry_snapshot(){Some(registry)=>registry,None=>{
        crate::plugins::load_parser_registry()?;crate::dotnet::registry_snapshot().ok_or("Installed managed registry unavailable")?
    }};
    let mut configs=Vec::new();
    for store in tools.iter().filter(|store|store["Enable"]==true){
        let name=store["Path"].as_str().ok_or("Enabled global tool Path is missing")?;
        let mut entries=registry.plugins.iter().filter(|entry|entry.metadata.category=="tool"&&entry.metadata.supported&&entry.config.type_name==name);
        let entry=entries.next().ok_or_else(||format!("Enabled global tool {name} is not installed"))?;
        if entries.any(|other|other.config.path!=entry.config.path){return Err(format!("Global tool {name} is ambiguous across installed assemblies"));}
        let mut config=entry.config.clone();config.enabled=true;config.settings_json=Profile::managed_store_settings(store)?;config.validate()?;configs.push(config);
    }Ok(configs)
}
#[derive(Clone,Debug,Serialize,Deserialize)]
pub enum Command {
    Status, Detect, Start, Stop, Shutdown,
    SuspendTools,StopReaders{shutdown:bool},
    ReserveUpdate{expected:Option<WorkerIdentity>},DrainUpdate{token:String},ReadyUpdate{token:String},CancelUpdate{token:String},ResumeUpdate{token:String},FinishUpdate{token:String},
    Select { expected:WorkerIdentity,id:String },
    GetProfile { id:String,generation:u64 },
    Apply { expected:WorkerIdentity,id:String,generation:u64,profile_toml:String,binding_inhibit:Option<u32> },
    StartDevice { expected:WorkerIdentity,id:String,generation:u64 },
    StopDevice { expected:WorkerIdentity,id:String,generation:u64 },
    Original { expected:Option<WorkerIdentity>,source:Option<(String,u64)>,binding_owner:Option<u32>,method:String,params:Value },
}
struct Completion {value:Value,tools:Option<crate::global_tools::Receipt>}
struct Call { command:Command, reply:SyncSender<Result<Completion,String>> }
#[derive(Clone)]
pub struct Handle { tx:SyncSender<Call>,cold:Arc<crate::cold_services::Services> }
impl Handle {
    pub fn call(&self,command:Command) -> Result<Value,String> {
        if matches!(&command,Command::Stop|Command::Shutdown){let shutdown=matches!(&command,Command::Shutdown);
            self.call(Command::SuspendTools)?;return self.call(Command::StopReaders{shutdown});}
        if let Command::Original{expected,source:_,binding_owner:_,method,params}=&command{
            if let Some(value)=self.cold.invoke(self,method,params,expected.clone())?{return Ok(value);}
        }
        let completion=self.submit(command)?;
        if let Some(receipt)=completion.tools{let _=receipt.wait(Duration::from_secs(60))?;}
        Ok(completion.value)
    }
    fn submit(&self,command:Command)->Result<Completion,String>{
        let(reply,rx)=mpsc::sync_channel(1);
        self.tx.try_send(Call{command,reply}).map_err(|_|"Native control queue is full or owner stopped".to_owned())?;
        rx.recv_timeout(Duration::from_secs(60)).map_err(|_|"Native control timeout; query state before retrying mutation".to_owned())?
    }
    pub(crate) fn reserve_update(&self,expected:Option<WorkerIdentity>)->Result<(String,Option<crate::global_tools::Receipt>),String>{
        let completion=self.submit(Command::ReserveUpdate{expected})?;
        Ok((completion.value.as_str().ok_or("Missing update reservation token")?.to_owned(),completion.tools))
    }

}
struct UpdateReservation{token:String,was_enabled:bool,tools:Vec<otd_core::plugins::PluginConfig>,ready:bool}
struct State {
    identity:WorkerIdentity, slots:BTreeMap<String,Slot>, next_id:u64,next_reader:u64,
    selected:Option<String>, enabled:bool, shutdown:bool,retiring:bool,update:Option<UpdateReservation>,next_update:u64,tools_configs:Vec<otd_core::plugins::PluginConfig>,
    settings:Value, logs:VecDeque<Value>, log_sequence:u64,resynchronize:u64,
    tools:crate::global_tools::Handle,pending_tools:Option<crate::global_tools::Receipt>,tools_configured:bool,
    bindings:SyncSender<(u64,crate::binding_presets::Request)>,
}
impl State {
    fn expected(&self,expected:&WorkerIdentity) -> Result<(),String> {
        if expected != &self.identity { Err("Stale daemon identity/generation; refresh before mutation".into()) } else { Ok(()) }
    }
    fn changed(&mut self) -> Result<(),String> {
        self.identity.generation = self.identity.generation.checked_add(1).ok_or("Daemon generation exhausted")?;
        self.resynchronize = self.resynchronize.checked_add(1).ok_or("Resynchronize generation exhausted")?;
        Ok(())
    }
    fn scan(&mut self,platform:&Arc<dyn Platform>) -> Result<(),String> {
        let devices = platform.discover()?;
        if devices.len() > MAX_DEVICES { return Err("Discovery exceeds 64 device owners".into()); }
        let mut keys = BTreeSet::new();
        for device in devices { keys.insert(device.key());
            if let Some(slot) = self.slots.values_mut().find(|slot| slot.device.key()==device.key()) {
                slot.device = device;
                if !slot.session.connected { slot.session.connected=true; slot.session.state=SessionState::Detected; }
                continue;
            }
            if self.slots.len() >= MAX_DEVICES { return Err("Retained device owner limit reached".into()); }
            self.next_id = self.next_id.checked_add(1).ok_or("Device ID exhausted")?;
            let id = format!("device-{}",self.next_id);
            let mut profile = match crate::plugins::load_original_tablet_profile(&device.configuration.name)? {
                Some(profile)=>profile,
                None=>import_profile(&crate::upstream_settings::defaults(std::slice::from_ref(&device.configuration),platform.screen()?)?,0)?,
            };
            if let Some(saved) = self.settings["Profiles"].as_array().and_then(|rows| rows.iter().position(|row| row["Tablet"]==device.configuration.name)) {
                profile = import_profile(&self.settings,saved)?;
            }
            if let Some(explicit)=platform.default_profile(&device.configuration.name)?{profile=explicit;}
            profile = profile.for_tablet(otd_core::spec::TabletSpec::from_configuration(&device.configuration)?)?;
            let session = Session { id:id.clone(),device_generation:1,reader_generation:0,
                tablet:device.configuration.name.clone(),connected:true,state:SessionState::Detected,
                properties:device.configuration.clone(),identifiers:vec![device.identifier.clone()],last_error:None };
            self.slots.insert(id.clone(),Slot { device,session,profile,worker:None,explicitly_stopped:false });
            if self.selected.is_none() { self.selected=Some(id); }
        }
        for slot in self.slots.values_mut() {
            if !keys.contains(&slot.device.key()) {
                slot.session.connected=false;
                if let Some(mut worker)=slot.worker.take() {
                    if let Err(error)=worker.retire() { slot.session.last_error=Some(error); slot.session.state=SessionState::Failed; continue; }
                }
                slot.session.state=SessionState::Waiting;
            } else if slot.worker.as_ref().is_some_and(|worker| worker.join.as_ref().is_some_and(JoinHandle::is_finished)) {
                let mut worker=slot.worker.take().unwrap();
                slot.session.last_error=worker.retire().err();
                slot.session.state=if slot.session.last_error.is_some() {SessionState::Failed} else {SessionState::Waiting};
            }
        }
        if self.enabled {
            let ids:Vec<_> = self.slots.iter().filter(|(_,slot)| slot.session.connected && slot.worker.is_none()
                && !slot.explicitly_stopped && slot.session.state!=SessionState::Failed).map(|(id,_)| id.clone()).collect();
            for id in ids { let profile=self.slots[&id].profile.clone(); let generation=self.slots[&id].session.device_generation;
                if let Err(error)=self.replace(platform,&id,generation,profile) {
                    let slot=self.slots.get_mut(&id).unwrap(); slot.session.last_error=Some(error);slot.session.state=SessionState::Failed;
                }
            }
        }
        Ok(())
    }
    fn replace(&mut self,platform:&Arc<dyn Platform>,id:&str,expected:u64,profile:Profile) -> Result<(),String> {
        if self.retiring{return Err("Daemon retirement owns the transaction; device replacement is blocked".into());}
        let slot=self.slots.get_mut(id).ok_or("Device session does not exist")?;
        if slot.session.device_generation!=expected { return Err("Stale device generation".into()); }
        let profile=profile.for_tablet(otd_core::spec::TabletSpec::from_configuration(&slot.device.configuration)?)?;
        if profile.tablet_name()?.is_some_and(|name| name!=slot.device.configuration.name) { return Err("Profile belongs to another tablet".into()); }
        crate::plugins::validate_runtime_profile(&profile)?;
        let _=otd_core::pipeline::ReportPipeline::new(&profile)?;
        let next=expected.checked_add(1).ok_or("Device generation exhausted")?;
        if !slot.session.connected || slot.explicitly_stopped || !self.enabled {
            slot.profile=profile;slot.session.device_generation=next;return self.changed();
        }
        self.next_reader=self.next_reader.checked_add(1).ok_or("Reader generation exhausted")?;
        let mut candidate=Worker::prepare(platform.clone(),slot.device.clone(),profile.clone(),id.into(),next,self.next_reader,self.bindings.clone())?;
        if let Some(mut old)=slot.worker.take() {
            if let Err(error)=old.retire() {
                let _=candidate.retire();slot.session.state=SessionState::Failed;slot.session.last_error=Some(error.clone());
                return Err(format!("Old worker cleanup failed; replacement blocked: {error}"));
            }
        }
        if let Err(error)=candidate.start() {
            let cleanup=candidate.retire();
            slot.session.state=SessionState::Failed;slot.session.last_error=Some(error.clone());
            if cleanup.is_err() { return Err(format!("Candidate activation failed: {error}; cleanup failed: {}",cleanup.unwrap_err())); }
            // Rollback retains the old authored profile and owns this generation.
            self.next_reader=self.next_reader.checked_add(1).ok_or("Reader generation exhausted")?;
            let rollback=Worker::prepare(platform.clone(),slot.device.clone(),slot.profile.clone(),id.into(),expected,self.next_reader,self.bindings.clone())
                .and_then(|mut worker| {worker.start()?;Ok(worker)});
            match rollback { Ok(worker)=>{slot.worker=Some(worker);slot.session.reader_generation=self.next_reader;slot.session.state=SessionState::Running;},
                Err(rollback)=>return Err(format!("Activation failed: {error}; rollback failed: {rollback}")) }
            return Err(format!("Activation failed and prior profile restored: {error}"));
        }
        slot.worker=Some(candidate);slot.profile=profile;slot.profile.binding_inhibit=None;
        slot.session.device_generation=next;slot.session.reader_generation=self.next_reader;
        slot.session.state=SessionState::Running;slot.session.last_error=None;
        self.changed()
    }
    fn stop_device(&mut self,id:&str,generation:u64) -> Result<(),String> {
        if self.retiring{return Err("Daemon retirement owns the transaction; individual stop is blocked".into());}
        let slot=self.slots.get_mut(id).ok_or("Device session does not exist")?;
        if slot.session.device_generation!=generation { return Err("Stale device generation".into()); }
        if let Some(mut worker)=slot.worker.take() { worker.retire()?; }
        slot.explicitly_stopped=true;slot.session.state=SessionState::Stopped;
        slot.session.device_generation=generation.checked_add(1).ok_or("Device generation exhausted")?;
        self.changed()
    }
    fn stop_readers(&mut self)->Result<(),String>{
        self.enabled=false;let mut errors=Vec::new();
        for slot in self.slots.values_mut(){if let Some(mut worker)=slot.worker.take(){if let Err(error)=worker.retire(){slot.session.last_error=Some(error.clone());errors.push(error);slot.session.state=SessionState::Failed;continue;}}slot.session.state=SessionState::Stopped;}
        if errors.is_empty(){Ok(())}else{Err(format!("Output cleanup failed: {}",errors.join("; ")))}
    }
    fn tablets(&self) -> Vec<Value> {
        let owned=crate::shared_devices::owned_metadata();
        self.slots.values().filter(|slot|slot.session.connected&&slot.session.state==SessionState::Running).filter_map(|slot|{
            let endpoints=owned.iter().filter(|endpoint|endpoint["session_id"]==slot.session.id&&endpoint["reader_generation"]==slot.session.reader_generation).collect::<Vec<_>>();
            if !endpoints.iter().any(|endpoint|endpoint["auxiliary"]==false){return None;}
            Some(json!({"Properties":slot.device.configuration,"Identifiers":endpoints.iter().map(|endpoint|endpoint["Identifier"].clone()).collect::<Vec<_>>()}))
        }).collect()
    }
    fn settings(&self,platform:&Arc<dyn Platform>) -> Result<Value,String> {
        let mut value=self.settings.clone();
        let rows=value["Profiles"].as_array_mut().ok_or("Stored settings Profiles are invalid")?;
        for slot in self.slots.values().filter(|slot|slot.session.connected) {
            let exported=crate::upstream_settings::canonical_copy(&slot.profile,&slot.device.configuration,platform.screen()?)?.to_otd_json()?;
            let exported:Value=serde_json::from_str(&exported).map_err(|error|error.to_string())?;
            let index=slot.profile.imported_otd.as_ref().map_or(0,|source|source.selected_profile);
            let row=exported["Profiles"][index].clone();
            if let Some(index)=rows.iter().position(|row|row["Tablet"]==slot.device.configuration.name) { rows[index]=row; } else { rows.push(row); }
        }
        Ok(value)
    }
    fn apply_settings(&mut self,platform:&Arc<dyn Platform>,settings:Value,source:Option<(String,u64)>,owner:Option<u32>) -> Result<(),String> {
        if self.retiring{return Err("Daemon retirement owns the transaction; settings replacement is blocked".into());}
        if self.tools.pending()?{return Err("Global tools are still changing; refresh before applying settings".into());}
        let profiles=settings["Profiles"].as_array().ok_or("Settings require Profiles array")?;
        if profiles.len()>512 { return Err("Settings exceed 512 profiles".into()); }
        let mut names=BTreeSet::new();
        for row in profiles { let name=row["Tablet"].as_str().filter(|name|!name.is_empty()).ok_or("Profile Tablet must be a nonempty string")?;
            if !names.insert(name) { return Err("Ambiguous duplicate tablet profiles".into()); }
        }
        if let Some((id,generation))=&source {
            let slot=self.slots.get(id).ok_or("Preset source session disappeared")?;
            if slot.session.device_generation!=*generation||slot.session.state!=SessionState::Running { return Err("Preset source worker is stale".into()); }
        }
        let mut work=Vec::new();
        for (id,slot) in &self.slots {
            if let Some(index)=profiles.iter().position(|row|row["Tablet"]==slot.device.configuration.name) {
                let mut profile=import_profile(&settings,index)?;
                if source.as_ref().is_some_and(|(source,_)|source==id) { profile.binding_inhibit=owner; }
                work.push((id.clone(),slot.session.device_generation,profile,slot.profile.clone()));
            }
        }
        let tools=tool_configs(&settings)?;
        let mut applied:Vec<(String,u64,Profile)>=Vec::new();
        for (id,generation,profile,old) in work {
            if let Err(error)=self.replace(platform,&id,generation,profile) {
                let mut failures=Vec::new();
                for (id,generation,old) in applied.into_iter().rev() { if let Err(error)=self.replace(platform,&id,generation,old) { failures.push(format!("{id}: {error}")); } }
                return Err(format!("Settings apply failed: {error}; rollback failures: {}",failures.join("; ")));
            }
            let accepted=self.slots[&id].session.device_generation;
            applied.push((id,accepted,old));
        }
        // Pinned SetToolSettings disposes old tools before constructing the new
        // collection; constructors cannot overlap duplicated global timers.
        let generation=self.tools.snapshot()?.generation;
        self.pending_tools=Some(self.tools.set(generation,tools.clone())?);self.tools_configs=tools;self.tools_configured=true;
        self.settings=settings;self.changed()
    }
    fn update_token(&self,token:&str)->Result<(),String>{if self.update.as_ref().is_some_and(|update|update.token==token){Ok(())}else{Err("Update reservation is stale or absent".into())}}
    fn command(&mut self,platform:&Arc<dyn Platform>,command:Command) -> Result<Value,String> {
        if self.retiring&&matches!(&command,Command::Start|Command::StartDevice{..}|Command::Select{..}){return Err("Daemon retirement owns the transaction; start/selection is blocked".into());}
        if self.update.is_some()&&matches!(&command,Command::SuspendTools|Command::Stop|Command::Shutdown|Command::StopReaders{..}){return Err("Update reservation owns daemon lifetime".into());}
        if self.tools.pending()?&&matches!(&command,Command::Start|Command::Stop|Command::Shutdown){return Err("Global tools are still changing; refresh before changing daemon lifetime".into());}
        match command {
            Command::Status=>Ok(json!({"identity":self.identity,"sessions":self.slots.values().map(|slot|&slot.session).collect::<Vec<_>>(),"selected_id":self.selected,"enabled":self.enabled,"update":self.update.as_ref().map(|update|json!({"token":update.token,"ready":update.ready}))})),
            Command::Detect=>{if self.retiring{return Err("Daemon retirement reserves discovery".into());}self.scan(platform)?;Ok(json!(self.tablets()))},
            Command::Start=>{platform.prepare_start()?;self.enabled=true;if !self.tools_configured{let generation=self.tools.snapshot()?.generation;let configs=match platform.startup_tools(){Some(configs)=>configs,None=>tool_configs(&self.settings)?};self.pending_tools=Some(self.tools.set(generation,configs.clone())?);self.tools_configs=configs;self.tools_configured=true;}for slot in self.slots.values_mut(){slot.explicitly_stopped=false;if slot.session.state==SessionState::Failed{slot.session.state=SessionState::Detected;}}self.scan(platform)?;self.changed()?;Ok(Value::Null)},
            Command::SuspendTools=>{if self.tools.pending()?{return Err("Global tools are still changing; refresh before stopping".into());}self.retiring=true;self.enabled=false;
                if self.tools_configured{let generation=self.tools.snapshot()?.generation;self.pending_tools=Some(self.tools.drain(generation)?);self.tools_configured=false;}self.changed()?;Ok(Value::Null)},
            Command::Stop|Command::Shutdown|Command::StopReaders{..}=>{
                let shutdown=matches!(&command,Command::Shutdown|Command::StopReaders{shutdown:true});
                if self.tools_configured||self.tools.pending()?{return Err("Global tools have not completed disposal".into());}
                self.stop_readers()?;
                self.retiring=false;if shutdown{self.shutdown=true;}self.changed()?;Ok(Value::Null)
            },
            Command::ReserveUpdate{expected}=>{
                if let Some(expected)=expected{self.expected(&expected)?;}
                if self.retiring||self.update.is_some()||self.tools.pending()?{return Err("Daemon/tools already reserved or changing".into());}
                let next=self.next_update.checked_add(1).ok_or("Update token exhausted")?;
                let generation=self.tools.snapshot()?.generation;let receipt=self.tools.drain(generation)?;self.next_update=next;
                let token=format!("{}-update-{}",self.identity.instance,self.next_update);
                self.update=Some(UpdateReservation{token:token.clone(),was_enabled:self.enabled,tools:self.tools_configs.clone(),ready:false});
                self.enabled=false;self.retiring=true;self.tools_configured=false;self.pending_tools=Some(receipt);self.changed()?;Ok(json!(token))
            },
            Command::DrainUpdate{token}=>{self.update_token(&token)?;
                if self.tools.pending()?||!self.tools.snapshot()?.failures.is_empty(){return Err("Global tool disposal has not completed cleanly".into());}
                self.stop_readers()?;self.changed()?;Ok(Value::Null)
            },
            Command::ReadyUpdate{token}=>{self.update_token(&token)?;if self.slots.values().any(|slot|slot.worker.is_some())||self.tools.pending()?{return Err("Update owners are not drained".into());}
                self.update.as_mut().unwrap().ready=true;self.changed()?;Ok(Value::Null)},
            Command::CancelUpdate{token}=>{self.update_token(&token)?;if self.tools.pending()?{return Err("Update cleanup is still pending".into());}
                let generation=self.tools.snapshot()?.generation;let configs=self.update.as_ref().unwrap().tools.clone();
                self.pending_tools=Some(self.tools.set(generation,configs)?);self.tools_configured=true;self.changed()?;Ok(Value::Null)},
            Command::ResumeUpdate{token}=>{self.update_token(&token)?;if self.tools.pending()?{return Err("Update rollback tools are still changing".into());}
                let update=self.update.take().unwrap();self.retiring=false;self.enabled=update.was_enabled;self.scan(platform)?;self.changed()?;Ok(Value::Null)},
            Command::FinishUpdate{token}=>{self.update_token(&token)?;if !self.update.as_ref().unwrap().ready{return Err("Update reservation is not ready".into());}self.shutdown=true;self.changed()?;Ok(Value::Null)},
            Command::Select{expected,id}=>{self.expected(&expected)?;if !self.slots.contains_key(&id){return Err("Unknown device session".into());}self.selected=Some(id);self.changed()?;Ok(Value::Null)},
            Command::GetProfile{id,generation}=>{let slot=self.slots.get(&id).ok_or("Unknown device session")?;if slot.session.device_generation!=generation{return Err("Stale device generation".into());}Ok(json!(slot.profile.to_toml()?))},
            Command::Apply{expected,id,generation,profile_toml,binding_inhibit}=>{self.expected(&expected)?;
                let mut profile=Profile::from_toml_text(&profile_toml,std::path::Path::new("native-control-profile.toml"))?;profile.binding_inhibit=binding_inhibit;
                self.replace(platform,&id,generation,profile)?;Ok(json!({"id":id,"device_generation":self.slots[&id].session.device_generation,"accepted_pending":false}))},
            Command::StartDevice{expected,id,generation}=>{self.expected(&expected)?;let slot=self.slots.get_mut(&id).ok_or("Unknown device session")?;if slot.session.device_generation!=generation{return Err("Stale device generation".into());}slot.explicitly_stopped=false;slot.session.state=SessionState::Detected;let profile=slot.profile.clone();self.enabled=true;self.replace(platform,&id,generation,profile)?;Ok(Value::Null)},
            Command::StopDevice{expected,id,generation}=>{self.expected(&expected)?;self.stop_device(&id,generation)?;Ok(Value::Null)},
            Command::Original{expected,source,binding_owner,method,params}=>{
                if let Some(expected)=expected{self.expected(&expected)?;}
                self.original(platform,&method,params,source,binding_owner)
            },
        }
    }
    fn original(&mut self,platform:&Arc<dyn Platform>,method:&str,params:Value,source:Option<(String,u64)>,binding_owner:Option<u32>) -> Result<Value,String> {
        let first=||params.as_array().and_then(|values|values.first()).ok_or("Method requires a parameter".to_owned());
        match method {
            "GetDevices"=>platform.inventory(),"GetTablets"=>Ok(json!(self.tablets())),"DetectTablets"=>{if self.retiring{return Err("Daemon retirement reserves discovery".into());}self.scan(platform)?;Ok(json!(self.tablets()))},
            "GetSettings"=>self.settings(platform),
            "SetSettings"=>{let settings=first()?.clone();self.apply_settings(platform,settings,source,binding_owner)?;Ok(Value::Null)},
            "ResetSettings"=>{let tablets:Vec<_>=self.slots.values().filter(|slot|slot.session.connected).map(|slot|slot.device.configuration.clone()).collect();let settings=crate::upstream_settings::defaults(&tablets,platform.screen()?)?;self.apply_settings(platform,settings,source,None)?;Ok(Value::Null)},
            "GetApplicationInfo"=>crate::upstream_rpc::original_application_info(),
            "GetCurrentLog"=>Ok(json!(self.logs)),
            "WriteMessage"=>{let message=first()?.clone();if serde_json::to_vec(&message).map_err(|error|error.to_string())?.len()>32768{return Err("Log record exceeds 32 KiB".into());}self.log_sequence=self.log_sequence.checked_add(1).ok_or("Log sequence exhausted")?;self.logs.push_back(message);while self.logs.len()>64{self.logs.pop_front();}Ok(Value::Null)},
            "ForceResynchronize"=>{self.resynchronize=self.resynchronize.checked_add(1).ok_or("Resynchronize generation exhausted")?;Ok(Value::Null)},
            "LoadPlugins"=>{crate::plugins::load_parser_registry()?;Ok(Value::Null)},
            "InstallPlugin"=>{let path=first()?.as_str().ok_or("filePath must be a string")?;crate::plugin_catalog::install_file(std::path::Path::new(path))?;Ok(json!(true))},
            "UninstallPlugin"=>{let name=first()?.as_str().ok_or("friendlyName/directoryPath must be a string")?;
                let installed=crate::plugin_catalog::installed()?.into_iter().filter(|(path,metadata)|path.to_string_lossy()==name||metadata.name==name).collect::<Vec<_>>();
                if installed.len()!=1{return Err("Plugin must uniquely identify an installed name or directory".into());}
                crate::plugin_catalog::uninstall(&installed[0].0)?;Ok(json!(true))},
            "DownloadPlugin"=>{let metadata:crate::plugin_catalog::PluginMetadata=serde_json::from_value(first()?.clone()).map_err(|error|error.to_string())?;
                if !metadata.supports_driver(){return Err("Plugin does not support pinned OpenTabletDriver 0.6.7".into());}
                crate::plugin_catalog::install(&metadata)?;Ok(json!(true))},
            "GetDiagnosticInfo"=>Ok(json!({"App Version":format!("OpenTabletDriver Rust v{}",env!("OTD_RELEASE_VERSION")),"Build Date":env!("OTD_BUILD_DATE"),
                "Operating System":crate::cold_services::operating_system()?,"Environment Variables":std::env::vars().collect::<BTreeMap<_,_>>(),
                "HID Devices":platform.inventory()?,"Console Log":self.logs})),
            _=>Err(format!("Unknown original daemon method {method}")),
        }
    }
    fn snapshot(&self,platform:&Arc<dyn Platform>,version:u64) -> crate::managed_services::Snapshot {
        crate::managed_services::Snapshot { diagnostic_app_version:Some(env!("OTD_RELEASE_VERSION").into()),diagnostic_build_date:Some(env!("OTD_BUILD_DATE").into()),version,daemon_identity:Some(self.identity.clone()),
            source_sessions:self.slots.values().map(|slot|crate::managed_services::SourceSession {id:slot.session.id.clone(),tablet:slot.session.tablet.clone(),device_generation:slot.session.device_generation,pending_generation:None,connected:slot.session.connected,state:slot.session.state}).collect(),
            settings:self.settings(platform).ok(),application_info:crate::upstream_rpc::original_application_info().ok(),
            devices:platform.inventory().ok(),tablets:Some(json!(self.tablets())),
            configurations:otd_core::config::configured_tablets().ok().and_then(|db|serde_json::to_value(db.entries().iter().filter_map(|entry|entry.usable()).collect::<Vec<_>>()).ok()),
            logs:Some(json!(self.logs)),log_sequence:self.log_sequence,resynchronize:self.resynchronize,
        }
    }
}
fn import_profile(settings:&Value,index:usize) -> Result<Profile,String> {
    let text=settings.to_string();
    let rows=settings["Profiles"].as_array().ok_or("Profiles array missing")?;
    let name=rows[index]["Tablet"].as_str().ok_or("Tablet name missing")?;
    // Use the verified installed-store resolver with EXACT selected row.
    let mut selected=settings.clone();selected["Profiles"]=json!([rows[index].clone()]);
    let profile=crate::plugins::import_otd_with_installed(&selected.to_string(),std::path::Path::new("upstream-rpc-settings.json"),&[name.to_owned()])?;
    let mut profile=profile; if let Some(source)=profile.imported_otd.as_mut(){source.settings_json=text;source.selected_profile=index;}
    Ok(profile)
}
struct Services { handle:Handle,platform:Arc<dyn Platform> }
impl crate::managed_services::Backend for Services {
    fn maintain(&self,lane:usize){if lane==4{crate::input_owner::maintain();}}
    fn shutdown(&self,lane:usize){if lane==4{crate::input_owner::shutdown();}}
    fn execute(&self,request:crate::managed_services::Request) -> Result<Value,String> {
        use crate::managed_services::Operation;
        match request.operation {
            Operation::Daemon=>{
                let method=request.payload["method"].as_str().ok_or("Managed request method is missing")?.to_owned();
                self.handle.cold.allow_plugin_call(&method)?;
                if let Some(value)=crate::plugin_manager::invoke(&method,&request.payload.get("params").cloned().unwrap_or(json!([])),Some(&self.handle.cold.stopped))?{return Ok(value);}
                let binding_owner=request.payload.get("source_binding_owner").map(|value|value.as_u64().and_then(|value|u32::try_from(value).ok()).ok_or("Invalid source binding owner")).transpose()?;
                self.handle.call(Command::Original {expected:request.expected_daemon,source:request.expected_source,binding_owner,method,params:request.payload.get("params").cloned().unwrap_or(json!([]))})
            },
            // Constructor Detect must not enqueue behind the transaction that
            // is waiting for that constructor. Only actually opened registrations
            // establish a detected output owner here; discovery never opens one.
            Operation::Detect=>Ok(json!(!crate::shared_devices::owned_metadata().is_empty())),
            Operation::Snapshot=>Err("Snapshot is served by its real native cache".into()),
            Operation::DeviceString|Operation::OpenStream|Operation::ReadStream|Operation::WriteStream|Operation::GetFeature|Operation::SetFeature|Operation::CloseStream|Operation::DeviceReports|Operation::OutputOwner|Operation::InputHold|Operation::InputRelease=>self.platform.service_io(request),
        }
    }
}
pub struct Owner { pub handle:Handle, stop:Arc<AtomicBool>, join:Option<JoinHandle<Result<(),String>>>,host:Option<crate::managed_services::Host> }
impl Owner {
    pub fn start(platform:Arc<dyn Platform>,stop:Arc<AtomicBool>) -> Result<Self,String> {
        let instance=format!("unix-{}-{}",std::process::id(),SystemTime::now().duration_since(UNIX_EPOCH).map_err(|error|error.to_string())?.as_nanos());
        let settings=match otd_core::config::otd_settings_path(){Some(path)if path.is_file()=>{
            let text=std::fs::read_to_string(path).map_err(|error|error.to_string())?;
            let document:Value=serde_json::from_str(&text).map_err(|error|error.to_string())?;
            if !document["Profiles"].is_array(){return Err("Original settings require Profiles array".into());}document
        },_=>crate::upstream_settings::collection::empty()};
        let mut tools=crate::global_tools::Owner::start(|line|eprintln!("{line}"))?;
        let(bindings,binding_rx)=mpsc::sync_channel(64);
        let state=State { identity:WorkerIdentity {instance,generation:1},slots:BTreeMap::new(),next_id:0,next_reader:0,selected:None,enabled:false,shutdown:false,retiring:false,update:None,next_update:0,tools_configs:Vec::new(),
            settings,logs:VecDeque::new(),log_sequence:0,resynchronize:0,tools:tools.handle.clone(),pending_tools:None,tools_configured:false,bindings };
        let (tx,rx)=mpsc::sync_channel::<Call>(64);let handle=Handle {tx,cold:Arc::new(crate::cold_services::Services::new(platform.clone(),stop.clone()))};
        let services=Arc::new(Services {handle:handle.clone(),platform:platform.clone()});
        let snapshot=state.snapshot(&platform,1);
        let host=crate::managed_services::Host::start(snapshot.clone(),services)?;
        crate::managed_host::install(host.publisher(),snapshot)?;
        let owner_stop=stop.clone();
        let join=std::thread::Builder::new().name("unix-native-owner".into()).spawn(move || {
            let mut state=state;let mut next_scan=Instant::now();let mut version=1u64;
            let mut discovery_error=None;
            while !owner_stop.load(Ordering::Acquire)&&!state.shutdown {
                for _ in 0..16{let Ok((reader,request))=binding_rx.try_recv()else{break;};
                    let source=state.slots.iter().find(|(_,slot)|slot.session.reader_generation==reader&&slot.session.state==SessionState::Running)
                        .map(|(id,slot)|(id.clone(),slot.session.device_generation));
                    if let Some((id,generation))=source{
                        let result=crate::binding_presets::load(request).and_then(|profile|state.replace(&platform,&id,generation,profile));
                        if let Err(error)=result{eprintln!("Preset binding failed: {error}");if let Some(slot)=state.slots.get_mut(&id){slot.session.last_error=Some(error);}}
                    }
                }
                if !state.retiring&&Instant::now()>=next_scan {
                    discovery_error=state.scan(&platform).err();next_scan=Instant::now()+Duration::from_secs(2);
                    version=version.checked_add(1).ok_or("Snapshot generation exhausted")?;
                    crate::managed_host::publish(state.snapshot(&platform,version))?;
                }
                if let Ok(call)=rx.recv_timeout(Duration::from_millis(100)) {
                    let result=state.command(&platform,call.command);
                    version=version.checked_add(1).ok_or("Snapshot generation exhausted")?;
                    let published=crate::managed_host::publish(state.snapshot(&platform,version));
                    let result=result.and_then(|value|published.map(|_|Completion{value,tools:state.pending_tools.take()}));let _=call.reply.send(result);
                }
            }
            // Dispose global tools first, keeping actual readers, service lanes
            // and read-only daemon callbacks live until their teardown joins.
            state.retiring=true;state.enabled=false;
            let tool_retirement=match tools.retire(){Ok(Some(join))=>{while !join.is_finished(){
                if let Ok(call)=rx.recv_timeout(Duration::from_millis(50)){
                    let result=state.command(&platform,call.command);let _=call.reply.send(result.map(|value|Completion{value,tools:None}));
                }
            }join.join().map_err(|_|"Global tool owner panicked during disposal".to_owned())},Ok(None)=>Ok(()),Err(error)=>Err(error)};
            state.tools_configured=false;
            // Mandatory physical cleanup proceeds even if a failed tool worker
            // left its generation reservation pending. The error is retained.
            let result=state.stop_readers();
            let retired=crate::dotnet::drain_managed_retirements(Duration::from_secs(15));
            // No report owner remains when the input lane performs final retry.
            drop(rx);
            owner_stop.store(true,Ordering::Release);
            tool_retirement?;result?;retired?;if let Some(error)=discovery_error{eprintln!("Last discovery error: {error}");}Ok(())
        }).map_err(|error|error.to_string())?;
        Ok(Self {handle,stop,join:Some(join),host:Some(host)})
    }
    /// Join tablet/tool retirement while callback I/O lanes still exist. The
    /// caller can then stop original RPC/Instance before dropping this owner.
    pub fn drain(&mut self)->Result<(),String>{self.join.take().ok_or("Owner already joined")?.join().map_err(|_|"Native owner panicked")?}
    pub fn join(mut self) -> Result<(),String> { self.drain() }
}
impl Drop for Owner {fn drop(&mut self){self.stop.store(true,Ordering::Release);if let Some(join)=self.join.take(){let _=join.join();}drop(self.host.take());crate::managed_host::clear();}}
