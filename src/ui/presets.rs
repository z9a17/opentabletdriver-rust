//! File > Presets: named settings, as in OpenTabletDriver's preset menu.
//! Choosing a native preset loads an unsaved draft and applies it if attached;
//! original JSON presets apply their whole collection through the daemon. Saving writes a new preset, or replaces one after the
//! save dialog's overwrite prompt.
use super::commands::{append, shell_open};
use super::*;
use otd_core::presets::{PresetName, PresetStore};

pub(super) const CMD_PRESET_FIRST: u16 = 6400;
pub(super) const PRESET_CHOICES: u16 = 64;
pub(super) const CMD_PRESET_SAVE: u16 = 6200;
pub(super) const CMD_PRESET_FOLDER: u16 = 6201;
pub(super) const CMD_PRESET_PREVIOUS: u16 = 6202;
pub(super) const CMD_PRESET_NEXT: u16 = 6203;

/// Called by the background worker; menu creation only uses its cached result.
pub(super) fn list_names() -> Result<Vec<String>, String> {
    let store=PresetStore::user()?;
    let mut names:Vec<_>=store.list()?.presets.into_iter().filter(|preset|preset.error.is_none()).map(|preset|preset.name).collect();
    if store.directory().exists(){
        for entry in std::fs::read_dir(store.directory()).map_err(|error|error.to_string())?{
            let path=entry.map_err(|error|error.to_string())?.path();
            if path.extension().is_some_and(|extension|extension.eq_ignore_ascii_case("json")){
                if let Some(name)=path.file_stem().and_then(|name|name.to_str()).filter(|name|PresetName::parse(name).is_ok()) {names.push(format!("{}{name}",original_settings::PRESET_PREFIX));}
            }
        }
    }
    names.sort();Ok(names)
}

/// Appends the Presets submenu and remembers the listed names.
pub(super) fn append_menu(menu: HMENU, app: &mut App) {
    let presets = unsafe { CreatePopupMenu() };
    let count=usize::from(PRESET_CHOICES);
    let pages=app.preset_names.len().div_ceil(count).max(1);
    app.preset_page=app.preset_page.min(pages-1);
    let names:Vec<_>=app.preset_names.iter().skip(app.preset_page*count).take(count).cloned().collect();
    if pages>1{
        append(presets,if app.preset_page==0{MF_GRAYED}else{MF_STRING},CMD_PRESET_PREVIOUS,"Previous preset page");
        append(presets,if app.preset_page+1==pages{MF_GRAYED}else{MF_STRING},CMD_PRESET_NEXT,"Next preset page");
        append(presets,MF_GRAYED,0,&format!("Page {} of {pages}",app.preset_page+1));
        unsafe{AppendMenuW(presets,MF_SEPARATOR,0,ptr::null())};
    }
    app.refresh_presets();
    if names.is_empty() {
        append(presets, MF_GRAYED, 0, if !app.presets_loaded && app.preset_scan_pending { "Loading presets..." } else { "No presets saved" });
    }
    for (index, name) in names.iter().enumerate() {
        append(presets, MF_STRING, CMD_PRESET_FIRST + index as u16, name);
    }
    unsafe { AppendMenuW(presets, MF_SEPARATOR, 0, ptr::null()) };
    append(
        presets,
        MF_STRING,
        CMD_PRESET_SAVE,
        "Save settings as preset...",
    );
    append(presets, MF_STRING, CMD_PRESET_FOLDER, "Open presets folder");
    // Keep the command mapping fixed while a menu is open. A worker may
    // update preset_names during the modal Windows menu message loop.
    app.preset_choices = names;
    unsafe { AppendMenuW(menu, MF_POPUP, presets as usize, wide("Presets").as_ptr()) };
}

/// Loads the chosen preset into the editor as unsaved settings.
pub(super) fn apply(app: &mut App, index: usize) {
    let Some(name) = app.preset_choices.get(index).cloned() else {
        return;
    };
    if let Some(name)=name.strip_prefix(original_settings::PRESET_PREFIX){original_settings::apply_preset(app,name);return;}
    let loaded = PresetName::parse(&name)
        .and_then(|name| PresetStore::user()?.load(&name))
        .map(|preset| preset.profile().clone());
    match loaded {
        Ok(profile) => {
            app.replace_profile(profile, None, true);
            app.log(Level::Info,"Presets",format!("Loaded preset {name} as an unsaved draft."));
            if app.running.is_some(){app.apply();}
        }
        Err(error) => app.log(Level::Error, "Presets", error),
    }
}

/// Saves the current settings as a preset chosen in a save dialog opened in
/// the presets folder.
pub(super) fn save(window: HWND) {
    let guard=with_app(|app|(app.edit_revision,app.metadata_generation));
    let result = (|| -> Result<Option<String>, String> {
        let store = PresetStore::user()?;
        std::fs::create_dir_all(store.directory()).map_err(|error| error.to_string())?;
        let Some(path) = dialog(window, store.directory())? else {
            return Ok(None);
        };
        if path
            .parent()
            .map(std::path::absolute)
            .transpose()
            .map_err(|e| e.to_string())?
            != Some(store.directory().to_path_buf())
        {
            return Err("Save presets in the presets folder the dialog opens.".into());
        }
        let stem = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .ok_or("invalid preset file name")?;
        let name = PresetName::parse(stem)?;
        let profile = with_app(|app|{
            if guard!=Some((app.edit_revision,app.metadata_generation))||app.closing||app.update_restart_pending{return Err("editor changed while choosing a preset file".into());}
            app.checked_profile()
        }).ok_or("the panel is closing")??;
        // The dialog already asked before replacing an existing preset.
        let previous = path.exists().then(|| store.load(&name)).transpose()?;
        store.save(&name, &profile, previous.as_ref())?;
        Ok(Some(name.as_str().to_owned()))
    })();
    with_app(|app| match result {
        Ok(Some(name)) => {
            app.log(Level::Info, "Presets", format!("Saved preset {name}."));
            app.refresh_presets();
        }
        Ok(None) => {}
        Err(error) => app.log(Level::Error, "Presets", error),
    });
}

pub(super) fn open_folder(window: HWND) {
    match PresetStore::user() {
        Ok(store) => {
            let _ = std::fs::create_dir_all(store.directory());
            shell_open(window, &store.directory().display().to_string());
        }
        Err(error) => {
            with_app(|app| app.log(Level::Error, "Presets", error));
        }
    }
}

fn dialog(window: HWND, directory: &std::path::Path) -> Result<Option<PathBuf>, String> {
    let mut buffer = vec![0u16; 32_768];
    let filter = wide("Preset (*.toml)\0*.toml\0\0");
    let extension = wide("toml");
    let title = wide("Save settings as preset");
    let initial = wide(&directory.display().to_string());
    let mut dialog = OPENFILENAMEW {
        lStructSize: size_of::<OPENFILENAMEW>() as u32,
        hwndOwner: window,
        lpstrFilter: filter.as_ptr(),
        lpstrFile: buffer.as_mut_ptr(),
        nMaxFile: buffer.len() as u32,
        lpstrDefExt: extension.as_ptr(),
        lpstrTitle: title.as_ptr(),
        lpstrInitialDir: initial.as_ptr(),
        Flags: OFN_NOCHANGEDIR | OFN_PATHMUSTEXIST | OFN_OVERWRITEPROMPT,
        ..Default::default()
    };
    if unsafe { GetSaveFileNameW(&mut dialog) } == 0 {
        let code = unsafe { CommDlgExtendedError() };
        return if code == 0 {
            Ok(None)
        } else {
            Err(format!("file dialog failed (0x{code:x})"))
        };
    }
    let length = buffer
        .iter()
        .position(|c| *c == 0)
        .ok_or("invalid file dialog path")?;
    Ok(Some(OsString::from_wide(&buffer[..length]).into()))
}
