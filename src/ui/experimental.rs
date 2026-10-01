//! Modal editor for optional scheduling settings. Validation is local; saving
//! and daemon control run on the background client after the dialog closes.
use super::*;
use crate::experimental::{Settings, format_cpus, parse_cpus};
use windows_sys::Win32::UI::HiDpi::{DDC_DISABLE_ALL, SetDialogDpiChangeBehavior};

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
}

impl Dialog {
    fn add(&mut self, class: &str, title: &str, id: u16, style: u32, bounds: RECT) -> Result<HWND, String> {
        let control = unsafe { CreateWindowExW(0, wide(class).as_ptr(), wide(title).as_ptr(),
            WS_CHILD | WS_VISIBLE | style, 0, 0, 0, 0, self.window, id as usize as _,
            GetModuleHandleW(ptr::null()), ptr::null()) };
        if control.is_null() { return Err(std::io::Error::last_os_error().to_string()); }
        self.controls.push((control, bounds));
        Ok(control)
    }

    fn initialize(&mut self, window: HWND) -> Result<(), String> {
        self.window = window;
        if unsafe { SetDialogDpiChangeBehavior(window, DDC_DISABLE_ALL, DDC_DISABLE_ALL) } == 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        set_text(window, "Experimental settings");
        self.add("STATIC", "CPU affinity", 0, SS_LEFT, rect(16,16,604,38))?;
        self.add("STATIC", "Choose logical CPUs independently for the GUI and driver process. Use All for automatic scheduling, or numbers such as 0,2,4-7. CPU numbers start at 0; they are logical processors, not physical cores.",
            0, SS_LEFT | SS_NOPREFIX, rect(16,44,604,98))?;
        for (index, label) in ["&GUI CPUs", "&Driver CPUs"].iter().enumerate() {
            let top = 112 + index as i32 * 42;
            self.add("STATIC", label, 0, SS_LEFT, rect(16,top+4,124,top+28))?;
            let cpus = if index == 0 { &self.settings.ui_cpus } else { &self.settings.driver_cpus };
            self.fields[index] = self.add("EDIT", &format_cpus(cpus), 100 + index as u16,
                WS_TABSTOP | WS_BORDER | ES_AUTOHSCROLL as u32, rect(130,top,604,top+28))?;
            unsafe { SendMessageW(self.fields[index], EM_LIMITTEXT, 256, 0); }
        }
        let available = crate::experimental::masks().map(|(_, mask)| {
            let cpus: Vec<u16> = (0..64).filter(|cpu| mask & (1usize << cpu) != 0).collect();
            format!("Available logical CPUs: {}.", format_cpus(&cpus))
        }).unwrap_or_else(|error| error);
        self.add("STATIC", &available, 0, SS_LEFT | SS_NOPREFIX, rect(16,198,604,242))?;
        self.add("STATIC", "Save and apply stores these choices for future launches and updates both processes without restarting tablet input. All resets a previous CPU selection. On systems with multiple Windows processor groups, keep both fields set to All.",
            0, SS_LEFT | SS_NOPREFIX, rect(16,250,604,306))?;
        self.notice = self.add("STATIC", self.load_warning.clone().unwrap_or_else(||
            "Changes are experimental. The Console reports save/application failures.".into()).as_str(),
            0, SS_LEFT | SS_NOPREFIX, rect(16,316,604,374))?;
        self.add("BUTTON", "&All CPUs", 102, WS_TABSTOP | BS_PUSHBUTTON as u32, rect(16,388,124,420))?;
        self.add("BUTTON", "&Save and apply", IDOK as u16, WS_TABSTOP | BS_DEFPUSHBUTTON as u32, rect(328,388,474,420))?;
        self.add("BUTTON", "Cancel", IDCANCEL as u16, WS_TABSTOP | BS_PUSHBUTTON as u32, rect(490,388,604,420))?;
        self.resize(unsafe { GetDpiForWindow(window) }.max(96), None);
        unsafe { SetFocus(self.fields[0]); }
        Ok(())
    }

    fn resize(&mut self, dpi: u32, suggested: Option<RECT>) {
        let fonts = FontSet::new(dpi);
        for (control, bounds) in &self.controls {
            unsafe {
                SendMessageW(*control, WM_SETFONT, fonts.fonts.ui as usize, 1);
                SetWindowPos(*control, ptr::null_mut(), scale(bounds.left,dpi), scale(bounds.top,dpi),
                    scale(bounds.right-bounds.left,dpi), scale(bounds.bottom-bounds.top,dpi),
                    SWP_NOZORDER | SWP_NOACTIVATE);
            }
        }
        self.fonts = Some(fonts);
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
            Ok(settings) => { self.result = Some(settings); unsafe { EndDialog(self.window, IDOK as isize); } }
            Err(error) => set_text(self.notice, &error),
        }
    }
}

unsafe extern "system" fn procedure(window: HWND, message: u32, wp: WPARAM, lp: LPARAM) -> isize {
    if message == WM_INITDIALOG { unsafe { SetWindowLongPtrW(window, GWLP_USERDATA, lp); } }
    let state = unsafe { GetWindowLongPtrW(window, GWLP_USERDATA) } as *const RefCell<Dialog>;
    if state.is_null() { return 0; }
    // Native control creation/text changes can dispatch messages synchronously.
    let Ok(mut state) = (unsafe { &*state }).try_borrow_mut() else { return 0; };
    match message {
        WM_INITDIALOG => {
            if let Err(error) = state.initialize(window) {
                state.error = Some(error);
                unsafe { EndDialog(window, -1); }
            }
            0
        }
        WM_COMMAND => {
            match (wp & 0xffff) as u16 {
                id if id == IDOK as u16 => state.accept(),
                id if id == IDCANCEL as u16 => { unsafe { EndDialog(window, 0); } }
                102 => { for field in state.fields { set_text(field,"All"); } }
                _ => return 0,
            }
            1
        }
        WM_DPICHANGED => {
            state.resize((wp & 0xffff) as u32, (lp != 0).then(|| unsafe { *(lp as *const RECT) }));
            1
        }
        WM_CLOSE => { unsafe { EndDialog(window,0); } 1 }
        _ => 0,
    }
}

pub(super) fn show(parent: HWND) -> Result<Option<Settings>, String> {
    let (settings, load_warning) = match crate::experimental::load() {
        Ok(settings) => (settings,None),
        Err(error) => (Settings::default(), Some(format!("Could not load saved choices: {error}. Save and apply replaces them with the values shown and retains a backup."))),
    };
    let state = RefCell::new(Dialog { window: ptr::null_mut(), settings, fields: [ptr::null_mut();2],
        controls: Vec::new(), fonts: None, notice: ptr::null_mut(), result: None, error: None, load_warning });
    let template = Template { dialog: DLGTEMPLATE {
        style: WS_POPUP | WS_CAPTION | WS_SYSMENU | DS_MODALFRAME as u32,
        dwExtendedStyle: WS_EX_DLGMODALFRAME, cx: 340, cy: 250, ..Default::default()
    }, menu: 0, class: 0, title: 0 };
    let result = unsafe { DialogBoxIndirectParamW(GetModuleHandleW(ptr::null()), &template.dialog,
        parent, Some(procedure), &state as *const RefCell<Dialog> as isize) };
    let state = state.into_inner();
    if let Some(error) = state.error { return Err(error); }
    if result == -1 { return Err(std::io::Error::last_os_error().to_string()); }
    Ok(state.result)
}
