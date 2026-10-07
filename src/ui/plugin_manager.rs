//! Plugins > Plugin manager: OpenTabletDriver's plugin catalog with install,
//! update, remove and "add to settings", like its plugin manager window.
//! Downloads and file work run on a background thread.
use super::commands::shell_open;
use super::*;
use crate::plugin_catalog::{self, PluginMetadata};
use std::sync::mpsc::{self, Receiver, Sender};
use windows_sys::Win32::UI::Controls::{
    CDDS_ITEMPREPAINT, CDDS_POSTPAINT, CDRF_NOTIFYITEMDRAW, CDRF_NOTIFYPOSTPAINT,
    EM_SETRECT, HDITEMW, HDI_TEXT, HDM_GETITEMCOUNT,
    HDM_GETITEMRECT, HDM_GETITEMW, LVCF_TEXT, LVCF_WIDTH, LVCOLUMNW, LVIF_TEXT,
    LVIS_SELECTED, LVITEMW, LVM_DELETEALLITEMS, LVM_GETCOLUMNWIDTH,
    LVM_GETHEADER, LVM_GETITEMSTATE, LVM_GETITEMTEXTW, LVM_GETNEXTITEM,
    LVM_INSERTCOLUMNW, LVM_INSERTITEMW, LVM_SETBKCOLOR,
    LVM_SETCOLUMNWIDTH, LVM_SETEXTENDEDLISTVIEWSTYLE, LVM_SETTEXTBKCOLOR, LVM_SETTEXTCOLOR,
    LVM_SETITEMTEXTW, LVN_ITEMCHANGED, LVNI_SELECTED, LVS_EX_DOUBLEBUFFER, LVS_EX_FULLROWSELECT,
    LVS_REPORT, LVS_SHOWSELALWAYS, LVS_SINGLESEL, NMHDR, NM_KILLFOCUS, NM_SETFOCUS,
};
use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};

const CLASS: &str = "OpenTabletDriverRustPluginManager";
const WM_PLUGINS: u32 = WM_APP + 22;
const LIST: u16 = 100;
const INSTALL: u16 = 101;
const REMOVE: u16 = 102;
const ADD: u16 = 103;
const PAGE: u16 = 104;
const REFRESH: u16 = 105;
const FROM_FILE: u16 = 106;
const DETAILS: u16 = 107;
const STATUS: u16 = 108;
const TITLE: u16 = 109;
const DETAILS_TITLE: u16 = 110;
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
    Removed(Result<(String, PathBuf), String>),
}

struct Completion {
    result: Done,
    installed: Result<Vec<(PathBuf, PluginMetadata)>, String>,
}

struct Manager {
    window: HWND,
    list: HWND,
    details: HWND,
    status: HWND,
    title: HWND,
    details_title: HWND,
    buttons: Vec<HWND>,
    rows: Vec<Row>,
    catalog: Vec<PluginMetadata>,
    installed: Vec<(PathBuf, PluginMetadata)>,
    busy: bool,
    sender: Sender<Completion>,
    results: Receiver<Completion>,
    dpi: u32,
    dark_mode: theme::DarkMode,
    _fonts: FontSet,
    icons: [HICON; 2],
}

impl Drop for Manager {
    fn drop(&mut self) {
        for icon in self.icons {
            if !icon.is_null() {
                unsafe { DestroyIcon(icon) };
            }
        }
    }
}

thread_local! {
    static MANAGER: RefCell<Option<Manager>> = const { RefCell::new(None) };
    // Modal dialogs keep Manager borrowed while pumping messages. Shutdown
    // and update requests must remain visible during those nested callbacks.
    static MANAGER_WINDOW: Cell<HWND> = const { Cell::new(ptr::null_mut()) };
    static MANAGER_ACTIVE: Cell<bool> = const { Cell::new(false) };
    static MANAGER_CLOSE_PENDING: Cell<bool> = const { Cell::new(false) };
    static MANAGER_RESTART_PENDING: Cell<bool> = const { Cell::new(false) };
    static MANAGER_THEME_PENDING: Cell<bool> = const { Cell::new(false) };
}

pub(super) fn open() -> Result<(), String> {
    let owner = with_app(|app| (!app.closing && !app.update_restart_pending).then_some(app.hwnd))
        .flatten()
        .filter(|owner| unsafe { IsWindow(*owner) } != 0)
        .ok_or_else(|| "The control panel is closing or restarting.".to_owned())?;
    let window = MANAGER_WINDOW.get();
    if !window.is_null() && unsafe { IsWindow(window) } != 0 {
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
        hbrBackground: ptr::null_mut(),
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
            owner,
            ptr::null_mut(),
            instance,
            ptr::null(),
        )
    };
    if window.is_null() {
        return Err(std::io::Error::last_os_error().to_string());
    }
    MANAGER_WINDOW.set(window);
    MANAGER_CLOSE_PENDING.set(false);
    MANAGER_RESTART_PENDING.set(false);
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
        LVS_REPORT | LVS_SINGLESEL | LVS_SHOWSELALWAYS,
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
        "Select a plugin to see its details.",
        DETAILS,
        WS_VSCROLL | (ES_MULTILINE | ES_READONLY | ES_AUTOVSCROLL) as u32,
    );
    let status = child("STATIC", "Loading the plugin catalog...", STATUS, SS_LEFT | SS_NOPREFIX);
    let title = child("STATIC", "Plugins", TITLE, SS_LEFT | SS_NOPREFIX);
    let details_title = child("STATIC", "Plugin details", DETAILS_TITLE, SS_LEFT | SS_NOPREFIX);
    unsafe {
        for control in [status, title, details_title] {
            SetWindowLongPtrW(control, GWL_STYLE, GetWindowLongPtrW(control, GWL_STYLE) & !(WS_TABSTOP as isize));
        }
        SendMessageW(title, WM_SETFONT, fonts.fonts.bold as usize, 0);
        SendMessageW(details_title, WM_SETFONT, fonts.fonts.bold as usize, 0);
        SetWindowSubclass(list, Some(list_proc), LIST as usize, 0);
    }
    let buttons = BUTTONS
        .iter()
        .map(|(id, text)| child("BUTTON", text, *id, BS_PUSHBUTTON as u32))
        .collect();
    let (sender, results) = mpsc::channel();
    MANAGER.with(|slot| {
        *slot.borrow_mut() = Some(Manager {
            window,
            list,
            details,
            status,
            title,
            details_title,
            buttons,
            rows: Vec::new(),
            catalog: Vec::new(),
            installed: Vec::new(),
            busy: false,
            sender,
            results,
            dpi,
            dark_mode: theme::DarkMode::load(),
            _fonts: fonts,
            icons: [ptr::null_mut(); 2],
        })
    });
    with_manager(|manager| {
        manager.set_icons();
        manager.apply_theme();
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
    let mut borrowed = false;
    let result = MANAGER.with(|slot| {
        let mut slot = slot.try_borrow_mut().ok()?;
        let manager = slot.as_mut()?;
        borrowed = true;
        MANAGER_ACTIVE.set(true);
        Some(f(manager))
    });
    if borrowed {
        MANAGER_ACTIVE.set(false);
        if MANAGER_CLOSE_PENDING.get() {
            // The callback and its RefCell borrow have both ended. Destroying
            // now lets WM_DESTROY release Manager without a nested borrow.
            close();
            MANAGER.with(|slot| { slot.borrow_mut().take(); });
        } else if MANAGER_RESTART_PENDING.get() {
            // A native modal dialog may re-enable its owner when it returns.
            set_restart_pending(true);
        }
        if !MANAGER_CLOSE_PENDING.get() && MANAGER_THEME_PENDING.get() {
            apply_theme();
        }
    }
    result
}

fn actions_allowed(window: HWND) -> bool {
    if MANAGER_CLOSE_PENDING.get() || MANAGER_RESTART_PENDING.get()
        || MANAGER_WINDOW.get() != window || unsafe { IsWindow(window) } == 0
    {
        return false;
    }
    let owner = unsafe { GetWindow(window, GW_OWNER) };
    !owner.is_null() && unsafe { IsWindow(owner) } != 0
        && with_app(|app| app.hwnd == owner && !app.closing && !app.update_restart_pending)
            .unwrap_or(false)
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
        let button_height = s(30);
        let bottom = height - s(52) - button_height;
        let details_height = s(120);
        let details_top = bottom - details_height - s(16);
        place(self.title, s(16), s(12), width - s(32), s(24));
        place(self.details_title, s(16), details_top - s(27), width - s(32), s(21));
        place(
            self.list,
            s(17),
            s(47),
            width - s(34),
            details_top - s(40) - s(47),
        );
        place(
            self.details,
            s(17),
            details_top + s(1),
            width - s(34),
            details_height - s(2),
        );
        // Use the available width for names instead of an empty header tail.
        // Other column widths retain native drag/resize behavior.
        let columns_width: i32 = (1..5)
            .map(|column| unsafe { SendMessageW(self.list, LVM_GETCOLUMNWIDTH, column, 0) } as i32)
            .sum();
        let name_width = (client_rect(self.list).right - columns_width).max(s(260));
        unsafe { SendMessageW(self.list, LVM_SETCOLUMNWIDTH, 0, name_width as isize) };
        let format = draw::inset(client_rect(self.details), s(9), s(7));
        unsafe { SendMessageW(self.details, EM_SETRECT, 0, &format as *const RECT as isize) };
        let mut x = s(16);
        for button in &self.buttons {
            place(*button, x, bottom, s(118), button_height);
            x += s(124);
        }
        place(
            self.status,
            s(16),
            height - s(35),
            width - s(32),
            s(22),
        );
        unsafe { InvalidateRect(self.window, ptr::null(), 0) };
    }

    fn apply_theme(&self) {
        let Some(palette) = with_look(|look| look.style.palette) else { return; };
        self.dark_mode.apply_title_bar(self.window, palette.dark);
        self.dark_mode.apply_control(self.list, palette.dark);
        self.dark_mode.apply_control(self.details, palette.dark);
        let header = unsafe { SendMessageW(self.list, LVM_GETHEADER, 0, 0) } as HWND;
        if !header.is_null() {
            self.dark_mode.apply_control(header, palette.dark);
            unsafe { SendMessageW(header, WM_SETFONT, self._fonts.fonts.bold as usize, 0) };
        }
        unsafe {
            SendMessageW(self.list, LVM_SETBKCOLOR, 0, palette.field.colorref() as isize);
            SendMessageW(self.list, LVM_SETTEXTBKCOLOR, 0, palette.field.colorref() as isize);
            SendMessageW(self.list, LVM_SETTEXTCOLOR, 0, palette.text.colorref() as isize);
            RedrawWindow(self.window, ptr::null(), ptr::null_mut(), RDW_INVALIDATE | RDW_ERASE | RDW_FRAME | RDW_ALLCHILDREN);
        }
    }

    fn set_icons(&mut self) {
        for (index, (kind, metric)) in [(ICON_BIG, SM_CXICON), (ICON_SMALL, SM_CXSMICON)]
            .into_iter().enumerate()
        {
            let size = unsafe { GetSystemMetricsForDpi(metric, self.dpi) }.max(16);
            let icon = canvas::app_icon(size);
            if icon.is_null() { continue; }
            unsafe { SendMessageW(self.window, WM_SETICON, kind as usize, icon as isize) };
            let previous = std::mem::replace(&mut self.icons[index], icon);
            if !previous.is_null() { unsafe { DestroyIcon(previous) }; }
        }
    }

    fn paint(&self) {
        let mut paint = PAINTSTRUCT::default();
        let dc = unsafe { BeginPaint(self.window, &mut paint) };
        with_look(|look| {
            let client = client_rect(self.window);
            let Some(mut canvas) = canvas::Canvas::new(dc, client) else { return; };
            let mut style = look.style;
            style.fonts = self._fonts.fonts;
            style.scale = unsafe { GetDpiForWindow(self.window) }.max(96) as f32 / 96.0;
            canvas.fill(client, style.palette.window);
            for control in [self.list, self.details] {
                let mut bounds = RECT::default();
                unsafe {
                    GetWindowRect(control, &mut bounds);
                    MapWindowPoints(ptr::null_mut(), self.window, (&mut bounds as *mut RECT).cast(), 2);
                }
                draw::field_frame(&mut canvas, draw::inset(bounds, -1, -1), &style, unsafe { GetFocus() } == control, false, false);
            }
            canvas.present(dc);
        });
        unsafe { EndPaint(self.window, &paint) };
    }

    fn set_dpi(&mut self, dpi: u32) {
        let dpi = dpi.max(96);
        let fonts = FontSet::new(dpi);
        for control in [self.list, self.details, self.status].into_iter().chain(self.buttons.iter().copied()) {
            unsafe { SendMessageW(control, WM_SETFONT, fonts.fonts.ui as usize, 1) };
        }
        for control in [self.title, self.details_title] {
            unsafe { SendMessageW(control, WM_SETFONT, fonts.fonts.bold as usize, 1) };
        }
        let header = unsafe { SendMessageW(self.list, LVM_GETHEADER, 0, 0) } as HWND;
        let columns = unsafe { SendMessageW(header, HDM_GETITEMCOUNT, 0, 0) }.max(0) as usize;
        for column in 0..columns {
            let width = unsafe { SendMessageW(self.list, LVM_GETCOLUMNWIDTH, column, 0) };
            let scaled = (width as i64 * i64::from(dpi) + i64::from(self.dpi) / 2) / i64::from(self.dpi);
            unsafe { SendMessageW(self.list, LVM_SETCOLUMNWIDTH, column, scaled as isize) };
        }
        unsafe { SendMessageW(header, WM_SETFONT, fonts.fonts.bold as usize, 0) };
        self.dpi = dpi;
        self._fonts = fonts;
        self.set_icons();
        self.apply_theme();
    }

    fn set_status(&self, text: &str) {
        unsafe { SetWindowTextW(self.status, wide(text).as_ptr()) };
    }

    fn start(&mut self, label: &str, work: impl FnOnce() -> Done + Send + 'static) {
        if self.busy || !actions_allowed(self.window) {
            return;
        }
        self.busy = true;
        self.set_status(label);
        self.update_buttons();
        let (sender, window) = (self.sender.clone(), self.window as isize);
        if let Err(error) = std::thread::Builder::new().name("plugin-manager".into()).spawn(move || {
            let done = work();
            let installed = plugin_catalog::installed();
            let _ = sender.send(Completion { result: done, installed });
            unsafe { PostMessageW(window as HWND, WM_PLUGINS, 0, 0) };
        }) {
            self.busy = false;
            self.set_status(&format!("Could not start plugin work: {error}"));
            self.update_buttons();
        }
    }

    fn refresh_catalog(&mut self) {
        self.start("Loading the plugin catalog...", || {
            Done::Catalog(plugin_catalog::fetch())
        });
    }

    /// Rebuilds the rows from the catalog and the installed plugins.
    fn rebuild(&mut self) {
        let installed = &self.installed;
        let mut rows: Vec<Row> = self
            .catalog
            .iter()
            .map(|plugin| Row {
                installed: installed
                    .iter()
                    .find(|(_, local)| local.same_identity(plugin))
                    .map(|(folder, local)| (folder.clone(), local.plugin_version.clone())),
                plugin: plugin.clone(),
                listed: true,
            })
            .collect();
        for (folder, local) in installed {
            if !rows
                .iter()
                .any(|row| row.plugin.same_identity(local))
            {
                rows.push(Row {
                    installed: Some((folder.clone(), local.plugin_version.clone())),
                    plugin: local.clone(),
                    listed: false,
                });
            }
        }
        let selected = self
            .selected()
            .map(|row| row.plugin.clone());
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
                .is_some_and(|selected| selected.same_identity(&row.plugin))
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
        let text = self.selected().map_or_else(|| "Select a plugin to see its details.".into(), |row| {
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
        if !actions_allowed(self.window) {
            return;
        }
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
                        Done::Removed(plugin_catalog::uninstall(&folder).map(|()| (name, folder)))
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
        if !actions_allowed(self.window) {
            return;
        }
        let selected = super::commands::file_dialog(
            self.window,
            false,
            super::commands::FileKind::Package,
            "Install a plugin from a file",
        );
        if !actions_allowed(self.window) {
            return;
        }
        let file = match selected {
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
        if answer != IDYES || !actions_allowed(self.window) {
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
        while let Ok(Completion { result: done, installed }) = self.results.try_recv() {
            self.busy = false;
            let inventory_error = match installed {
                Ok(installed) => { self.installed = installed; None },
                Err(error) => Some(error),
            };
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
                        "Installed {name}. Discovering its filters and defaults..."
                    ));
                    log(
                        Level::Info,
                        format!("Installed plugin {name} in {}.", folder.display()),
                    );
                    with_app(|app| app.installed_plugin_changed(folder, name));
                }
                Done::Installed(Err(error)) => {
                    self.set_status("The plugin was not installed.");
                    log(Level::Error, format!("Plugin install failed: {error}"));
                }
                Done::Removed(Ok((name, folder))) => {
                    self.set_status(&format!("Removed {name}."));
                    log(
                        Level::Info,
                        format!("Removed plugin {name}. Settings that use it need editing."),
                    );
                    with_app(|app| app.plugin_removed(&folder));
                }
                Done::Removed(Err(error)) => {
                    self.set_status("The plugin was not removed.");
                    log(Level::Error, error);
                }
            }
            if let Some(error) = inventory_error {
                self.set_status(&format!("Could not refresh installed plugins: {error}. The previous list was kept."));
                log(Level::Warning, format!("Could not refresh installed plugins: {error}"));
            }
        }
        self.rebuild();
    }
}

pub(super) fn set_restart_pending(pending: bool) {
    MANAGER_RESTART_PENDING.set(pending);
    let window = MANAGER_WINDOW.get();
    if !window.is_null() && unsafe { IsWindow(window) } != 0 {
        unsafe { EnableWindow(window, i32::from(!pending && !MANAGER_CLOSE_PENDING.get())); }
    }
}

pub(super) fn close() {
    MANAGER_CLOSE_PENDING.set(true);
    let window = MANAGER_WINDOW.get();
    if !window.is_null() && unsafe { IsWindow(window) } != 0 {
        unsafe { EnableWindow(window, 0) };
        if !MANAGER_ACTIVE.get() {
            unsafe { DestroyWindow(window) };
        }
    }
}

/// Called after the main window updates LOOK, including app theme changes.
pub(super) fn apply_theme() {
    MANAGER_THEME_PENDING.set(true);
    with_manager(|manager| {
        MANAGER_THEME_PENDING.set(false);
        manager.apply_theme();
    });
}

fn control_style(control: HWND, look: &Look) -> Style {
    let mut style = look.style;
    style.scale = unsafe { GetDpiForWindow(control) }.max(96) as f32 / 96.0;
    style.fonts.ui = unsafe { SendMessageW(control, WM_GETFONT, 0, 0) } as HFONT;
    style
}

fn draw_button(custom: &NMCUSTOMDRAW) -> LRESULT {
    if custom.dwDrawStage != CDDS_PREPAINT {
        return CDRF_DODEFAULT as LRESULT;
    }
    with_look(|look| {
        let control = custom.hdr.hwndFrom;
        let bounds = client_rect(control);
        let Some(mut canvas) = canvas::Canvas::new(custom.hdc, bounds) else {
            return CDRF_DODEFAULT as LRESULT;
        };
        let style = control_style(control, look);
        let flags = custom.uItemState;
        draw::button(
            &mut canvas, bounds, &text(control), &style, style.palette.window,
            State {
                hot: flags & CDIS_HOT != 0,
                pressed: flags & CDIS_SELECTED != 0,
                focus: flags & CDIS_FOCUS != 0,
                disabled: unsafe { IsWindowEnabled(control) } == 0,
                cues: flags & CDIS_SHOWKEYBOARDCUES != 0,
                ..State::default()
            },
        );
        canvas.present(custom.hdc);
        CDRF_SKIPDEFAULT as LRESULT
    }).unwrap_or(CDRF_DODEFAULT as LRESULT)
}

fn draw_catalog_row(custom: &NMCUSTOMDRAW) -> LRESULT {
    if custom.dwDrawStage == CDDS_PREPAINT {
        return CDRF_NOTIFYITEMDRAW as LRESULT;
    }
    if custom.dwDrawStage != CDDS_ITEMPREPAINT {
        return CDRF_DODEFAULT as LRESULT;
    }
    with_look(|look| {
        let list = custom.hdr.hwndFrom;
        let style = control_style(list, look);
        let p = style.palette;
        let selected = unsafe { SendMessageW(list, LVM_GETITEMSTATE, custom.dwItemSpec, LVIS_SELECTED as isize) } != 0;
        let client = client_rect(list);
        let bounds = RECT { left: 0, right: client.right, ..custom.rc };
        let Some(mut canvas) = canvas::Canvas::new(custom.hdc, bounds) else {
            return CDRF_DODEFAULT as LRESULT;
        };
        let enabled = unsafe { IsWindowEnabled(list) } != 0;
        let foreground = if selected { p.selection_text() } else if enabled { p.text } else { p.disabled };
        canvas.fill(bounds, if selected { p.selection } else { p.field });
        let header = unsafe { SendMessageW(list, LVM_GETHEADER, 0, 0) } as HWND;
        let columns = unsafe { SendMessageW(header, HDM_GETITEMCOUNT, 0, 0) }.max(0) as usize;
        for column in 0..columns {
            let mut cell = RECT::default();
            if unsafe { SendMessageW(header, HDM_GETITEMRECT, column, &mut cell as *mut RECT as isize) } == 0 {
                continue;
            }
            // Header bounds preserve alignment during scrolling and resizing.
            unsafe { MapWindowPoints(header, list, (&mut cell as *mut RECT).cast(), 2) };
            cell.top = bounds.top;
            cell.bottom = bounds.bottom;
            if cell.right <= bounds.left || cell.left >= bounds.right {
                continue;
            }
            let mut buffer = [0u16; 1024];
            let mut item = LVITEMW {
                iSubItem: column as i32,
                pszText: buffer.as_mut_ptr(),
                cchTextMax: buffer.len() as i32,
                ..Default::default()
            };
            let length = unsafe { SendMessageW(list, LVM_GETITEMTEXTW, custom.dwItemSpec, &mut item as *mut LVITEMW as isize) };
            let length = (length.max(0) as usize).min(buffer.len() - 1);
            canvas.text(draw::inset(cell, style.ipx(8.0), 0), &String::from_utf16_lossy(&buffer[..length]), style.fonts.ui, foreground, draw::TEXT_LEFT | DT_NOPREFIX);
        }
        if selected && custom.uItemState & CDIS_FOCUS != 0 && unsafe { GetFocus() } == list {
            canvas.round_rect(draw::inset(bounds, 1, 1), [0.0; 4], None, Some((p.accent, 1.0)));
        }
        canvas.present(custom.hdc);
        CDRF_SKIPDEFAULT as LRESULT
    }).unwrap_or(CDRF_DODEFAULT as LRESULT)
}

fn draw_catalog_header(custom: &NMCUSTOMDRAW) -> LRESULT {
    if custom.dwDrawStage == CDDS_PREPAINT {
        // Header SKIPDEFAULT is only supported at ITEMPREPAINT. Painting the
        // whole header here lets native painting overwrite it with white.
        return CDRF_NOTIFYPOSTPAINT as LRESULT;
    }
    if custom.dwDrawStage != CDDS_POSTPAINT {
        return CDRF_DODEFAULT as LRESULT;
    }
    with_look(|look| {
        let header = custom.hdr.hwndFrom;
        let style = control_style(header, look);
        let p = style.palette;
        let bounds = client_rect(header);
        let Some(mut canvas) = canvas::Canvas::new(custom.hdc, bounds) else {
            return CDRF_DODEFAULT as LRESULT;
        };
        // Paint the entire header, including the space after the last column.
        canvas.fill(bounds, p.group);
        let count = unsafe { SendMessageW(header, HDM_GETITEMCOUNT, 0, 0) }.max(0) as usize;
        for column in 0..count {
            let mut cell = RECT::default();
            if unsafe { SendMessageW(header, HDM_GETITEMRECT, column, &mut cell as *mut RECT as isize) } == 0 {
                continue;
            }
            let mut buffer = [0u16; 256];
            let mut item = HDITEMW {
                mask: HDI_TEXT,
                pszText: buffer.as_mut_ptr(),
                cchTextMax: buffer.len() as i32,
                ..Default::default()
            };
            unsafe { SendMessageW(header, HDM_GETITEMW, column, &mut item as *mut HDITEMW as isize) };
            let length = buffer.iter().position(|c| *c == 0).unwrap_or(buffer.len());
            canvas.text(draw::inset(cell, style.ipx(8.0), 0), &String::from_utf16_lossy(&buffer[..length]), style.fonts.ui, p.text, draw::TEXT_LEFT | DT_NOPREFIX);
            canvas.fill(RECT { left: cell.right - 1, top: cell.top + style.ipx(5.0), bottom: cell.bottom - style.ipx(5.0), ..cell }, p.border);
        }
        canvas.fill(RECT { top: bounds.bottom - 1, ..bounds }, p.border);
        canvas.present(custom.hdc);
        CDRF_DODEFAULT as LRESULT
    }).unwrap_or(CDRF_DODEFAULT as LRESULT)
}

/// Header notifications are sent to the ListView, not to the manager window.
unsafe extern "system" fn list_proc(
    window: HWND, message: u32, wparam: WPARAM, lparam: LPARAM,
    subclass: usize, _data: usize,
) -> LRESULT {
    if message == WM_NOTIFY && lparam != 0 {
        let notification = unsafe { &*(lparam as *const NMHDR) };
        let header = unsafe { SendMessageW(window, LVM_GETHEADER, 0, 0) } as HWND;
        if notification.hwndFrom == header && notification.code == NM_CUSTOMDRAW {
            return draw_catalog_header(unsafe { &*(lparam as *const NMCUSTOMDRAW) });
        }
    }
    if message == WM_NCDESTROY {
        unsafe { RemoveWindowSubclass(window, Some(list_proc), subclass) };
    }
    unsafe { DefSubclassProc(window, message, wparam, lparam) }
}

fn log(level: Level, message: String) {
    with_app(|app| app.log(level, "Plugins", message));
}

/// Adds the filters of every DLL in an installed plugin's folder to the
/// settings, as Plugins > Add .NET plugin does for one DLL. Inspecting a DLL
/// runs its code, as that command does.
fn add_to_settings(folder: &Path, name: &str) {
    with_app(|app| app.add_plugin_folder(folder.to_owned(), name.to_owned()));
}

unsafe extern "system" fn window_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_ERASEBKGND => {
            with_look(|look| unsafe { FillRect(wparam as HDC, &client_rect(window), look.brush(look.style.palette.window)) });
            1
        }
        WM_PAINT => {
            with_manager(|manager| manager.paint()).map_or_else(
                || unsafe { DefWindowProcW(window, message, wparam, lparam) },
                |()| 0,
            )
        }
        WM_SIZE => {
            with_manager(|manager| manager.layout());
            0
        }
        WM_GETMINMAXINFO => {
            let dpi = unsafe { GetDpiForWindow(window) }.max(96);
            let info = unsafe { &mut *(lparam as *mut MINMAXINFO) };
            info.ptMinTrackSize.x = scale(800, dpi);
            info.ptMinTrackSize.y = scale(500, dpi);
            0
        }
        WM_DPICHANGED => {
            with_manager(|manager| manager.set_dpi((wparam & 0xFFFF) as u32));
            let bounds = unsafe { &*(lparam as *const RECT) };
            unsafe { SetWindowPos(window, ptr::null_mut(), bounds.left, bounds.top, bounds.right - bounds.left, bounds.bottom - bounds.top, SWP_NOZORDER | SWP_NOACTIVATE) };
            with_manager(|manager| manager.layout());
            0
        }
        WM_THEMECHANGED => {
            apply_theme();
            unsafe { DefWindowProcW(window, message, wparam, lparam) }
        }
        WM_CTLCOLORSTATIC | WM_CTLCOLOREDIT => {
            with_look(|look| {
                let control = lparam as HWND;
                let p = look.style.palette;
                let id = unsafe { GetDlgCtrlID(control) } as u16;
                let background = if id == DETAILS { p.field } else { p.window };
                let color = if unsafe { IsWindowEnabled(control) } == 0 { p.disabled } else if id == STATUS { p.muted } else { p.text };
                unsafe {
                    SetTextColor(wparam as HDC, color.colorref());
                    SetBkColor(wparam as HDC, background.colorref());
                }
                look.brush(background) as LRESULT
            }).unwrap_or_else(|| unsafe { DefWindowProcW(window, message, wparam, lparam) })
        }
        WM_COMMAND => {
            let id = wparam as u16;
            let notification = (wparam >> 16) as u16;
            if id == DETAILS && (notification == EN_SETFOCUS as u16 || notification == EN_KILLFOCUS as u16) {
                unsafe { InvalidateRect(window, ptr::null(), 0) };
                return 0;
            }
            with_manager(|manager| manager.command(id));
            0
        }
        WM_NOTIFY => {
            if lparam == 0 { return 0; }
            let header = unsafe { &*(lparam as *const NMHDR) };
            if header.idFrom == LIST as usize && (header.code == NM_SETFOCUS || header.code == NM_KILLFOCUS) {
                unsafe { InvalidateRect(window, ptr::null(), 0) };
            }
            if header.code == NM_CUSTOMDRAW {
                let custom = unsafe { &*(lparam as *const NMCUSTOMDRAW) };
                if header.idFrom == LIST as usize {
                    return draw_catalog_row(custom);
                }
                if BUTTONS.iter().any(|(id, _)| header.idFrom == *id as usize) {
                    return draw_button(custom);
                }
            }
            if header.idFrom == LIST as usize && header.code == LVN_ITEMCHANGED {
                with_manager(|manager| manager.selection_changed());
            }
            0
        }
        WM_PLUGINS => {
            with_manager(Manager::finished);
            0
        }
        WM_CLOSE => {
            close();
            0
        }
        WM_DESTROY => {
            if MANAGER_WINDOW.get() == window {
                MANAGER_WINDOW.set(ptr::null_mut());
                MANAGER_CLOSE_PENDING.set(true);
            }
            0
        }
        WM_NCDESTROY => {
            // Native children still exist during WM_DESTROY. Keep their fonts
            // alive until WM_NCDESTROY, which follows child destruction.
            // Never panic in a window procedure; see the tablet debugger.
            let _ = MANAGER.try_with(|slot| slot.try_borrow_mut().map(|mut slot| slot.take()));
            unsafe { DefWindowProcW(window, message, wparam, lparam) }
        }
        _ => unsafe { DefWindowProcW(window, message, wparam, lparam) },
    }
}
