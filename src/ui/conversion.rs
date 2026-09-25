//! Explicit area conversion: a modal editor owns its candidate until accepted.
//! No profile, daemon, plugin or device operations occur inside the dialog.
use super::*;
use otd_core::areas::Conversion;
use otd_core::tablets::{Database, DigitizerSpecifications};
use windows_sys::Win32::UI::HiDpi::{DDC_DISABLE_ALL, SetDialogDpiChangeBehavior};

const FORMAT: u16 = 1000;
const VALUE: u16 = 1010;
const PREVIEW: u16 = 1020;
const USE_AREA: u16 = 1021;
const FORMATS: [(Conversion, &str); 5] = [
    (Conversion::Percentage, "Percentage (fractions)"),
    (Conversion::WacomVeikk, "Wacom / VEIKK"),
    (Conversion::XpPen, "XP-Pen"),
    (Conversion::GaomonV2Otd067, "Gaomon V2 - OTD 0.6.7 formula"),
    (
        Conversion::GaomonV2Corrected,
        "Gaomon V2 - corrected Y offset",
    ),
];

// Standard empty dialog template followed by the required zero-terminated
// menu, class and title fields. DLGTEMPLATE is packed(2), size 18: these
// WORD fields begin at offsets 18/20/22; only the outer alignment is raised.
// Windows requires DWORD alignment for the start of the template.
#[repr(C, align(4))]
struct Template {
    dialog: DLGTEMPLATE,
    menu: u16,
    class: u16,
    title: u16,
}

struct Dialog {
    window: HWND,
    name: String,
    tablet: &'static DigitizerSpecifications,
    controls: Vec<(HWND, RECT)>,
    format: HWND,
    labels: [HWND; 4],
    fields: [HWND; 4],
    units: HWND,
    notice: HWND,
    preview: HWND,
    accept: HWND,
    // Keep dialog-owned fonts alive until all child windows are destroyed.
    _fonts: Option<FontSet>,
    candidate: Option<OtdArea>,
    result: Option<OtdArea>,
    error: Option<String>,
}

impl Dialog {
    fn add(
        &mut self,
        class: &str,
        title: &str,
        id: u16,
        style: u32,
        rect: RECT,
    ) -> Result<HWND, String> {
        let control = unsafe {
            CreateWindowExW(
                0,
                wide(class).as_ptr(),
                wide(title).as_ptr(),
                WS_CHILD | WS_VISIBLE | style,
                0,
                0,
                0,
                0,
                self.window,
                id as usize as _,
                GetModuleHandleW(ptr::null()),
                ptr::null(),
            )
        };
        if control.is_null() {
            return Err(format!(
                "Cannot create conversion control: {}",
                std::io::Error::last_os_error()
            ));
        }
        self.controls.push((control, rect));
        Ok(control)
    }

    fn initialize(&mut self, window: HWND) -> Result<(), String> {
        self.window = window;
        // We own control positions/fonts and the dialog frame. Do not also let
        // the PMv2 dialog manager scale the same controls from template units.
        if unsafe { SetDialogDpiChangeBehavior(window, DDC_DISABLE_ALL, DDC_DISABLE_ALL) } == 0 {
            return Err(format!(
                "Cannot configure conversion dialog scaling: {}",
                std::io::Error::last_os_error()
            ));
        }
        set_text(window, &format!("Convert tablet area - {}", self.name));
        self.add(
            "STATIC",
            "Source &format",
            0,
            SS_LEFT,
            rect(16, 16, 604, 36),
        )?;
        self.format = self.add(
            "COMBOBOX",
            "",
            FORMAT,
            WS_TABSTOP | WS_VSCROLL | CBS_DROPDOWNLIST as u32,
            rect(16, 40, 604, 220),
        )?;
        for (_, label) in FORMATS {
            unsafe { SendMessageW(self.format, CB_ADDSTRING, 0, wide(label).as_ptr() as isize) };
        }
        unsafe { SendMessageW(self.format, CB_SETCURSEL, 0, 0) };
        self.units = self.add(
            "STATIC",
            "",
            0,
            SS_LEFT | SS_NOPREFIX,
            rect(16, 76, 604, 98),
        )?;
        self.notice = self.add(
            "STATIC",
            "",
            0,
            SS_LEFT | SS_NOPREFIX,
            rect(16, 102, 604, 150),
        )?;
        for (index, initial) in ["0", "0", "1", "1"].iter().enumerate() {
            let left = 16 + (index % 2) as i32 * 302;
            let top = 158 + (index / 2) as i32 * 44;
            self.labels[index] = self.add(
                "STATIC",
                "",
                0,
                SS_LEFT,
                rect(left, top + 4, left + 92, top + 26),
            )?;
            self.fields[index] = self.add(
                "EDIT",
                initial,
                VALUE + index as u16,
                WS_TABSTOP | WS_BORDER | ES_AUTOHSCROLL as u32,
                rect(left + 94, top, left + 286, top + 28),
            )?;
            unsafe { SendMessageW(self.fields[index], EM_LIMITTEXT, 128, 0) };
        }
        self.add("STATIC",
            "Use area copies the exact preview into unsaved tablet settings, with rotation 0°. Area/aspect locks are not applied; the running driver and saved file are unchanged.",
            0, SS_LEFT | SS_NOPREFIX, rect(16, 246, 604, 282))?;
        self.add(
            "BUTTON",
            "&Preview",
            PREVIEW,
            WS_TABSTOP | BS_DEFPUSHBUTTON as u32,
            rect(16, 290, 126, 322),
        )?;
        self.preview = self.add(
            "EDIT",
            "Enter four values, then choose Preview.",
            0,
            WS_TABSTOP | WS_BORDER | WS_VSCROLL | ES_MULTILINE as u32 | ES_READONLY as u32,
            rect(16, 334, 604, 402),
        )?;
        self.accept = self.add(
            "BUTTON",
            "&Use area",
            USE_AREA,
            WS_TABSTOP | BS_PUSHBUTTON as u32,
            rect(374, 416, 486, 448),
        )?;
        self.add(
            "BUTTON",
            "Cancel",
            IDCANCEL as u16,
            WS_TABSTOP | BS_PUSHBUTTON as u32,
            rect(498, 416, 604, 448),
        )?;
        self.format_changed();
        self.resize(window, unsafe { GetDpiForWindow(window) }.max(96), None);
        unsafe {
            SendMessageW(window, DM_SETDEFID, PREVIEW as usize, 0);
            SetFocus(self.format);
        }
        Ok(())
    }

    fn resize(&mut self, window: HWND, dpi: u32, suggested: Option<RECT>) {
        let fonts = FontSet::new(dpi);
        for (control, area) in &self.controls {
            unsafe {
                SendMessageW(*control, WM_SETFONT, fonts.fonts.ui as usize, 1);
                SetWindowPos(
                    *control,
                    ptr::null_mut(),
                    scale(area.left, dpi),
                    scale(area.top, dpi),
                    scale(area.right - area.left, dpi),
                    scale(area.bottom - area.top, dpi),
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
            }
        }
        self._fonts = Some(fonts);
        let mut bounds = rect(0, 0, scale(620, dpi), scale(464, dpi));
        unsafe {
            AdjustWindowRectExForDpi(
                &mut bounds,
                WS_CAPTION | WS_SYSMENU | DS_MODALFRAME as u32,
                0,
                WS_EX_DLGMODALFRAME,
                dpi,
            );
            let mut parent = RECT::default();
            GetWindowRect(GetParent(window), &mut parent);
            let (width, height) = (bounds.right - bounds.left, bounds.bottom - bounds.top);
            let (left, top) = if let Some(suggested) = suggested {
                (suggested.left, suggested.top)
            } else {
                (
                    (parent.left + parent.right - width) / 2,
                    (parent.top + parent.bottom - height) / 2,
                )
            };
            SetWindowPos(
                window,
                ptr::null_mut(),
                left,
                top,
                width,
                height,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
            // Let the dialog manager bring its full frame into the work area.
            SendMessageW(window, DM_REPOSITION, 0, 0);
        }
    }

    fn selected(&self) -> Option<Conversion> {
        let index = unsafe { SendMessageW(self.format, CB_GETCURSEL, 0, 0) };
        usize::try_from(index)
            .ok()
            .and_then(|index| FORMATS.get(index))
            .map(|entry| entry.0)
    }

    fn invalidate_preview(&mut self) {
        self.candidate = None;
        unsafe { EnableWindow(self.accept, 0) };
        set_text(
            self.preview,
            "Values changed. Choose Preview before using the area.",
        );
    }

    fn format_changed(&mut self) {
        self.invalidate_preview();
        if let Some(format) = self.selected() {
            for (index, (label, caption)) in self.labels.iter().zip(format.labels()).enumerate() {
                set_text(*label, &format!("&{} {caption}", index + 1));
            }
            set_text(
                self.units,
                &format!("Input: {}. Output: millimetres.", format.input_units()),
            );
            set_text(self.notice, format.notice().unwrap_or(
                "Uses the pinned OpenTabletDriver 0.6.7 conversion formula. Width and height must be positive; all values must be finite."));
        }
    }

    fn calculate(&self) -> Result<OtdArea, String> {
        let format = self.selected().ok_or("Select a source format.")?;
        let mut values = [0.0; 4];
        for (index, field) in self.fields.iter().enumerate() {
            values[index] = model::parse_number(&text(*field)).ok_or_else(|| {
                format!(
                    "{} must be a finite number ({}).",
                    format.labels()[index],
                    format.input_units()
                )
            })?;
        }
        format.convert(self.tablet, values)
    }

    fn preview(&mut self) {
        self.invalidate_preview();
        match self.calculate() {
            Ok(area) => {
                let number = |value: f64| value.to_string();
                set_text(
                    self.preview,
                    &format!(
                        "Width: {} mm    Height: {} mm\r\nCenter X: {} mm    Center Y: {} mm\r\nRotation: 0°; exact result, without bounds/aspect adjustment.",
                        number(area.width),
                        number(area.height),
                        number(area.x),
                        number(area.y)
                    ),
                );
                self.candidate = Some(area);
                unsafe { EnableWindow(self.accept, 1) };
            }
            Err(error) => set_text(self.preview, &error),
        }
    }
}

unsafe extern "system" fn procedure(window: HWND, message: u32, wp: WPARAM, lp: LPARAM) -> isize {
    if message == WM_INITDIALOG {
        unsafe { SetWindowLongPtrW(window, GWLP_USERDATA, lp) };
    }
    let pointer = unsafe { GetWindowLongPtrW(window, GWLP_USERDATA) } as *const RefCell<Dialog>;
    if pointer.is_null() {
        return 0;
    }
    // Programmatic text/font updates can reenter with control notifications.
    // Ignore those while the outer update owns the dialog state.
    let Ok(mut state) = (unsafe { &*pointer }).try_borrow_mut() else {
        return 0;
    };
    match message {
        WM_INITDIALOG => {
            if let Err(error) = state.initialize(window) {
                state.error = Some(error);
                unsafe { EndDialog(window, -2) };
            }
            0 // initialize chose keyboard focus explicitly
        }
        WM_COMMAND => {
            let id = wp as u16;
            let code = (wp >> 16) as u32;
            match id {
                FORMAT if code == CBN_SELCHANGE => state.format_changed(),
                id if (VALUE..VALUE + 4).contains(&id) && code == EN_CHANGE => {
                    state.invalidate_preview()
                }
                PREVIEW => state.preview(),
                USE_AREA if state.candidate.is_some() => {
                    // Revalidate even if an unexpected control change missed its notification.
                    match state.calculate() {
                        Ok(area) if Some(area) == state.candidate => {
                            state.result = Some(area);
                            unsafe { EndDialog(window, USE_AREA as isize) };
                        }
                        _ => state.preview(),
                    }
                }
                id if id == IDCANCEL as u16 => {
                    unsafe { EndDialog(window, 0) };
                }
                id if id == IDOK as u16 => state.preview(),
                _ => return 0,
            }
            1
        }
        WM_DPICHANGED => {
            let suggested = (lp != 0).then(|| unsafe { *(lp as *const RECT) });
            state.resize(window, (wp & 0xffff) as u32, suggested);
            1
        }
        WM_CLOSE => {
            unsafe { EndDialog(window, 0) };
            1
        }
        _ => 0,
    }
}

fn show(parent: HWND, name: &str) -> Result<Option<OtdArea>, String> {
    let tablet = Database::builtin()
        .entries()
        .iter()
        .filter_map(|entry| entry.usable())
        .find(|tablet| tablet.name == name)
        .and_then(|tablet| tablet.specifications.as_ref())
        .and_then(|specs| specs.digitizer.as_ref())
        .ok_or_else(|| format!("The digitizer specifications of {name} are unavailable."))?;
    let state = RefCell::new(Dialog {
        window: ptr::null_mut(),
        name: name.to_owned(),
        tablet,
        controls: Vec::new(),
        format: ptr::null_mut(),
        labels: [ptr::null_mut(); 4],
        fields: [ptr::null_mut(); 4],
        units: ptr::null_mut(),
        notice: ptr::null_mut(),
        preview: ptr::null_mut(),
        accept: ptr::null_mut(),
        _fonts: None,
        candidate: None,
        result: None,
        error: None,
    });
    let template = Template {
        dialog: DLGTEMPLATE {
            style: WS_POPUP | WS_CAPTION | WS_SYSMENU | DS_MODALFRAME as u32,
            dwExtendedStyle: WS_EX_DLGMODALFRAME,
            cx: 340,
            cy: 260,
            ..Default::default()
        },
        menu: 0,
        class: 0,
        title: 0,
    };
    let result = unsafe {
        DialogBoxIndirectParamW(
            GetModuleHandleW(ptr::null()),
            &template.dialog,
            parent,
            Some(procedure),
            &state as *const RefCell<Dialog> as isize,
        )
    };
    let state = state.into_inner();
    if let Some(error) = state.error {
        return Err(error);
    }
    if result == -1 {
        return Err(format!(
            "Cannot open conversion dialog: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(if result == USE_AREA as isize {
        state.result
    } else {
        None
    })
}

pub(super) fn open(window: HWND) {
    let source = with_app(|app| -> Result<(PathBuf, String, String), String> {
        if app.editor.mode() != OutputMode::Absolute {
            return Err("Area conversion is available in Absolute Mode.".into());
        }
        app.editor.profile.validate_runtime_tablet()?;
        // Do not discard an invalid field while replacing the visible area.
        app.checked_profile()?;
        Ok((
            app.profile_path.clone(),
            app.editor.profile.to_toml()?,
            app.tablet_label(),
        ))
    });
    let result = match source {
        Some(Ok((path, source, tablet))) => {
            show(window, &tablet).map(|area| ((path, source), area))
        }
        Some(Err(error)) => Err(error),
        None => return,
    };
    with_app(|app| match result {
        Ok(((path, source), Some(area))) => {
            if app.closing {
                return;
            }
            if app.profile_path != path || app.editor.profile.to_toml().as_ref() != Ok(&source) {
                app.log(Level::Warning, "Settings", "The profile changed while conversion was open. Reopen conversion to use the current settings; no area was replaced.");
                return;
            }
            let mut mapping = app.editor.absolute(&app.displays);
            mapping.tablet = area;
            app.editor.set_absolute(mapping);
            app.mark_dirty();
            app.sync_areas(None);
            app.log(Level::Info, "Settings", "Converted tablet area copied into unsaved settings. Save or Apply is a separate action.");
        }
        Ok((_, None)) => {}
        Err(error) => app.log(Level::Error, "Settings", error),
    });
}
