//! Platform CLI and original desktop launcher. Only explicit invocation starts
//! a native owner or GUI. Metadata/help/version do not initialize CoreCLR.
use std::io::{self,BufRead,Read};
use std::sync::{Arc,atomic::{AtomicBool,Ordering}};
use std::time::Duration;
use crate::daemon::{Command,Platform};

#[derive(Default)]
pub struct Options { pub upstream_pipe:Option<String>,pub owner_stdin:bool }
impl Options {
    pub fn parse(arguments:impl IntoIterator<Item=String>) -> Result<Self,String> {
        let mut args=arguments.into_iter();let mut options=Self::default();
        while let Some(argument)=args.next(){match argument.as_str(){
            "--upstream-rpc"=>options.upstream_pipe=Some("OpenTabletDriver.Daemon".into()),
            "--upstream-pipe"=>{
                let name=args.next().ok_or("--upstream-pipe requires a name")?;
                if name.is_empty()||name.len()>128||!name.bytes().all(|byte|byte.is_ascii_alphanumeric()||b"._-".contains(&byte)){return Err("Upstream pipe requires a safe 1..128 byte name".into());}
                options.upstream_pipe=Some(name);
            },
            "--owner-stdin"=>options.owner_stdin=true,
            _=>return Err(format!("Unknown daemon option {argument}")),
        }}Ok(options)
    }
}
pub fn daemon(platform:Arc<dyn Platform>,options:Options,signal:&'static AtomicBool) -> Result<(),String> {
    let stop=Arc::new(AtomicBool::new(false));
    let monitor_stop=stop.clone();
    let monitor=std::thread::Builder::new().name("unix-stop-signals".into()).spawn(move||{
        while !monitor_stop.load(Ordering::Acquire){if signal.load(Ordering::Acquire){monitor_stop.store(true,Ordering::Release);break;}std::thread::sleep(Duration::from_millis(100));}
    }).map_err(|error|error.to_string())?;
    if options.owner_stdin {
        let input_stop=stop.clone();std::thread::Builder::new().name("unix-launcher-lifetime".into()).spawn(move||{
            let mut input=io::stdin().lock();let mut buffer=[0u8;64];
            loop{match input.read(&mut buffer){Ok(0)|Err(_)=>break,Ok(_)=>{}}}input_stop.store(true,Ordering::Release);
        }).map_err(|error|error.to_string())?;
    }
    let result=(||{
        let lease=crate::local_control::Ownership::reserve()?;crate::update::remove_leftovers()?;
        let mut owner=crate::daemon::Owner::start(platform,stop.clone())?;
        // Admission/listener ownership precedes Start: another daemon can never
        // fail its lease after this instance has started injecting input.
        let server=crate::local_control::Server::start_reserved(owner.handle.clone(),stop.clone(),lease)?;
        let mut rpc=options.upstream_pipe.as_deref().map(crate::dotnet::HostedRpc::start).transpose()?;
        owner.handle.call(Command::Start)?;
        while !stop.load(Ordering::Acquire){std::thread::sleep(Duration::from_millis(100));}
        // Drain native owners first, then stop original responder tasks while
        // their callback services remain alive. Both failures reach the caller.
        let native_result=owner.drain();
        let rpc_result=rpc.as_mut().map_or(Ok(()),crate::dotnet::HostedRpc::stop);
        drop(rpc);drop(owner);drop(server);
        match (native_result,rpc_result) {
            (Err(native),Err(rpc))=>Err(format!("Native shutdown: {native}; original RPC shutdown: {rpc}")),
            (Err(error),_)|(_,Err(error))=>Err(error),
            (Ok(()),Ok(()))=>Ok(()),
        }
    })();
    stop.store(true,Ordering::Release);let _=monitor.join();result
}
pub fn control(command:&str,arguments:Vec<String>) -> Result<(),String> {
    let value=match command {
        "status"=>{if !arguments.is_empty(){return Err("status takes no arguments".into());}crate::local_control::request(Command::Status)?},
        "start"=>crate::local_control::request(Command::Start)?,
        "stop"=>crate::local_control::request(Command::Stop)?,
        "shutdown"=>crate::local_control::request(Command::Shutdown)?,
        "detect"=>crate::local_control::request(Command::Detect)?,
        "request"=>{if arguments.len()!=1{return Err("request requires one JSON Command".into());}
            crate::local_control::request(serde_json::from_str(&arguments[0]).map_err(|error|error.to_string())?)?},
        "console"=>{
            if !arguments.is_empty(){return Err("console takes JSON commands on stdin".into());}
            let mut input=io::stdin().lock();let mut line=String::new();
            loop{line.clear();let mut bounded=Read::by_ref(&mut input).take(256*1024+2);let read=bounded.read_line(&mut line).map_err(|error|error.to_string())?;
                if read==0{break;}if read>256*1024||!line.ends_with('\n'){return Err("Console command exceeds 256 KiB or is unterminated".into());}
                let result=serde_json::from_str::<Command>(&line).map_err(|error|error.to_string()).and_then(crate::local_control::request);
                println!("{}",match result{Ok(value)=>serde_json::json!({"ok":true,"result":value}),Err(error)=>serde_json::json!({"ok":false,"error":error})});
            }return Ok(());
        },
        _=>return Err(format!("Unknown native control command {command}")),
    };println!("{}",serde_json::to_string_pretty(&value).map_err(|error|error.to_string())?);Ok(())
}
pub fn ui(arguments:Vec<String>) -> Result<(),String> {
    let base=std::env::current_exe().map_err(|error|error.to_string())?.parent().ok_or("Executable directory unavailable")?.to_owned();
    let mut command=if cfg!(target_os="linux") {
        let path=base.join("OpenTabletDriver.UX.Gtk.dll");
        if !path.is_file(){return Err("Original GTK frontend is absent; install the complete GUI distribution".into());}
        let mut command=std::process::Command::new("dotnet");command.arg(path);command
    } else {
        let path=base.join("OpenTabletDriver.UX.MacOS");
        if !path.is_file(){return Err("Original macOS frontend apphost is absent; install the complete GUI distribution".into());}
        std::process::Command::new(path)
    };
    let status=command.current_dir(base).args(arguments).status().map_err(|error|error.to_string())?;
    if status.success(){Ok(())}else{Err(format!("Original frontend exited with {status}"))}
}
/// Pinned original CLI retains every original alias, binding editor, plugin
/// command and settings workflow. Its client uses the original fixed pipe.
pub fn original_console(arguments:Vec<String>)->Result<(),String>{
    let base=std::env::current_exe().map_err(|error|error.to_string())?.parent().ok_or("Executable directory unavailable")?.to_owned();
    let path=base.join("OpenTabletDriver.Console.dll");
    if !path.is_file(){return Err("Original console is absent; install the complete distribution".into());}
    let status=std::process::Command::new("dotnet").arg(path).args(arguments).current_dir(base).status().map_err(|error|error.to_string())?;
    if status.success(){Ok(())}else{Err(format!("Original console exited with {status}"))}
}

/// The CLI stages through the same drained transaction as original RPC. A
/// stopped machine gets a cold owner/listener lease, never a tablet Start.
pub fn update(platform:Arc<dyn Platform>,arguments:Vec<String>)->Result<(),String>{
    let check=match arguments.as_slice(){[]=>false,[arg]if arg=="--check"||arg=="check"=>true,[arg]if arg=="install"=>false,_=>return Err("update [--check | check | install]".into())};
    let original=|method:&str|Command::Original{expected:None,source:None,binding_owner:None,method:method.into(),params:serde_json::json!([])};
    let perform=|handle:Option<&crate::daemon::Handle>|->Result<(),String>{
        let request=|command|match handle{Some(handle)=>handle.call(command),None=>crate::local_control::request(command)};
        let value=request(original("CheckForUpdates"))?;
        if value.is_null(){println!("The current version is up to date.");return Ok(());}
        println!("Update available: {}",value["Version"].as_str().ok_or("Update version missing")?);
        if !check{request(original("InstallUpdate"))?;println!("Update staged. Restart the driver and frontend.");
            if let Some(handle)=handle{use std::io::Write;io::stdout().flush().map_err(|e|e.to_string())?;handle.call(original("FinishUpdate"))?;}}
        Ok(())
    };
    if crate::local_control::available()?{return perform(None);}
    let lease=crate::local_control::Ownership::reserve()?;crate::update::remove_leftovers()?;
    let stop=Arc::new(AtomicBool::new(false));let mut owner=crate::daemon::Owner::start(platform,stop.clone())?;
    let server=crate::local_control::Server::start_reserved(owner.handle.clone(),stop.clone(),lease)?;
    let result=perform(Some(&owner.handle));stop.store(true,Ordering::Release);let cleaned=owner.join();drop(server);result.and(cleaned)
}
