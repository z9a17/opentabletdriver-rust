//! Modal capture of a key or shortcut for a binding, like upstream's
//! `BindingEditorDialog`. The capture field takes every key, Alt, F10, Tab,
//! Enter and Escape included. Once a shortcut is captured and released,
//! Enter accepts it and Escape cancels; any other key starts a new one.
use super::*;
use otd_core::actions::KeyboardUsage;
use windows_sys::Win32::UI::HiDpi::{DDC_DISABLE_ALL, SetDialogDpiChangeBehavior};

const CLEAR: u16 = 102;
const PROMPT: u16 = 103;
const HELP: u16 = 104;
const CAPTURE: u16 = 105;
const CAPTURE_CLASS: &str = "OpenTabletDriverRustShortcut";
/// Sent by the capture field to the dialog: WPARAM is the key message,
/// LPARAM its own LPARAM.
const WM_CAPTURED_KEY: u32 = WM_APP + 40;
const WAITING: &str = "Press a key or shortcut";
const HELP_TEXT: &str = "Hold modifiers such as Control or Shift, then press the key. Release, then press Enter or choose OK. Escape cancels once a shortcut is captured.";
const ENTER: u16 = 0x28;
const ESCAPE: u16 = 0x29;

thread_local! {
    static WINDOW: Cell<HWND> = const { Cell::new(ptr::null_mut()) };
}

/// Called with the other secondary windows after the panel updates LOOK.
pub(super) fn refresh_theme() {
    WINDOW.with(|window| {
        if !window.get().is_null() {
            unsafe {
                SendMessageW(window.get(), WM_THEMECHANGED, 0, 0);
            }
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
    /// What is being bound, for the title.
    target: String,
    keys: Vec<KeyboardUsage>,
    held: Vec<KeyboardUsage>,
    capture: HWND,
    controls: Vec<(HWND, RECT)>,
    fonts: Option<FontSet>,
    result: Option<Vec<KeyboardUsage>>,
    error: Option<String>,
    unsupported: bool,
    dark_mode: theme::DarkMode,
    icons: [HICON; 2],
}

fn is_modifier(key: KeyboardUsage) -> bool {
    (0xe0..=0xe7).contains(&key.usage())
}

impl Dialog {
    fn new(target: String) -> Self {
        Self {
            window: ptr::null_mut(),
            target,
            keys: Vec::new(),
            held: Vec::new(),
            capture: ptr::null_mut(),
            controls: Vec::new(),
            fonts: None,
            result: None,
            error: None,
            unsupported: false,
            dark_mode: theme::DarkMode::load(),
            icons: [ptr::null_mut(); 2],
        }
    }

    fn add(&mut self, class: &str, title: &str, id: u16, style: u32, bounds: RECT) -> Result<HWND, String> {
        let control = unsafe {
            CreateWindowExW(0, wide(class).as_ptr(), wide(title).as_ptr(), WS_CHILD | WS_VISIBLE | style,
                0, 0, 0, 0, self.window, id as usize as _, GetModuleHandleW(ptr::null()), ptr::null())
        };
        if control.is_null() {
            return Err(std::io::Error::last_os_error().to_string());
        }
        let kind = match class {
            "BUTTON" => Kind::Button,
            CAPTURE_CLASS => Kind::Field,
            _ => Kind::Label,
        };
        update_look(|look| {
            look.controls.insert(control as isize, ControlInfo {
                kind,
                surface: if kind == Kind::Field { Surface::Group } else { Surface::Window },
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
        register_capture_class()?;
        set_text(window, "Key binding");
        let prompt = format!("Shortcut for {}", self.target);
        self.add("STATIC", &prompt, PROMPT, SS_LEFT | SS_NOPREFIX, rect(16, 16, 444, 38))?;
        self.capture = self.add(CAPTURE_CLASS, WAITING, CAPTURE, WS_TABSTOP, rect(16, 46, 444, 86))?;
        self.add("STATIC", HELP_TEXT, HELP, SS_LEFT | SS_NOPREFIX, rect(16, 96, 444, 140))?;
        self.add("BUTTON", "Clear", CLEAR, WS_TABSTOP | BS_PUSHBUTTON as u32, rect(16, 152, 116, 184))?;
        self.add("BUTTON", "OK", IDOK as u16, WS_TABSTOP | BS_DEFPUSHBUTTON as u32, rect(236, 152, 336, 184))?;
        self.add("BUTTON", "Cancel", IDCANCEL as u16, WS_TABSTOP | BS_PUSHBUTTON as u32, rect(344, 152, 444, 184))?;
        self.resize(unsafe { GetDpiForWindow(window) }.max(96), None);
        self.apply_theme();
        self.update();
        unsafe {
            SetFocus(self.capture);
        }
        Ok(())
    }

    fn apply_theme(&self) {
        let Some(palette) = with_look(|look| look.style.palette) else {
            return;
        };
        self.dark_mode.apply_title_bar(self.window, palette.dark);
        unsafe {
            RedrawWindow(self.window, ptr::null(), ptr::null_mut(),
                RDW_INVALIDATE | RDW_ERASE | RDW_FRAME | RDW_ALLCHILDREN);
        }
    }

    fn paint(&self, dc: HDC) {
        with_look(|look| {
            let client = client_rect(self.window);
            let Some(mut canvas) = canvas::Canvas::new(dc, client) else {
                return;
            };
            canvas.fill(client, look.style.palette.window);
            canvas.present(dc);
        });
    }

    fn resize(&mut self, dpi: u32, suggested: Option<RECT>) {
        let fonts = FontSet::new(dpi);
        for (control, bounds) in &self.controls {
            let font = if unsafe { GetDlgCtrlID(*control) } == i32::from(PROMPT) {
                fonts.fonts.bold
            } else {
                fonts.fonts.ui
            };
            unsafe {
                SendMessageW(*control, WM_SETFONT, font as usize, 1);
                SetWindowPos(*control, ptr::null_mut(), scale(bounds.left, dpi), scale(bounds.top, dpi),
                    scale(bounds.right - bounds.left, dpi), scale(bounds.bottom - bounds.top, dpi),
                    SWP_NOZORDER | SWP_NOACTIVATE);
            }
        }
        self.fonts = Some(fonts);
        for (index, (kind, metric)) in [(ICON_BIG, SM_CXICON), (ICON_SMALL, SM_CXSMICON)].into_iter().enumerate() {
            let icon = canvas::app_icon(unsafe { GetSystemMetricsForDpi(metric, dpi) }.max(16));
            if !icon.is_null() {
                unsafe {
                    SendMessageW(self.window, WM_SETICON, kind as usize, icon as isize);
                }
                let previous = std::mem::replace(&mut self.icons[index], icon);
                if !previous.is_null() {
                    unsafe {
                        DestroyIcon(previous);
                    }
                }
            }
        }
        let mut bounds = rect(0, 0, scale(460, dpi), scale(200, dpi));
        unsafe {
            AdjustWindowRectExForDpi(&mut bounds, WS_CAPTION | WS_SYSMENU | DS_MODALFRAME as u32,
                0, WS_EX_DLGMODALFRAME, dpi);
            let mut parent = RECT::default();
            GetWindowRect(GetParent(self.window), &mut parent);
            let (width, height) = (bounds.right - bounds.left, bounds.bottom - bounds.top);
            let (left, top) = suggested.map(|area| (area.left, area.top)).unwrap_or((
                (parent.left + parent.right - width) / 2,
                (parent.top + parent.bottom - height) / 2,
            ));
            SetWindowPos(self.window, ptr::null_mut(), left, top, width, height, SWP_NOZORDER | SWP_NOACTIVATE);
            SendMessageW(self.window, DM_REPOSITION, 0, 0);
        }
    }

    /// Shows the shortcut so far and enables OK once there is one.
    fn update(&self) {
        let shown = if self.keys.is_empty() {
            if self.unsupported {
                "That key cannot be bound; press another".to_owned()
            } else {
                WAITING.to_owned()
            }
        } else {
            otd_core::keys::chord_text(&self.keys)
        };
        set_text(self.capture, &shown);
        unsafe {
            EnableWindow(GetDlgItem(self.window, IDOK), (!self.keys.is_empty()).into());
            InvalidateRect(self.capture, ptr::null(), 0);
        }
    }

    /// A key message from the capture field. Returns whether the dialog ends.
    fn key(&mut self, message: u32, lp: LPARAM) -> bool {
        let down = matches!(message, WM_KEYDOWN | WM_SYSKEYDOWN);
        let scan = ((lp >> 16) & 0xff) as u16;
        let extended = (lp >> 24) & 1 != 0;
        // Windows prefixes some extended keys with a fake Shift (E0 2A).
        let key = if matches!(scan, 0x2a | 0x36) && extended {
            None
        } else {
            crate::action_output::usage_for_scan_code(scan, extended)
                .filter(|key| otd_core::keys::name_of(*key).is_some())
        };
        if !down {
            if let Some(key) = key {
                self.held.retain(|held| *held != key);
            }
            return false;
        }
        // Auto-repeat of a held key.
        if lp & (1 << 30) != 0 {
            return false;
        }
        if self.held.is_empty() && !self.keys.is_empty() {
            match key.map(KeyboardUsage::usage) {
                Some(ENTER) => {
                    self.accept();
                    return true;
                }
                Some(ESCAPE) => {
                    unsafe {
                        EndDialog(self.window, 0);
                    }
                    return true;
                }
                _ => self.keys.clear(),
            }
        }
        self.unsupported = key.is_none();
        if let Some(key) = key {
            if !self.held.contains(&key) {
                self.held.push(key);
            }
            if !self.keys.contains(&key) && self.keys.len() < 8 {
                // Modifiers first, as the binding presses them.
                let at = if is_modifier(key) {
                    self.keys.iter().take_while(|held| is_modifier(**held)).count()
                } else {
                    self.keys.len()
                };
                self.keys.insert(at, key);
            }
        }
        self.update();
        false
    }

    fn accept(&mut self) {
        if self.keys.is_empty() {
            return;
        }
        self.result = Some(self.keys.clone());
        unsafe {
            EndDialog(self.window, IDOK as isize);
        }
    }

    fn clear(&mut self) {
        self.keys.clear();
        self.held.clear();
        self.unsupported = false;
        self.update();
        unsafe {
            SetFocus(self.capture);
        }
    }
}

impl Drop for Dialog {
    fn drop(&mut self) {
        update_look(|look| {
            for (control, _) in &self.controls {
                look.controls.remove(&(*control as isize));
            }
        });
        for icon in self.icons {
            if !icon.is_null() {
                unsafe {
                    DestroyIcon(icon);
                }
            }
        }
    }
}

fn register_capture_class() -> Result<(), String> {
    let name = wide(CAPTURE_CLASS);
    let class = WNDCLASSW {
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(capture_procedure),
        hInstance: unsafe { GetModuleHandleW(ptr::null()) },
        hCursor: unsafe { LoadCursorW(ptr::null_mut(), IDC_ARROW) },
        lpszClassName: name.as_ptr(),
        ..Default::default()
    };
    if unsafe { RegisterClassW(&class) } == 0
        && unsafe { GetLastError() } != windows_sys::Win32::Foundation::ERROR_CLASS_ALREADY_EXISTS
    {
        return Err(std::io::Error::last_os_error().to_string());
    }
    Ok(())
}

/// The capture field: a focusable box that wants every key and paints the
/// shortcut in the panel's field style. Keys go to the dialog.
unsafe extern "system" fn capture_procedure(window: HWND, message: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match message {
        WM_GETDLGCODE => (DLGC_WANTALLKEYS | DLGC_WANTCHARS) as LRESULT,
        WM_KEYDOWN | WM_SYSKEYDOWN | WM_KEYUP | WM_SYSKEYUP => {
            unsafe {
                SendMessageW(GetParent(window), WM_CAPTURED_KEY, message as usize, lp);
            }
            0
        }
        // No menu activation, mnemonics or beeps while capturing.
        WM_CHAR | WM_SYSCHAR | WM_DEADCHAR | WM_SYSDEADCHAR => 0,
        WM_SETFONT => {
            unsafe {
                SetWindowLongPtrW(window, GWLP_USERDATA, wp as isize);
            }
            0
        }
        WM_SETFOCUS | WM_KILLFOCUS | WM_SETTEXT | WM_ENABLE => {
            let result = unsafe { DefWindowProcW(window, message, wp, lp) };
            unsafe {
                InvalidateRect(window, ptr::null(), 0);
            }
            result
        }
        WM_LBUTTONDOWN => {
            unsafe {
                SetFocus(window);
            }
            0
        }
        WM_ERASEBKGND => 1,
        WM_PAINT => {
            let mut paint = PAINTSTRUCT::default();
            let dc = unsafe { BeginPaint(window, &mut paint) };
            paint_capture(window, dc);
            unsafe {
                EndPaint(window, &paint);
            }
            0
        }
        WM_PRINTCLIENT => {
            paint_capture(window, wp as HDC);
            0
        }
        _ => unsafe { DefWindowProcW(window, message, wp, lp) },
    }
}

fn paint_capture(window: HWND, dc: HDC) {
    with_look(|look| {
        let client = client_rect(window);
        let Some(mut canvas) = canvas::Canvas::new(dc, client) else {
            return;
        };
        let palette = look.style.palette;
        let parent = unsafe { GetParent(window) };
        canvas.fill(client, palette.window);
        let dpi = unsafe { GetDpiForWindow(window) }.max(96);
        let font = unsafe { GetWindowLongPtrW(window, GWLP_USERDATA) } as HFONT;
        let style = Style { scale: dpi as f32 / 96.0, ..look.style };
        let focused = unsafe { GetFocus() } == window;
        draw::field_frame(&mut canvas, client, &style, focused, false, false);
        let shown = text(window);
        let color = if shown == WAITING || parent.is_null() {
            palette.muted
        } else {
            palette.text
        };
        canvas.text(
            draw::inset(client, scale(10, dpi), 0),
            &shown,
            if font.is_null() { look.style.fonts.ui } else { font },
            color,
            DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX | DT_END_ELLIPSIS,
        );
        canvas.present(dc);
    });
}

unsafe extern "system" fn procedure(window: HWND, message: u32, wp: WPARAM, lp: LPARAM) -> isize {
    if message == WM_INITDIALOG {
        unsafe {
            SetWindowLongPtrW(window, GWLP_USERDATA, lp);
        }
    }
    let state = unsafe { GetWindowLongPtrW(window, GWLP_USERDATA) } as *const RefCell<Dialog>;
    if state.is_null() {
        return 0;
    }
    // Painting can reenter while handlers hold Dialog; read shared LOOK only.
    match message {
        WM_CTLCOLORDLG => {
            return with_look(|look| look.brush(look.style.palette.window) as isize).unwrap_or(0);
        }
        WM_CTLCOLORSTATIC => {
            let control = lp as HWND;
            let result = ctl_color(wp as HDC, control).unwrap_or(0);
            if unsafe { GetDlgCtrlID(control) } as u16 == HELP {
                with_look(|look| unsafe {
                    SetTextColor(wp as HDC, look.style.palette.muted.colorref());
                });
            }
            return result;
        }
        WM_NOTIFY if lp != 0 => {
            let header = unsafe { &*(lp as *const NMHDR) };
            if header.code == NM_CUSTOMDRAW {
                let result = custom_draw(unsafe { &mut *(lp as *mut NMCUSTOMDRAW) });
                // Dialog procedures return notifications through DWLP_MSGRESULT.
                unsafe {
                    SetWindowLongPtrW(window, DWLP_MSGRESULT as i32, result);
                }
                return 1;
            }
        }
        WM_SETTINGCHANGE | WM_SYSCOLORCHANGE => {
            with_app(App::apply_theme);
        }
        WM_NCDESTROY => {
            WINDOW.with(|active| {
                if active.get() == window {
                    active.set(ptr::null_mut());
                }
            });
            unsafe {
                SetWindowLongPtrW(window, GWLP_USERDATA, 0);
            }
            return 0;
        }
        _ => {}
    }
    let Ok(mut state) = (unsafe { &*state }).try_borrow_mut() else {
        return 0;
    };
    match message {
        WM_INITDIALOG => {
            WINDOW.with(|active| active.set(window));
            if let Err(error) = state.initialize(window) {
                state.error = Some(error);
                unsafe {
                    EndDialog(window, -1);
                }
            }
            // Focus was set on the capture field.
            0
        }
        WM_CAPTURED_KEY => {
            state.key(wp as u32, lp);
            1
        }
        WM_COMMAND => {
            match (wp & 0xffff) as u16 {
                id if id == IDOK as u16 => state.accept(),
                id if id == IDCANCEL as u16 => unsafe {
                    EndDialog(window, 0);
                },
                CLEAR => state.clear(),
                _ => return 0,
            }
            1
        }
        WM_DPICHANGED => {
            state.resize((wp & 0xffff) as u32, (lp != 0).then(|| unsafe { *(lp as *const RECT) }));
            1
        }
        WM_THEMECHANGED => {
            state.apply_theme();
            1
        }
        WM_ERASEBKGND => {
            unsafe {
                SetWindowLongPtrW(window, DWLP_MSGRESULT as i32, 1);
            }
            1
        }
        WM_PAINT => {
            let mut paint = PAINTSTRUCT::default();
            let dc = unsafe { BeginPaint(window, &mut paint) };
            state.paint(dc);
            unsafe {
                EndPaint(window, &paint);
            }
            1
        }
        WM_PRINTCLIENT => {
            state.paint(wp as HDC);
            1
        }
        WM_CLOSE => {
            unsafe {
                EndDialog(window, 0);
            }
            1
        }
        _ => 0,
    }
}

fn template() -> Template {
    Template {
        dialog: DLGTEMPLATE {
            style: WS_POPUP | WS_CAPTION | WS_SYSMENU | DS_MODALFRAME as u32,
            dwExtendedStyle: WS_EX_DLGMODALFRAME,
            cx: 250,
            cy: 120,
            ..Default::default()
        },
        menu: 0,
        class: 0,
        title: 0,
    }
}

/// Asks for a key or shortcut for `target`. `None` when cancelled.
pub(super) fn show(parent: HWND, target: &str) -> Result<Option<Vec<KeyboardUsage>>, String> {
    let state = RefCell::new(Dialog::new(target.to_owned()));
    let template = template();
    let result = unsafe {
        DialogBoxIndirectParamW(GetModuleHandleW(ptr::null()), &template.dialog, parent,
            Some(procedure), &state as *const RefCell<Dialog> as isize)
    };
    WINDOW.with(|window| window.set(ptr::null_mut()));
    let mut state = state.into_inner();
    if let Some(error) = state.error.take() {
        return Err(error);
    }
    if result == -1 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    Ok(state.result.take())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A WM_KEYDOWN/WM_KEYUP LPARAM for a scan code.
    fn key_lparam(scan: isize, extended: bool, up: bool) -> LPARAM {
        (scan << 16) | (isize::from(extended) << 24) | if up { 3 << 30 } else { 0 }
    }

    /// The real dialog and capture field procedures, hidden, with no daemon,
    /// application singleton or preferences.
    #[test]
    fn hidden_dialog_captures_shortcuts_and_uses_panel_colors() {
        let fonts = FontSet::new(96);
        LOOK.with(|slot| *slot.borrow_mut() = Some(Look {
            style: Style { palette: Palette::dark(), fonts: fonts.fonts, scale: 1.0 },
            controls: HashMap::new(), tab: Tab::Output, menu_open: 0,
            filters: Vec::new(), log: VecDeque::new(), log_columns: [0; 3], brushes: RefCell::new(Vec::new()),
        }));
        let state = RefCell::new(Dialog::new("Express Key 1".into()));
        let template = template();
        let window = unsafe {
            CreateDialogIndirectParamW(GetModuleHandleW(ptr::null()), &template.dialog, ptr::null_mut(),
                Some(procedure), &state as *const RefCell<Dialog> as isize)
        };
        assert!(!window.is_null());
        assert!(state.borrow().error.is_none());
        assert_eq!(unsafe { IsWindowVisible(window) }, 0, "verification must stay hidden");
        let capture = state.borrow().capture;
        assert_eq!(
            unsafe { SendMessageW(capture, WM_GETDLGCODE, 0, 0) } as u32 & DLGC_WANTALLKEYS,
            DLGC_WANTALLKEYS,
            "Tab, Enter and Escape reach the capture field"
        );
        let ok = unsafe { GetDlgItem(window, IDOK) };
        assert_eq!(unsafe { IsWindowEnabled(ok) }, 0, "nothing captured yet");
        let send = |scan: isize, extended: bool, up: bool| unsafe {
            SendMessageW(capture, if up { WM_KEYUP } else { WM_KEYDOWN }, 0, key_lparam(scan, extended, up));
        };
        // Right Control (E0 1D), Shift, Z, then a repeat of Z.
        send(0x1d, true, false);
        send(0x2a, false, false);
        send(0x2c, false, false);
        unsafe {
            SendMessageW(capture, WM_KEYDOWN, 0, key_lparam(0x2c, false, false) | 1 << 30);
        }
        assert_eq!(text(capture), "RightControl+LeftShift+Z");
        assert_ne!(unsafe { IsWindowEnabled(ok) }, 0);
        for (scan, extended) in [(0x2c, false), (0x2a, false), (0x1d, true)] {
            send(scan, extended, true);
        }
        // A new key after release starts again.
        send(0x3f, false, false);
        assert_eq!(text(capture), "F5");
        send(0x3f, false, true);
        // After Clear, Escape is an ordinary key to bind.
        let clear = unsafe { GetDlgItem(window, i32::from(CLEAR)) };
        unsafe {
            SendMessageW(window, WM_COMMAND, usize::from(CLEAR), clear as isize);
        }
        assert_eq!(text(capture), WAITING);
        send(0x01, false, false);
        assert_eq!(text(capture), "Escape");
        send(0x01, false, true);
        // Enter after a released shortcut accepts it.
        send(0x1c, false, false);
        assert_eq!(
            state.borrow().result.as_deref().map(otd_core::keys::chord_text).as_deref(),
            Some("Escape")
        );
        // Painting uses the panel palette in every theme.
        let dc = unsafe { CreateCompatibleDC(ptr::null_mut()) };
        let bitmap = unsafe { CreateCompatibleBitmap(GetDC(ptr::null_mut()), 64, 64) };
        let previous = unsafe { SelectObject(dc, bitmap) };
        for palette in [Palette::dark(), Palette::light(), Palette::high_contrast()] {
            update_look(|look| look.style.palette = palette);
            refresh_theme();
            unsafe {
                SendMessageW(window, WM_PRINTCLIENT, dc as usize, 0);
            }
            assert_eq!(unsafe { GetPixel(dc, 2, 2) }, palette.window.colorref());
            let help = unsafe { GetDlgItem(window, i32::from(HELP)) };
            unsafe {
                SendMessageW(window, WM_CTLCOLORSTATIC, dc as usize, help as isize);
            }
            assert_eq!(unsafe { GetTextColor(dc) }, palette.muted.colorref());
            let mut custom = NMCUSTOMDRAW {
                hdr: NMHDR { hwndFrom: ok, idFrom: IDOK as usize, code: NM_CUSTOMDRAW },
                dwDrawStage: CDDS_PREPAINT, hdc: dc, ..Default::default()
            };
            assert_eq!(
                unsafe { SendMessageW(window, WM_NOTIFY, IDOK as usize, &mut custom as *mut NMCUSTOMDRAW as isize) },
                CDRF_SKIPDEFAULT as isize,
                "buttons use the shared custom draw"
            );
        }
        unsafe {
            SelectObject(dc, previous);
            DeleteObject(bitmap);
            DeleteDC(dc);
            DestroyWindow(window);
        }
        assert!(WINDOW.with(Cell::get).is_null());
        drop(state);
        assert_eq!(with_look(|look| look.controls.len()), Some(0), "dialog metadata must be removed");
        LOOK.with(|slot| drop(slot.borrow_mut().take()));
    }
}
