//! Deliberate managed output/binding assignment with capture-local drafts.
//! Inspection is asynchronous; plugin instances are never created by this UI.
use super::*;
use otd_core::output::buttons::ButtonAction;
use windows_sys::Win32::UI::HiDpi::{DDC_DISABLE_ALL, SetDialogDpiChangeBehavior};

const TYPE: u16 = 100;
const PREVIOUS: u16 = 101;
const NEXT: u16 = 102;
const NOTICE: u16 = 103;
const FIELD: u16 = 200;
const PAGE_SIZE: usize = 6;
thread_local! {
    static SERIAL: Cell<u64> = const { Cell::new(0) };
    static WINDOW: Cell<HWND> = const { Cell::new(ptr::null_mut()) };
    static READY: RefCell<Option<(Guard, Vec<crate::dotnet::InspectedFilter>)>> = const { RefCell::new(None) };
}
pub(super) fn new_token() -> u64 { SERIAL.with(|value| { let next = value.get().checked_add(1).expect("managed UI token exhausted"); value.set(next); next }) }
pub(super) fn refresh_theme() { WINDOW.with(|window| { if !window.get().is_null() { unsafe { SendMessageW(window.get(), WM_THEMECHANGED, 0, 0); } } }); }
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Target { Output, Tip, Eraser, Binding(bindings::BindingTarget) }
impl Target {
    fn category(self) -> &'static str { if self == Self::Output { "output" } else { "binding" } }
    fn name(self) -> &'static str { match self { Self::Output => "Output mode", Self::Tip => "Tip binding", Self::Eraser => "Eraser binding", Self::Binding(_) => "Button binding" } }
    fn config(self, app: &App) -> Option<PluginConfig> {
        match self {
            Self::Output => app.editor.profile.managed_output.clone(),
            Self::Tip => app.editor.profile.managed_tip_binding.clone(),
            Self::Eraser => app.editor.profile.managed_eraser_binding.clone(),
            Self::Binding(target) => match app.binding_action(target) { ButtonAction::Managed(config) => Some(config), _ => None },
        }
    }
}
#[derive(Clone)]
pub(super) struct Guard { token: u64, revision: u64, generation: u64, path: PathBuf, target: Target, current: Option<PluginConfig> }
impl Guard {
    fn valid(&self, app: &App) -> bool {
        self.token == app.managed_token && self.revision == app.edit_revision && self.generation == app.metadata_generation
            && self.path == app.profile_path && !app.closing && !app.control_busy && !app.update_restart_pending
            && self.current == self.target.config(app)
    }
}
/// The file/menu modal runs outside App; its completion checks the original
/// target and unique App/request identity before launching inspection.
pub(super) fn choose(window: HWND, control: HWND, target: Target) {
    let Some(guard) = with_app(|app| {
        if app.closing || app.control_busy || app.update_restart_pending { return None; }
        app.managed_token = new_token();
        Some(Guard { token: app.managed_token, revision: app.edit_revision, generation: app.metadata_generation,
            path: app.profile_path.clone(), target, current: target.config(app) })
    }).flatten() else { return; };
    let menu = unsafe { CreatePopupMenu() };
    if guard.current.is_some() { commands::append(menu, 0, 1, "Edit current managed settings..."); }
    commands::append(menu, 0, 2, "Choose an installed DLL...");
    commands::append(menu, 0, 3, "Browse for a DLL...");
    commands::append(menu, 0, 4, "Choose a registered type...");
    let path = match commands::popup(window, menu, control) {
        1 => guard.current.as_ref().map(|config| config.path.clone()),
        2 => {
            let installed = crate::plugin_catalog::installed().map(|plugins| plugins.into_iter().flat_map(|(folder, _)| crate::plugin_catalog::dlls(&folder)).take(256).collect::<Vec<_>>());
            match installed {
                Ok(paths) if !paths.is_empty() => {
                    let menu = unsafe { CreatePopupMenu() };
                    for (index, path) in paths.iter().enumerate() { commands::append(menu, 0, index as u16 + 1, &path.to_string_lossy()); }
                    let choice = commands::popup(window, menu, control);
                    choice.checked_sub(1).and_then(|index| paths.get(index as usize).cloned())
                }
                Ok(_) => { with_app(|app| app.log(Level::Warning, "Managed settings", "No installed plugin DLLs were found. Use Browse to choose one.")); None }
                Err(error) => { with_app(|app| app.log(Level::Error, "Managed settings", error)); None }
            }
        }
        3 => match commands::file_dialog(window, false, commands::FileKind::Dll, "Choose a managed plugin DLL") {
            Ok(path) => path,
            Err(error) => { with_app(|app| app.log(Level::Error, "Managed settings", error)); None }
        },
        4=>{
            with_app(|app|{
                if !guard.valid(app){return;}
                app.background("managed-registered-types",move||BackgroundResult::Managed{
                    guard,result:(||{
                        if let Some(registry)=crate::dotnet::registry_snapshot(){return Ok(registry.plugins.clone());}
                        let root=crate::plugin_catalog::plugins_directory()?;
                        crate::dotnet::reload_installed_plugins(&root).map(|registry|registry.plugins)
                    })(),
                });
            });return;
        },
        _ => None,
    };
    if let Some(path) = path { with_app(|app| {
        if !guard.valid(app) { return; }
        app.background("managed-endpoint-inspection", move || BackgroundResult::Managed {
            guard, result: crate::dotnet::inspect_details(&path),
        });
    }); }
}
pub(super) fn inspected(app: &mut App, guard: Guard, result: Result<Vec<crate::dotnet::InspectedFilter>, String>) {
    if !guard.valid(app) { return; }
    match result {
        Ok(entries) => {
            let entries: Vec<_> = entries.into_iter().filter(|entry| entry.metadata.supported && entry.metadata.category == guard.target.category()).collect();
            if entries.is_empty() { app.log(Level::Warning, "Managed settings", format!("No supported {} types.", guard.target.category())); return; }
            READY.with(|ready| *ready.borrow_mut() = Some((guard, entries)));
            unsafe { PostMessageW(app.hwnd, WM_MANAGED_SETTINGS, 0, 0); }
        }
        Err(error) => app.log(Level::Error, "Managed settings", error),
    }
}
pub(super) fn open_ready(window: HWND) {
    let Some((guard, entries)) = READY.with(|ready| ready.borrow_mut().take()) else { return; };
    if with_app(|app| guard.valid(app)) != Some(true) { return; }
    match show(window, guard.target.name(), entries, guard.current.as_ref()) {
        Ok(Some((config, metadata))) => { with_app(|app| {
            if !guard.valid(app) { return; }
            match guard.target {
                Target::Output => {
                    if metadata.relative_output && !metadata.absolute_output { app.editor.set_mode(OutputMode::Relative, &app.displays); }
                    else if metadata.absolute_output { app.editor.set_mode(OutputMode::Absolute, &app.displays); }
                    app.editor.profile.managed_output = Some(config);
                }
                Target::Tip => { app.editor.set_binding_enabled(false, true); app.editor.profile.managed_tip_binding = Some(config); }
                Target::Eraser => { app.editor.set_binding_enabled(true, true); app.editor.profile.managed_eraser_binding = Some(config); }
                Target::Binding(target) => { app.set_binding_action(target, ButtonAction::Managed(config)); return; }
            }
            app.mark_dirty(); app.sync_all(); app.layout();
        }); }
        Ok(None) => {}
        Err(error) => { with_app(|app| app.log(Level::Error, "Managed settings", error)); }
    }
}
#[repr(C, align(4))]
struct Template { dialog: DLGTEMPLATE, menu: u16, class: u16, title: u16 }
fn template() -> Template { Template { dialog: DLGTEMPLATE { style: WS_POPUP | WS_CAPTION | WS_SYSMENU | DS_MODALFRAME as u32,
    dwExtendedStyle: WS_EX_DLGMODALFRAME, cx: 330, cy: 260, ..Default::default() }, menu: 0, class: 0, title: 0 } }
struct Row { hwnd: HWND, field: model::PluginField, invalid: bool }
struct Dialog {
    window: HWND, title: String, entries: Vec<crate::dotnet::InspectedFilter>, drafts: Vec<PluginConfig>, selected: usize,
    page: usize, rows: Vec<Row>, controls: Vec<(HWND, RECT)>, permanent: usize, fonts: Option<FontSet>, dpi: u32,
    notice: HWND, picker: HWND, result: Option<(PluginConfig, FilterMetadata)>, error: Option<String>, dark_mode: theme::DarkMode,
}
impl Dialog {
    fn new(title: &str, entries: Vec<crate::dotnet::InspectedFilter>, current: Option<&PluginConfig>) -> Self {
        let selected = current.and_then(|current| entries.iter().position(|entry| entry.config.path == current.path && entry.config.type_name == current.type_name)).unwrap_or(0);
        let drafts = entries.iter().enumerate().map(|(index, entry)| {
            if index == selected && let Some(current) = current.filter(|current| current.path == entry.config.path && current.type_name == entry.config.type_name) { current.clone() }
            else { PluginConfig { enabled: true, settings_json: "{}".into(), ..entry.config.clone() } }
        }).collect();
        Self { window: ptr::null_mut(), title: title.into(), entries, drafts, selected, page: 0, rows: Vec::new(), controls: Vec::new(), permanent: 0,
            fonts: None, dpi: 96, notice: ptr::null_mut(), picker: ptr::null_mut(), result: None, error: None, dark_mode: theme::DarkMode::load() }
    }
    fn add(&mut self, class: &str, title: &str, id: u16, style: u32, bounds: RECT, kind: Kind) -> Result<HWND, String> {
        let control = unsafe { CreateWindowExW(0, wide(class).as_ptr(), wide(title).as_ptr(), WS_CHILD | WS_VISIBLE | style,
            0, 0, 0, 0, self.window, id as usize as _, GetModuleHandleW(ptr::null()), ptr::null()) };
        if control.is_null() { return Err(std::io::Error::last_os_error().to_string()); }
        update_look(|look| { look.controls.insert(control as isize, ControlInfo { kind, surface: if matches!(kind, Kind::Button | Kind::Dropdown) { Surface::Window } else { Surface::Group } }); });
        self.controls.push((control, bounds)); Ok(control)
    }
    fn initialize(&mut self, window: HWND) -> Result<(), String> {
        self.window = window;
        if unsafe { SetDialogDpiChangeBehavior(window, DDC_DISABLE_ALL, DDC_DISABLE_ALL) } == 0 { return Err(std::io::Error::last_os_error().to_string()); }
        set_text(window, &format!("Managed {}", self.title));
        self.add("STATIC", "Plugin type", 0, SS_LEFT, rect(20, 22, 135, 46), Kind::Label)?;
        self.picker = self.add("BUTTON", "", TYPE, WS_TABSTOP | BS_PUSHBUTTON as u32, rect(145, 18, 550, 50), Kind::Dropdown)?;
        self.notice = self.add("STATIC", "", NOTICE, SS_LEFT | SS_NOPREFIX, rect(20, 336, 550, 386), Kind::Label)?;
        self.add("BUTTON", "Previous", PREVIOUS, WS_TABSTOP | BS_PUSHBUTTON as u32, rect(20, 404, 115, 436), Kind::Button)?;
        self.add("BUTTON", "Next", NEXT, WS_TABSTOP | BS_PUSHBUTTON as u32, rect(125, 404, 220, 436), Kind::Button)?;
        self.add("BUTTON", "OK", IDOK as u16, WS_TABSTOP | BS_DEFPUSHBUTTON as u32, rect(352, 404, 445, 436), Kind::Button)?;
        self.add("BUTTON", "Cancel", IDCANCEL as u16, WS_TABSTOP | BS_PUSHBUTTON as u32, rect(457, 404, 550, 436), Kind::Button)?;
        self.permanent = self.controls.len(); self.rebuild()?; self.resize(unsafe { GetDpiForWindow(window) }.max(96), None); self.apply_theme(); Ok(())
    }
    fn fields(&self) -> Result<Vec<model::PluginField>, String> {
        model::plugin_editor_fields(&self.drafts[self.selected].settings_json, Some(&self.entries[self.selected].metadata)).ok_or_else(|| "Managed settings must be an object.".into())
    }
    fn rebuild(&mut self) -> Result<(), String> {
        for (control, _) in self.controls.drain(self.permanent..) { update_look(|look| { look.controls.remove(&(control as isize)); }); unsafe { DestroyWindow(control); } }
        self.rows.clear();
        let fields = self.fields()?;
        let pages = fields.len().div_ceil(PAGE_SIZE).max(1); self.page = self.page.min(pages - 1);
        let metadata = &self.entries[self.selected].metadata;
        set_text(self.picker, metadata.display_name.as_deref().unwrap_or(&metadata.type_name));
        for (index, field) in fields.into_iter().skip(self.page * PAGE_SIZE).take(PAGE_SIZE).enumerate() {
            let top = 70 + index as i32 * 42;
            let label = if field.unit.is_empty() { field.label.clone() } else { format!("{} ({})", field.label, field.unit) };
            self.add("STATIC", &label, 0, SS_LEFT | SS_NOPREFIX, rect(20, top + 4, 250, top + 30), Kind::Label)?;
            let choices = actual_choices(&field.value);
            let bool_value = matches!(field.value, model::PropertyValue::Bool(_));
            let (class, style, kind) = if !choices.is_empty() { ("BUTTON", WS_TABSTOP | BS_PUSHBUTTON as u32, Kind::Dropdown) }
                else if bool_value { ("BUTTON", WS_TABSTOP | BS_AUTOCHECKBOX as u32, Kind::Check) }
                else { ("EDIT", WS_TABSTOP | ES_AUTOHSCROLL as u32, Kind::Field) };
            let displayed = if field.value.field_writable() { field.value.display_text() } else { "Preserved value (read-only)".into() };
            let hwnd = self.add(class, &displayed, FIELD + index as u16, style, rect(262, top, 550, top + 30), kind)?;
            if let model::PropertyValue::Bool(value) = &field.value { unsafe { SendMessageW(hwnd, BM_SETCHECK, usize::from(*value), 0); } }
            unsafe { EnableWindow(hwnd, i32::from(field.value.field_writable())); }
            if class == "EDIT" {
                unsafe { SendMessageW(hwnd, EM_LIMITTEXT, 4096, 0); }
                if field.value.uses_default() { let cue = wide("Constructor / declared value"); unsafe { SendMessageW(hwnd, EM_SETCUEBANNER, 1, cue.as_ptr() as isize); } }
            }
            self.rows.push(Row { hwnd, field, invalid: false });
        }
        let supported = self.rows.iter().any(|row| !row.field.value.field_writable());
        set_text(self.notice, &format!("Page {} of {}. {}", self.page + 1, pages, if supported { "Unsupported properties are preserved and shown read-only." } else { "Untouched missing or null values keep their original meaning." }));
        unsafe { EnableWindow(GetDlgItem(self.window, IDOK), 1); EnableWindow(GetDlgItem(self.window, PREVIOUS as i32), i32::from(self.page > 0)); EnableWindow(GetDlgItem(self.window, NEXT as i32), i32::from(self.page + 1 < pages)); }
        self.resize(self.dpi, None); self.apply_theme(); Ok(())
    }
    fn set_value(&mut self, index: usize, value: Result<serde_json::Value, String>) {
        let result = value.and_then(|value| model::set_plugin_property(&self.drafts[self.selected].settings_json, &self.rows[index].field.key, value.clone()).map(|json| (json, value)));
        match result {
            Ok((json, value)) => {
                let mut candidate = self.drafts[self.selected].clone(); candidate.settings_json = json;
                if let Err(error) = candidate.validate() { self.rows[index].invalid = true; set_text(self.notice, &error); unsafe { EnableWindow(GetDlgItem(self.window, IDOK), 0); } return; }
                self.drafts[self.selected] = candidate;
                if let model::PropertyValue::Typed { saved, .. } = &mut self.rows[index].field.value { *saved = Some(value); }
                self.rows[index].invalid = false;
                set_text(self.notice, "The draft is updated. OK assigns it; Save and Apply remain separate.");
            }
            Err(error) => { self.rows[index].invalid = true; set_text(self.notice, &error); }
        }
        unsafe { EnableWindow(GetDlgItem(self.window, IDOK), i32::from(!self.rows.iter().any(|row| row.invalid))); InvalidateRect(self.window, ptr::null(), 0); }
    }
    fn apply_theme(&self) {
        let Some(palette) = with_look(|look| look.style.palette) else { return; }; self.dark_mode.apply_title_bar(self.window, palette.dark);
        for (control, _) in &self.controls { self.dark_mode.apply_control(*control, palette.dark); }
        unsafe { RedrawWindow(self.window, ptr::null(), ptr::null_mut(), RDW_INVALIDATE | RDW_ERASE | RDW_FRAME | RDW_ALLCHILDREN); }
    }
    fn paint(&self, dc: HDC) {
        with_look(|look| {
            let Some(fonts) = &self.fonts else { return; };
            let style = Style { fonts: fonts.fonts, scale: self.dpi as f32 / 96.0, ..look.style };
            let client = client_rect(self.window); let Some(mut canvas) = canvas::Canvas::new(dc, client) else { return; };
            canvas.fill(client, style.palette.window);
            draw::group_box(&mut canvas, rect(scale(8, self.dpi), scale(8, self.dpi), scale(562, self.dpi), scale(393, self.dpi)), &style, style.palette.group);
            for row in &self.rows {
                if with_look(|look| look.controls.get(&(row.hwnd as isize)).is_some_and(|info| info.kind == Kind::Field)) != Some(true) { continue; }
                let mut bounds = RECT::default(); unsafe { GetWindowRect(row.hwnd, &mut bounds); MapWindowPoints(ptr::null_mut(), self.window, (&mut bounds as *mut RECT).cast(), 2); }
                draw::field_frame(&mut canvas, draw::inset(bounds, -scale(3, self.dpi), -scale(3, self.dpi)), &style, unsafe { GetFocus() } == row.hwnd, row.invalid, false);
            }
            canvas.present(dc);
        });
    }
    fn resize(&mut self, dpi: u32, suggested: Option<RECT>) {
        self.dpi = dpi; let fonts = FontSet::new(dpi);
        for (control, bounds) in &self.controls {
            let field = with_look(|look| look.controls.get(&(*control as isize)).is_some_and(|info| info.kind == Kind::Field)).unwrap_or(false);
            let bounds = if field { draw::inset(*bounds, 3, 3) } else { *bounds };
            unsafe { SendMessageW(*control, WM_SETFONT, fonts.fonts.ui as usize, 1); SetWindowPos(*control, ptr::null_mut(), scale(bounds.left, dpi), scale(bounds.top, dpi), scale(bounds.right - bounds.left, dpi), scale(bounds.bottom - bounds.top, dpi), SWP_NOZORDER | SWP_NOACTIVATE); }
        }
        self.fonts = Some(fonts); let mut bounds = rect(0, 0, scale(570, dpi), scale(450, dpi));
        unsafe {
            AdjustWindowRectExForDpi(&mut bounds, WS_CAPTION | WS_SYSMENU | DS_MODALFRAME as u32, 0, WS_EX_DLGMODALFRAME, dpi);
            let mut parent = RECT::default(); GetWindowRect(GetParent(self.window), &mut parent);
            let (width, height) = (bounds.right - bounds.left, bounds.bottom - bounds.top);
            let (left, top) = suggested.map(|bounds| (bounds.left, bounds.top)).unwrap_or(((parent.left + parent.right - width) / 2, (parent.top + parent.bottom - height) / 2));
            SetWindowPos(self.window, ptr::null_mut(), left, top, width, height, SWP_NOZORDER | SWP_NOACTIVATE); SendMessageW(self.window, DM_REPOSITION, 0, 0);
        }
    }
}
fn actual_choices(value: &model::PropertyValue) -> Vec<(String, serde_json::Value)> { value.choices().into_iter().filter(|(_, value)| !value.is_null()).collect() }
impl Drop for Dialog { fn drop(&mut self) { update_look(|look| { for (control, _) in &self.controls { look.controls.remove(&(*control as isize)); } }); } }
unsafe extern "system" fn procedure(window: HWND, message: u32, wp: WPARAM, lp: LPARAM) -> isize {
    if message == WM_INITDIALOG { unsafe { SetWindowLongPtrW(window, GWLP_USERDATA, lp); } }
    let state = unsafe { GetWindowLongPtrW(window, GWLP_USERDATA) } as *const RefCell<Dialog>; if state.is_null() { return 0; }
    match message {
        WM_CTLCOLORDLG => return with_look(|look| look.brush(look.style.palette.window) as isize).unwrap_or(0),
        WM_CTLCOLOREDIT | WM_CTLCOLORSTATIC => return ctl_color(wp as HDC, lp as HWND).unwrap_or(0),
        WM_NOTIFY if lp != 0 => { let header = unsafe { &*(lp as *const NMHDR) }; if header.code == NM_CUSTOMDRAW { let result = custom_draw(unsafe { &mut *(lp as *mut NMCUSTOMDRAW) }); unsafe { SetWindowLongPtrW(window, DWLP_MSGRESULT as i32, result); } return 1; } }
        WM_SETTINGCHANGE | WM_SYSCOLORCHANGE => { with_app(App::apply_theme); }
        WM_NCDESTROY => { WINDOW.with(|active| { if active.get() == window { active.set(ptr::null_mut()); } }); unsafe { SetWindowLongPtrW(window, GWLP_USERDATA, 0); } return 0; }
        _ => {}
    }
    // Release the dialog borrow before opening a popup's nested message loop.
    if message == WM_COMMAND && (wp >> 16) as u32 == BN_CLICKED {
        let id = (wp & 0xffff) as u16;
        let choices = (unsafe { &*state }).try_borrow().ok().and_then(|dialog| {
            if id == TYPE { Some(dialog.entries.iter().enumerate().map(|(index, entry)| (entry.metadata.display_name.as_ref().map_or_else(|| entry.metadata.type_name.clone(), |name| format!("{name} - {}", entry.metadata.type_name)), serde_json::json!(index))).collect::<Vec<_>>()) }
            else if id >= FIELD { dialog.rows.get((id - FIELD) as usize).map(|row| actual_choices(&row.field.value)).filter(|choices| !choices.is_empty()) }
            else { None }
        });
        if let Some(choices) = choices {
            let menu = unsafe { CreatePopupMenu() }; for (index, (label, _)) in choices.iter().enumerate() { commands::append(menu, 0, index as u16 + 1, label); }
            let choice = commands::popup(window, menu, lp as HWND);
            if unsafe { IsWindow(window) } != 0 && let Some((_, value)) = choice.checked_sub(1).and_then(|index| choices.get(index as usize)) && let Ok(mut dialog) = (unsafe { &*state }).try_borrow_mut() {
                if id == TYPE { dialog.selected = value.as_u64().unwrap() as usize; dialog.page = 0; if let Err(error) = dialog.rebuild() { dialog.error = Some(error); unsafe { EndDialog(window, -1); } } }
                else { let index = (id - FIELD) as usize; dialog.set_value(index, Ok(value.clone())); set_text(dialog.rows[index].hwnd, &dialog.rows[index].field.value.display_text()); }
            }
            return 1;
        }
    }
    let Ok(mut dialog) = (unsafe { &*state }).try_borrow_mut() else { return 0; };
    match message {
        WM_INITDIALOG => { WINDOW.with(|active| active.set(window)); if let Err(error) = dialog.initialize(window) { dialog.error = Some(error); unsafe { EndDialog(window, -1); } } 1 }
        WM_COMMAND => {
            let id = (wp & 0xffff) as u16; let notification = (wp >> 16) as u32;
            if id >= FIELD && let Some(index) = dialog.rows.iter().position(|row| row.hwnd == lp as HWND) {
                if notification == EN_CHANGE { let value = model::parse_property(&text(lp as HWND), &dialog.rows[index].field.value); dialog.set_value(index, value); }
                else if notification == BN_CLICKED && matches!(dialog.rows[index].field.value, model::PropertyValue::Bool(_)) { let checked = unsafe { SendMessageW(lp as HWND, BM_GETCHECK, 0, 0) } == BST_CHECKED as isize; dialog.set_value(index, Ok(checked.into())); }
                else if matches!(notification, EN_SETFOCUS | EN_KILLFOCUS) {
                    if notification == EN_SETFOCUS && let Some(help) = &dialog.rows[index].field.tooltip { set_text(dialog.notice, help); }
                    unsafe { InvalidateRect(window, ptr::null(), 0); }
                }
                return 1;
            }
            match id {
                id if id == IDOK as u16 => { if !dialog.rows.iter().any(|row| row.invalid) { let mut config = dialog.drafts[dialog.selected].clone(); config.enabled = true; match config.validate() { Ok(()) => { dialog.result = Some((config, dialog.entries[dialog.selected].metadata.clone())); unsafe { EndDialog(window, IDOK as isize); } }, Err(error) => set_text(dialog.notice, &error) } } }
                id if id == IDCANCEL as u16 => { unsafe { EndDialog(window, 0); } }
                PREVIOUS | NEXT => { if dialog.rows.iter().any(|row| row.invalid) { set_text(dialog.notice, "Correct the invalid field before changing pages."); } else { dialog.page = if id == NEXT { dialog.page + 1 } else { dialog.page.saturating_sub(1) }; if let Err(error) = dialog.rebuild() { dialog.error = Some(error); unsafe { EndDialog(window, -1); } } } }
                _ => return 0,
            } 1
        }
        WM_DPICHANGED => { dialog.resize((wp & 0xffff) as u32, (lp != 0).then(|| unsafe { *(lp as *const RECT) })); 1 }
        WM_THEMECHANGED => { dialog.apply_theme(); 1 }
        WM_ERASEBKGND => { unsafe { SetWindowLongPtrW(window, DWLP_MSGRESULT as i32, 1); } 1 }
        WM_PAINT => { let mut paint = PAINTSTRUCT::default(); let dc = unsafe { BeginPaint(window, &mut paint) }; dialog.paint(dc); unsafe { EndPaint(window, &paint); } 1 }
        WM_PRINTCLIENT => { dialog.paint(wp as HDC); 1 }
        WM_CLOSE => { unsafe { EndDialog(window, 0); } 1 }
        _ => 0,
    }
}
fn show(parent: HWND, title: &str, entries: Vec<crate::dotnet::InspectedFilter>, current: Option<&PluginConfig>) -> Result<Option<(PluginConfig, FilterMetadata)>, String> {
    let state = RefCell::new(Dialog::new(title, entries, current)); let template = template();
    let result = unsafe { DialogBoxIndirectParamW(GetModuleHandleW(ptr::null()), &template.dialog, parent, Some(procedure), &state as *const RefCell<Dialog> as isize) };
    let mut state = state.into_inner(); if let Some(error) = state.error.take() { return Err(error); }
    if result == -1 { return Err(std::io::Error::last_os_error().to_string()); } Ok(state.result.take())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn entry() -> crate::dotnet::InspectedFilter {
        crate::dotnet::InspectedFilter {
            config: PluginConfig { path: PathBuf::from("E:/AgentWork/tmp/fixture.dll"), kind: PluginKind::Dotnet,
                enabled: false, type_name: "Fixture.Binding".into(), settings_json: "{}".into() },
            metadata: FilterMetadata { category: "binding".into(), type_name: "Fixture.Binding".into(), properties: vec![
                crate::dotnet::PropertyMetadata { name: "Count".into(), property_type: "System.Int32".into(), writable: true, ..Default::default() },
                crate::dotnet::PropertyMetadata { name: "Enabled".into(), property_type: "System.Boolean".into(), writable: true, default_value: Some(true.into()), ..Default::default() },
                crate::dotnet::PropertyMetadata { name: "Missing".into(), property_type: "System.Double".into(), writable: true, default_value: Some(4.into()), ..Default::default() },
            ], ..Default::default() },
        }
    }
    #[test]
    fn stale_inspection_and_modal_guards_reject_edits_selection_reuse_and_shutdown() {
        READY.with(|ready| *ready.borrow_mut() = None);
        let mut app = super::super::area_preview_tests::fixture(ptr::null_mut());
        let guard = Guard { token: app.managed_token, revision: app.edit_revision, generation: app.metadata_generation,
            path: app.profile_path.clone(), target: Target::Output, current: None };
        assert!(guard.valid(&app));
        app.edit_revision += 1;
        inspected(&mut app, guard.clone(), Ok(vec![entry()]));
        assert!(READY.with(|ready| ready.borrow().is_none()));
        app.edit_revision -= 1;
        app.profile_path = PathBuf::from("other-device.toml"); assert!(!guard.valid(&app)); app.profile_path = guard.path.clone();
        app.editor.profile.managed_output = Some(entry().config); assert!(!guard.valid(&app)); app.editor.profile.managed_output = None;
        app.closing = true; assert!(!guard.valid(&app)); app.closing = false;
        app.control_busy = true; assert!(!guard.valid(&app)); app.control_busy = false;
        app.update_restart_pending = true; assert!(!guard.valid(&app)); app.update_restart_pending = false;
        // A new App/request cannot inherit an old result even if HWND, path,
        // editor revision and initial settings happen to be identical.
        app.managed_token = new_token(); assert!(!guard.valid(&app));
        inspected(&mut app, guard, Ok(vec![entry()])); assert!(READY.with(|ready| ready.borrow().is_none()));
        drop(app); LOOK.with(|slot| slot.borrow_mut().take());
    }
    #[test]
    fn drafts_preserve_missing_null_unknown_settings_and_real_choice_types() {
        let item = entry(); let config = PluginConfig { settings_json: r#"{"Count":2,"Enabled":null,"Unknown":{"keep":1}}"#.into(), ..item.config.clone() };
        let dialog = Dialog::new("binding", vec![item], Some(&config));
        assert_eq!(dialog.drafts[0].settings_json, config.settings_json);
        let fields = dialog.fields().unwrap();
        let missing = fields.iter().find(|field| field.key == "Missing").unwrap();
        assert!(matches!(missing.value, model::PropertyValue::Typed { saved: None, .. }));
        let boolean = fields.iter().find(|field| field.key == "Enabled").unwrap();
        assert_eq!(actual_choices(&boolean.value), vec![("True".into(), true.into()), ("False".into(), false.into())]);
        let count = fields.iter().find(|field| field.key == "Count").unwrap();
        assert!(model::parse_property("2.5", &count.value).is_err());
        let json = model::set_plugin_property(&config.settings_json, "Count", model::parse_property("3", &count.value).unwrap()).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["Count"], 3); assert!(value["Enabled"].is_null()); assert!(value.get("Missing").is_none()); assert_eq!(value["Unknown"]["keep"], 1);
        let fresh = Dialog::new("binding", vec![entry()], None);
        assert_eq!(fresh.drafts[0].settings_json, "{}", "attribute metadata must not populate a fresh store");
    }
    #[test]
    fn hidden_managed_editor_validates_and_paints_theme_dpi_without_loading_dll() {
        let fonts = FontSet::new(96);
        LOOK.with(|slot| *slot.borrow_mut() = Some(Look { style: Style { palette: Palette::dark(), fonts: fonts.fonts, scale: 1.0 },
            controls: HashMap::new(), tab: Tab::Output, menu_open: 0, filters: Vec::new(), log: VecDeque::new(), log_columns: [0; 3], brushes: RefCell::new(Vec::new()) }));
        let state = RefCell::new(Dialog::new("binding", vec![entry()], None)); let template = template();
        let window = unsafe { CreateDialogIndirectParamW(GetModuleHandleW(ptr::null()), &template.dialog, ptr::null_mut(), Some(procedure), &state as *const RefCell<Dialog> as isize) };
        assert!(!window.is_null()); assert!(state.borrow().error.is_none()); assert_eq!(unsafe { IsWindowVisible(window) }, 0);
        let count = state.borrow().rows[0].hwnd;
        set_text(count, "invalid"); assert!(state.borrow().rows[0].invalid);
        set_text(count, "7"); assert!(!state.borrow().rows[0].invalid);
        let json: serde_json::Value = serde_json::from_str(&state.borrow().drafts[0].settings_json).unwrap();
        assert_eq!(json["Count"], 7); assert!(json.get("Enabled").is_none()); assert!(json.get("Missing").is_none());
        let dc = unsafe { CreateCompatibleDC(ptr::null_mut()) };
        let info = BITMAPINFO { bmiHeader: BITMAPINFOHEADER { biSize: size_of::<BITMAPINFOHEADER>() as u32, biWidth: 1200, biHeight: -1000,
            biPlanes: 1, biBitCount: 32, biCompression: BI_RGB, ..Default::default() }, ..Default::default() };
        let mut bits = ptr::null_mut(); let bitmap = unsafe { CreateDIBSection(dc, &info, DIB_RGB_COLORS, &mut bits, ptr::null_mut(), 0) };
        assert!(!dc.is_null() && !bitmap.is_null()); let previous = unsafe { SelectObject(dc, bitmap) };
        for palette in [Palette::dark().with_accent(Accent::Red), Palette::light().with_accent(Accent::Green), Palette::high_contrast()] {
            update_look(|look| look.style.palette = palette);
            for dpi in [96, 144, 192] {
                state.borrow_mut().resize(dpi, None); unsafe { SendMessageW(window, WM_THEMECHANGED, 0, 0); SendMessageW(window, WM_PRINTCLIENT, dc as usize, 0); }
                assert_eq!(unsafe { GetPixel(dc, 0, 0) }, palette.window.colorref());
                unsafe { SendMessageW(window, WM_CTLCOLOREDIT, dc as usize, count as isize); }
                assert_eq!(unsafe { GetTextColor(dc) }, palette.text.colorref()); assert_eq!(unsafe { GetBkColor(dc) }, palette.field.colorref());
            }
        }
        let controls: Vec<_> = state.borrow().controls.iter().map(|(control, _)| *control as isize).collect();
        unsafe { DestroyWindow(window); SelectObject(dc, previous); DeleteObject(bitmap); DeleteDC(dc); }
        drop(state);
        assert!(with_look(|look| controls.iter().all(|control| !look.controls.contains_key(control))).unwrap());
        LOOK.with(|slot| slot.borrow_mut().take());
    }
}
