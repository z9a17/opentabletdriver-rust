//! Explicit original collection files/presets, separate from selected native profiles.
use super::*;
use serde_json::{Value,json};
pub(super) const CMD_LOAD:u16=280;
pub(super) const CMD_EXPORT:u16=281;
pub(super) const CMD_APPLY:u16=282;
pub(super) const CMD_SAVE_PRESET:u16=283;
pub(super) const PRESET_PREFIX:&str="Original: ";

fn read(path:&Path)->Result<String,String>{
    use std::io::Read;
    let mut bytes=Vec::new();std::fs::File::open(path).map_err(|error|error.to_string())?
        .take((crate::control::MAX_PROFILE_BYTES+1)as u64).read_to_end(&mut bytes).map_err(|error|error.to_string())?;
    if bytes.len()>crate::control::MAX_PROFILE_BYTES{return Err("original settings exceed 128 KiB".into());}
    String::from_utf8(bytes).map_err(|error|error.to_string())
}
pub(super) fn load(app:&mut App,path:PathBuf){
    if app.import_pending||app.original_pending||app.closing||app.update_restart_pending{return;}
    let generation=app.metadata_generation;let edit_revision=app.edit_revision;
    let selected=app.selected_device.as_ref().map(|device|device.tablet.clone());
    app.import_pending=app.background("original-settings-import",move||BackgroundResult::Import{
        generation,edit_revision,result:(||{
            let text=read(&path)?;
            let names=crate::hid::connected_tablets_for_import()?;
            let names=background::import_tablet_order(names,selected.as_deref());
            crate::plugins::import_otd_with_installed(&text,&path,&names).map(Box::new)
        })(),
    });
}
pub(super) fn export(app:&mut App,path:PathBuf,preset:bool){
    if app.original_pending||app.closing||app.update_restart_pending{return;}
    let document=match app.checked_profile().and_then(|profile|profile.to_otd_json()){
        Ok(document)=>document,Err(error)=>{app.log(Level::Error,"Original settings",error);return;}
    };
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
fn apply_document(app:&mut App,document:Value,source:Option<PathBuf>){
    if app.original_pending||app.control_busy||app.closing||app.update_restart_pending{return;}
    let revision=app.edit_revision;let generation=app.metadata_generation;
    let expected=app.daemon_instance.clone();
    let names=background::import_tablet_order(app.connected_tablets.clone(),app.selected_device.as_ref().map(|device|device.tablet.as_str()));
    app.original_pending=app.background("original-settings-apply",move||{
        let result=(||{
            let status=crate::daemon::call(crate::control::Command::Status)?;
            if let(crate::control::Reply::Status{status},Some(expected))=(&status,expected){
                if status.instance!=expected{return Err("daemon changed before original collection apply".into());}
            }
            let imported=if let Some(source)=source{Some(Box::new(crate::plugins::import_otd_with_installed(&document.to_string(),&source,&names)?))}else{None};
            let mut client=crate::cli::Client::connect()?;
            client.call("GetSettings",json!([]))?;
            client.call("SetSettings",json!([document]))?;
            Ok(("Applied original settings collection. The draft remains unsaved until explicitly saved.".into(),imported))
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
    if id==CMD_APPLY{
        if commands::message_box(window,"Apply this original collection to every matching connected tablet? Unsaved native profile files are not changed.","Apply original settings collection",MB_OKCANCEL|MB_ICONQUESTION)==IDOK{with_app(apply);}return;
    }
    if id==CMD_LOAD&&!commands::confirm_discard(window){return;}
    let guard=with_app(|app|(app.edit_revision,app.metadata_generation));
    let kind=if id==CMD_SAVE_PRESET{commands::FileKind::OriginalPreset}else{commands::FileKind::Original};
    match commands::file_dialog(window,id!=CMD_LOAD,kind,if id==CMD_LOAD{"Load original settings collection"}else if id==CMD_SAVE_PRESET{"Save original collection as a new preset"}else{"Export original settings as a new file"}){
        Ok(Some(path))=>{
            if id==CMD_SAVE_PRESET{
                let directory=otd_core::presets::PresetStore::user().map(|store|store.directory().to_path_buf());
                if directory.as_ref().ok()!=path.parent().and_then(|parent|std::path::absolute(parent).ok()).as_ref(){with_app(|app|app.log(Level::Error,"Presets","Save original presets in the configured presets directory."));return;}
                let Some(name)=path.file_stem().and_then(|name|name.to_str())else{return;};
                if let Err(error)=otd_core::presets::PresetName::parse(name){with_app(|app|app.log(Level::Error,"Presets",error));return;}
            }
            with_app(|app|{
                if guard!=Some((app.edit_revision,app.metadata_generation))||app.closing||app.update_restart_pending{return;}
                if id==CMD_LOAD{load(app,path);}else{export(app,path,id==CMD_SAVE_PRESET);}
            });
        }
        Ok(None)=>{},Err(error)=>{with_app(|app|app.log(Level::Error,"Original settings",error));}
    }
}
