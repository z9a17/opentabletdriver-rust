//! Arbitrary device strings and the pinned 1..255 dump, never on the UI thread.
use super::*;
use serde_json::{Value,json};
use std::sync::{Arc,atomic::{AtomicBool,AtomicU64,Ordering},mpsc};
use windows_sys::Win32::UI::HiDpi::{DDC_DISABLE_ALL,SetDialogDpiChangeBehavior};
const DEVICE:u16=101;const VENDOR:u16=102;const PRODUCT:u16=103;const INDEX:u16=104;
const REQUEST:u16=105;const DUMP:u16=106;const RECONNECT:u16=107;const CANCEL:u16=108;const COPY:u16=109;
const WM_RESULT:u32=WM_APP+51;
static SERIAL:AtomicU64=AtomicU64::new(1);
thread_local!{static WINDOW:Cell<HWND>=const{Cell::new(ptr::null_mut())};}
pub(super) fn refresh_theme(){WINDOW.with(|window|{if !window.get().is_null(){unsafe{SendMessageW(window.get(),WM_THEMECHANGED,0,0);}}});}
enum Event{Devices(Result<Vec<Value>,String>),String(u64,u8,Result<String,String>),Done(u64)}
#[repr(C,align(4))]struct Template{dialog:DLGTEMPLATE,menu:u16,class:u16,title:u16}
struct Dialog{
    window:HWND,fields:[HWND;3],device:HWND,result:HWND,reconnect:HWND,controls:Vec<(HWND,RECT)>,fonts:Option<FontSet>,
    dpi:u32,token:u64,devices:Vec<Value>,sender:mpsc::SyncSender<Event>,receiver:mpsc::Receiver<Event>,
    cancel:Arc<AtomicBool>,cancellations:Vec<Arc<AtomicBool>>,query_epoch:u64,resume:Option<mpsc::SyncSender<bool>>,busy:bool,dump:bool,output:String,error:Option<String>,dark_mode:theme::DarkMode,
}
impl Dialog{
    fn new()->Self{let(sender,receiver)=mpsc::sync_channel(16);Self{window:ptr::null_mut(),fields:[ptr::null_mut();3],device:ptr::null_mut(),result:ptr::null_mut(),reconnect:ptr::null_mut(),controls:Vec::new(),fonts:None,dpi:96,token:SERIAL.fetch_add(1,Ordering::Relaxed),devices:Vec::new(),sender,receiver,cancel:Arc::new(AtomicBool::new(false)),cancellations:Vec::new(),query_epoch:0,resume:None,busy:false,dump:false,output:String::new(),error:None,dark_mode:theme::DarkMode::load()}}
    fn add(&mut self,class:&str,title:&str,id:u16,style:u32,bounds:RECT,kind:Kind)->Result<HWND,String>{
        let control=unsafe{CreateWindowExW(0,wide(class).as_ptr(),wide(title).as_ptr(),WS_CHILD|WS_VISIBLE|style,0,0,0,0,self.window,id as usize as _,GetModuleHandleW(ptr::null()),ptr::null())};
        if control.is_null(){return Err(std::io::Error::last_os_error().to_string());}
        update_look(|look|{look.controls.insert(control as isize,ControlInfo{kind,surface:Surface::Window});});self.controls.push((control,bounds));Ok(control)
    }
    fn initialize(&mut self,window:HWND)->Result<(),String>{
        self.window=window;if unsafe{SetDialogDpiChangeBehavior(window,DDC_DISABLE_ALL,DDC_DISABLE_ALL)}==0{return Err(std::io::Error::last_os_error().to_string());}
        set_text(window,"Device string reader");self.device=self.add("BUTTON","Choose connected device...",DEVICE,WS_TABSTOP|BS_PUSHBUTTON as u32,rect(20,18,580,50),Kind::Dropdown)?;
        for(index,(label,id))in[("Vendor ID (decimal)",VENDOR),("Product ID (decimal)",PRODUCT),("String index (0..255)",INDEX)].into_iter().enumerate(){let x=20+index as i32*190;self.add("STATIC",label,0,SS_LEFT,rect(x,62,x+180,86),Kind::Label)?;self.fields[index]=self.add("EDIT",if index==2{"1"}else{""},id,WS_TABSTOP|ES_AUTOHSCROLL as u32|ES_NUMBER as u32,rect(x,90,x+180,118),Kind::Field)?;}
        self.reconnect=self.add("BUTTON","Require reconnect on fail",RECONNECT,WS_TABSTOP|BS_AUTOCHECKBOX as u32,rect(20,136,272,166),Kind::Check)?;
        self.add("BUTTON","Send request",REQUEST,WS_TABSTOP|BS_DEFPUSHBUTTON as u32,rect(288,136,424,168),Kind::Button)?;
        self.add("BUTTON","Dump all",DUMP,WS_TABSTOP|BS_PUSHBUTTON as u32,rect(436,136,580,168),Kind::Button)?;
        self.result=self.add("EDIT","",0,WS_TABSTOP|WS_VSCROLL|ES_MULTILINE as u32|ES_READONLY as u32|ES_AUTOVSCROLL as u32,rect(20,184,580,366),Kind::Field)?;
        unsafe{SendMessageW(self.result,EM_LIMITTEXT,crate::control::MAX_PROFILE_BYTES,0);}
        self.add("BUTTON","Copy",COPY,WS_TABSTOP|BS_PUSHBUTTON as u32,rect(20,388,140,420),Kind::Button)?;
        self.add("BUTTON","Cancel request",CANCEL,WS_TABSTOP|BS_PUSHBUTTON as u32,rect(288,388,424,420),Kind::Button)?;
        self.add("BUTTON","Close",IDCANCEL as u16,WS_TABSTOP|BS_PUSHBUTTON as u32,rect(436,388,580,420),Kind::Button)?;
        if let Some(device)=with_app(|app|app.selected_device.clone()).flatten(){set_text(self.fields[0],&device.digitizer.vendor_id.unwrap_or(0).to_string());set_text(self.fields[1],&device.digitizer.product_id.unwrap_or(0).to_string());}
        self.resize(unsafe{GetDpiForWindow(window)}.max(96),None);self.theme();
        let sender=self.sender.clone();let window=window as isize;let token=self.token;let stop=Arc::clone(&self.cancel);self.cancellations.push(Arc::clone(&stop));
        std::thread::Builder::new().name("string-reader-device-list".into()).spawn(move||{
            let devices=crate::cli::Client::connect_cancellable(Arc::clone(&stop)).and_then(|mut client|client.call("GetDevices",json!([]))).and_then(|value|value.as_array().cloned().ok_or_else(||"device result is not an array".into()));
            send(&sender,Event::Devices(devices),window,token,&stop);
        }).map_err(|error|error.to_string())?;Ok(())
    }
    fn theme(&self){with_look(|look|{self.dark_mode.apply_title_bar(self.window,look.style.palette.dark);self.dark_mode.apply_control(self.result,look.style.palette.dark);});unsafe{InvalidateRect(self.window,ptr::null(),1);}}
    fn query(&mut self,all:bool)->Result<(),String>{
        if self.busy{return Err("a string request is already pending".into());}
        let vendor:u16=text(self.fields[0]).parse().map_err(|_|"Vendor ID must be decimal 0..65535")?;let product:u16=text(self.fields[1]).parse().map_err(|_|"Product ID must be decimal 0..65535")?;let index:u8=text(self.fields[2]).parse().map_err(|_|"String index must be 0..255")?;
        self.cancellations.retain(|stop|Arc::strong_count(stop)>1);self.cancel=Arc::new(AtomicBool::new(false));let stop=Arc::clone(&self.cancel);self.cancellations.push(Arc::clone(&stop));self.query_epoch=self.query_epoch.checked_add(1).ok_or("string request generation exhausted")?;let epoch=self.query_epoch;let sender=self.sender.clone();let window=self.window as isize;let token=self.token;
        let (resume,answers)=mpsc::sync_channel(1);self.resume=Some(resume);
        let reconnect=unsafe{SendMessageW(self.reconnect,BM_GETCHECK,0,0)}==BST_CHECKED as isize;
        std::thread::Builder::new().name("device-string-query".into()).spawn(move||{
            let mut client=None;let indices:Vec<u8>=if all{(1..=255).collect()}else{vec![index]};
            for index in indices{
                loop{
                    if stop.load(Ordering::Acquire){break;}
                    let result=(||{
                        if client.as_ref().is_none_or(|client:&crate::cli::Client|!client.usable()){
                            client=Some(crate::cli::Client::connect_cancellable(Arc::clone(&stop))?);
                        }
                        client.as_mut().unwrap().call_with_timeout("RequestDeviceString",json!([vendor,product,index]),Duration::from_secs(5)).and_then(|value|value.as_str().map(str::to_owned).ok_or_else(||"device string is not text".into()))
                    })();
                    let failed=result.is_err();if !send(&sender,Event::String(epoch,index,result),window,token,&stop){return;}
                    if !failed||!all||!reconnect{break;}
                    let retry=loop{if stop.load(Ordering::Acquire){return;}match answers.recv_timeout(Duration::from_millis(50)){Ok(value)=>break value,Err(mpsc::RecvTimeoutError::Timeout)=>{},Err(_)=>return,}};
                    if !retry{send(&sender,Event::Done(epoch),window,token,&stop);return;}
                    client=None;
                }
                if stop.load(Ordering::Acquire){break;}
            }
            send(&sender,Event::Done(epoch),window,token,&stop);
        }).map_err(|error|error.to_string())?;
        self.set_busy(true);self.dump=all;self.output.clear();set_text(self.result,"");Ok(())
    }
    fn set_busy(&mut self,busy:bool){
        self.busy=busy;
        for id in [DEVICE,VENDOR,PRODUCT,INDEX,REQUEST,DUMP,RECONNECT]{unsafe{EnableWindow(GetDlgItem(self.window,id as i32),i32::from(!busy));}}
    }
    fn resize(&mut self,dpi:u32,suggested:Option<RECT>){
        self.dpi=dpi;let fonts=FontSet::new(dpi);let mut work=RECT::default();unsafe{SystemParametersInfoW(SPI_GETWORKAREA,0,(&mut work as *mut RECT).cast(),0);}
        let height=442.min(((work.bottom-work.top-72)*96/dpi as i32).max(280));
        for(control,bounds)in &self.controls{let mut bounds=*bounds;if *control==self.result{bounds.bottom=height-76;}else if bounds.top>=388{bounds.top=height-54;bounds.bottom=height-22;}unsafe{SendMessageW(*control,WM_SETFONT,fonts.fonts.ui as usize,1);SetWindowPos(*control,ptr::null_mut(),scale(bounds.left,dpi),scale(bounds.top,dpi),scale(bounds.right-bounds.left,dpi),scale(bounds.bottom-bounds.top,dpi),SWP_NOZORDER|SWP_NOACTIVATE);}}
        self.fonts=Some(fonts);let mut bounds=rect(0,0,scale(600,dpi),scale(height,dpi));unsafe{AdjustWindowRectExForDpi(&mut bounds,WS_CAPTION|WS_SYSMENU|DS_MODALFRAME as u32,0,WS_EX_DLGMODALFRAME,dpi);let mut parent=RECT::default();GetWindowRect(GetParent(self.window),&mut parent);let(width,height)=(bounds.right-bounds.left,bounds.bottom-bounds.top);let(left,top)=suggested.map(|r|(r.left,r.top)).unwrap_or(((parent.left+parent.right-width)/2,(parent.top+parent.bottom-height)/2));SetWindowPos(self.window,ptr::null_mut(),left,top,width,height,SWP_NOZORDER|SWP_NOACTIVATE);SendMessageW(self.window,DM_REPOSITION,0,0);}
    }
}
fn send(sender:&mpsc::SyncSender<Event>,mut event:Event,window:isize,token:u64,stop:&AtomicBool)->bool{
    loop{if stop.load(Ordering::Acquire){return false;}match sender.try_send(event){Ok(())=>{unsafe{PostMessageW(window as HWND,WM_RESULT,token as usize,0);}return true;},Err(mpsc::TrySendError::Disconnected(_))=>return false,Err(mpsc::TrySendError::Full(value))=>{event=value;std::thread::sleep(Duration::from_millis(5));}}}
}
impl Drop for Dialog{fn drop(&mut self){for stop in &self.cancellations{stop.store(true,Ordering::Release);}self.cancel.store(true,Ordering::Release);update_look(|look|{for(control,_)in &self.controls{look.controls.remove(&(*control as isize));}});}}
unsafe extern "system" fn procedure(window:HWND,message:u32,wp:WPARAM,lp:LPARAM)->isize{
    if message==WM_INITDIALOG{unsafe{SetWindowLongPtrW(window,GWLP_USERDATA,lp);}}let state=unsafe{GetWindowLongPtrW(window,GWLP_USERDATA)}as *const RefCell<Dialog>;if state.is_null(){return 0;}
    match message{
        WM_CTLCOLORDLG=>return with_look(|look|look.brush(look.style.palette.window)as isize).unwrap_or(0),WM_CTLCOLORSTATIC|WM_CTLCOLOREDIT=>return ctl_color(wp as HDC,lp as HWND).unwrap_or(0),
        WM_NOTIFY if lp!=0=>{if unsafe{&*(lp as *const NMHDR)}.code==NM_CUSTOMDRAW{let result=custom_draw(unsafe{&mut*(lp as *mut NMCUSTOMDRAW)});unsafe{SetWindowLongPtrW(window,DWLP_MSGRESULT as i32,result);}return 1;}},
        WM_SETTINGCHANGE|WM_SYSCOLORCHANGE=>{with_app(App::apply_theme);},
        WM_NCDESTROY=>{WINDOW.with(|active|{if active.get()==window{active.set(ptr::null_mut());}});if let Ok(state)=unsafe{&*state}.try_borrow(){state.cancel.store(true,Ordering::Release);}unsafe{SetWindowLongPtrW(window,GWLP_USERDATA,0);}return 0;},
        WM_COMMAND if (wp&0xffff)as u16==DEVICE=>{
            let Ok(view)=unsafe{&*state}.try_borrow()else{return 0;};let devices=view.devices.clone();let anchor=view.device;drop(view);
            let menu=unsafe{CreatePopupMenu()};for(index,device)in devices.iter().enumerate(){commands::append(menu,MF_STRING,index as u16+1,&format!("{} ({}:{})",device["ProductName"].as_str().unwrap_or("Device"),device["VendorID"],device["ProductID"]));}
            let choice=commands::popup(window,menu,anchor);if let Some(device)=choice.checked_sub(1).and_then(|index|devices.get(index as usize)){if let Ok(state)=unsafe{&*state}.try_borrow(){set_text(state.fields[0],&device["VendorID"].to_string());set_text(state.fields[1],&device["ProductID"].to_string());set_text(state.device,device["ProductName"].as_str().unwrap_or("Connected device"));}}return 1;
        },
        WM_RESULT=>{
            let mut reconnect=None;{
                let Ok(mut state)=unsafe{&*state}.try_borrow_mut()else{return 0;};if state.token!=wp as u64{return 0;}
                while let Ok(event)=state.receiver.try_recv(){match event{
                    Event::Devices(Ok(devices))=>state.devices=devices,Event::Devices(Err(error))=>{state.output=format!("Cannot list connected devices: {error}\r\nEnter VID/PID manually.");set_text(state.result,&state.output);},
                    Event::String(epoch,index,result)=>{if epoch!=state.query_epoch{continue;}let failed=result.is_err();let value=result.map(|value|serde_json::to_string(&value).unwrap_or(value)).unwrap_or_else(|error|format!("Error: {error}"));state.output.push_str(&format!("String index {index}: {value}\r\n"));if state.output.len()>crate::control::MAX_PROFILE_BYTES{state.cancel.store(true,Ordering::Release);state.output.push_str("Output exceeds 128 KiB; dump stopped.");state.set_busy(false);}set_text(state.result,&state.output);if failed&&state.dump&&unsafe{SendMessageW(state.reconnect,BM_GETCHECK,0,0)}==BST_CHECKED as isize{reconnect=state.resume.clone();break;}},
                    Event::Done(epoch)=>{if epoch==state.query_epoch{state.set_busy(false);state.resume=None;}},
                }}
            }
            if let Some(resume)=reconnect{let retry=commands::message_box(window,"Reconnect the device, then choose OK to retry this index. Cancel stops the dump.","Device string failed",MB_OKCANCEL|MB_ICONQUESTION)==IDOK;let _=resume.try_send(retry);}return 1;
        },
        _=>{}
    }
    let Ok(mut state)=unsafe{&*state}.try_borrow_mut()else{return 0;};
    match message{
        WM_INITDIALOG=>{WINDOW.with(|active|active.set(window));if let Err(error)=state.initialize(window){state.error=Some(error);unsafe{EndDialog(window,-1);}}1},
        WM_COMMAND=>{match(wp&0xffff)as u16{REQUEST|DUMP=>{if let Err(error)=state.query((wp&0xffff)as u16==DUMP){set_text(state.result,&error);}},CANCEL=>{state.cancel.store(true,Ordering::Release);state.query_epoch=state.query_epoch.saturating_add(1);state.set_busy(false);state.resume=None;},COPY=>{commands::copy_to_clipboard(window,&state.output);},id if id==IDCANCEL as u16=>{state.cancel.store(true,Ordering::Release);unsafe{EndDialog(window,IDCANCEL as isize);}},_=>return 0}1},
        WM_CLOSE=>{state.cancel.store(true,Ordering::Release);unsafe{EndDialog(window,IDCANCEL as isize);}1},WM_THEMECHANGED=>{state.theme();1},WM_DPICHANGED=>{state.resize((wp&0xffff)as u32,if lp!=0{Some(unsafe{*(lp as *const RECT)})}else{None});1},
        WM_PAINT=>{let mut paint=PAINTSTRUCT::default();let dc=unsafe{BeginPaint(window,&mut paint)};with_look(|look|{if let Some(mut canvas)=canvas::Canvas::new(dc,client_rect(window)){canvas.fill(client_rect(window),look.style.palette.window);canvas.present(dc);}});unsafe{EndPaint(window,&paint);}1},WM_ERASEBKGND=>1,_=>0
    }
}
pub(super) fn show(parent:HWND)->Result<(),String>{
    let state=RefCell::new(Dialog::new());let template=Template{dialog:DLGTEMPLATE{style:WS_POPUP|WS_CAPTION|WS_SYSMENU|DS_MODALFRAME as u32,dwExtendedStyle:WS_EX_DLGMODALFRAME,cx:360,cy:260,..Default::default()},menu:0,class:0,title:0};let result=unsafe{DialogBoxIndirectParamW(GetModuleHandleW(ptr::null()),&template.dialog,parent,Some(procedure),&state as *const RefCell<Dialog>as isize)};if let Some(error)=state.borrow_mut().error.take(){return Err(error);}if result== -1{return Err(std::io::Error::last_os_error().to_string());}Ok(())
}
