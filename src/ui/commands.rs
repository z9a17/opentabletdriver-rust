//! Menus, dialogs and the `WM_COMMAND` dispatcher. Modal UI (menus, file
//! dialogs, message boxes) runs here, outside any `App` borrow.
use super::*;

pub(super) fn checked(value: bool) -> u32 {
    if value { MF_CHECKED } else { MF_UNCHECKED }
}

pub(super) fn append(menu: HMENU, flags: u32, id: u16, label: &str) {
    unsafe { AppendMenuW(menu, flags, id as usize, wide(label).as_ptr()) };
}

pub(super) fn copy_to_clipboard(owner: HWND, value: &str) -> bool {
    let mut data: Vec<u16> = value.encode_utf16().collect();
    data.push(0);
    unsafe {
        if OpenClipboard(owner) == 0 {
            return false;
        }
        EmptyClipboard();
        let memory = GlobalAlloc(GMEM_MOVEABLE, data.len() * 2);
        let mut copied = false;
        if !memory.is_null() {
            let target = GlobalLock(memory).cast::<u16>();
            if !target.is_null() {
                ptr::copy_nonoverlapping(data.as_ptr(), target, data.len());
                GlobalUnlock(memory);
                copied = !SetClipboardData(CF_UNICODETEXT, memory).is_null();
            }
            if !copied {
                GlobalFree(memory);
            }
        }
        CloseClipboard();
        copied
    }
}

pub(super) fn accelerator_table() -> HACCEL {
    let key = |c: char| c as u16;
    let control = FVIRTKEY | FCONTROL;
    let entries = [
        (control, key('O'), CMD_LOAD),
        (control, key('S'), CMD_SAVE),
        (control | FSHIFT, key('S'), CMD_SAVE_AS),
        (control, VK_RETURN, CMD_APPLY),
        (control, key('Q'), CMD_QUIT),
        (control, key('D'), CMD_DETECT),
        (FVIRTKEY, VK_F1, CMD_ABOUT),
        (control, VK_TAB, CMD_NEXT_TAB),
        (control | FSHIFT, VK_TAB, CMD_PREV_TAB),
        (control, VK_NEXT, CMD_NEXT_TAB),
        (control, VK_PRIOR, CMD_PREV_TAB),
    ];
    let table: Vec<ACCEL> = entries
        .iter()
        .map(|(flags, key, cmd)| ACCEL {
            fVirt: *flags,
            key: *key,
            cmd: *cmd,
        })
        .collect();
    unsafe { CreateAcceleratorTableW(table.as_ptr(), table.len() as i32) }
}

pub(super) fn file_dialog(
    window: HWND,
    save: bool,
    dll: bool,
    title: &str,
) -> Result<Option<PathBuf>, String> {
    let mut buffer = vec![0u16; 32_768];
    let filter = if dll {
        wide("Plugin DLL (*.dll)\0*.dll\0\0")
    } else {
        wide("Rust profile (*.toml)\0*.toml\0All files\0*.*\0\0")
    };
    let extension = wide(if dll { "dll" } else { "toml" });
    let title = wide(title);
    let mut dialog = OPENFILENAMEW {
        lStructSize: size_of::<OPENFILENAMEW>() as u32,
        hwndOwner: window,
        lpstrFilter: filter.as_ptr(),
        lpstrFile: buffer.as_mut_ptr(),
        nMaxFile: buffer.len() as u32,
        lpstrDefExt: extension.as_ptr(),
        lpstrTitle: title.as_ptr(),
        Flags: OFN_NOCHANGEDIR
            | OFN_PATHMUSTEXIST
            | if save {
                0 // Save As creates a new file; storage rejects existing paths.
            } else {
                OFN_FILEMUSTEXIST
            },
        ..Default::default()
    };
    let success = unsafe {
        if save {
            GetSaveFileNameW(&mut dialog)
        } else {
            GetOpenFileNameW(&mut dialog)
        }
    };
    if success == 0 {
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

pub(super) fn message_box(
    window: HWND,
    text: &str,
    caption: &str,
    flags: MESSAGEBOX_STYLE,
) -> MESSAGEBOX_RESULT {
    unsafe { MessageBoxW(window, wide(text).as_ptr(), wide(caption).as_ptr(), flags) }
}

pub(super) fn confirm_discard(window: HWND) -> bool {
    !with_app(|app| app.dirty).unwrap_or(false)
        || message_box(
            window,
            "Discard unsaved profile edits?",
            "Unsaved profile",
            MB_YESNO | MB_ICONQUESTION,
        ) == IDYES
}

fn offer_backup_recovery(window: HWND, primary: PathBuf) {
    let backup = match otd_core::storage::backup_path(&primary) {
        Ok(backup) => backup,
        Err(error) => {
            with_app(|app| app.log(Level::Error, "Settings", error));
            return;
        }
    };
    if message_box(
        window,
        &format!(
            "Load the backup {} into the editor?\n\nThe original and backup will remain unchanged. Recovered settings will be unsaved and must be saved with a new file name.",
            backup.display()
        ),
        "Recover profile backup",
        MB_YESNO | MB_ICONQUESTION,
    ) == IDYES
    {
        with_app(|app| app.recover_profile_backup(primary));
    }
}

pub(super) fn shell_open(window: HWND, target: &str) {
    unsafe {
        ShellExecuteW(
            window,
            wide("open").as_ptr(),
            wide(target).as_ptr(),
            ptr::null(),
            ptr::null(),
            SW_SHOWNORMAL,
        );
    }
}

/// Shows a popup menu below `anchor` and returns the chosen command.
pub(super) fn popup(window: HWND, menu: HMENU, anchor: HWND) -> u16 {
    let mut r = RECT::default();
    unsafe { GetWindowRect(anchor, &mut r) };
    update_look(|look| look.menu_open = anchor as isize);
    unsafe { InvalidateRect(anchor, ptr::null(), 0) };
    let params = TPMPARAMS {
        cbSize: size_of::<TPMPARAMS>() as u32,
        rcExclude: r,
    };
    let command = unsafe {
        TrackPopupMenuEx(
            menu,
            TPM_LEFTALIGN | TPM_TOPALIGN | TPM_RETURNCMD | TPM_VERTICAL,
            r.left,
            r.bottom,
            window,
            &params,
        )
    };
    update_look(|look| look.menu_open = 0);
    unsafe {
        InvalidateRect(anchor, ptr::null(), 0);
        DestroyMenu(menu);
    }
    command as u16
}

pub(super) fn menu_bar_popup(window: HWND, index: usize) {
    let Some((menu, anchor)) = with_app(|app| {
        let menu = unsafe { CreatePopupMenu() };
        let running = app.running.is_some();
        match index {
            0 => {
                append(menu, MF_STRING, CMD_LOAD, "Load settings...\tCtrl+O");
                append(
                    menu,
                    MF_STRING,
                    CMD_RECOVER_BACKUP,
                    "Recover backup as unsaved settings...",
                );
                append(menu, MF_STRING, CMD_SAVE, "Save settings\tCtrl+S");
                append(
                    menu,
                    MF_STRING,
                    CMD_SAVE_AS,
                    "Save settings as...\tCtrl+Shift+S",
                );
                append(menu, MF_STRING, CMD_RESET, "Reset to defaults");
                append(
                    menu,
                    if running { MF_STRING } else { MF_GRAYED },
                    CMD_APPLY,
                    "Apply settings\tCtrl+Enter",
                );
                unsafe { AppendMenuW(menu, MF_SEPARATOR, 0, ptr::null()) };
                append(
                    menu,
                    MF_STRING,
                    CMD_IMPORT,
                    "Import OpenTabletDriver settings",
                );
                append(
                    menu,
                    MF_STRING,
                    CMD_OPEN_FOLDER,
                    "Open settings directory...",
                );
                presets::append_menu(menu, app);
                append(menu, MF_STRING, CMD_SAVE_LOG, "Save console log...");
                unsafe { AppendMenuW(menu, MF_SEPARATOR, 0, ptr::null()) };
                append(menu, MF_STRING, CMD_QUIT, "Quit\tCtrl+Q");
            }
            1 => {
                append(menu, MF_STRING, CMD_DETECT, "Detect tablet\tCtrl+D");
                append(menu, MF_STRING, CMD_DEBUGGER, "Tablet debugger...");
                // The profile's tablet: whichever is connected, a connected
                // tablet, or the one it already names.
                let target = app.editor.profile.target_tablet.clone();
                let mut choices = crate::hid::connected_tablets();
                if let Some(name) = &target
                    && !choices.contains(name)
                {
                    choices.push(name.clone());
                }
                choices.truncate(usize::from(TABLET_CHOICES));
                let tablets = unsafe { CreatePopupMenu() };
                append(
                    tablets,
                    checked(target.is_none()),
                    CMD_TABLET_ANY,
                    "Any connected tablet",
                );
                for (index, name) in choices.iter().enumerate() {
                    append(
                        tablets,
                        checked(target.as_ref() == Some(name)),
                        CMD_TABLET_FIRST + index as u16,
                        name,
                    );
                }
                app.tablet_choices = choices;
                unsafe {
                    AppendMenuW(
                        menu,
                        MF_POPUP,
                        tablets as usize,
                        wide("Settings for tablet").as_ptr(),
                    )
                };
                append(
                    menu,
                    if app.editor.mode() == OutputMode::Absolute {
                        MF_STRING
                    } else {
                        MF_GRAYED
                    },
                    AREA_CONVERT,
                    "Convert tablet area from...",
                );
                append(
                    menu,
                    MF_STRING,
                    CMD_START_STOP,
                    if running {
                        "Stop driver"
                    } else {
                        "Start driver"
                    },
                );
                unsafe { AppendMenuW(menu, MF_SEPARATOR, 0, ptr::null()) };
                append(
                    menu,
                    checked(app.prefs.start_driver_on_launch),
                    CMD_AUTOSTART,
                    "Start driver when the panel opens",
                );
                append(
                    menu,
                    checked(startup::enabled()),
                    CMD_START_WITH_WINDOWS,
                    "Start with Windows (in the tray)",
                );
            }
            2 => {
                append(menu, MF_STRING, CMD_ADD_DOTNET, "Add .NET plugin...");
                append(menu, MF_STRING, CMD_ADD_NATIVE, "Add native plugin...");
                unsafe { AppendMenuW(menu, MF_SEPARATOR, 0, ptr::null()) };
                append(menu, MF_STRING, CMD_PLUGIN_MANAGER, "Plugin manager...");
            }
            3 => {
                let theme = unsafe { CreatePopupMenu() };
                let mode = app.prefs.theme;
                append(
                    theme,
                    checked(mode == ThemeMode::System),
                    CMD_THEME_SYSTEM,
                    "Use system setting",
                );
                append(
                    theme,
                    checked(mode == ThemeMode::Light),
                    CMD_THEME_LIGHT,
                    "Light",
                );
                append(
                    theme,
                    checked(mode == ThemeMode::Dark),
                    CMD_THEME_DARK,
                    "Dark",
                );
                unsafe { AppendMenuW(menu, MF_POPUP, theme as usize, wide("Theme").as_ptr()) };
                append(menu, MF_STRING, CMD_NEXT_TAB, "Next tab\tCtrl+Tab");
                append(
                    menu,
                    MF_STRING,
                    CMD_PREV_TAB,
                    "Previous tab\tCtrl+Shift+Tab",
                );
            }
            _ => {
                append(menu, MF_STRING, CMD_DOCS, "Open documentation...");
                append(menu, MF_STRING, CMD_CHECK_UPDATES, "Check for updates...");
                append(
                    menu,
                    checked(app.prefs.check_for_updates),
                    CMD_UPDATE_ON_OPEN,
                    "Check for updates when the panel opens",
                );
                append(menu, MF_STRING, CMD_ABOUT, "About...\tF1");
            }
        }
        (menu, app.c.menus[index])
    }) else {
        return;
    };
    let command = popup(window, menu, anchor);
    if command != 0 {
        on_command(window, command, 0, ptr::null_mut());
    }
}

pub(super) fn about(window: HWND) {
    let text = format!(
        "OpenTabletDriver Rust {}\n\nA Windows USB tablet driver written in Rust, using OpenTabletDriver's tablet configurations. This control panel follows the layout of OpenTabletDriver's UX.\n\nLicensed under GPL-3.0-only. The built-in Radial Follow filter is a Rust port of AbstractQbit's RadialFollow 0.3.0.",
        env!("CARGO_PKG_VERSION")
    );
    message_box(
        window,
        &text,
        "About OpenTabletDriver Rust",
        MB_OK | MB_ICONINFORMATION,
    );
}

pub(super) fn on_command(window: HWND, id: u16, code: u32, control: HWND) {
    if !control.is_null() {
        match code {
            EN_CHANGE => {
                if QUIET.with(Cell::get) == 0 {
                    with_app(|app| app.field_changed(control));
                }
                return;
            }
            EN_SETFOCUS | EN_KILLFOCUS => {
                with_app(|app| {
                    app.invalidate_frame(control);
                    if code == EN_KILLFOCUS {
                        app.field_committed(control);
                    }
                });
                return;
            }
            LBN_SELCHANGE if id == ID_FILTER_LIST => {
                with_app(App::select_filter);
                return;
            }
            BN_CLICKED => {}
            _ => return,
        }
    }
    match id {
        // Enter in a field: commit it like leaving the field.
        1 => {
            let focus = unsafe { GetFocus() };
            with_app(|app| app.field_committed(focus));
        }
        id if (ID_MENU..ID_MENU + MENUS.len() as u16).contains(&id) => {
            menu_bar_popup(window, (id - ID_MENU) as usize);
        }
        id if (ID_TAB..ID_TAB + TABS.len() as u16).contains(&id) => {
            with_app(|app| app.select_tab(TABS[(id - ID_TAB) as usize].0));
        }
        CMD_NEXT_TAB => {
            with_app(|app| app.cycle_tab(1));
        }
        CMD_PREV_TAB => {
            with_app(|app| app.cycle_tab(-1));
        }
        CMD_LOAD => {
            if confirm_discard(window) {
                match file_dialog(window, false, false, "Load settings") {
                    Ok(Some(path)) => {
                        let loaded = with_app(|app| app.load_file(path.clone())).unwrap_or(false);
                        if !loaded
                            && otd_core::storage::backup_path(&path)
                                .is_ok_and(|backup| backup.is_file())
                        {
                            offer_backup_recovery(window, path);
                        }
                    }
                    Ok(None) => {}
                    Err(error) => {
                        with_app(|app| app.log(Level::Error, "UI", error));
                    }
                }
            }
        }
        CMD_RECOVER_BACKUP => {
            if confirm_discard(window)
                && let Some(primary) = with_app(|app| app.profile_path.clone())
            {
                offer_backup_recovery(window, primary);
            }
        }
        CMD_SAVE | CMD_SAVE_AS => {
            let save_as =
                id == CMD_SAVE_AS || with_app(|app| app.recovered_backup).unwrap_or(false);
            if save_as {
                match file_dialog(window, true, false, "Save settings as a new file") {
                    Ok(Some(path)) => {
                        with_app(|app| app.save_as_to(path));
                    }
                    Ok(None) => {}
                    Err(error) => {
                        with_app(|app| app.log(Level::Error, "UI", error));
                    }
                }
            } else {
                with_app(|app| app.save_to(app.profile_path.clone()));
            }
        }
        CMD_RESET => {
            if message_box(
                window,
                "Reset settings to default?",
                "Reset to defaults",
                MB_OKCANCEL | MB_ICONQUESTION,
            ) == IDOK
            {
                with_app(|app| {
                    app.replace_profile(Profile::default(), None, true);
                    app.log(
                        Level::Info,
                        "Settings",
                        "Settings were reset to the built-in full-area defaults.",
                    );
                });
            }
        }
        CMD_APPLY => {
            with_app(App::apply);
        }
        CMD_IMPORT => {
            if confirm_discard(window) {
                with_app(App::import_otd);
            }
        }
        CMD_OPEN_FOLDER => {
            if let Some(directory) =
                with_app(|app| app.profile_path.parent().map(Path::to_path_buf)).flatten()
            {
                let _ = std::fs::create_dir_all(&directory);
                shell_open(window, &directory.to_string_lossy());
            }
        }
        CMD_QUIT => unsafe {
            PostMessageW(window, WM_CLOSE, 0, 0);
        },
        CMD_DETECT => {
            with_app(App::detect_tablet);
        }
        CMD_TABLET_ANY => {
            with_app(|app| app.choose_tablet(None));
        }
        id if (CMD_TABLET_FIRST..CMD_TABLET_FIRST + TABLET_CHOICES).contains(&id) => {
            with_app(|app| {
                let name = app
                    .tablet_choices
                    .get(usize::from(id - CMD_TABLET_FIRST))
                    .cloned();
                if name.is_some() {
                    app.choose_tablet(name);
                }
            });
        }
        CMD_DEBUGGER => {
            if let Err(error) = debugger::open() {
                with_app(|app| {
                    app.log(
                        Level::Error,
                        "Tablet",
                        format!("Cannot open the tablet debugger: {error}"),
                    )
                });
            }
        }
        CMD_ADD_DOTNET | CMD_ADD_NATIVE => {
            if !with_app(App::can_leave_filter).unwrap_or(false) {
                return;
            }
            let dotnet = id == CMD_ADD_DOTNET;
            let title = if dotnet {
                "Add .NET plugin"
            } else {
                "Add native plugin"
            };
            match file_dialog(window, false, true, title) {
                Ok(Some(path)) => {
                    with_app(|app| app.add_plugin(path, dotnet));
                }
                Ok(None) => {}
                Err(error) => {
                    with_app(|app| app.log(Level::Error, "UI", error));
                }
            }
        }
        CMD_PLUGIN_MANAGER => {
            if let Err(error) = plugin_manager::open() {
                with_app(|app| app.log(Level::Error, "Plugins", error));
            }
        }
        CMD_REMOVE_FILTER => {
            with_app(App::remove_filter);
        }
        CMD_FILTER_UP | CMD_FILTER_DOWN => {
            with_app(|app| app.move_filter(id == CMD_FILTER_DOWN));
        }
        CMD_FILTER_DEFAULTS => {
            with_app(App::reset_filter);
        }
        CMD_FILTER_JSON => {
            with_app(App::toggle_filter_json);
        }
        CMD_PROPERTY_PREV | CMD_PROPERTY_NEXT => {
            with_app(|app| {
                app.property_page = if id == CMD_PROPERTY_NEXT {
                    app.property_page.saturating_add(1)
                } else {
                    app.property_page.saturating_sub(1)
                };
                app.layout();
            });
        }
        CMD_THEME_SYSTEM | CMD_THEME_LIGHT | CMD_THEME_DARK => {
            let mode = match id {
                CMD_THEME_LIGHT => ThemeMode::Light,
                CMD_THEME_DARK => ThemeMode::Dark,
                _ => ThemeMode::System,
            };
            with_app(|app| app.set_theme(mode));
        }
        CMD_DOCS => shell_open(window, DOCS_URL),
        CMD_ABOUT => about(window),
        CMD_START_STOP => {
            with_app(|app| {
                if app.running.is_some() {
                    // The daemon cancels any pending restart when Stop is accepted.
                    app.stop();
                } else {
                    app.start();
                }
            });
        }
        CMD_AUTOSTART => {
            with_app(|app| {
                app.prefs.start_driver_on_launch = !app.prefs.start_driver_on_launch;
                app.save_prefs();
            });
        }
        CMD_CHECK_UPDATES => updates::check(window, true),
        presets::CMD_PRESET_SAVE => presets::save(window),
        presets::CMD_PRESET_FOLDER => presets::open_folder(window),
        id if (presets::CMD_PRESET_FIRST..presets::CMD_PRESET_FIRST + presets::PRESET_CHOICES)
            .contains(&id) =>
        {
            let index = usize::from(id - presets::CMD_PRESET_FIRST);
            with_app(|app| presets::apply(app, index));
        }
        CMD_START_WITH_WINDOWS => {
            let enable = !startup::enabled();
            let result = startup::set(enable);
            with_app(|app| match result {
                Ok(()) => app.log(
                    Level::Info,
                    "UI",
                    if enable {
                        "The panel will start in the tray when you sign in to Windows."
                    } else {
                        "The panel no longer starts with Windows."
                    },
                ),
                Err(error) => app.log(
                    Level::Error,
                    "UI",
                    format!("Cannot change startup: {error}"),
                ),
            });
        }
        CMD_UPDATE_ON_OPEN => {
            with_app(|app| {
                app.prefs.check_for_updates = !app.prefs.check_for_updates;
                app.save_prefs();
            });
        }
        CMD_SHOW => tray::show_panel(window),
        CMD_COPY_LOG => {
            with_app(|app| app.copy_log(true));
        }
        CMD_SAVE_LOG => {
            match text_save_dialog(window, "Save console log", "opentabletdriver-rust-log.txt") {
                Ok(Some(path)) => with_app(|app| {
                    let text = app.log_text();
                    match std::fs::write(&path, text) {
                        Ok(()) => app.log(
                            Level::Info,
                            "UI",
                            format!("Saved the log to {}.", path.display()),
                        ),
                        Err(error) => {
                            app.log(Level::Error, "UI", format!("Cannot save the log: {error}"))
                        }
                    }
                })
                .unwrap_or(()),
                Ok(None) => {}
                Err(error) => {
                    with_app(|app| app.log(Level::Error, "UI", error));
                }
            }
        }
        CMD_CLEAR_LOG => {
            with_app(App::clear_log);
        }
        ID_MODE => {
            let mode = with_app(|app| app.editor.mode());
            let menu = unsafe { CreatePopupMenu() };
            append(
                menu,
                MFT_RADIOCHECK | checked(mode == Some(OutputMode::Absolute)),
                1,
                "Absolute Mode",
            );
            append(
                menu,
                MFT_RADIOCHECK | checked(mode == Some(OutputMode::Relative)),
                2,
                "Relative Mode",
            );
            match popup(window, menu, control) {
                1 => {
                    with_app(|app| app.set_output_mode(OutputMode::Absolute));
                }
                2 => {
                    with_app(|app| app.set_output_mode(OutputMode::Relative));
                }
                _ => {}
            }
        }
        ID_TIP_BINDING | ID_ERASER_BINDING => {
            let eraser = id == ID_ERASER_BINDING;
            let enabled = with_app(|app| app.editor.binding_enabled(eraser)).unwrap_or(true);
            let menu = unsafe { CreatePopupMenu() };
            append(
                menu,
                MFT_RADIOCHECK | checked(enabled),
                1,
                if eraser { "Eraser" } else { "Tip" },
            );
            append(menu, MFT_RADIOCHECK | checked(!enabled), 2, "None");
            match popup(window, menu, control) {
                1 => {
                    with_app(|app| app.set_binding(eraser, true));
                }
                2 => {
                    with_app(|app| app.set_binding(eraser, false));
                }
                _ => {}
            }
        }
        ID_FILTER_ENABLE => {
            with_app(App::filter_toggled);
        }
        id if (ID_PROPERTY_DEFAULT..ID_PROPERTY_DEFAULT + MAX_PROPERTY_ROWS).contains(&id) => {
            with_app(|app| {
                if let Some(hwnd) = app
                    .properties
                    .iter()
                    .find(|row| row.default_control == Some(control))
                    .map(|row| row.hwnd)
                {
                    app.choose_property(hwnd, serde_json::Value::Null);
                }
            });
        }
        id if (ID_PROPERTY..ID_PROPERTY + MAX_PROPERTY_ROWS).contains(&id) => {
            let choices = with_app(|app| {
                let row = app.properties.iter().find(|row| row.hwnd == control)?;
                let PropertyTarget::Plugin(_, value) = &row.target else {
                    return None;
                };
                let choices = value.choices();
                if choices.is_empty() || !value.writable() {
                    return None;
                }
                let menu = unsafe { CreatePopupMenu() };
                for (index, (label, choice)) in choices.iter().enumerate() {
                    append(
                        menu,
                        MFT_RADIOCHECK | checked(value.choice_selected(choice)),
                        index as u16 + 1,
                        label,
                    );
                }
                Some((menu, choices))
            })
            .flatten();
            if let Some((menu, choices)) = choices {
                let command = popup(window, menu, control);
                if command > 0
                    && let Some((_, value)) = choices.get(command as usize - 1)
                {
                    with_app(|app| app.choose_property(control, value.clone()));
                }
            } else {
                with_app(|app| app.property_toggled(control));
            }
        }
        AREA_CONVERT => conversion::open(window),
        id if (AREA_ALIGN..=AREA_DISPLAY + 32).contains(&id) => {
            with_app(|app| app.area_action(id));
        }
        _ => {}
    }
}

/// A save dialog for a text file, asking before replacing one.
pub(super) fn text_save_dialog(
    window: HWND,
    title: &str,
    name: &str,
) -> Result<Option<PathBuf>, String> {
    let mut buffer = vec![0u16; 32_768];
    for (slot, unit) in buffer.iter_mut().zip(name.encode_utf16()) {
        *slot = unit;
    }
    let filter = wide("Text file (*.txt)\0*.txt\0All files\0*.*\0\0");
    let extension = wide("txt");
    let title = wide(title);
    let mut dialog = OPENFILENAMEW {
        lStructSize: size_of::<OPENFILENAMEW>() as u32,
        hwndOwner: window,
        lpstrFilter: filter.as_ptr(),
        lpstrFile: buffer.as_mut_ptr(),
        nMaxFile: buffer.len() as u32,
        lpstrDefExt: extension.as_ptr(),
        lpstrTitle: title.as_ptr(),
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
