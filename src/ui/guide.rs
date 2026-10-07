//! Pinned Greeter page sequence, using the panel's themed native controls.
use super::*;
use windows_sys::Win32::UI::HiDpi::{DDC_DISABLE_ALL,SetDialogDpiChangeBehavior};
const PREVIOUS:u16=101;
const NEXT:u16=102;
const DOCUMENTATION:u16=103;
const PAGES:[(&str,&str);6]=[
    ("Welcome to OpenTabletDriver","The panel edits settings and talks to a separate daemon. Select a detected tablet in Tablets > Device sessions. Each physical tablet keeps its own settings.\r\n\r\nSave stores your profile; Apply activates it. The state and Console show whether preparation completed. An empty tablet canvas means no selected tablet is detected. Detect tablet refreshes discovery.\r\n\r\nUse Previous and Next to explore areas, bindings, plugins, the system tray and further help."),
    ("Display and tablet areas","Output contains the display area in desktop pixels and the tablet area in millimeters. Drag the area to move it, or enter width, height, center and rotation.\r\n\r\nRight-click an area for alignment, full area, aspect ratio and usable-area locks. The display menu maps to an individual screen or the whole desktop. Tablet context actions become available when the selected device is detected.\r\n\r\nConvert tablet area from... imports another driver's units. Relative output uses sensitivity and reset time instead of absolute areas."),
    ("Bindings","Pen Settings edits tip, eraser and pen buttons. Auxiliary Settings edits express keys and wheels. Mouse Settings edits mouse buttons and scrolling.\r\n\r\nChoose a key or chord to capture it, a native action, toggle, or preset. Managed binding opens the typed plugin selector and property editor. Tip and eraser pressure thresholds, drag bindings, pressure and tilt switches belong to the selected tablet.\r\n\r\nApply when the bindings are ready. Changed settings do not silently activate while you edit."),
    ("Plugins and tools","Plugins > Plugin Manager installs and removes packages. Add .NET plugin selects a DLL for filters or tools. Choose its row to edit typed properties and enable or disable it.\r\n\r\nOutput and binding selectors assign installed managed classes to their actual slots. The Tools tab edits configured tools. Plugin constructors and tools execute trusted code when explicitly selected or activated.\r\n\r\nA missing plugin, dependency or runtime appears as an error in Console; installing a package is separate from activating it."),
    ("System tray and startup","The notification icon can show the panel, select presets and control the daemon. Minimize keeps the icon available; Quit closes the panel and waits for daemon cleanup.\r\n\r\nPreferences choose driver startup and Start with Windows. Theme follows Windows unless selected explicitly; blue remains the default accent.\r\n\r\nCtrl+O loads, Ctrl+S saves, Ctrl+Enter applies and Ctrl+D detects. Ctrl+Tab changes tabs. Closing a secondary editor does not close the panel."),
    ("Further help","Help opens documentation, diagnostics and updates. Console supports copying and saving messages. Tablet debugger displays reports and can record the selected session; loss and decode continuity are reported separately.\r\n\r\nThe device string reader queries a selected USB/HID device. Export diagnostics produces a bundle or copies it to the clipboard.\r\n\r\nHardware and plugin behavior still depends on the device, operating system and installed prerequisites. Documentation includes setup and troubleshooting. Choose Close to return to your settings."),
];
thread_local!{static WINDOW:Cell<HWND>=const{Cell::new(ptr::null_mut())};}
pub(super) fn refresh_theme(){WINDOW.with(|active|{if !active.get().is_null(){unsafe{SendMessageW(active.get(),WM_THEMECHANGED,0,0);}}});}
#[repr(C,align(4))]
struct Template{dialog:DLGTEMPLATE,menu:u16,class:u16,title:u16}
struct Dialog{
    window:HWND,page:usize,title:HWND,body:HWND,previous:HWND,next:HWND,
    controls:Vec<(HWND,RECT)>,fonts:Option<FontSet>,dpi:u32,error:Option<String>,dark_mode:theme::DarkMode,
}
impl Dialog{
    fn new()->Self{Self{window:ptr::null_mut(),page:0,title:ptr::null_mut(),body:ptr::null_mut(),previous:ptr::null_mut(),next:ptr::null_mut(),controls:Vec::new(),fonts:None,dpi:96,error:None,dark_mode:theme::DarkMode::load()}}
    fn add(&mut self,class:&str,title:&str,id:u16,style:u32,bounds:RECT)->Result<HWND,String>{
        let control=unsafe{CreateWindowExW(0,wide(class).as_ptr(),wide(title).as_ptr(),WS_CHILD|WS_VISIBLE|style,0,0,0,0,self.window,id as usize as _,GetModuleHandleW(ptr::null()),ptr::null())};
        if control.is_null(){return Err(std::io::Error::last_os_error().to_string());}
        update_look(|look|{look.controls.insert(control as isize,ControlInfo{kind:if class=="BUTTON"{Kind::Button}else if class=="EDIT"{Kind::Field}else{Kind::Label},surface:Surface::Window});});
        self.controls.push((control,bounds));Ok(control)
    }
    fn initialize(&mut self,window:HWND)->Result<(),String>{
        self.window=window;
        if unsafe{SetDialogDpiChangeBehavior(window,DDC_DISABLE_ALL,DDC_DISABLE_ALL)}==0{return Err(std::io::Error::last_os_error().to_string());}
        set_text(window,"OpenTabletDriver Guide");
        self.title=self.add("STATIC","",0,SS_LEFT,rect(24,22,584,56))?;
        self.body=self.add("EDIT","",0,WS_TABSTOP|WS_VSCROLL|ES_MULTILINE as u32|ES_READONLY as u32|ES_AUTOVSCROLL as u32,rect(24,68,584,340))?;
        self.previous=self.add("BUTTON","&Previous",PREVIOUS,WS_TABSTOP|BS_PUSHBUTTON as u32,rect(264,366,360,398))?;
        self.next=self.add("BUTTON","&Next",NEXT,WS_TABSTOP|BS_DEFPUSHBUTTON as u32,rect(368,366,464,398))?;
        self.add("BUTTON","&Documentation",DOCUMENTATION,WS_TABSTOP|BS_PUSHBUTTON as u32,rect(24,366,176,398))?;
        self.add("BUTTON","Cancel",IDCANCEL as u16,WS_TABSTOP|BS_PUSHBUTTON as u32,rect(472,366,584,398))?;
        self.update();self.resize(unsafe{GetDpiForWindow(window)}.max(96),None);
        with_look(|look|{self.dark_mode.apply_title_bar(window,look.style.palette.dark);self.dark_mode.apply_control(self.body,look.style.palette.dark);});Ok(())
    }
    fn update(&self){set_text(self.title,&format!("{} of {} — {}",self.page+1,PAGES.len(),PAGES[self.page].0));set_text(self.body,PAGES[self.page].1);set_text(self.next,if self.page+1==PAGES.len(){"&Close"}else{"&Next"});unsafe{EnableWindow(self.previous,i32::from(self.page>0));InvalidateRect(self.window,ptr::null(),1);}}
    fn resize(&mut self,dpi:u32,suggested:Option<RECT>){
        self.dpi=dpi;let fonts=FontSet::new(dpi);
        let mut work=RECT::default();unsafe{SystemParametersInfoW(SPI_GETWORKAREA,0,(&mut work as *mut RECT).cast(),0);}
        let height=420.min(((work.bottom-work.top-72)*96/dpi as i32).max(240));
        for(control,bounds)in &self.controls{
            let mut bounds=*bounds;
            if *control==self.body{bounds.bottom=height-80;}
            else if bounds.top>=366{bounds.top=height-54;bounds.bottom=height-22;}
            unsafe{SendMessageW(*control,WM_SETFONT,fonts.fonts.ui as usize,1);SetWindowPos(*control,ptr::null_mut(),scale(bounds.left,dpi),scale(bounds.top,dpi),scale(bounds.right-bounds.left,dpi),scale(bounds.bottom-bounds.top,dpi),SWP_NOZORDER|SWP_NOACTIVATE);}}
        self.fonts=Some(fonts);let mut bounds=rect(0,0,scale(608,dpi),scale(height,dpi));
        unsafe{AdjustWindowRectExForDpi(&mut bounds,WS_CAPTION|WS_SYSMENU|DS_MODALFRAME as u32,0,WS_EX_DLGMODALFRAME,dpi);let mut parent=RECT::default();GetWindowRect(GetParent(self.window),&mut parent);let(width,height)=(bounds.right-bounds.left,bounds.bottom-bounds.top);let(left,top)=suggested.map(|r|(r.left,r.top)).unwrap_or(((parent.left+parent.right-width)/2,(parent.top+parent.bottom-height)/2));SetWindowPos(self.window,ptr::null_mut(),left,top,width,height,SWP_NOZORDER|SWP_NOACTIVATE);SendMessageW(self.window,DM_REPOSITION,0,0);}
    }
    fn paint(&self,dc:HDC){with_look(|look|{let Some(mut canvas)=canvas::Canvas::new(dc,client_rect(self.window))else{return;};canvas.fill(client_rect(self.window),look.style.palette.window);canvas.present(dc);});}
}
impl Drop for Dialog{fn drop(&mut self){update_look(|look|{for(control,_)in &self.controls{look.controls.remove(&(*control as isize));}});}}
unsafe extern "system" fn procedure(window:HWND,message:u32,wp:WPARAM,lp:LPARAM)->isize{
    if message==WM_INITDIALOG{unsafe{SetWindowLongPtrW(window,GWLP_USERDATA,lp);}}
    let state=unsafe{GetWindowLongPtrW(window,GWLP_USERDATA)}as *const RefCell<Dialog>;if state.is_null(){return 0;}
    match message{
        WM_CTLCOLORDLG=>return with_look(|look|look.brush(look.style.palette.window)as isize).unwrap_or(0),
        WM_CTLCOLORSTATIC|WM_CTLCOLOREDIT=>return ctl_color(wp as HDC,lp as HWND).unwrap_or(0),
        WM_NOTIFY if lp!=0=>{if unsafe{&*(lp as *const NMHDR)}.code==NM_CUSTOMDRAW{let result=custom_draw(unsafe{&mut*(lp as *mut NMCUSTOMDRAW)});unsafe{SetWindowLongPtrW(window,DWLP_MSGRESULT as i32,result);}return 1;}},
        WM_SETTINGCHANGE|WM_SYSCOLORCHANGE=>{with_app(App::apply_theme);},
        WM_NCDESTROY=>{WINDOW.with(|active|{if active.get()==window{active.set(ptr::null_mut());}});unsafe{SetWindowLongPtrW(window,GWLP_USERDATA,0);}return 0;},
        WM_COMMAND if (wp&0xffff)as u16==DOCUMENTATION=>{commands::shell_open(window,DOCS_URL);return 1;},
        _=>{}
    }
    let Ok(mut state)=(unsafe{&*state}).try_borrow_mut()else{return 0;};
    match message{
        WM_INITDIALOG=>{WINDOW.with(|active|active.set(window));if let Err(error)=state.initialize(window){state.error=Some(error);unsafe{EndDialog(window,-1);}}1},
        WM_COMMAND=>{match(wp&0xffff)as u16{PREVIOUS=>{state.page=state.page.saturating_sub(1);state.update();},NEXT=>{if state.page+1==PAGES.len(){unsafe{EndDialog(window,IDOK as isize);}}else{state.page+=1;state.update();}},id if id==IDCANCEL as u16=>{unsafe{EndDialog(window,IDCANCEL as isize);}},_=>return 0}1},
        WM_CLOSE=>{unsafe{EndDialog(window,IDCANCEL as isize);}1},
        WM_THEMECHANGED=>{with_look(|look|{state.dark_mode.apply_title_bar(window,look.style.palette.dark);state.dark_mode.apply_control(state.body,look.style.palette.dark);});unsafe{InvalidateRect(window,ptr::null(),1);}1},
        WM_DPICHANGED=>{state.resize((wp&0xffff)as u32,if lp!=0{Some(unsafe{*(lp as *const RECT)})}else{None});1},
        WM_PAINT=>{let mut paint=PAINTSTRUCT::default();let dc=unsafe{BeginPaint(window,&mut paint)};state.paint(dc);unsafe{EndPaint(window,&paint);}1},
        WM_ERASEBKGND=>1,
        _=>0
    }
}
pub(super) fn show(parent:HWND)->Result<(),String>{
    let state=RefCell::new(Dialog::new());let template=Template{dialog:DLGTEMPLATE{style:WS_POPUP|WS_CAPTION|WS_SYSMENU|DS_MODALFRAME as u32,dwExtendedStyle:WS_EX_DLGMODALFRAME,cx:360,cy:250,..Default::default()},menu:0,class:0,title:0};
    let result=unsafe{DialogBoxIndirectParamW(GetModuleHandleW(ptr::null()),&template.dialog,parent,Some(procedure),&state as *const RefCell<Dialog>as isize)};
    let mut state=state.into_inner();if let Some(error)=state.error.take(){return Err(error);}if result== -1{return Err(std::io::Error::last_os_error().to_string());}Ok(())
}
