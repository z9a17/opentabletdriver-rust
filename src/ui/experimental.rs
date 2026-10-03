//! Experimental tab for optional scheduling settings. Validation is local;
//! saving and daemon control run on the panel's background client.
use super::*;
use crate::experimental::{Settings, format_cpus, parse_cpus};
use windows_sys::Win32::UI::HiDpi::{DDC_DISABLE_ALL, SetDialogDpiChangeBehavior};

const TITLE: u16 = 104;
const NOTICE: u16 = 103;
const DESCRIPTION: u16 = 105;
const AVAILABLE: u16 = 106;
const HELP: u16 = 107;

thread_local! {
    static WINDOW: Cell<HWND> = const { Cell::new(ptr::null_mut()) };
}

/// Called with the other secondary windows after the panel updates LOOK.
pub(super) fn refresh_theme() {
    WINDOW.with(|window| {
        if !window.get().is_null() {
            unsafe { SendMessageW(window.get(), WM_THEMECHANGED, 0, 0); }
        }
    });
}

#[repr(C, align(4))]
struct Template {
    dialog: DLGTEMPLATE,
    menu: u16,
    class: u16,
    title: u16,
}

struct Dialog {
    window: HWND,
    settings: Settings,
    fields: [HWND; 2],
    controls: Vec<(HWND, RECT)>,
    fonts: Option<FontSet>,
    notice: HWND,
    result: Option<Settings>,
    error: Option<String>,
    load_warning: Option<String>,
    invalid: bool,
    busy: bool,
    dark_mode: theme::DarkMode,
    icons: [HICON; 2],
}

impl Dialog {
    fn add(&mut self, class: &str, title: &str, id: u16, style: u32, bounds: RECT) -> Result<HWND, String> {
        let control = unsafe { CreateWindowExW(0, wide(class).as_ptr(), wide(title).as_ptr(),
            WS_CHILD | WS_VISIBLE | style, 0, 0, 0, 0, self.window, id as usize as _,
            GetModuleHandleW(ptr::null()), ptr::null()) };
        if control.is_null() { return Err(std::io::Error::last_os_error().to_string()); }
        let kind = match class {
            "EDIT" => Kind::Field,
            "BUTTON" => Kind::Button,
            _ => Kind::Label,
        };
        update_look(|look| {
            look.controls.insert(control as isize, ControlInfo {
                kind, surface: if kind == Kind::Button {
                    if self.embedded() { Surface::Page } else { Surface::Window }
                } else { Surface::Group },
            });
        });
        self.controls.push((control, bounds));
        Ok(control)
    }

    fn initialize(&mut self, window: HWND) -> Result<(), String> {
        self.window = window;
        if unsafe { SetDialogDpiChangeBehavior(window, DDC_DISABLE_ALL, DDC_DISABLE_ALL) } == 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        set_text(window, "Experimental settings");
        self.add("STATIC", "CPU affinity", TITLE, SS_LEFT, rect(16,16,604,38))?;
        self.add("STATIC", "Choose logical CPUs independently for the GUI and driver process. Use All for automatic scheduling, or numbers such as 0,2,4-7. CPU numbers start at 0; they are logical processors, not physical cores.",
            DESCRIPTION, SS_LEFT | SS_NOPREFIX, rect(16,44,604,98))?;
        for (index, label) in ["&GUI CPUs", "&Driver CPUs"].iter().enumerate() {
            let top = 112 + index as i32 * 42;
            self.add("STATIC", label, 0, SS_LEFT, rect(16,top+4,124,top+28))?;
            let cpus = if index == 0 { &self.settings.ui_cpus } else { &self.settings.driver_cpus };
            self.fields[index] = self.add("EDIT", &format_cpus(cpus), 100 + index as u16,
                WS_TABSTOP | ES_AUTOHSCROLL as u32, rect(130,top,604,top+28))?;
            unsafe {
                SendMessageW(self.fields[index], EM_LIMITTEXT, 256, 0);
                SendMessageW(self.fields[index], EM_SETMARGINS,
                    (EC_LEFTMARGIN | EC_RIGHTMARGIN) as usize, 0);
            }
        }
        let available = crate::experimental::masks().map(|(_, mask)| {
            let cpus: Vec<u16> = (0..64).filter(|cpu| mask & (1usize << cpu) != 0).collect();
            format!("Available logical CPUs: {}.", format_cpus(&cpus))
        }).unwrap_or_else(|error| error);
        self.add("STATIC", &available, AVAILABLE, SS_LEFT | SS_NOPREFIX, rect(16,198,604,242))?;
        self.add("STATIC", "Save and apply stores these choices for future launches and updates both processes without restarting tablet input. All resets a previous CPU selection. On systems with multiple Windows processor groups, keep both fields set to All.",
            HELP, SS_LEFT | SS_NOPREFIX, rect(16,250,604,306))?;
        self.notice = self.add("STATIC", self.load_warning.clone().unwrap_or_else(||
            "Changes are experimental. The Console reports save/application failures.".into()).as_str(),
            NOTICE, SS_LEFT | SS_NOPREFIX, rect(16,316,604,374))?;
        self.add("BUTTON", "&All CPUs", 102, WS_TABSTOP | BS_PUSHBUTTON as u32, rect(16,388,124,420))?;
        self.add("BUTTON", "&Save and apply", IDOK as u16, WS_TABSTOP | BS_DEFPUSHBUTTON as u32, rect(328,388,474,420))?;
        self.add("BUTTON", if self.embedded() { "&Reload saved" } else { "Cancel" }, IDCANCEL as u16,
            WS_TABSTOP | BS_PUSHBUTTON as u32, rect(490,388,604,420))?;
        self.resize(unsafe { GetDpiForWindow(window) }.max(96), None);
        self.apply_theme();
        Ok(())
    }

    fn apply_theme(&self) {
        let Some(palette) = with_look(|look| look.style.palette) else { return; };
        self.dark_mode.apply_title_bar(self.window, palette.dark);
        for field in self.fields {
            self.dark_mode.apply_control(field, palette.dark);
        }
        unsafe {
            RedrawWindow(self.window, ptr::null(), ptr::null_mut(),
                RDW_INVALIDATE | RDW_ERASE | RDW_FRAME | RDW_ALLCHILDREN);
        }
    }

    fn paint(&self, dc: HDC) {
        with_look(|look| {
            let Some(fonts) = &self.fonts else { return; };
            let dpi = unsafe { GetDpiForWindow(self.window) }.max(96);
            let style = Style { fonts: fonts.fonts, scale: dpi as f32 / 96.0, ..look.style };
            let client = client_rect(self.window);
            let Some(mut canvas) = canvas::Canvas::new(dc, client) else { return; };
            canvas.fill(client, if self.embedded() { style.palette.page } else { style.palette.window });
            draw::group_box(&mut canvas, rect(scale(8,dpi),scale(8,dpi),scale(612,dpi),scale(382,dpi)),
                &style, style.palette.group);
            for field in self.fields {
                let mut bounds = RECT::default();
                unsafe {
                    GetWindowRect(field, &mut bounds);
                    MapWindowPoints(ptr::null_mut(), self.window, (&mut bounds as *mut RECT).cast(), 2);
                }
                let padding = scale(3,dpi);
                draw::field_frame(&mut canvas, draw::inset(bounds,-padding,-padding), &style,
                    unsafe { GetFocus() } == field, self.invalid, unsafe { IsWindowEnabled(field) } == 0);
            }
            canvas.present(dc);
        });
    }

    fn resize(&mut self, dpi: u32, suggested: Option<RECT>) {
        let fonts = FontSet::new(dpi);
        for (control, bounds) in &self.controls {
            let font = if unsafe { GetDlgCtrlID(*control) } == i32::from(TITLE) {
                fonts.fonts.bold
            } else { fonts.fonts.ui };
            let bounds = if self.fields.contains(control) {
                draw::inset(*bounds,3,3)
            } else { *bounds };
            unsafe {
                SendMessageW(*control, WM_SETFONT, font as usize, 1);
                SetWindowPos(*control, ptr::null_mut(), scale(bounds.left,dpi), scale(bounds.top,dpi),
                    scale(bounds.right-bounds.left,dpi), scale(bounds.bottom-bounds.top,dpi),
                    SWP_NOZORDER | SWP_NOACTIVATE);
            }
        }
        self.fonts = Some(fonts);
        if self.embedded() { return; }
        for (index, (kind, metric)) in [(ICON_BIG, SM_CXICON), (ICON_SMALL, SM_CXSMICON)].into_iter().enumerate() {
            let icon = canvas::app_icon(unsafe { GetSystemMetricsForDpi(metric,dpi) }.max(16));
            if !icon.is_null() {
                unsafe { SendMessageW(self.window, WM_SETICON, kind as usize, icon as isize); }
                let previous = std::mem::replace(&mut self.icons[index], icon);
                if !previous.is_null() { unsafe { DestroyIcon(previous); } }
            }
        }
        let mut bounds = rect(0,0,scale(620,dpi),scale(436,dpi));
        unsafe {
            AdjustWindowRectExForDpi(&mut bounds, WS_CAPTION | WS_SYSMENU | DS_MODALFRAME as u32,
                0, WS_EX_DLGMODALFRAME, dpi);
            let mut parent = RECT::default();
            GetWindowRect(GetParent(self.window), &mut parent);
            let (width,height) = (bounds.right-bounds.left,bounds.bottom-bounds.top);
            let (left,top) = suggested.map(|area| (area.left,area.top)).unwrap_or(
                ((parent.left+parent.right-width)/2,(parent.top+parent.bottom-height)/2));
            SetWindowPos(self.window, ptr::null_mut(),left,top,width,height,SWP_NOZORDER | SWP_NOACTIVATE);
            SendMessageW(self.window, DM_REPOSITION, 0, 0);
        }
    }

    fn accept(&mut self) {
        let candidate = (|| {
            let settings = Settings { ui_cpus: parse_cpus(&text(self.fields[0]))?,
                driver_cpus: parse_cpus(&text(self.fields[1]))? };
            crate::experimental::validate(&settings)?;
            Ok::<_, String>(settings)
        })();
        match candidate {
            Ok(settings) => {
                self.result = Some(settings);
                if self.embedded() {
                    unsafe { PostMessageW(GetParent(self.window), WM_EXPERIMENTAL_APPLY, 0, 0); }
                } else { unsafe { EndDialog(self.window, IDOK as isize); } }
            }
            Err(error) => {
                self.invalid = true;
                set_text(self.notice, &error);
                unsafe {
                    InvalidateRect(self.window, ptr::null(), 0);
                    InvalidateRect(self.notice, ptr::null(), 1);
                }
            }
        }
    }

    fn clear_validation(&mut self) {
        if !self.invalid { return; }
        self.invalid = false;
        set_text(self.notice, self.load_warning.as_deref().unwrap_or(
            "Changes are experimental. The Console reports save/application failures."));
        unsafe {
            InvalidateRect(self.window, ptr::null(), 0);
            InvalidateRect(self.notice, ptr::null(), 1);
        }
    }

    fn embedded(&self) -> bool {
        (unsafe { GetWindowLongW(self.window, GWL_STYLE) }) as u32 & WS_CHILD != 0
    }

    fn reload(&mut self) {
        let (settings, warning) = load_choices();
        self.settings = settings;
        self.load_warning = warning;
        self.invalid = false;
        self.result = None;
        for (field, cpus) in self.fields.iter().zip([&self.settings.ui_cpus, &self.settings.driver_cpus]) {
            set_text(*field, &format_cpus(cpus));
        }
        set_text(self.notice, self.load_warning.as_deref().unwrap_or("Reloaded saved CPU choices. Changes apply only when you choose Save and apply."));
        unsafe { RedrawWindow(self.window, ptr::null(), ptr::null_mut(), RDW_INVALIDATE | RDW_ALLCHILDREN); }
    }
}

impl Drop for Dialog {
    fn drop(&mut self) {
        update_look(|look| {
            for (control, _) in &self.controls { look.controls.remove(&(*control as isize)); }
        });
        for icon in self.icons {
            if !icon.is_null() { unsafe { DestroyIcon(icon); } }
        }
    }
}

unsafe extern "system" fn procedure(window: HWND, message: u32, wp: WPARAM, lp: LPARAM) -> isize {
    if message == WM_INITDIALOG { unsafe { SetWindowLongPtrW(window, GWLP_USERDATA, lp); } }
    let state = unsafe { GetWindowLongPtrW(window, GWLP_USERDATA) } as *const RefCell<Dialog>;
    if state.is_null() { return 0; }
    // Painting can reenter while initialize/resize/text changes hold Dialog.
    // Read shared LOOK without borrowing the mutable dialog state.
    match message {
        WM_CTLCOLORDLG => return with_look(|look| {
            let embedded = unsafe { GetWindowLongW(window, GWL_STYLE) } as u32 & WS_CHILD != 0;
            look.brush(if embedded { look.style.palette.page } else { look.style.palette.window }) as isize
        }).unwrap_or(0),
        WM_CTLCOLOREDIT | WM_CTLCOLORSTATIC => {
            let control = lp as HWND;
            let result = ctl_color(wp as HDC, control).unwrap_or(0);
            with_look(|look| {
                let palette = look.style.palette;
                let color = match unsafe { GetDlgCtrlID(control) } as u16 {
                    DESCRIPTION | AVAILABLE | HELP => palette.muted,
                    NOTICE => {
                        let state = unsafe { &*state }.try_borrow().ok();
                        if state.as_ref().is_some_and(|state| state.invalid) { palette.error }
                        else if state.as_ref().is_some_and(|state| state.load_warning.is_some()) { palette.warning }
                        else { palette.muted }
                    }
                    _ => return,
                };
                unsafe { SetTextColor(wp as HDC, color.colorref()); }
            });
            return result;
        }
        WM_NOTIFY if lp != 0 => {
            let header = unsafe { &*(lp as *const NMHDR) };
            if header.code == NM_CUSTOMDRAW {
                let result = custom_draw(unsafe { &mut *(lp as *mut NMCUSTOMDRAW) });
                // Dialog procedures return notifications through DWLP_MSGRESULT.
                unsafe { SetWindowLongPtrW(window, DWLP_MSGRESULT as i32, result); }
                return 1;
            }
        }
        WM_SETTINGCHANGE | WM_SYSCOLORCHANGE => { with_app(App::apply_theme); }
        WM_NCDESTROY => {
            WINDOW.with(|active| { if active.get() == window { active.set(ptr::null_mut()); } });
            unsafe { SetWindowLongPtrW(window,GWLP_USERDATA,0); }
            return 0;
        }
        _ => {}
    }
    // Native control creation/text changes can dispatch messages synchronously.
    let Ok(mut state) = (unsafe { &*state }).try_borrow_mut() else { return 0; };
    match message {
        WM_INITDIALOG => {
            WINDOW.with(|active| active.set(window));
            if let Err(error) = state.initialize(window) {
                state.error = Some(error);
                unsafe { EndDialog(window, -1); }
            }
            1
        }
        WM_COMMAND => {
            let notification = (wp >> 16) as u16;
            if state.fields.contains(&(lp as HWND)) {
                if notification == EN_SETFOCUS as u16 || notification == EN_KILLFOCUS as u16 {
                    unsafe { InvalidateRect(window, ptr::null(), 0); }
                }
                if notification == EN_CHANGE as u16 { state.clear_validation(); }
                return 1;
            }
            match (wp & 0xffff) as u16 {
                id if id == IDOK as u16 => state.accept(),
                id if id == IDCANCEL as u16 => {
                    if state.embedded() { state.reload(); } else { unsafe { EndDialog(window, 0); } }
                }
                102 => { for field in state.fields { set_text(field,"All"); } state.clear_validation(); }
                _ => return 0,
            }
            1
        }
        WM_DPICHANGED => {
            state.resize((wp & 0xffff) as u32, (lp != 0).then(|| unsafe { *(lp as *const RECT) }));
            1
        }
        WM_THEMECHANGED => { state.apply_theme(); 1 }
        WM_ERASEBKGND => { unsafe { SetWindowLongPtrW(window,DWLP_MSGRESULT as i32,1); } 1 }
        WM_PAINT => {
            let mut paint = PAINTSTRUCT::default();
            let dc = unsafe { BeginPaint(window,&mut paint) };
            state.paint(dc);
            unsafe { EndPaint(window,&paint); }
            1
        }
        WM_PRINTCLIENT => { state.paint(wp as HDC); 1 }
        WM_CLOSE => {
            if !state.embedded() { unsafe { EndDialog(window,0); } }
            1
        }
        _ => 0,
    }
}

fn load_choices() -> (Settings, Option<String>) {
    match crate::experimental::load() {
        Ok(settings) => (settings,None),
        Err(error) => (Settings::default(), Some(format!("Could not load saved choices: {error}. Save and apply replaces them with the values shown and retains a backup."))),
    }
}

pub(super) struct Page {
    window: HWND,
    dpi: Cell<u32>,
    // Box keeps GWLP_USERDATA stable when the parent App moves.
    state: Box<RefCell<Dialog>>,
}

impl Page {
    pub(super) fn create(parent: HWND) -> Result<Self, String> {
        let (settings, load_warning) = load_choices();
        let state = Box::new(RefCell::new(Dialog {
            window: ptr::null_mut(), settings, fields: [ptr::null_mut(); 2], controls: Vec::new(),
            fonts: None, notice: ptr::null_mut(), result: None, error: None, load_warning,
            invalid: false, busy: false, dark_mode: theme::DarkMode::load(), icons: [ptr::null_mut(); 2],
        }));
        let template = Template { dialog: DLGTEMPLATE {
            style: WS_CHILD | DS_CONTROL as u32,
            dwExtendedStyle: WS_EX_CONTROLPARENT, cx: 340, cy: 250, ..Default::default()
        }, menu: 0, class: 0, title: 0 };
        let window = unsafe { CreateDialogIndirectParamW(GetModuleHandleW(ptr::null()), &template.dialog,
            parent, Some(procedure), state.as_ref() as *const RefCell<Dialog> as isize) };
        if window.is_null() { return Err(std::io::Error::last_os_error().to_string()); }
        let page = Self { window, state, dpi: Cell::new(unsafe { GetDpiForWindow(window) }.max(96)) };
        let error = page.state.borrow_mut().error.take();
        if let Some(error) = error { return Err(error); }
        Ok(page)
    }

    pub(super) fn window(&self) -> HWND { self.window }

    pub(super) fn set_dpi(&self, dpi: u32) {
        if self.dpi.replace(dpi) != dpi {
            self.state.borrow_mut().resize(dpi, None);
        }
    }

    pub(super) fn take_result(&self) -> Option<Settings> { self.state.borrow_mut().result.take() }

    pub(super) fn accept(&self) { self.state.borrow_mut().accept(); }

    #[cfg(test)]
    pub(super) fn render(&self, dc: HDC) {
        let parent = unsafe { GetParent(self.window) };
        let mut origin = POINT::default();
        unsafe {
            MapWindowPoints(self.window, parent, &mut origin, 1);
            let saved = SaveDC(dc);
            SetViewportOrgEx(dc, origin.x, origin.y, ptr::null_mut());
            SendMessageW(self.window, WM_PRINTCLIENT, dc as usize, 0);
            RestoreDC(dc, saved);
        }
        for (control, _) in &self.state.borrow().controls {
            let mut origin = POINT::default();
            unsafe {
                MapWindowPoints(*control, parent, &mut origin, 1);
                let saved = SaveDC(dc);
                SetViewportOrgEx(dc, origin.x, origin.y, ptr::null_mut());
                SendMessageW(*control, WM_PRINT, dc as usize, (PRF_CLIENT | PRF_ERASEBKGND) as isize);
                RestoreDC(dc, saved);
            }
        }
    }

    pub(super) fn set_busy(&self, busy: bool) {
        self.state.borrow_mut().busy = busy;
        if busy { set_text(self.state.borrow().notice, "Saving and applying CPU affinity. The Console reports any failure."); }
        unsafe { EnableWindow(self.window, i32::from(!busy)); }
    }

    pub(super) fn complete(&self, result: &Result<(), String>) {
        let mut state = self.state.borrow_mut();
        if !state.busy { return; }
        state.busy = false;
        state.load_warning = result.as_ref().err().cloned();
        set_text(state.notice, state.load_warning.as_deref().unwrap_or("Saved and applied CPU choices. Tablet input was not restarted."));
        unsafe { EnableWindow(self.window, 1); RedrawWindow(self.window, ptr::null(), ptr::null_mut(), RDW_INVALIDATE | RDW_ALLCHILDREN); }
    }
}

impl Drop for Page {
    fn drop(&mut self) { unsafe { DestroyWindow(self.window); } }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Bitmap { dc: HDC, bitmap: HBITMAP, previous: HGDIOBJ, bits: *mut u32, width: i32, height: i32 }
    impl Bitmap {
        fn new(width: i32, height: i32) -> Self {
            let info = BITMAPINFO { bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32, biWidth: width, biHeight: -height,
                biPlanes: 1, biBitCount: 32, biCompression: BI_RGB, ..Default::default()
            }, ..Default::default() };
            unsafe {
                let dc = CreateCompatibleDC(ptr::null_mut());
                assert!(!dc.is_null());
                let mut bits = ptr::null_mut();
                let bitmap = CreateDIBSection(dc,&info,DIB_RGB_COLORS,&mut bits,ptr::null_mut(),0);
                assert!(!bitmap.is_null() && !bits.is_null());
                let previous = SelectObject(dc,bitmap);
                Self { dc,bitmap,previous,bits: bits.cast(),width,height }
            }
        }
        fn pixel(&self, x: i32, y: i32) -> u32 { unsafe { GetPixel(self.dc,x,y) } }
        fn save(&self, path: &Path) {
            unsafe { GdiFlush(); }
            let pixels = unsafe { std::slice::from_raw_parts(self.bits, (self.width*self.height) as usize) };
            let mut bytes = Vec::new();
            bytes.extend_from_slice(b"BM");
            bytes.extend_from_slice(&(54 + pixels.len() as u32*4).to_le_bytes());
            bytes.extend_from_slice(&[0;4]);
            bytes.extend_from_slice(&54u32.to_le_bytes());
            bytes.extend_from_slice(&40u32.to_le_bytes());
            bytes.extend_from_slice(&self.width.to_le_bytes());
            bytes.extend_from_slice(&(-self.height).to_le_bytes());
            bytes.extend_from_slice(&1u16.to_le_bytes());
            bytes.extend_from_slice(&32u16.to_le_bytes());
            bytes.extend_from_slice(&[0;24]);
            for pixel in pixels { bytes.extend_from_slice(&pixel.to_le_bytes()); }
            std::fs::write(path,bytes).unwrap();
        }
    }
    impl Drop for Bitmap {
        fn drop(&mut self) {
            unsafe { SelectObject(self.dc,self.previous); DeleteObject(self.bitmap); DeleteDC(self.dc); }
        }
    }

    /// Real dialog/control procedures and shared custom draw, with no visible
    /// window, daemon client, application singleton or preferences writes.
    #[test]
    fn hidden_dialog_uses_panel_colors_and_drawing_across_themes() {
        let fonts = FontSet::new(96);
        LOOK.with(|slot| *slot.borrow_mut() = Some(Look {
            style: Style { palette: Palette::dark(), fonts: fonts.fonts, scale: 1.0 },
            controls: HashMap::new(), tab: Tab::Output, menu_open: 0,
            filters: Vec::new(), log: VecDeque::new(), log_columns: [0;3], brushes: RefCell::new(Vec::new()),
        }));
        let state = RefCell::new(Dialog { window: ptr::null_mut(), settings: Settings::default(),
            fields: [ptr::null_mut();2], controls: Vec::new(), fonts: None, notice: ptr::null_mut(),
            result: None, error: None, load_warning: None, invalid: false, busy: false,
            dark_mode: theme::DarkMode::load(), icons: [ptr::null_mut();2] });
        let template = Template { dialog: DLGTEMPLATE {
            style: WS_POPUP | WS_CAPTION | WS_SYSMENU | DS_MODALFRAME as u32,
            dwExtendedStyle: WS_EX_DLGMODALFRAME, cx: 340, cy: 250, ..Default::default()
        }, menu: 0, class: 0, title: 0 };
        let window = unsafe { CreateDialogIndirectParamW(GetModuleHandleW(ptr::null()), &template.dialog,
            ptr::null_mut(), Some(procedure), &state as *const RefCell<Dialog> as isize) };
        assert!(!window.is_null());
        assert!(state.borrow().error.is_none());
        assert_eq!(unsafe { IsWindowVisible(window) }, 0, "verification must stay hidden");
        assert_eq!(WINDOW.with(Cell::get), window);
        let dpi = unsafe { GetDpiForWindow(window) }.max(96);
        let client = client_rect(window);
        let bitmap = Bitmap::new(client.right,client.bottom);
        let high_contrast = Palette::high_contrast();
        for (name,palette,background,field,text_color,button) in [
            ("dark",Palette::dark(),0x00202020,0x001c1c1c,0x00f2f2f2,0x00373737),
            ("light",Palette::light(),0x00f0f0f0,0x00ffffff,0x001b1b1b,0x00e1e1e1),
            ("high-contrast",high_contrast,unsafe { GetSysColor(COLOR_WINDOW) },
                unsafe { GetSysColor(COLOR_WINDOW) },unsafe { GetSysColor(COLOR_WINDOWTEXT) },
                unsafe { GetSysColor(COLOR_BTNFACE) }),
        ] {
            update_look(|look| look.style.palette = palette);
            refresh_theme();
            unsafe { SendMessageW(window,WM_PRINTCLIENT,bitmap.dc as usize,0); }
            assert_eq!(bitmap.pixel(2,2),background,"{name} dialog background");
            for control in state.borrow().fields {
                assert_eq!(unsafe { GetWindowLongPtrW(control,GWL_STYLE) } as u32 & WS_BORDER, 0);
                let brush = unsafe { SendMessageW(window,WM_CTLCOLOREDIT,bitmap.dc as usize,control as isize) } as HBRUSH;
                assert_eq!(unsafe { GetBkColor(bitmap.dc) },field,"{name} edit background");
                assert_eq!(unsafe { GetTextColor(bitmap.dc) },text_color,"{name} edit text");
                let mut color = LOGBRUSH::default();
                assert_ne!(unsafe { GetObjectW(brush,size_of::<LOGBRUSH>() as i32,(&mut color as *mut LOGBRUSH).cast()) },0);
                assert_eq!(color.lbColor,field,"{name} edit brush");
            }
            let save = unsafe { GetDlgItem(window,IDOK) };
            let mut custom = NMCUSTOMDRAW { hdr: NMHDR { hwndFrom: save,idFrom: IDOK as usize,code: NM_CUSTOMDRAW },
                dwDrawStage: CDDS_PREPAINT,hdc: bitmap.dc,..Default::default() };
            let result = unsafe { SendMessageW(window,WM_NOTIFY,IDOK as usize,&mut custom as *mut NMCUSTOMDRAW as isize) };
            assert_eq!(result,CDRF_SKIPDEFAULT as isize,"{name} button must use shared custom draw");
            assert_eq!(bitmap.pixel(scale(10,dpi),scale(5,dpi)),button,"{name} button fill");
            // Rendering also works during synchronous control updates while
            // mutable dialog state is held, without falling back to white.
            {
                let _borrow = state.borrow_mut();
                let control = unsafe { GetDlgItem(window,100) };
                unsafe { SendMessageW(window,WM_CTLCOLOREDIT,bitmap.dc as usize,control as isize); }
                assert_eq!(unsafe { GetBkColor(bitmap.dc) },field);
                assert_eq!(unsafe { SendMessageW(window,WM_NOTIFY,IDOK as usize,&mut custom as *mut NMCUSTOMDRAW as isize) },CDRF_SKIPDEFAULT as isize);
            }
            // Capture the actual hidden control render, not a reconstruction.
            unsafe { SendMessageW(window,WM_PRINTCLIENT,bitmap.dc as usize,0); }
            for (control,_) in &state.borrow().controls {
                let mut origin = POINT::default();
                unsafe {
                    MapWindowPoints(*control,window,&mut origin,1);
                    let saved = SaveDC(bitmap.dc);
                    SetViewportOrgEx(bitmap.dc,origin.x,origin.y,ptr::null_mut());
                    SendMessageW(*control,WM_PRINT,bitmap.dc as usize,(PRF_CLIENT | PRF_ERASEBKGND) as isize);
                    RestoreDC(bitmap.dc,saved);
                }
            }
            if let Some(directory) = std::env::var_os("OTD_THEME_CAPTURE_DIR") {
                let directory = PathBuf::from(directory);
                std::fs::create_dir_all(&directory).unwrap();
                bitmap.save(&directory.join(format!("experimental-{name}.bmp")));
            }
        }
        update_look(|look| look.style.palette = Palette::dark());
        let field = state.borrow().fields[0];
        set_text(field,"invalid-cpu");
        state.borrow_mut().accept();
        assert!(state.borrow().invalid);
        let notice = state.borrow().notice;
        unsafe { SendMessageW(window,WM_CTLCOLORSTATIC,bitmap.dc as usize,notice as isize); }
        assert_eq!(unsafe { GetTextColor(bitmap.dc) },0x00a499ff,"dark validation error text");
        set_text(field,"All");
        assert!(!state.borrow().invalid,"editing must clear validation styling");
        set_text(field,"invalid-cpu");
        state.borrow_mut().accept();
        let reset = unsafe { GetDlgItem(window,102) };
        unsafe { SendMessageW(window,WM_COMMAND,102,reset as isize); }
        assert!(!state.borrow().invalid,"All CPUs must clear validation styling");
        assert!(state.borrow().fields.iter().all(|field| text(*field) == "All"));
        state.borrow_mut().resize(144,None);
        let font = unsafe { SendMessageW(field,WM_GETFONT,0,0) } as HFONT;
        let mut font_info = LOGFONTW::default();
        assert_ne!(unsafe { GetObjectW(font,size_of::<LOGFONTW>() as i32,(&mut font_info as *mut LOGFONTW).cast()) },0);
        assert_eq!(font_info.lfHeight,-18,"the dialog owns its 144-DPI font");
        unsafe { DestroyWindow(window); }
        assert!(WINDOW.with(Cell::get).is_null());
        drop(state);
        assert_eq!(with_look(|look| look.controls.len()),Some(0),"dialog metadata must be removed");
        LOOK.with(|slot| drop(slot.borrow_mut().take()));
    }
}
