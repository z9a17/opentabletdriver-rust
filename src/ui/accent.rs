//! Themed custom accent editor. Preferences change only after acceptance.
use super::*;
use windows_sys::Win32::UI::HiDpi::{DDC_DISABLE_ALL, SetDialogDpiChangeBehavior};

const HEX: u16 = 100;
const RED: u16 = 101;
const NOTICE: u16 = 104;
const HELP: &str = "Enter RGB values from 0 to 255, or a hex color such as #FF8C00.";

thread_local! {
    static WINDOW: Cell<HWND> = const { Cell::new(ptr::null_mut()) };
}

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

fn template() -> Template {
    Template {
        dialog: DLGTEMPLATE {
            style: WS_POPUP | WS_CAPTION | WS_SYSMENU | DS_MODALFRAME as u32,
            dwExtendedStyle: WS_EX_DLGMODALFRAME,
            cx: 260, cy: 190, ..Default::default()
        },
        menu: 0, class: 0, title: 0,
    }
}

struct Dialog {
    window: HWND,
    color: Rgb,
    fields: [HWND; 4],
    notice: HWND,
    controls: Vec<(HWND, RECT)>,
    fonts: Option<FontSet>,
    dpi: u32,
    invalid: bool,
    result: Option<Rgb>,
    error: Option<String>,
    dark_mode: theme::DarkMode,
}

impl Dialog {
    fn new(color: Rgb) -> Self {
        Self {
            window: ptr::null_mut(), color, fields: [ptr::null_mut(); 4],
            notice: ptr::null_mut(), controls: Vec::new(), fonts: None, dpi: 96,
            invalid: false, result: None, error: None, dark_mode: theme::DarkMode::load(),
        }
    }

    fn add(&mut self, class: &str, title: &str, id: u16, style: u32, bounds: RECT) -> Result<HWND, String> {
        let control = unsafe {
            CreateWindowExW(0, wide(class).as_ptr(), wide(title).as_ptr(), WS_CHILD | WS_VISIBLE | style,
                0, 0, 0, 0, self.window, id as usize as _, GetModuleHandleW(ptr::null()), ptr::null())
        };
        if control.is_null() { return Err(std::io::Error::last_os_error().to_string()); }
        let kind = match class {
            "EDIT" => Kind::Field,
            "BUTTON" => Kind::Button,
            _ => Kind::Label,
        };
        update_look(|look| {
            look.controls.insert(control as isize, ControlInfo {
                kind, surface: if kind == Kind::Button { Surface::Window } else { Surface::Group },
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
        set_text(window, "Custom accent color");
        self.add("STATIC", "Preview", 0, SS_LEFT, rect(244, 24, 392, 46))?;
        for (index, (label, value)) in [("&Red", self.color.0), ("&Green", self.color.1), ("&Blue", self.color.2)].into_iter().enumerate() {
            let top = 52 + index as i32 * 42;
            self.add("STATIC", label, 0, SS_LEFT, rect(24, top + 4, 96, top + 28))?;
            self.fields[index + 1] = self.add("EDIT", &value.to_string(), RED + index as u16,
                WS_TABSTOP | ES_AUTOHSCROLL as u32 | ES_NUMBER as u32, rect(100, top, 220, top + 28))?;
        }
        self.add("STATIC", "&Hex", 0, SS_LEFT, rect(24, 190, 96, 214))?;
        self.fields[0] = self.add("EDIT", &self.color.text(), HEX,
            WS_TABSTOP | ES_AUTOHSCROLL as u32, rect(100, 186, 392, 214))?;
        self.notice = self.add("STATIC", HELP, NOTICE, SS_LEFT | SS_NOPREFIX, rect(24, 226, 392, 266))?;
        self.add("BUTTON", "OK", IDOK as u16, WS_TABSTOP | BS_DEFPUSHBUTTON as u32, rect(204, 290, 292, 322))?;
        self.add("BUTTON", "Cancel", IDCANCEL as u16, WS_TABSTOP | BS_PUSHBUTTON as u32, rect(304, 290, 392, 322))?;
        for (index, field) in self.fields.iter().enumerate() {
            unsafe { SendMessageW(*field, EM_LIMITTEXT, if index == 0 { 7 } else { 3 }, 0); }
        }
        self.resize(unsafe { GetDpiForWindow(window) }.max(96), None);
        self.apply_theme();
        Ok(())
    }

    fn apply_theme(&self) {
        let Some(palette) = with_look(|look| look.style.palette) else { return; };
        self.dark_mode.apply_title_bar(self.window, palette.dark);
        for field in self.fields { self.dark_mode.apply_control(field, palette.dark); }
        unsafe { RedrawWindow(self.window, ptr::null(), ptr::null_mut(), RDW_INVALIDATE | RDW_ERASE | RDW_FRAME | RDW_ALLCHILDREN); }
    }

    fn changed(&mut self, field: HWND) {
        let candidate = if field == self.fields[0] {
            Rgb::parse(&text(field))
        } else {
            let values = self.fields[1..].iter().map(|field| text(*field).trim().parse::<u8>()).collect::<Result<Vec<_>, _>>();
            values.ok().map(|values| Rgb(values[0], values[1], values[2]))
        };
        self.invalid = candidate.is_none();
        if let Some(color) = candidate {
            self.color = color;
            if field == self.fields[0] {
                for (field, value) in self.fields[1..].iter().zip([color.0, color.1, color.2]) {
                    set_text(*field, &value.to_string());
                }
            } else {
                set_text(self.fields[0], &color.text());
            }
        }
        set_text(self.notice, if self.invalid { "Use RGB values from 0 to 255 or a six-digit hex color starting with #." } else { HELP });
        unsafe { InvalidateRect(self.window, ptr::null(), 0); InvalidateRect(self.notice, ptr::null(), 1); }
    }

    fn accept(&mut self) {
        if !self.invalid {
            self.result = Some(self.color);
            unsafe { EndDialog(self.window, IDOK as isize); }
        }
    }

    fn paint(&self, dc: HDC) {
        with_look(|look| {
            let Some(fonts) = &self.fonts else { return; };
            let style = Style { fonts: fonts.fonts, scale: self.dpi as f32 / 96.0, ..look.style };
            let client = client_rect(self.window);
            let Some(mut canvas) = canvas::Canvas::new(dc, client) else { return; };
            let s = |value| scale(value, self.dpi);
            canvas.fill(client, style.palette.window);
            draw::group_box(&mut canvas, rect(s(8), s(8), s(412), s(278)), &style, style.palette.group);
            canvas.round_rect(rect(s(244), s(52), s(392), s(164)), [style.px(4.0); 4], Some(self.color), Some((style.palette.border, 1.0)));
            for field in self.fields {
                let mut bounds = RECT::default();
                unsafe { GetWindowRect(field, &mut bounds); MapWindowPoints(ptr::null_mut(), self.window, (&mut bounds as *mut RECT).cast(), 2); }
                draw::field_frame(&mut canvas, draw::inset(bounds, -s(3), -s(3)), &style,
                    unsafe { GetFocus() } == field, self.invalid, false);
            }
            canvas.present(dc);
        });
    }

    fn resize(&mut self, dpi: u32, suggested: Option<RECT>) {
        self.dpi = dpi;
        let fonts = FontSet::new(dpi);
        for (control, bounds) in &self.controls {
            let bounds = if self.fields.contains(control) { draw::inset(*bounds, 3, 3) } else { *bounds };
            unsafe {
                SendMessageW(*control, WM_SETFONT, fonts.fonts.ui as usize, 1);
                SetWindowPos(*control, ptr::null_mut(), scale(bounds.left, dpi), scale(bounds.top, dpi),
                    scale(bounds.right - bounds.left, dpi), scale(bounds.bottom - bounds.top, dpi), SWP_NOZORDER | SWP_NOACTIVATE);
            }
        }
        self.fonts = Some(fonts);
        let mut bounds = rect(0, 0, scale(420, dpi), scale(338, dpi));
        unsafe {
            AdjustWindowRectExForDpi(&mut bounds, WS_CAPTION | WS_SYSMENU | DS_MODALFRAME as u32, 0, WS_EX_DLGMODALFRAME, dpi);
            let mut parent = RECT::default();
            GetWindowRect(GetParent(self.window), &mut parent);
            let (width, height) = (bounds.right - bounds.left, bounds.bottom - bounds.top);
            let (left, top) = suggested.map(|area| (area.left, area.top)).unwrap_or(((parent.left + parent.right - width) / 2, (parent.top + parent.bottom - height) / 2));
            SetWindowPos(self.window, ptr::null_mut(), left, top, width, height, SWP_NOZORDER | SWP_NOACTIVATE);
            SendMessageW(self.window, DM_REPOSITION, 0, 0);
        }
    }
}

impl Drop for Dialog {
    fn drop(&mut self) {
        update_look(|look| {
            for (control, _) in &self.controls { look.controls.remove(&(*control as isize)); }
        });
    }
}

unsafe extern "system" fn procedure(window: HWND, message: u32, wp: WPARAM, lp: LPARAM) -> isize {
    if message == WM_INITDIALOG { unsafe { SetWindowLongPtrW(window, GWLP_USERDATA, lp); } }
    let state = unsafe { GetWindowLongPtrW(window, GWLP_USERDATA) } as *const RefCell<Dialog>;
    if state.is_null() { return 0; }
    // Text changes and painting can reenter while the editor is borrowed.
    match message {
        WM_CTLCOLORDLG => return with_look(|look| look.brush(look.style.palette.window) as isize).unwrap_or(0),
        WM_CTLCOLOREDIT | WM_CTLCOLORSTATIC => {
            let control = lp as HWND;
            let result = ctl_color(wp as HDC, control).unwrap_or(0);
            if unsafe { GetDlgCtrlID(control) } as u16 == NOTICE {
                let invalid = unsafe { &*state }.try_borrow().is_ok_and(|state| state.invalid);
                with_look(|look| unsafe { SetTextColor(wp as HDC, if invalid { look.style.palette.error } else { look.style.palette.muted }.colorref()); });
            }
            return result;
        }
        WM_NOTIFY if lp != 0 => {
            let header = unsafe { &*(lp as *const NMHDR) };
            if header.code == NM_CUSTOMDRAW {
                let result = custom_draw(unsafe { &mut *(lp as *mut NMCUSTOMDRAW) });
                unsafe { SetWindowLongPtrW(window, DWLP_MSGRESULT as i32, result); }
                return 1;
            }
        }
        WM_SETTINGCHANGE | WM_SYSCOLORCHANGE => { with_app(App::apply_theme); }
        WM_NCDESTROY => {
            WINDOW.with(|active| { if active.get() == window { active.set(ptr::null_mut()); } });
            unsafe { SetWindowLongPtrW(window, GWLP_USERDATA, 0); }
            return 0;
        }
        _ => {}
    }
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
            let field = lp as HWND;
            if state.fields.contains(&field) {
                match (wp >> 16) as u32 {
                    EN_CHANGE => state.changed(field),
                    EN_SETFOCUS | EN_KILLFOCUS => unsafe { InvalidateRect(window, ptr::null(), 0); },
                    _ => {}
                }
                return 1;
            }
            match (wp & 0xffff) as u16 {
                id if id == IDOK as u16 => state.accept(),
                id if id == IDCANCEL as u16 => unsafe { EndDialog(window, 0); },
                _ => return 0,
            }
            1
        }
        WM_DPICHANGED => { state.resize((wp & 0xffff) as u32, (lp != 0).then(|| unsafe { *(lp as *const RECT) })); 1 }
        WM_THEMECHANGED => { state.apply_theme(); 1 }
        WM_ERASEBKGND => { unsafe { SetWindowLongPtrW(window, DWLP_MSGRESULT as i32, 1); } 1 }
        WM_PAINT => {
            let mut paint = PAINTSTRUCT::default();
            let dc = unsafe { BeginPaint(window, &mut paint) };
            state.paint(dc);
            unsafe { EndPaint(window, &paint); }
            1
        }
        WM_PRINTCLIENT => { state.paint(wp as HDC); 1 }
        WM_CLOSE => { unsafe { EndDialog(window, 0); } 1 }
        _ => 0,
    }
}

pub(super) fn show(parent: HWND, color: Rgb) -> Result<Option<Rgb>, String> {
    let state = RefCell::new(Dialog::new(color));
    let template = template();
    let result = unsafe { DialogBoxIndirectParamW(GetModuleHandleW(ptr::null()), &template.dialog,
        parent, Some(procedure), &state as *const RefCell<Dialog> as isize) };
    let mut state = state.into_inner();
    if let Some(error) = state.error.take() { return Err(error); }
    if result == -1 { return Err(std::io::Error::last_os_error().to_string()); }
    Ok(state.result.take())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hidden_editor_validates_rgb_hex_and_paints_the_selected_theme() {
        let fonts = FontSet::new(96);
        LOOK.with(|slot| *slot.borrow_mut() = Some(Look {
            style: Style { palette: Palette::dark(), fonts: fonts.fonts, scale: 1.0 },
            controls: HashMap::new(), tab: Tab::Output, menu_open: 0,
            filters: Vec::new(), log: VecDeque::new(), log_columns: [0; 3], brushes: RefCell::new(Vec::new()),
        }));
        let state = RefCell::new(Dialog::new(Rgb::hex(0x0078D7)));
        let template = template();
        let window = unsafe { CreateDialogIndirectParamW(GetModuleHandleW(ptr::null()), &template.dialog,
            ptr::null_mut(), Some(procedure), &state as *const RefCell<Dialog> as isize) };
        assert!(!window.is_null());
        assert!(state.borrow().error.is_none());
        assert_eq!(unsafe { IsWindowVisible(window) }, 0);
        let fields = state.borrow().fields;
        set_text(fields[0], "#FF8C00");
        assert_eq!(state.borrow().color, Rgb(255, 140, 0));
        assert_eq!([text(fields[1]), text(fields[2]), text(fields[3])], ["255", "140", "0"]);
        set_text(fields[2], "42");
        assert_eq!(text(fields[0]), "#FF2A00");
        assert_eq!(state.borrow().color, Rgb(255, 42, 0));
        set_text(fields[1], "256");
        unsafe { SendMessageW(window, WM_COMMAND, IDOK as usize, 0); }
        assert!(state.borrow().invalid && state.borrow().result.is_none());
        set_text(fields[0], "#zzzzzz");
        assert!(state.borrow().invalid);
        set_text(fields[0], "#12AB9F");
        assert!(!state.borrow().invalid);
        assert_eq!(state.borrow().color, Rgb(18, 171, 159));
        unsafe { SendMessageW(window, WM_COMMAND, IDCANCEL as usize, 0); }
        assert!(state.borrow().result.is_none(), "Cancel must not apply a color");

        let dc = unsafe { CreateCompatibleDC(ptr::null_mut()) };
        let info = BITMAPINFO { bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32, biWidth: 1000, biHeight: -800,
            biPlanes: 1, biBitCount: 32, biCompression: BI_RGB, ..Default::default()
        }, ..Default::default() };
        let mut bits = ptr::null_mut();
        let bitmap = unsafe { CreateDIBSection(dc, &info, DIB_RGB_COLORS, &mut bits, ptr::null_mut(), 0) };
        assert!(!dc.is_null() && !bitmap.is_null());
        let previous = unsafe { SelectObject(dc, bitmap) };
        for palette in [Palette::dark().with_accent(Accent::Red), Palette::light().with_accent(Accent::Green), Palette::high_contrast()] {
            update_look(|look| look.style.palette = palette);
            unsafe { SendMessageW(window, WM_THEMECHANGED, 0, 0); }
            for dpi in [96, 144, 192] {
                state.borrow_mut().resize(dpi, None);
                unsafe { SendMessageW(window, WM_PRINTCLIENT, dc as usize, 0); }
                assert_eq!(unsafe { GetPixel(dc, 0, 0) }, palette.window.colorref());
                assert_eq!(unsafe { GetPixel(dc, scale(260, dpi), scale(70, dpi)) }, Rgb(18, 171, 159).colorref());
                unsafe { SendMessageW(window, WM_CTLCOLOREDIT, dc as usize, fields[0] as isize); }
                assert_eq!(unsafe { GetTextColor(dc) }, palette.text.colorref());
                assert_eq!(unsafe { GetBkColor(dc) }, palette.field.colorref());
                if dpi == 96 {
                    if let Some(directory) = std::env::var_os("OTD_AREA_TEST_OUTPUT") {
                        for (control, _) in &state.borrow().controls {
                            let mut origin = POINT::default();
                            unsafe {
                                MapWindowPoints(*control, window, &mut origin, 1);
                                let saved = SaveDC(dc);
                                SetViewportOrgEx(dc, origin.x, origin.y, ptr::null_mut());
                                SendMessageW(*control, WM_PRINT, dc as usize, (PRF_CLIENT | PRF_ERASEBKGND) as isize);
                                RestoreDC(dc, saved);
                            }
                        }
                        unsafe { GdiFlush(); }
                        let pixels = unsafe { std::slice::from_raw_parts(bits as *const u32, 1000 * 800) };
                        let client = client_rect(window);
                        let (width, height) = (client.right, client.bottom);
                        let mut bmp = Vec::new();
                        bmp.extend_from_slice(b"BM");
                        bmp.extend_from_slice(&(54 + (width * height) as u32 * 4).to_le_bytes());
                        bmp.extend_from_slice(&[0; 4]);
                        bmp.extend_from_slice(&54u32.to_le_bytes());
                        bmp.extend_from_slice(&40u32.to_le_bytes());
                        bmp.extend_from_slice(&width.to_le_bytes());
                        bmp.extend_from_slice(&(-height).to_le_bytes());
                        bmp.extend_from_slice(&1u16.to_le_bytes());
                        bmp.extend_from_slice(&32u16.to_le_bytes());
                        bmp.extend_from_slice(&[0; 24]);
                        for row in pixels.chunks(1000).take(height as usize) {
                            for pixel in row.iter().take(width as usize) { bmp.extend_from_slice(&pixel.to_le_bytes()); }
                        }
                        let directory = PathBuf::from(directory);
                        std::fs::create_dir_all(&directory).unwrap();
                        let name = if palette.high_contrast { "contrast" } else if palette.dark { "dark" } else { "light" };
                        std::fs::write(directory.join(format!("custom-accent-{name}.bmp")), bmp).unwrap();
                    }
                }
            }
        }
        unsafe { SendMessageW(window, WM_COMMAND, IDOK as usize, 0); }
        assert_eq!(state.borrow().result, Some(Rgb(18, 171, 159)));
        unsafe { DestroyWindow(window); SelectObject(dc, previous); DeleteObject(bitmap); DeleteDC(dc); }
        drop(state);
        assert!(with_look(|look| look.controls.is_empty()).unwrap());
        assert!(WINDOW.with(Cell::get).is_null());
        LOOK.with(|slot| slot.borrow_mut().take());
    }
}
