//! Explicit original collection files/presets, separate from selected native profiles.
use super::*;
use serde_json::{Value,json};
pub(super) const CMD_LOAD:u16=280;
pub(super) const CMD_EXPORT:u16=281;
pub(super) const CMD_APPLY:u16=282;
pub(super) const CMD_SAVE_PRESET:u16=283;
pub(super) const CMD_SAVE_DEFAULT:u16=284;
pub(super) const CMD_RESET:u16=285;
pub(super) const CMD_SAVE_AS:u16=286;
pub(super) const PRESET_PREFIX:&str="Original: ";

fn read(path:&Path)->Result<String,String>{
    use std::io::Read;
    let mut bytes=Vec::new();std::fs::File::open(path).map_err(|error|error.to_string())?
        .take((crate::control::MAX_PROFILE_BYTES+1)as u64).read_to_end(&mut bytes).map_err(|error|error.to_string())?;
    if bytes.len()>crate::control::MAX_PROFILE_BYTES{return Err("original settings exceed 128 KiB".into());}
    String::from_utf8(bytes).map_err(|error|error.to_string())
}
pub(super) fn load(app:&mut App,path:PathBuf){
    if app.import_pending||app.original_pending||app.control_busy||app.closing||app.update_restart_pending{return;}
    transact(app,Operation::Load(path));
}
pub(super) fn export(app:&mut App,path:PathBuf,preset:bool){
    if app.original_pending||app.closing||app.update_restart_pending{return;}
    let document=match app.checked_profile().and_then(|profile|profile.to_otd_json()){
        Ok(document)=>document,Err(error)=>{app.log(Level::Error,"Original settings",error);return;}
    };
    if document.len()>crate::control::MAX_PROFILE_BYTES{app.log(Level::Error,"Original settings","Original collection exceeds 128 KiB.");return;}
    let revision=app.edit_revision;let generation=app.metadata_generation;
    app.original_pending=app.background("original-settings-export",move||{
        let result=(||{
            otd_core::storage::save(&path,document.as_bytes(),otd_core::storage::SaveMode::CreateNew)?;
            Ok((format!("Saved original {} {}.",if preset{"preset"}else{"settings"},path.display()),None))
        })();BackgroundResult::Original{generation,revision,result}
    });
}
pub(super) fn apply(app:&mut App){
    let document=match app.checked_profile().and_then(|profile|profile.to_otd_json())
        .and_then(|text|serde_json::from_str(&text).map_err(|error|error.to_string())){
        Ok(document)=>document,Err(error)=>{app.log(Level::Error,"Original settings",error);return;}
    };
    apply_document(app,document,None);
}
enum Operation{Apply(Value,Option<PathBuf>),Load(PathBuf),SaveDefault(Value),SaveAs(Value,PathBuf),Reset}
fn draft(app:&App)->Result<Value,String>{
    app.checked_profile().and_then(|profile|profile.to_otd_json()).and_then(|text|serde_json::from_str(&text).map_err(|error|error.to_string()))
}
fn apply_document(app:&mut App,document:Value,source:Option<PathBuf>){transact(app,Operation::Apply(document,source));}
fn transact(app:&mut App,operation:Operation){
    if app.import_pending||app.original_pending||app.control_busy||app.closing||app.update_restart_pending{return;}
    let revision=app.edit_revision;let generation=app.metadata_generation;
    let expected=app.daemon_instance.clone();
    let names=background::import_tablet_order(app.connected_tablets.clone(),app.selected_device.as_ref().map(|device|device.tablet.as_str()));
    let profile_path=app.profile_path.clone();
    app.original_pending=app.background("original-settings-operation",move||{
        let result=(||{
            let mut client=crate::cli::Client::connect()?;
            if expected.as_ref().is_some_and(|expected|expected!=client.instance()){
                return Err("daemon changed before original collection operation".into());
            }
            client.call("GetSettings",json!([]))?;
            let (document,source,save,save_as)=match operation{
                Operation::Apply(document,source)=>(Some(document),source,false,None),
                Operation::Load(path)=>{let document=serde_json::from_str(&read(&path)?).map_err(|error|error.to_string())?;(Some(document),Some(path),false,None)},
                Operation::SaveDefault(document)=>(Some(document),None,true,None),
                Operation::SaveAs(document,path)=>(Some(document),None,false,Some(path)),
                Operation::Reset=>{client.call("ResetSettings",json!([]))?;(None,None,false,None)},
            };
            // A retained original collection can be empty or contain only
            // disconnected/unknown models. Its optional native editor projection
            // must not reject the daemon's legitimate offline settings document.
            let imported:Result<Option<Box<Profile>>,String>=if let(Some(document),Some(source))=(&document,source){
                crate::plugins::import_otd_with_installed(&document.to_string(),&source,&names).map(|profile|Some(Box::new(profile)))
            }else{Ok(None)};
            if let Some(path)=&save_as{
                let bytes=serde_json::to_vec_pretty(document.as_ref().ok_or("settings document missing")?).map_err(|error|error.to_string())?;
                if bytes.len()>crate::control::MAX_PROFILE_BYTES{return Err("original collection exceeds 128 KiB".into());}
                otd_core::storage::save(path,&bytes,otd_core::storage::SaveMode::CreateNew)?;
            }
            if let Some(document)=document{
                if let Err(error)=client.call("SetSettings",json!([document])){
                    return Err(if let Some(path)=save_as{format!("Saved {} but collection apply failed: {error}",path.display())}else{error});
                }
            }else{
                let document=client.call("GetSettings",json!([]))?;
                let imported=if document["Profiles"].as_array().is_some_and(|rows|!rows.is_empty()){
                    crate::plugins::import_otd_with_installed(&document.to_string(),&profile_path,&names)
                }else{Ok(Profile::default())};
                let message="Reset the original daemon settings collection to actual defaults. Native profile files are unchanged.";
                return Ok(match imported {
                    Ok(profile)=>(message.into(),Some(Box::new(profile))),
                    Err(error)=>(format!("{message} The existing native draft was retained because its projection is unavailable: {error}"),None),
                });
            }
            if save{client.call("SaveSettings",json!([])).map_err(|error|format!("Collection applied but default-file save failed: {error}"))?;}
            let message=if save{"Applied and saved the original default collection; selected native files are unchanged.".into()}
                else if let Some(path)=save_as{format!("Saved {} and applied the original collection.",path.display())}
                else{"Applied original settings collection. The draft remains unsaved until explicitly saved.".to_owned()};
            Ok(match imported {
                Ok(profile)=>(message,profile),
                Err(error)=>(format!("{message} The existing native draft was retained because this collection has no editable native projection: {error}"),None),
            })
        })();BackgroundResult::Original{generation,revision,result}
    });
}
pub(super) fn apply_preset(app:&mut App,name:&str){
    let result=(||{
        let name=otd_core::presets::PresetName::parse(name)?;
        let store=otd_core::presets::PresetStore::user()?;
        let path=store.directory().join(format!("{}.json",name.as_str()));
        let text=read(&path)?;
        serde_json::from_str(&text).map(|document|(document,path)).map_err(|error|error.to_string())
    })();
    match result{Ok((document,path))=>apply_document(app,document,Some(path)),Err(error)=>app.log(Level::Error,"Original preset",error)}
}
pub(super) fn command(window:HWND,id:u16){
    let guard=with_app(|app|(app.managed_token,app.edit_revision,app.metadata_generation));
    if matches!(id,CMD_APPLY|CMD_SAVE_DEFAULT|CMD_RESET){
        let text=if id==CMD_RESET{"Reset the original daemon collection for every matching tablet? Native profile files are unchanged."}
            else if id==CMD_SAVE_DEFAULT{"Apply this original collection and save it as the daemon's original default settings? Selected native files are unchanged."}
            else{"Apply this original collection to every matching connected tablet? Native profile files are unchanged."};
        if commands::message_box(window,text,"Original settings collection",MB_OKCANCEL|MB_ICONQUESTION)==IDOK{
            with_app(|app|{
                if guard!=Some((app.managed_token,app.edit_revision,app.metadata_generation))||app.closing||app.update_restart_pending{return;}
                if id==CMD_RESET{transact(app,Operation::Reset);}
                else if id==CMD_APPLY{apply(app);}
                else{match draft(app){Ok(document)=>transact(app,Operation::SaveDefault(document)),Err(error)=>app.log(Level::Error,"Original settings",error)}}
            });
        }return;
    }
    if id==CMD_LOAD&&!commands::confirm_discard(window){return;}
    let kind=if id==CMD_SAVE_PRESET{commands::FileKind::OriginalPreset}else{commands::FileKind::Original};
    match commands::file_dialog(window,id!=CMD_LOAD,kind,if id==CMD_LOAD{"Load and apply original settings collection"}else if id==CMD_SAVE_PRESET{"Save original collection as a new preset"}else if id==CMD_SAVE_AS{"Save and apply original collection as a new file"}else{"Export original settings as a new file"}){
        Ok(Some(path))=>{
            if id==CMD_SAVE_PRESET{
                if !path.extension().is_some_and(|extension|extension.eq_ignore_ascii_case("json")){with_app(|app|app.log(Level::Error,"Presets","Original presets must use the .json extension."));return;}
                let directory=otd_core::presets::PresetStore::user().map(|store|store.directory().to_path_buf());
                if directory.as_ref().ok()!=path.parent().and_then(|parent|std::path::absolute(parent).ok()).as_ref(){with_app(|app|app.log(Level::Error,"Presets","Save original presets in the configured presets directory."));return;}
                let Some(name)=path.file_stem().and_then(|name|name.to_str())else{return;};
                if let Err(error)=otd_core::presets::PresetName::parse(name){with_app(|app|app.log(Level::Error,"Presets",error));return;}
            }
            with_app(|app|{
                if guard!=Some((app.managed_token,app.edit_revision,app.metadata_generation))||app.closing||app.update_restart_pending{return;}
                if id==CMD_LOAD{load(app,path);}else if id==CMD_SAVE_AS{match draft(app){Ok(document)=>transact(app,Operation::SaveAs(document,path)),Err(error)=>app.log(Level::Error,"Original settings",error)}}else{export(app,path,id==CMD_SAVE_PRESET);}
            });
        }
        Ok(None)=>{},Err(error)=>{with_app(|app|app.log(Level::Error,"Original settings",error));}
    }
}
