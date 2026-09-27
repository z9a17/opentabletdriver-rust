//! Plugins > Plugin manager: OpenTabletDriver's plugin catalog with install,
//! update, remove and "add to settings", like its plugin manager window.
//! Downloads and file work run on a background thread.
use super::commands::shell_open;
use super::*;
use crate::plugin_catalog::{self, PluginMetadata};
use std::sync::mpsc::{self, Receiver, Sender};
use windows_sys::Win32::UI::Controls::{
    LVCF_TEXT, LVCF_WIDTH, LVCOLUMNW, LVIF_TEXT, LVIS_SELECTED, LVITEMW, LVM_DELETEALLITEMS,
    LVM_GETNEXTITEM, LVM_INSERTCOLUMNW, LVM_INSERTITEMW, LVM_SETEXTENDEDLISTVIEWSTYLE,
    LVM_SETITEMTEXTW, LVN_ITEMCHANGED, LVNI_SELECTED, LVS_EX_DOUBLEBUFFER, LVS_EX_FULLROWSELECT,
    LVS_REPORT, LVS_SHOWSELALWAYS, LVS_SINGLESEL, NMHDR,
};

const CLASS: &str = "OpenTabletDriverRustPluginManager";
const WM_PLUGINS: u32 = WM_APP + 22;
const LIST: u16 = 100;
const INSTALL: u16 = 101;
const REMOVE: u16 = 102;
const ADD: u16 = 103;
const PAGE: u16 = 104;
const REFRESH: u16 = 105;
const FROM_FILE: u16 = 106;
const BUTTONS: [(u16, &str); 6] = [
    (INSTALL, "&Install"),
    (REMOVE, "&Remove"),
    (ADD, "&Add to settings"),
    (PAGE, "Open &page"),
    (REFRESH, "Re&fresh"),
    (FROM_FILE, "From fi&le..."),
];

#[derive(Clone)]
struct Row {
    plugin: PluginMetadata,
    /// Folder and version of the installed copy.
    installed: Option<(PathBuf, String)>,
    /// Whether the catalog lists it (installed plugins may not be listed).
    listed: bool,
}

enum Done {
    Catalog(Result<Vec<PluginMetadata>, String>),
    Installed(Result<(String, PathBuf), String>),
    Removed(Result<String, String>),
}

struct Manager {
    window: HWND,
    list: HWND,
    details: HWND,
    status: HWND,
    buttons: Vec<HWND>,
    rows: Vec<Row>,
    catalog: Vec<PluginMetadata>,
    busy: bool,
    sender: Sender<Done>,
    results: Receiver<Done>,
    _fonts: FontSet,
}

thread_local! {
    static MANAGER: RefCell<Option<Manager>> = const { RefCell::new(None) };
}

pub(super) fn open() -> Result<(), String> {
    if let Some(window) = MANAGER.with(|slot| slot.borrow().as_ref().map(|m| m.window)) {
        unsafe {
            ShowWindow(window, SW_RESTORE);
            SetForegroundWindow(window);
        }
        return Ok(());
    }
    let instance = unsafe { GetModuleHandleW(ptr::null()) };
    let class = wide(CLASS);
    let registration = WNDCLASSW {
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(window_proc),
        hInstance: instance,
        lpszClassName: class.as_ptr(),
        hCursor: unsafe { LoadCursorW(ptr::null_mut(), IDC_ARROW) },
        hbrBackground: unsafe { GetSysColorBrush(COLOR_BTNFACE) },
        ..Default::default()
    };
    unsafe { RegisterClassW(&registration) };
    let window = unsafe {
        CreateWindowExW(
            WS_EX_CONTROLPARENT,
            class.as_ptr(),
            wide("Plugin manager").as_ptr(),
            WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            900,
            600,
            ptr::null_mut(),
            ptr::null_mut(),
            instance,
            ptr::null(),
        )
    };
    if window.is_null() {
        return Err(std::io::Error::last_os_error().to_string());
    }
    let dpi = unsafe { GetDpiForWindow(window) }.max(96);
    let fonts = FontSet::new(dpi);
    let child = |class: &str, text: &str, id: u16, style: u32| unsafe {
        let control = CreateWindowExW(
            0,
            wide(class).as_ptr(),
            wide(text).as_ptr(),
            WS_CHILD | WS_VISIBLE | WS_TABSTOP | style,
            0,
            0,
            0,
            0,
            window,
            id as usize as _,
            instance,
            ptr::null(),
        );
        SendMessageW(control, WM_SETFONT, fonts.fonts.ui as usize, 1);
        control
    };
    let list = child(
        "SysListView32",
        "",
        LIST,
        WS_BORDER | LVS_REPORT | LVS_SINGLESEL | LVS_SHOWSELALWAYS,
    );
    unsafe {
        SendMessageW(
            list,
            LVM_SETEXTENDEDLISTVIEWSTYLE,
            0,
            (LVS_EX_FULLROWSELECT | LVS_EX_DOUBLEBUFFER) as isize,
        )
    };
    for (index, (title, width)) in [
        ("Name", 260),
        ("Version", 90),
        ("Installed", 90),
        ("Author", 150),
        ("License", 110),
    ]
    .iter()
    .enumerate()
    {
        let text = wide(title);
        let column = LVCOLUMNW {
            mask: LVCF_TEXT | LVCF_WIDTH,
            cx: scale(*width, dpi),
            pszText: text.as_ptr() as *mut u16,
            ..Default::default()
        };
        unsafe { SendMessageW(list, LVM_INSERTCOLUMNW, index, &column as *const _ as isize) };
    }
    let details = child(
        "EDIT",
        "",
        0,
        WS_BORDER | WS_VSCROLL | (ES_MULTILINE | ES_READONLY | ES_AUTOVSCROLL) as u32,
    );
    let status = child("STATIC", "Loading the plugin catalog...", 0, SS_LEFT);
    let buttons = BUTTONS
        .iter()
        .map(|(id, text)| child("BUTTON", text, *id, BS_PUSHBUTTON as u32))
        .collect();
    if let Some(dark) = with_look(|look| look.style.palette.dark) {
        let theme = theme::DarkMode::load();
        theme.apply_title_bar(window, dark);
        theme.apply_control(list, dark);
    }
    let (sender, results) = mpsc::channel();
    MANAGER.with(|slot| {
        *slot.borrow_mut() = Some(Manager {
            window,
            list,
            details,
            status,
            buttons,
            rows: Vec::new(),
            catalog: Vec::new(),
            busy: false,
            sender,
            results,
            _fonts: fonts,
        })
    });
    with_manager(|manager| {
        manager.layout();
        manager.refresh_catalog();
    });
    unsafe {
        ShowWindow(window, SW_SHOW);
        UpdateWindow(window);
    }
    Ok(())
}

fn with_manager<R>(f: impl FnOnce(&mut Manager) -> R) -> Option<R> {
    MANAGER.with(|slot| {
        let mut slot = slot.try_borrow_mut().ok()?;
        slot.as_mut().map(f)
    })
}

impl Manager {
    fn layout(&self) {
        let client = client_rect(self.window);
        let dpi = unsafe { GetDpiForWindow(self.window) }.max(96);
        let s = |value: i32| scale(value, dpi);
        let (width, height) = (client.right, client.bottom);
        let place = |control: HWND, x: i32, y: i32, w: i32, h: i32| unsafe {
            SetWindowPos(
                control,
                ptr::null_mut(),
                x,
                y,
                w.max(0),
                h.max(0),
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
        };
        let button_height = s(28);
        let bottom = height - s(10) - button_height;
        let details_height = s(110);
        place(
            self.list,
            s(10),
            s(10),
            width - s(20),
            bottom - details_height - s(30),
        );
        place(
            self.details,
            s(10),
            bottom - details_height - s(12),
            width - s(20),
            details_height,
        );
        let mut x = s(10);
        for button in &self.buttons {
            place(*button, x, bottom, s(118), button_height);
            x += s(124);
        }
        place(
            self.status,
            x + s(8),
            bottom + s(6),
            width - x - s(18),
            button_height,
        );
    }

    fn set_status(&self, text: &str) {
        unsafe { SetWindowTextW(self.status, wide(text).as_ptr()) };
    }

    fn start(&mut self, label: &str, work: impl FnOnce() -> Done + Send + 'static) {
        if self.busy {
            return;
        }
        self.busy = true;
        self.set_status(label);
        self.update_buttons();
        let (sender, window) = (self.sender.clone(), self.window as isize);
        std::thread::spawn(move || {
            let _ = sender.send(work());
            unsafe { PostMessageW(window as HWND, WM_PLUGINS, 0, 0) };
        });
    }

    fn refresh_catalog(&mut self) {
        self.start("Loading the plugin catalog...", || {
            Done::Catalog(plugin_catalog::fetch())
        });
    }

    /// Rebuilds the rows from the catalog and the installed plugins.
    fn rebuild(&mut self) {
        let installed = plugin_catalog::installed();
        let mut rows: Vec<Row> = self
            .catalog
            .iter()
            .map(|plugin| Row {
                installed: installed
                    .iter()
                    .find(|(_, local)| local.name == plugin.name && local.owner == plugin.owner)
                    .map(|(folder, local)| (folder.clone(), local.plugin_version.clone())),
                plugin: plugin.clone(),
                listed: true,
            })
            .collect();
        for (folder, local) in installed {
            if !rows
                .iter()
                .any(|row| row.plugin.name == local.name && row.plugin.owner == local.owner)
            {
                rows.push(Row {
                    installed: Some((folder, local.plugin_version.clone())),
                    plugin: local,
                    listed: false,
                });
            }
        }
        let selected = self
            .selected()
            .map(|row| (row.plugin.name.clone(), row.plugin.owner.clone()));
        self.rows = rows;
        unsafe { SendMessageW(self.list, LVM_DELETEALLITEMS, 0, 0) };
        for (index, row) in self.rows.iter().enumerate() {
            let cells = [
                row.plugin.name.clone(),
                if row.listed {
                    row.plugin.plugin_version.clone()
                } else {
                    "not listed".into()
                },
                row.installed
                    .as_ref()
                    .map_or(String::new(), |(_, version)| version.clone()),
                row.plugin.owner.clone(),
                row.plugin.license_identifier.clone().unwrap_or_default(),
            ];
            for (column, text) in cells.iter().enumerate() {
                let text = wide(text);
                let mut item = LVITEMW {
                    mask: LVIF_TEXT,
                    iItem: index as i32,
                    iSubItem: column as i32,
                    pszText: text.as_ptr() as *mut u16,
                    ..Default::default()
                };
                let message = if column == 0 {
                    LVM_INSERTITEMW
                } else {
                    LVM_SETITEMTEXTW
                };
                unsafe { SendMessageW(self.list, message, index, &mut item as *mut _ as isize) };
            }
            if selected
                .as_ref()
                .is_some_and(|(name, owner)| *name == row.plugin.name && *owner == row.plugin.owner)
            {
                let mut item = LVITEMW {
                    stateMask: LVIS_SELECTED,
                    state: LVIS_SELECTED,
                    ..Default::default()
                };
                unsafe {
                    SendMessageW(
                        self.list,
                        windows_sys::Win32::UI::Controls::LVM_SETITEMSTATE,
                        index,
                        &mut item as *mut _ as isize,
                    )
                };
            }
        }
        self.selection_changed();
    }

    fn selected_index(&self) -> Option<usize> {
        let index = unsafe {
            SendMessageW(
                self.list,
                LVM_GETNEXTITEM,
                usize::MAX,
                LVNI_SELECTED as isize,
            )
        };
        usize::try_from(index)
            .ok()
            .filter(|index| *index < self.rows.len())
    }

    fn selected(&self) -> Option<&Row> {
        self.selected_index().and_then(|index| self.rows.get(index))
    }

    fn selection_changed(&self) {
        let text = self.selected().map_or(String::new(), |row| {
            let plugin = &row.plugin;
            let mut text = format!(
                "{} {} by {}\r\n\r\n",
                plugin.name, plugin.plugin_version, plugin.owner
            );
            text.push_str(&plugin.description.replace('\n', "\r\n"));
            if let Some(url) = &plugin.repository_url {
                text.push_str(&format!("\r\n\r\nSource: {url}"));
            }
            if let Some((folder, version)) = &row.installed {
                text.push_str(&format!("\r\nInstalled {version} in {}", folder.display()));
            }
            text
        });
        unsafe { SetWindowTextW(self.details, wide(&text).as_ptr()) };
        self.update_buttons();
    }

    fn update_buttons(&self) {
        let row = self.selected();
        let install_label = match row {
            Some(row)
                if row.installed.as_ref().is_some_and(|(_, version)| {
                    plugin_catalog::version(version)
                        < plugin_catalog::version(&row.plugin.plugin_version)
                }) =>
            {
                "&Update"
            }
            Some(row) if row.installed.is_some() => "Re&install",
            _ => "&Install",
        };
        let enabled = |id: u16| match (id, row) {
            (REFRESH | FROM_FILE, _) => !self.busy,
            (_, None) => false,
            (INSTALL, Some(row)) => !self.busy && row.listed && row.plugin.download_url.is_some(),
            (REMOVE | ADD, Some(row)) => !self.busy && row.installed.is_some(),
            (PAGE, Some(row)) => {
                row.plugin.repository_url.is_some() || row.plugin.wiki_url.is_some()
            }
            _ => false,
        };
        for (button, (id, _)) in self.buttons.iter().zip(BUTTONS) {
            unsafe { EnableWindow(*button, i32::from(enabled(id))) };
            if id == INSTALL {
                unsafe { SetWindowTextW(*button, wide(install_label).as_ptr()) };
            }
        }
    }

    fn command(&mut self, id: u16) {
        if id == FROM_FILE {
            self.install_from_file();
            return;
        }
        let Some(row) = self.selected().cloned() else {
            if id == REFRESH {
                self.refresh_catalog();
            }
            return;
        };
        match id {
            REFRESH => self.refresh_catalog(),
            INSTALL => {
                let answer = super::commands::message_box(
                    self.window,
                    &format!(
                        "Install {} {} by {}?\n\nPlugins run inside the driver with your permissions. Install only plugins you trust.",
                        row.plugin.name, row.plugin.plugin_version, row.plugin.owner
                    ),
                    "Install plugin",
                    MB_YESNO | MB_ICONQUESTION,
                );
                if answer == IDYES {
                    let plugin = row.plugin.clone();
                    self.start(&format!("Installing {}...", plugin.name), move || {
                        Done::Installed(
                            plugin_catalog::install(&plugin)
                                .map(|folder| (plugin.name.clone(), folder)),
                        )
                    });
                }
            }
            REMOVE => {
                if let Some((folder, _)) = row.installed.clone() {
                    let name = row.plugin.name.clone();
                    self.start(&format!("Removing {name}..."), move || {
                        Done::Removed(plugin_catalog::uninstall(&folder).map(|()| name))
                    });
                }
            }
            ADD => {
                if let Some((folder, _)) = &row.installed {
                    add_to_settings(folder, &row.plugin.name);
                }
            }
            PAGE => {
                if let Some(url) = row
                    .plugin
                    .repository_url
                    .as_ref()
                    .or(row.plugin.wiki_url.as_ref())
                    && url.starts_with("https://")
                {
                    shell_open(self.window, url);
                }
            }
            _ => {}
        }
    }

    /// Installs a zip or DLL the user picks; upstream's plugin manager also
    /// installs local archives.
    fn install_from_file(&mut self) {
        let file = match super::commands::file_dialog(
            self.window,
            false,
            super::commands::FileKind::Package,
            "Install a plugin from a file",
        ) {
            Ok(Some(file)) => file,
            Ok(None) => return,
            Err(error) => {
                self.set_status(&error);
                return;
            }
        };
        let answer = super::commands::message_box(
            self.window,
            &format!(
                "Install {}?\n\nThis file is not from the catalog, so no published hash can check it. Plugins run inside the driver with your permissions. Install only files you trust.",
                file.display()
            ),
            "Install plugin",
            MB_YESNO | MB_ICONWARNING,
        );
        if answer != IDYES {
            return;
        }
        let name = file
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_default();
        self.start(&format!("Installing {name}..."), move || {
            Done::Installed(plugin_catalog::install_file(&file).map(|folder| (name, folder)))
        });
    }

    fn finished(&mut self) {
        while let Ok(done) = self.results.try_recv() {
            self.busy = false;
            match done {
                Done::Catalog(Ok(catalog)) => {
                    self.set_status(&format!(
                        "{} plugins support OpenTabletDriver 0.6.7.",
                        catalog.len()
                    ));
                    self.catalog = catalog;
                }
                Done::Catalog(Err(error)) => {
                    self.set_status(&format!("Could not load the catalog: {error}"))
                }
                Done::Installed(Ok((name, folder))) => {
                    self.set_status(&format!(
                        "Installed {name}. Add to settings uses its filters."
                    ));
                    log(
                        Level::Info,
                        format!("Installed plugin {name} in {}.", folder.display()),
                    );
                }
                Done::Installed(Err(error)) => {
                    self.set_status("The plugin was not installed.");
                    log(Level::Error, format!("Plugin install failed: {error}"));
                }
                Done::Removed(Ok(name)) => {
                    self.set_status(&format!("Removed {name}."));
                    log(
                        Level::Info,
                        format!("Removed plugin {name}. Settings that use it need editing."),
                    );
                }
                Done::Removed(Err(error)) => {
                    self.set_status("The plugin was not removed.");
                    log(Level::Error, error);
                }
            }
        }
        self.rebuild();
    }
}

fn log(level: Level, message: String) {
    with_app(|app| app.log(level, "Plugins", message));
}

/// Adds the filters of every DLL in an installed plugin's folder to the
/// settings, as Plugins > Add .NET plugin does for one DLL. Inspecting a DLL
/// runs its code, as that command does.
fn add_to_settings(folder: &Path, name: &str) {
    let dlls: Vec<PathBuf> = plugin_catalog::dlls(folder)
        .into_iter()
        .filter(|dll| crate::dotnet::inspect_details(dll).is_ok_and(|entries| !entries.is_empty()))
        .collect();
    if dlls.is_empty() {
        log(
            Level::Warning,
            format!(
                "{name} exports no position filters or tools this driver can run; it may provide bindings or output modes."
            ),
        );
        return;
    }
    for dll in dlls {
        with_app(|app| app.add_plugin(dll, true));
    }
}

unsafe extern "system" fn window_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_SIZE => {
            with_manager(|manager| manager.layout());
            0
        }
        WM_COMMAND => {
            let id = wparam as u16;
            with_manager(|manager| manager.command(id));
            0
        }
        WM_NOTIFY => {
            let header = unsafe { &*(lparam as *const NMHDR) };
            if header.idFrom == LIST as usize && header.code == LVN_ITEMCHANGED {
                with_manager(|manager| manager.selection_changed());
            }
            0
        }
        WM_PLUGINS => {
            with_manager(Manager::finished);
            0
        }
        WM_DESTROY => {
            MANAGER.with(|slot| slot.borrow_mut().take());
            0
        }
        _ => unsafe { DefWindowProcW(window, message, wparam, lparam) },
    }
}
