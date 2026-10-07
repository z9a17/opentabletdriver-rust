//! Pinned 0.6.7 Console workflows on the running native daemon's collection.
//! Native TOML/file commands remain separate from these live commands.
mod client;
mod settings;
pub(crate) use client::Client;
use serde_json::{Value,json};
use std::path::{Path,PathBuf};
use std::io::{self,BufRead};

pub fn recognizes(command:&str)->bool{matches!(command,
    "load"|"save"|"save-defaults"|"preset"|"savepreset"|"detect"|"installplugin"|"uninstallplugin"|
    "getstring"|"setoutputmode"|"enabletabletfilters"|"disabletabletfilters"|"resettabletfilters"|
    "enabletools"|"disabletools"|"setdisplayarea"|"maptodisplayindex"|"settabletarea"|"setsensitivity"|
    "settipbinding"|"setpenbinding"|"setauxbinding"|"setresettime"|"setenableclipping"|"setenablearealimiting"|
    "setlockaspectratio"|"log"|"getallsettings"|"getallsettingsjson"|"getoutputmode"|"getareas"|
    "getsensitivity"|"getbindings"|"getmiscsettings"|"getfilters"|"gettools"|"listplugins"|
    "hasupdate"|"installupdate"|"listoutputmodes"|"listfilters"|"listtools"|"listbindings"|
    "listpresets"|"listdisplays"|"getdiagnostics"|"stdio"|"edit")}
pub fn usage()-> &'static str{"Original daemon Console commands (live settings; no implicit daemon launch):
  load FILE.json | save FILE.json | save-defaults | preset NAME | savepreset NAME
  detect | installplugin FILE | uninstallplugin FOLDER | getstring VID PID INDEX
  setoutputmode TABLET TYPE | enabletabletfilters TABLET TYPES... | disabletabletfilters TABLET TYPES...
  resettabletfilters TABLET TYPES... | enabletools TYPES... | disabletools TYPES...
  setdisplayarea TABLET WIDTH HEIGHT X Y | maptodisplayindex TABLET INDEX
  settabletarea TABLET WIDTH HEIGHT X Y [ROTATION] | setsensitivity TABLET X Y [ROTATION]
  settipbinding TABLET TYPE THRESHOLD | setpenbinding TABLET TYPE INDEX | setauxbinding TABLET TYPE INDEX
  setresettime TABLET MS | setenableclipping TABLET BOOL | setenablearealimiting TABLET BOOL
  setlockaspectratio TABLET BOOL
  getoutputmode|getareas|getsensitivity|getbindings|getmiscsettings|getfilters TABLET
  getallsettings | getallsettingsjson | gettools | log | listplugins
  listoutputmodes | listfilters | listtools | listbindings | listpresets | listdisplays
  hasupdate | installupdate | getdiagnostics | stdio | edit
Quote tablet names/type names containing spaces. Binding indices are zero-based, as upstream.
Native file workflows: profiles/presets/devices; native-save FILE; native-save-defaults; native-stdio."}
pub fn run(args:Vec<String>)->Result<(),String>{
    if args.first().is_some_and(|arg|arg=="stdio"){
        exact(&args[1..],0)?;
        let input=io::stdin();let mut reader=input.lock();
        let mut connection=None;
        loop {
            let mut bytes=Vec::new();let mut bounded=(&mut reader).take((crate::control::MAX_PROFILE_BYTES+1)as u64);
            use std::io::Read;
            let read=bounded.read_until(b'\n',&mut bytes).map_err(|error|error.to_string())?;
            if read==0{break;}
            if bytes.len()>crate::control::MAX_PROFILE_BYTES{return Err("console command exceeds 128 KiB".into());}
            let line=std::str::from_utf8(&bytes).map_err(|error|error.to_string())?;
            let result=split_command(line).and_then(|args|if args.is_empty(){Ok(())}else{execute(&args,&mut connection)});
            if let Err(error)=result{eprintln!("{error}");}
            if connection.as_ref().is_some_and(|client|!client.usable()){connection=None;}
        }return Ok(());
    }
    execute(&args,&mut None)
}
fn exact(args:&[String],count:usize)->Result<(),String>{
    if args.len()==count{Ok(())}else{Err(format!("expected {count} arguments; see help"))}
}
fn connect(connection:&mut Option<Client>)->Result<&mut Client,String>{
    if connection.is_none(){*connection=Some(Client::connect()?);}
    connection.as_mut().ok_or_else(||"console connection unavailable".into())
}
fn number(value:&str)->Result<f64,String>{let value:f64=value.parse().map_err(|_|"expected a number")?;if value.is_finite(){Ok(value)}else{Err("number must be finite".into())}}
fn integer(value:&str)->Result<i64,String>{value.parse().map_err(|_|"expected an integer".into())}
fn boolean(value:&str)->Result<bool,String>{if value.eq_ignore_ascii_case("true"){Ok(true)}else if value.eq_ignore_ascii_case("false"){Ok(false)}else{Err("expected true or false".into())}}
fn path(info:&Value,field:&str)->Result<PathBuf,String>{info[field].as_str().map(PathBuf::from).ok_or_else(||format!("daemon has no {field}"))}
fn read_bytes(file:&Path)->Result<Vec<u8>,String>{
    use std::io::Read;
    let file=std::fs::File::open(file).map_err(|error|error.to_string())?;
    let mut bytes=Vec::new();file.take((crate::control::MAX_PROFILE_BYTES+1)as u64).read_to_end(&mut bytes).map_err(|error|error.to_string())?;
    if bytes.len()>crate::control::MAX_PROFILE_BYTES{return Err("settings exceeds 128 KiB".into());}Ok(bytes)
}
fn read_document(file:&Path)->Result<Value,String>{
    serde_json::from_slice(&read_bytes(file)?).map_err(|error|format!("Invalid settings file: {error}"))
}
fn write_document(file:&Path,document:&Value,new:bool)->Result<(),String>{
    let snapshot=otd_core::storage::capture(file)?;
    let bytes=serde_json::to_vec_pretty(document).map_err(|error|error.to_string())?;
    if bytes.len()>crate::control::MAX_PROFILE_BYTES{return Err("settings exceeds 128 KiB".into());}
    otd_core::storage::save(file,&bytes,if new{otd_core::storage::SaveMode::CreateNew}else{otd_core::storage::SaveMode::Replace(&snapshot)})?;Ok(())
}
fn preset_path(directory:&Path,name:&str)->Result<PathBuf,String>{
    // Reuse native name validation, but original collections always use .json.
    let name=otd_core::presets::PresetName::parse(name)?;
    Ok(directory.join(format!("{}.json",name.as_str())))
}
fn execute(args:&[String],connection:&mut Option<Client>)->Result<(),String>{
    let command=args.first().ok_or("console command required")?.as_str();let args=&args[1..];
    if command=="edit"{
        exact(args,0)?;let Some(editor)=std::env::var_os("EDITOR").filter(|editor|!editor.is_empty())else{println!("The EDITOR environment variable is not set.");return Ok(());};
        let editor=editor.to_str().ok_or("EDITOR is not Unicode")?;
        return edit(connect(connection)?,editor);
    }
    if !recognizes(command)||command=="stdio"{return Err(format!("unknown or nested Console command '{command}'"));}
    let client=connect(connection)?;
    match command{
        "load"=>{exact(args,1)?;let document=read_document(Path::new(&args[0]))?;client.call("GetSettings",json!([]))?;client.call("SetSettings",json!([document]))?;}
        "save"|"save-defaults"|"savepreset"=>{
            exact(args,usize::from(command!="save-defaults"))?;
            let info=client.call("GetApplicationInfo",json!([]))?;
            let file=match command{"save"=>PathBuf::from(&args[0]),"savepreset"=>preset_path(&path(&info,"PresetDirectory")?,&args[0])?,_=>path(&info,"SettingsFile")?};
            if command=="save-defaults" || (command=="save" && std::path::absolute(&file).map_err(|error|error.to_string())?==std::path::absolute(path(&info,"SettingsFile")?).map_err(|error|error.to_string())?){client.call("SaveSettings",json!([]))?;}else{let document=client.call("GetSettings",json!([]))?;write_document(&file,&document,command=="savepreset")?;}
        }
        "preset"=>{exact(args,1)?;let info=client.call("GetApplicationInfo",json!([]))?;let file=preset_path(&path(&info,"PresetDirectory")?,&args[0])?;let document=read_document(&file)?;client.call("GetSettings",json!([]))?;client.call("SetSettings",json!([document]))?;}
        "listpresets"=>{exact(args,0)?;let info=client.call("GetApplicationInfo",json!([]))?;let directory=path(&info,"PresetDirectory")?;std::fs::create_dir_all(&directory).map_err(|error|error.to_string())?;
            let mut names=Vec::new();for entry in std::fs::read_dir(directory).map_err(|error|error.to_string())?{let entry=entry.map_err(|error|error.to_string())?;let file=entry.path();if file.extension().is_some_and(|extension|extension.eq_ignore_ascii_case("json")){if let Some(name)=file.file_stem().and_then(|name|name.to_str()){names.push(name.to_owned());}}}names.sort();for name in names{println!("{name}");}}
        "detect"=>{exact(args,0)?;client.call("DetectTablets",json!([]))?;let document=client.call("GetSettings",json!([]))?;client.call("SetSettings",json!([document]))?;}
        "getstring"=>{exact(args,3)?;let values=args.iter().map(|value|integer(value)).collect::<Result<Vec<_>,_>>()?;println!("{}",client.call("RequestDeviceString",json!(values))?.as_str().ok_or("device string result is not text")?);}
        "installplugin"=>{exact(args,1)?;if client.call("InstallPlugin",json!([std::path::absolute(&args[0]).map_err(|error|error.to_string())?]))?==false{return Err("Unable to install plugin".into());}}
        "uninstallplugin"|"listplugins"=>{
            exact(args,usize::from(command=="uninstallplugin"))?;let info=client.call("GetApplicationInfo",json!([]))?;let root=path(&info,"PluginDirectory")?;
            if command=="listplugins"{let mut names=Vec::new();if root.exists(){for entry in std::fs::read_dir(root).map_err(|error|error.to_string())?{let entry=entry.map_err(|error|error.to_string())?;if entry.file_type().map_err(|error|error.to_string())?.is_dir(){names.push(entry.file_name().to_string_lossy().into_owned());}}}names.sort();for name in names{println!("{name}");}}
            else{let name=&args[0];if name.is_empty()||Path::new(name).components().count()!=1||name.contains(['/', '\\', ':'])||matches!(name.as_str(),"."|".."){return Err("plugin folder must be one directory name".into());}let folder=root.join(name);if !folder.is_dir(){return Err("installed plugin folder not found".into());}client.call("UninstallPlugin",json!([folder]))?;}
        }
        "hasupdate"|"installupdate"=>{exact(args,0)?;let update=client.call("CheckForUpdates",json!([]))?;if command=="hasupdate"{println!("{}",!update.is_null());}else if !update.is_null(){client.call("InstallUpdate",json!([]))?;}}
        "getdiagnostics"=>{exact(args,0)?;let value=client.call("GetDiagnosticInfo",json!([]))?;println!("{}",serde_json::to_string_pretty(&value).map_err(|error|error.to_string())?);}
        "log"=>{exact(args,0)?;let value=client.call("GetCurrentLog",json!([]))?;for message in value.as_array().ok_or("log result is not an array")?{println!("{} [{}] {}: {}",message["Time"].as_str().unwrap_or(""),message["Level"],message["Group"].as_str().unwrap_or(""),message["Message"].as_str().unwrap_or(""));}}
        "listoutputmodes"|"listfilters"|"listtools"|"listbindings"=>{exact(args,0)?;let category=match command{"listoutputmodes"=>"output","listfilters"=>"filter","listtools"=>"tool",_=>"binding"};let inventory=client.call("GetPluginTypes",json!([]))?;let mut found=false;for entry in inventory.as_array().ok_or("plugin types is not an array")?.iter().filter(|entry|entry["category"]==category){found=true;let name=entry["name"].as_str().filter(|name|!name.is_empty());let path=entry["path"].as_str().ok_or("plugin type has no path")?;if let Some(name)=name{println!("{path} [{name}]");}else{println!("{path}");}}if !found{println!("No types found");}}
        "listdisplays"=>{exact(args,0)?;let displays=crate::display::read_snapshot()?;for(index,display)in displays.monitors.iter().chain(std::iter::once(&displays.virtual_screen)).enumerate(){println!("{index}: {}x{} at {},{}",display.width(),display.height(),display.left,display.top);}}
        "getallsettings"|"getallsettingsjson"|"gettools"|"getoutputmode"|"getareas"|"getsensitivity"|"getbindings"|"getmiscsettings"|"getfilters"=>{
            exact(args,usize::from(!matches!(command,"getallsettings"|"getallsettingsjson"|"gettools")))?;
            let document=client.call("GetSettings",json!([]))?;
            match command{
                "getallsettingsjson"=>println!("{}",serde_json::to_string_pretty(&document).map_err(|error|error.to_string())?),
                "gettools"=>println!("Tools: {}",settings::format_stores(&document["Tools"])),
                "getallsettings"=>{println!("--- Generic Settings ---\nTools: {}",settings::format_stores(&document["Tools"]));for row in document["Profiles"].as_array().ok_or("daemon settings has no Profiles")?{println!("\n--- Profile for '{}' ---",row["Tablet"].as_str().unwrap_or(""));for getter in["getoutputmode","getareas","getsensitivity","getbindings","getmiscsettings","getfilters"]{settings::getter(getter,row);}}},
                _=>settings::getter(command,settings::profile(&document,&args[0])?),
            }
        }
        _=>modify(client,command,args)?,
    }Ok(())
}
fn modify(client:&mut Client,command:&str,args:&[String])->Result<(),String>{
    let mut document=client.call("GetSettings",json!([]))?;
    if matches!(command,"enabletools"|"disabletools"){
        settings::change_stores(client,&mut document["Tools"],args,"tool",if command=="enabletools"{"enable"}else{"disable"})?;
    }else{
        let tablet=args.first().ok_or("tablet argument required")?;let row=settings::profile_mut(&mut document,tablet)?;
        for field in ["AbsoluteModeSettings","RelativeModeSettings","Bindings"]{
            if row[field].is_null(){row[field]=json!({});}
            if !row[field].is_object(){return Err(format!("{field} must be an object"));}
        }
        for field in ["Display","Tablet"]{
            if row["AbsoluteModeSettings"][field].is_null(){row["AbsoluteModeSettings"][field]=json!({});}
            if !row["AbsoluteModeSettings"][field].is_object(){return Err(format!("{field} area must be an object"));}
        }
        let values=&args[1..];
        match command{
            "enabletabletfilters"|"disabletabletfilters"|"resettabletfilters"=>settings::change_stores(client,&mut row["Filters"],values,"filter",match command{"enabletabletfilters"=>"enable","disabletabletfilters"=>"disable",_=>"reset"})?,
            "setoutputmode"=>{exact(values,1)?;row["OutputMode"]=settings::store(client,&values[0],"output")?;}
            "settipbinding"|"setpenbinding"|"setauxbinding"=>{exact(values,2)?;let store=settings::store(client,&values[0],"binding")?;
                if command=="settipbinding"{row["Bindings"]["TipButton"]=store;row["Bindings"]["TipActivationThreshold"]=json!(number(&values[1])?);}
                else{let index:usize=values[1].parse().map_err(|_|"binding index must be zero or greater")?;let field=if command=="setpenbinding"{"PenButtons"}else{"AuxButtons"};let slots=row["Bindings"][field].as_array_mut().ok_or("button bindings are not an array")?;*slots.get_mut(index).ok_or("binding index is outside this tablet's configured slots")?=store;}
            }
            "setdisplayarea"|"settabletarea"=>{
                if values.len()!=4&&!(command=="settabletarea"&&values.len()==5){return Err("area needs WIDTH HEIGHT X Y and optional tablet ROTATION".into());}
                let field=if command=="setdisplayarea"{"Display"}else{"Tablet"};for(index,property)in["Width","Height","X","Y"].iter().enumerate(){row["AbsoluteModeSettings"][field][*property]=json!(number(&values[index])?);}
                if command=="settabletarea"{row["AbsoluteModeSettings"][field]["Rotation"]=json!(if values.len()==5{number(&values[4])?}else{0.0});}
            }
            "maptodisplayindex"=>{exact(values,1)?;let index:usize=values[0].parse().map_err(|_|"display index must be zero or greater")?;let displays=crate::display::read_snapshot()?;let display=displays.monitors.iter().chain(std::iter::once(&displays.virtual_screen)).nth(index).ok_or("display index is out of range")?;let x=if index==displays.monitors.len(){display.width()as f64/2.0}else{display.left as f64+display.width()as f64/2.0};let y=if index==displays.monitors.len(){display.height()as f64/2.0}else{display.top as f64+display.height()as f64/2.0};let area=&mut row["AbsoluteModeSettings"]["Display"];for(field,value)in[("Width",display.width()as f64),("Height",display.height()as f64),("X",x),("Y",y)]{area[field]=json!(value);}}
            "setsensitivity"=>{if !(2..=3).contains(&values.len()){return Err("sensitivity needs X Y [ROTATION]".into());}row["RelativeModeSettings"]["XSensitivity"]=json!(number(&values[0])?);row["RelativeModeSettings"]["YSensitivity"]=json!(number(&values[1])?);row["RelativeModeSettings"]["RelativeRotation"]=json!(if values.len()==3{number(&values[2])?}else{0.0});}
            "setresettime"=>{exact(values,1)?;let ms=integer(&values[0])?;if ms<0{return Err("reset time cannot be negative".into());}let sec=ms/1000;row["RelativeModeSettings"]["RelativeResetDelay"]=json!(format!("{}.{:02}:{:02}:{:02}.{:07}",sec/86400,(sec/3600)%24,(sec/60)%60,sec%60,(ms%1000)*10000));}
            "setenableclipping"|"setenablearealimiting"|"setlockaspectratio"=>{exact(values,1)?;let field=match command{"setenableclipping"=>"EnableClipping","setenablearealimiting"=>"EnableAreaLimiting",_=>"LockAspectRatio"};row["AbsoluteModeSettings"][field]=json!(boolean(&values[0])?);}
            _=>return Err(format!("unknown setter '{command}'")),
        }
    }
    client.call("SetSettings",json!([document]))?;Ok(())
}
struct EditedFile(PathBuf);
impl Drop for EditedFile{fn drop(&mut self){let _=std::fs::remove_file(&self.0);}}
fn edit(client:&mut Client,editor:&str)->Result<(),String>{
    let tokens=split_command(editor)?;let executable=tokens.first().ok_or("EDITOR is empty")?;
    let document=client.call("GetSettings",json!([]))?;let info=client.call("GetApplicationInfo",json!([]))?;
    let directory=path(&info,"TemporaryDirectory")?;std::fs::create_dir_all(&directory).map_err(|error|error.to_string())?;
    let nonce=std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_err(|error|error.to_string())?.as_nanos();
    let path=directory.join(format!("OpenTabletDriver-{}-{nonce}.json",std::process::id()));
    write_document(&path,&document,true)?;let file=EditedFile(path);let before=read_bytes(&file.0)?;
    let mut child=std::process::Command::new(executable).args(&tokens[1..]).arg(&file.0).spawn().map_err(|error|format!("cannot run EDITOR: {error}"))?;
    let mut keepalive=std::time::Instant::now();let mut heartbeat_error=None;
    let status=loop{
        if let Some(status)=child.try_wait().map_err(|error|error.to_string())?{break status;}
        if keepalive.elapsed()>=std::time::Duration::from_secs(30)&&heartbeat_error.is_none(){
            // Pure metadata heartbeat preserves the earlier GetSettings revision.
            if let Err(error)=client.call("GetApplicationInfo",json!([])){heartbeat_error=Some(error);}
            keepalive=std::time::Instant::now();
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    };
    if let Some(error)=heartbeat_error{return Err(format!("settings were not applied after editor: {error}"));}
    if !status.success(){return Err(format!("EDITOR exited with {status}; settings were not applied"));}
    let after=read_bytes(&file.0)?;
    if before==after{println!("The file was left unchanged. Settings will not be applied.");return Ok(());}
    let changed=read_document(&file.0)?;client.call("SetSettings",json!([changed]))?;println!("Settings were successfully applied.");Ok(())
}
/// Quotes protect spaces; no shell expansion, substitutions or command execution.
pub fn split_command(line:&str)->Result<Vec<String>,String>{
    let mut result=Vec::new();let mut token=String::new();let mut quote=None;let mut started=false;
    for character in line.chars(){
        if let Some(delimiter)=quote{if character==delimiter{quote=None;}else{token.push(character);}started=true;}
        else if matches!(character,'\''|'"'){quote=Some(character);started=true;}
        else if character.is_whitespace(){if started{result.push(std::mem::take(&mut token));started=false;}}
        else{token.push(character);started=true;}
    }
    if quote.is_some(){return Err("unterminated command quote".into());}if started{result.push(token);}Ok(result)
}
