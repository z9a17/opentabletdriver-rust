//! Installed managed registry and concrete report serialization. These APIs
//! belong to setup/control/background threads, never the native report tap.
use super::{InspectedFilter, bridge, inspect_metadata_bytes, last_error};
use std::ffi::c_void;
use std::marker::PhantomData;
use std::path::Path;
use std::rc::Rc;
use std::sync::{Arc, OnceLock, RwLock};

type HasParser = unsafe extern "C" fn(*const u8, i32) -> i32;
type Reload = unsafe extern "C" fn(*const u8, i32, *mut u8, i32) -> i32;
type CreateParser = unsafe extern "C" fn(*const u8, i32) -> *mut c_void;
type DecodeParser = unsafe extern "C" fn(*mut c_void, *const u8, u32, *mut u8, i32) -> i32;
type ResetParser = unsafe extern "C" fn(*mut c_void) -> i32;
type DestroyParser = unsafe extern "C" fn(*mut c_void);
type CreateRetainedTool = unsafe extern "C" fn(*const u8,usize,*mut u64)->*mut c_void;
type DestroyRetainedTool = unsafe extern "C" fn(*mut c_void)->u64;
type WaitRetirement = unsafe extern "C" fn(u64,u32)->i32;
type ReleaseRetirement = unsafe extern "C" fn(u64)->i32;
type PluginTypes = unsafe extern "C" fn(*mut u8,i32,i32)->i32;
type StartRpc = unsafe extern "C" fn(*const u8,u32)->isize;
type StopRpc = unsafe extern "C" fn(isize)->i32;
pub(super) struct Api { has_parser: HasParser, reload: Reload, mutate: Reload, create: CreateParser, decode: DecodeParser, reset: ResetParser, destroy: DestroyParser,
    store:Reload, types:PluginTypes, start_rpc:StartRpc, stop_rpc:StopRpc,
    create_tool:CreateRetainedTool,destroy_tool:DestroyRetainedTool,wait_retirement:WaitRetirement,release_retirement:ReleaseRetirement }
impl Api {
    pub(super) fn load(entry: &impl Fn(&str) -> Result<*mut c_void, String>) -> Result<Self, String> {
        Ok(unsafe { Self { has_parser: std::mem::transmute::<*mut c_void, HasParser>(entry("HasReportParser")?), reload: std::mem::transmute::<*mut c_void, Reload>(entry("ReloadRegistry")?),
            mutate: std::mem::transmute::<*mut c_void, Reload>(entry("MutateHostedRegistry")?),
            create: std::mem::transmute::<*mut c_void, CreateParser>(entry("CreateDebugParser")?),
            decode: std::mem::transmute::<*mut c_void, DecodeParser>(entry("DecodeDebugParser")?),
            reset: std::mem::transmute::<*mut c_void, ResetParser>(entry("ResetDebugParser")?),
            destroy: std::mem::transmute::<*mut c_void, DestroyParser>(entry("DestroyDebugParser")?),
            store:std::mem::transmute::<*mut c_void,Reload>(entry("ConstructPluginStore")?),
            types:std::mem::transmute::<*mut c_void,PluginTypes>(entry("GetPluginTypes")?),
            start_rpc:std::mem::transmute::<*mut c_void,StartRpc>(entry("StartHostedRpc")?),
            stop_rpc:std::mem::transmute::<*mut c_void,StopRpc>(entry("StopHostedRpc")?),
            create_tool:std::mem::transmute::<*mut c_void,CreateRetainedTool>(entry("CreateToolRetained")?),
            destroy_tool:std::mem::transmute::<*mut c_void,DestroyRetainedTool>(entry("DestroyToolRetained")?),
            wait_retirement:std::mem::transmute::<*mut c_void,WaitRetirement>(entry("WaitManagedRetirement")?),
            release_retirement:std::mem::transmute::<*mut c_void,ReleaseRetirement>(entry("ReleaseManagedRetirement")?) } })
    }
}
fn api() -> Result<&'static Api, String> { bridge()?.registry.as_ref().ok_or_else(|| "The installed .NET bridge lacks registry/concrete debug parser support; replace data/compat with this release's files.".into()) }
thread_local! {static RETIREMENTS:std::cell::RefCell<Vec<u64>>=const {std::cell::RefCell::new(Vec::new())};static RETIREMENT_ERRORS:std::cell::RefCell<Vec<String>>=const {std::cell::RefCell::new(Vec::new())};}
pub(super) fn create_retained_tool(settings:&str)->Result<*mut c_void,String> {
    let mut receipt=0;let handle=unsafe{(api()?.create_tool)(settings.as_ptr(),settings.len(),&mut receipt)};
    if receipt!=0 {RETIREMENTS.with(|pending|pending.borrow_mut().push(receipt));}
    if handle.is_null(){Err(last_error())}else{Ok(handle)}
}
pub(super) fn destroy_retained_tool(handle:*mut c_void) {
    match api() {
        Ok(api)=>{let receipt=unsafe{(api.destroy_tool)(handle)};
            if receipt!=0 {RETIREMENTS.with(|pending|pending.borrow_mut().push(receipt));}
            else {RETIREMENT_ERRORS.with(|errors|errors.borrow_mut().push(last_error()));}},
        Err(error)=>RETIREMENT_ERRORS.with(|errors|errors.borrow_mut().push(error)),
    }
}
/// Wait only on the cold thread which created/dropped the tool collection.
/// Pending receipts remain retained on timeout, permitting a later drain retry.
pub fn drain_managed_retirements(timeout:std::time::Duration)->Result<(),String> {
    let mut receipts=RETIREMENTS.with(|pending|std::mem::take(&mut *pending.borrow_mut()));
    let mut errors=RETIREMENT_ERRORS.with(|pending|std::mem::take(&mut *pending.borrow_mut()));
    if receipts.is_empty(){return if errors.is_empty(){Ok(())}else{Err(errors.join("; "))};}
    let api=api()?;let deadline=std::time::Instant::now()+timeout;
    let mut retained=Vec::new();
    for receipt in receipts.drain(..) {
        let left=deadline.saturating_duration_since(std::time::Instant::now());
        let millis=left.as_millis().min(60000) as u32;
        match unsafe{(api.wait_retirement)(receipt,millis)} {
            0=>{if unsafe{(api.release_retirement)(receipt)}!=0 {errors.push(last_error());retained.push(receipt);}},
            1=>retained.push(receipt),
            _=>{errors.push(last_error());if unsafe{(api.release_retirement)(receipt)}!=0 {retained.push(receipt);}},
        }
    }
    if !retained.is_empty(){errors.push("Owned managed retirements remain pending; no successful drain receipt was published".into());}
    RETIREMENTS.with(|pending|pending.borrow_mut().extend(retained));
    if errors.is_empty(){Ok(())}else{Err(errors.join("; "))}
}
fn copy_result(size:i32,copy:impl FnOnce(*mut u8,i32)->i32)->Result<serde_json::Value,String> {
    if size<0 {return Err(last_error());}
    if size==0 || size>4194304 {return Err("Invalid original metadata result size".into());}
    let mut bytes=vec![0;size as usize];let written=copy(bytes.as_mut_ptr(),size);
    if written!=size {return Err(if written<0 {last_error()}else{"Original metadata changed during capacity retry".into()});}
    serde_json::from_slice(&bytes).map_err(|error|error.to_string())
}
pub fn get_plugin_types()->Result<serde_json::Value,String> {
    let api=api()?;let size=unsafe{(api.types)(std::ptr::null_mut(),0,1)};
    copy_result(size,|out,cap|unsafe{(api.types)(out,cap,0)})
}
pub fn construct_plugin_store(path:&str,category:&str)->Result<serde_json::Value,String> {
    let json=serde_json::json!({"path":path,"category":category}).to_string();
    if json.len()>32768 {return Err("Plugin store request exceeds 32 KiB".into());}
    let api=api()?;let size=unsafe{(api.store)(json.as_ptr(),json.len() as i32,std::ptr::null_mut(),0)};
    copy_result(size,|out,cap|unsafe{(api.store)(std::ptr::null(),0,out,cap)})
}
/// The original server drains every client before its native owner retires.
pub struct HostedRpc { token:isize }
impl HostedRpc {
    pub fn start(pipe:&str)->Result<Self,String> {
        if pipe.is_empty() || pipe.len()>512 || pipe.contains('\0') {return Err("Invalid original RPC pipe name".into());}
        let token=unsafe{(api()?.start_rpc)(pipe.as_ptr(),pipe.len() as u32)};
        if token==0 {Err(last_error())}else{Ok(Self{token})}
    }
}
impl Drop for HostedRpc {fn drop(&mut self){if let Ok(api)=api(){if unsafe{(api.stop_rpc)(self.token)}<0 {eprintln!("Original RPC shutdown: {}",last_error());}}}}

#[derive(Clone, Debug)]
pub struct ManagedRegistryInfo {
    pub generation: u64,
    pub assemblies: usize,
    /// Complete constructor metadata for actual loaded installed classes.
    pub plugins: Vec<InspectedFilter>,
    pub skipped_native: Vec<String>,
}
static SNAPSHOT: OnceLock<RwLock<Option<Arc<ManagedRegistryInfo>>>> = OnceLock::new();
/// Pure cached metadata lookup: native profiles do not initialize CoreCLR.
pub fn registry_snapshot() -> Option<Arc<ManagedRegistryInfo>> { SNAPSHOT.get()?.read().ok()?.clone() }
pub fn known_report_parser(name: &str) -> Result<bool, String> {
    if name.is_empty() || name.len() > 4096 { return Ok(false); }
    match unsafe { (api()?.has_parser)(name.as_ptr(), name.len() as i32) } { 0 => Ok(false), 1 => Ok(true), _ => Err(last_error()) }
}
pub fn reload_installed_plugins(root: &Path) -> Result<ManagedRegistryInfo, String> {
    let root = std::path::absolute(root).map_err(|error| error.to_string())?;
    let root = root.to_str().ok_or("Installed plugin directory is not Unicode")?;
    if root.len() > 32768 { return Err("Installed plugin directory exceeds registry limit".into()); }
    let api = api()?;
    let size = unsafe { (api.reload)(root.as_ptr(), root.len() as i32, std::ptr::null_mut(), 0) };
    if size < 0 { return Err(last_error()); }
    if size == 0 || size > 1048576 { return Err("Invalid managed registry result size".into()); }
    let mut bytes = vec![0; size as usize];
    let copied = unsafe { (api.reload)(std::ptr::null(), 0, bytes.as_mut_ptr(), size) };
    if copied != size { return Err(if copied < 0 { last_error() } else { "Managed registry result changed during capacity retry".into() }); }
    publish_registry_bytes(&bytes)
}
/// Unload changes discovery only; running instances retain their assembly lease.
/// Cached reads do not undo exclusions. Explicit LoadPlugins starts a fresh registry.
pub fn mutate_installed_plugins(request: &serde_json::Value) -> Result<ManagedRegistryInfo, String> {
    let request = serde_json::to_vec(request).map_err(|error| error.to_string())?;
    if request.is_empty() || request.len() > 32768 { return Err("Registry mutation exceeds 32 KiB".into()); }
    let api = api()?;
    let size = unsafe { (api.mutate)(request.as_ptr(), request.len() as i32, std::ptr::null_mut(), 0) };
    if size < 0 { return Err(last_error()); }
    if size == 0 || size > 1048576 { return Err("Invalid managed registry mutation size".into()); }
    let mut bytes = vec![0; size as usize];
    let copied = unsafe { (api.mutate)(std::ptr::null(), 0, bytes.as_mut_ptr(), size) };
    if copied != size { return Err(if copied < 0 { last_error() } else { "Registry mutation result changed during capacity retry".into() }); }
    publish_registry_bytes(&bytes)
}
fn publish_registry_bytes(bytes: &[u8]) -> Result<ManagedRegistryInfo, String> {
    #[derive(serde::Deserialize)]
    struct TypeInfo { assembly_path: std::path::PathBuf, metadata: serde_json::Value }
    #[derive(serde::Deserialize)]
    struct Info { generation: u64, assemblies: usize, types: Vec<TypeInfo>, skipped_native: Vec<String> }
    let info: Info = serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
    let mut plugins = Vec::with_capacity(info.types.len());
    for item in info.types {
        let metadata = serde_json::to_vec(&vec![item.metadata]).map_err(|error| error.to_string())?;
        plugins.extend(inspect_metadata_bytes(&metadata, &item.assembly_path)?);
    }
    let info = ManagedRegistryInfo { generation: info.generation, assemblies: info.assemblies, plugins, skipped_native: info.skipped_native };
    let mut cache = SNAPSHOT.get_or_init(|| RwLock::new(None)).write().map_err(|_| "Managed registry snapshot lock poisoned")?;
    if cache.as_ref().is_none_or(|current| current.generation < info.generation) { *cache = Some(Arc::new(info.clone())); }
    Ok(info)
}

pub(super) fn create_parser(name: &str) -> Result<*mut c_void, String> {
    let context = unsafe { (api()?.create)(name.as_ptr(), name.len() as i32) };
    if context.is_null() { Err(last_error()) } else { Ok(context) }
}
pub(super) fn reset_parser(context: *mut c_void) -> Result<(), String> { if unsafe { (api()?.reset)(context) } < 0 { Err(last_error()) } else { Ok(()) } }
pub(super) fn destroy_parser(context: *mut c_void) { if let Ok(api) = api() { unsafe { (api.destroy)(context) }; } }

/// Wire fields match Desktop/RPC/DebugReportData's constructor exactly.
/// The caller attaches its actual TabletReference to the RPC envelope.
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub struct ManagedDebugReport {
    #[serde(rename = "Path")] pub path: String,
    #[serde(rename = "Data")] pub data: serde_json::Value,
}

/// One persistent original parser per source endpoint, bound to the thread
/// that constructs it. State begins at stream start, and resets after losses.
pub struct ManagedDebugDecoder {
    context: *mut c_void,
    buffer: Vec<u8>,
    _owner_thread: PhantomData<Rc<()>>,
}
impl ManagedDebugDecoder {
    pub fn new(parser_type: &str) -> Result<Self, String> {
        if parser_type.is_empty() || parser_type.len() > 4096 { return Err("Parser type length must be 1..4096 bytes".into()); }
        let context = unsafe { (api()?.create)(parser_type.as_ptr(), parser_type.len() as i32) };
        if context.is_null() { return Err(last_error()); }
        Ok(Self { context, buffer: Vec::new(), _owner_thread: PhantomData })
    }
    pub fn decode(&mut self, raw: &[u8]) -> Result<Option<ManagedDebugReport>, String> {
        if raw.is_empty() || raw.len() > 65535 { return Err("Debug raw packet length must be 1..65535".into()); }
        let api = api()?;
        let size = unsafe { (api.decode)(self.context, raw.as_ptr(), raw.len() as u32, std::ptr::null_mut(), 0) };
        if size < 0 { return Err(last_error()); }
        if size == 0 { return Ok(None); }
        if size > 262144 { return Err("Concrete debug report exceeds 256 KiB".into()); }
        self.buffer.resize(size as usize, 0);
        // Fetch retained serialized bytes; never invoke a stateful parser twice
        // because a large report needed a larger response buffer.
        let copied = unsafe { (api.decode)(self.context, std::ptr::null(), 0, self.buffer.as_mut_ptr(), size) };
        if copied != size { return Err(if copied < 0 { last_error() } else { "Concrete parser result changed during capacity retry".into() }); }
        serde_json::from_slice(&self.buffer).map(Some).map_err(|error| error.to_string())
    }
    pub fn reset(&mut self) -> Result<(), String> {
        let code = unsafe { (api()?.reset)(self.context) };
        if code < 0 { Err(last_error()) } else { Ok(()) }
    }
}
impl Drop for ManagedDebugDecoder { fn drop(&mut self) { if let Ok(api) = api() { unsafe { (api.destroy)(self.context) }; } } }
